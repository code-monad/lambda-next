use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::signal;
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio::time::{Duration, Instant, sleep};
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{Level, debug, error, info, instrument, warn};
use tracing_subscriber::EnvFilter;
use url::Url;

mod ckb;
mod config;
mod query;
mod utils;
mod ws;
mod spore;
mod db;
mod dob_decoder;

use ckb::CkbClient;
use config::Config;
use utils::{AppError, Result, SharedState};
use spore::preload_type_id_files;

// Channels for coordinating shutdown
struct ShutdownChannels {
    sender: broadcast::Sender<()>,
}

impl ShutdownChannels {
    fn new() -> Self {
        let (sender, _) = broadcast::channel(16);
        Self { sender }
    }

    fn sender(&self) -> broadcast::Sender<()> {
        self.sender.clone()
    }

    fn receiver(&self) -> broadcast::Receiver<()> {
        self.sender.subscribe()
    }
}

#[tokio::main]
#[instrument]
async fn main() -> Result<()> {
    // Set up tracing with console output only
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(Level::INFO.into()))
        .with_ansi(true) // Use colors in terminal output
        .init();

    // Start timing the application
    let app_start_time = Instant::now();
    info!("Starting application...");

    // Load configuration
    let config = Config::load().map_err(|e| {
        error!("Failed to load configuration: {}", e);
        AppError::ConfigError(e)
    })?;
    info!("Configuration loaded successfully");

    // Initialize database if enabled
    let db = if let Some(db_config) = &config.database {
        if db_config.enabled {
            info!("Initializing database connection");
            let db_config_for_db = db::DbConfig {
                url: db_config.url.clone(),
                max_connections: db_config.max_connections,
            };
            
            match db::SporeDb::new(&db_config_for_db).await {
                Ok(db) => {
                    info!("Database connection established successfully");
                    Some(Arc::new(db))
                }
                Err(e) => {
                    error!("Failed to connect to database: {}", e);
                    None
                }
            }
        } else {
            info!("Database is disabled in configuration");
            None
        }
    } else {
        info!("No database configuration found");
        None
    };

    // Preload Type IDs from all configured files
    info!("Preloading Type IDs from configured files...");
    if let Err(e) = preload_type_id_files(&config.spore_filters) {
        error!("Failed to preload Type IDs: {}", e);
        // Continue execution, as this is not fatal
    }

    // Parse the WebSocket URL
    let url = Url::parse(&config.websocket.url).map_err(|e| {
        error!("Failed to parse WebSocket URL: {}", e);
        AppError::UrlParseError(e)
    })?;

    // Create shared state for responses
    let state = Arc::new(Mutex::new(SharedState::new()));

    // Create task to manage WebSocket connection with automatic reconnection
    let ws_config = config.websocket.clone();
    let url_clone = url.clone();
    let state_clone = state.clone();
    let ckb_config = config.ckb.clone();

    let connection_manager_handle = tokio::spawn(async move {
        let mut last_error: Option<AppError> = None;

        loop {
            match last_error {
                Some(ref e) => {
                    info!("Reconnecting after error: {}", e);
                }
                None => {
                    info!("Establishing initial WebSocket connection");
                }
            }

            // Attempt to connect with retry logic
            match ws::connect_with_retry(url_clone.clone(), ws_config.clone()).await {
                Ok(ws_stream) => {
                    info!("WebSocket connection established");

                    // Create shutdown channels
                    let shutdown = ShutdownChannels::new();
                    let mut shutdown_receiver = shutdown.receiver();

                    // Split the WebSocket stream
                    let (write, read) = ws_stream.split();

                    // Wrap the writer in Arc<Mutex> so it can be shared between tasks
                    let shared_writer = Arc::new(Mutex::new(write));

                    // Create CKB client with a clone of the shared writer
                    let ckb_client = CkbClient::with_shared_writer(shared_writer.clone());

                    // Start a struct to track task completion
                    let (task_complete_tx, task_complete_rx) = oneshot::channel();
                    let mut task_complete_tx = Some(task_complete_tx);

                    // Start message handler task
                    let handler_state = state_clone.clone();
                    info!("Starting WebSocket message handler");
                    let receive_handle = tokio::spawn({
                        async move {
                            let result = ws::handle_websocket_messages(read, handler_state).await;
                            match result {
                                Ok(_) => {
                                    info!("WebSocket message handler completed normally");
                                }
                                Err(e) => {
                                    if let AppError::WebSocketError(ref ws_err) = e {
                                        if ws::is_eof_error(ws_err) {
                                            warn!(
                                                "WebSocket closed with unexpected EOF - this is a normal disconnection"
                                            );
                                        } else {
                                            error!("WebSocket message handler error: {}", e);
                                        }
                                    } else {
                                        error!("WebSocket message handler error: {}", e);
                                    }
                                }
                            }

                            // Signal that this task has completed
                            if let Some(tx) = task_complete_tx.take() {
                                let _ = tx.send("ws_handler");
                            }
                        }
                    });

                    // Start ping task using the shared writer
                    info!(
                        "Starting ping task with interval {} seconds",
                        ws_config.ping_interval
                    );
                    let ping_writer = shared_writer.clone();
                    let shutdown_sender = shutdown.sender();
                    let mut shutdown_ping = shutdown.receiver();
                    let ping_handle = tokio::spawn(async move {
                        let mut interval =
                            tokio::time::interval(Duration::from_secs(ws_config.ping_interval));

                        loop {
                            // Check if shutdown requested
                            if shutdown_ping.try_recv().is_ok() {
                                info!("Ping task received shutdown request");
                                break;
                            }

                            tokio::select! {
                                _ = interval.tick() => {
                                    debug!("Sending ping to keep WebSocket connection alive");

                                    // Lock the shared writer
                                    let mut writer_guard = ping_writer.lock().await;

                                    // Send ping
                                    match writer_guard.send(Message::Ping(vec![1, 2, 3, 4].into())).await {
                                        Ok(_) => {
                                            debug!("Ping sent successfully");
                                        },
                                        Err(e) => {
                                            if ws::is_eof_error(&e) {
                                                info!("Ping failed with EOF error - connection closed");
                                            } else {
                                                error!("Failed to send ping: {}", e);
                                            }

                                            // Signal shutdown to other tasks
                                            let _ = shutdown_sender.send(());
                                            break;
                                        }
                                    }
                                },
                                Ok(_) = shutdown_ping.recv() => {
                                    info!("Ping task received shutdown during sleep");
                                    break;
                                }
                            }
                        }

                        info!("Ping task shutting down gracefully");
                    });

                    // Start query task with shutdown channel
                    let query_state = state_clone.clone();
                    let type_script_code_hash = ckb_config.type_script_code_hash.clone();
                    let hash_type = ckb_config.type_script_hash_type.clone();
                    let limit = ckb_config.query_limit;
                    let interval_secs = ckb_config.query_interval_secs;
                    let shutdown_sender = shutdown.sender();
                    let mut shutdown_receiver_clone = shutdown.receiver();
                    let query_config = config.clone();

                    info!("Starting query task");
                    let db_clone = db.as_ref().map(|db_arc| db_arc.clone()); // Properly clone Option<Arc<T>>
                    let query_handle = tokio::spawn(async move {
                        let mut queries_run = 0;
                        let mut total_query_time = Duration::from_secs(0);

                        // Small delay to ensure connection is established
                        sleep(Duration::from_millis(500)).await;

                        loop {
                            // Check if shutdown requested
                            if shutdown_receiver_clone.try_recv().is_ok() {
                                info!("Query task received shutdown request");
                                break;
                            }

                            // Run the query
                            let start = Instant::now();
                            let db_for_query = db_clone.clone(); // Clone again for each loop iteration
                            let query_result = query::query_cells_ws(
                                ckb_client.clone(),
                                type_script_code_hash.clone(),
                                hash_type.clone(),
                                limit,
                                query_state.clone(),
                                query_config.clone(),
                                db_for_query,  // Use the cloned value
                            )
                            .await;

                            match query_result {
                                Ok(_) => {
                                    queries_run += 1;
                                    let duration = start.elapsed();
                                    total_query_time += duration;
                                    let avg_duration = if queries_run > 0 {
                                        total_query_time.as_secs_f64() / queries_run as f64
                                    } else {
                                        0.0
                                    };

                                    info!(
                                        queries_run = %queries_run,
                                        avg_duration_secs = %avg_duration,
                                        "Query cycle completed - sleeping before next run"
                                    );
                                }
                                Err(e) => {
                                    error!("Query failed: {}. Will try again at next interval.", e);
                                    // Check if this is a connection error that should trigger reconnection
                                    if let AppError::WebSocketError(_) = e {
                                        error!("WebSocket error in query, triggering reconnection");
                                        let _ = shutdown_sender.send(());
                                        break;
                                    }
                                    // We don't exit on other query errors, just continue with the next cycle
                                }
                            }

                            // Wait for either the interval timer or a shutdown signal
                            tokio::select! {
                                _ = sleep(Duration::from_secs(interval_secs)) => {
                                    // Continue with the next loop iteration
                                },
                                Ok(_) = shutdown_receiver_clone.recv() => {
                                    info!("Query task received shutdown during sleep");
                                    break;
                                }
                            }
                        }

                        info!("Query task shutting down gracefully");
                    });

                    // Wait for either a task to complete or a shutdown signal
                    let reason = tokio::select! {
                        task = task_complete_rx => {
                            match task {
                                Ok(name) => {
                                    warn!("Task {} completed, triggering reconnection", name);
                                    format!("Task {} exited", name)
                                },
                                Err(_) => {
                                    warn!("Task completion channel closed");
                                    "Task completion channel closed".to_string()
                                }
                            }
                        },
                        Ok(_) = shutdown_receiver.recv() => {
                            info!("Received shutdown signal");
                            "Shutdown requested".to_string()
                        }
                    };

                    // Signal shutdown to all tasks
                    info!("Shutting down all tasks: {}", reason);
                    let _ = shutdown.sender().send(());

                    // Cancel all running tasks
                    receive_handle.abort();
                    ping_handle.abort();
                    query_handle.abort();

                    // Short delay before reconnecting
                    info!("WebSocket connection issue detected, preparing to reconnect...");
                    sleep(Duration::from_secs(1)).await;

                    // Set the error for reconnection
                    last_error = Some(AppError::MessageHandlingError(reason));
                }
                Err(e) => {
                    error!("Failed to establish WebSocket connection: {}", e);
                    last_error = Some(e);

                    // Short delay before reconnecting
                    sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });

    // Create a future that completes when CTRL+C is pressed
    let ctrl_c = signal::ctrl_c();

    // Wait for CTRL+C signal
    info!("Application running. Press Ctrl+C to stop.");
    tokio::select! {
        _ = connection_manager_handle => {
            error!("Connection manager exited unexpectedly");
            return Err(AppError::MessageHandlingError("Connection manager exited unexpectedly".into()));
        }
        _ = ctrl_c => {
            info!("Received shutdown signal, gracefully terminating");
        }
    }

    // total application runtime. just for measuring
    let app_runtime = app_start_time.elapsed();

    info!(
        runtime_ms = %app_runtime.as_millis(),
        runtime_secs = %app_runtime.as_secs_f64(),
        runtime_mins = %(app_runtime.as_secs() as f64 / 60.0),
        "Application shutting down"
    );
    Ok(())
}
