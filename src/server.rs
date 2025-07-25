use crate::{
    ckb::CkbClient,
    config::Config,
    dob_decoder,
    spore::{preload_type_id_files, SporeCell},
    utils::SharedState,
    ws,
};
use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, oneshot};
use tokio::time::{sleep, Duration, Instant};
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, error, info, warn, Level};
use url::Url;
use crate::db::SporeDb;
use tracing_subscriber::EnvFilter;

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

pub struct App {
    pub config: Config,
    pub db: Option<Arc<SporeDb>>,
}

impl App {
    pub async fn new() -> Result<Self> {
        // Set up tracing with console output only
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env().add_directive(Level::INFO.into()))
            .with_ansi(true) // Use colors in terminal output
            .init();

        info!("Starting application...");

        // Load configuration
        let config = Config::load()?;
        info!("Configuration loaded successfully");

        // Initialize database if enabled
        let db = if let Some(db_config) = &config.database {
            if db_config.enabled {
                let db_config_for_db = crate::db::DbConfig {
                    url: db_config.url.clone(),
                    max_connections: db_config.max_connections,
                };
                let db = SporeDb::new(&db_config_for_db).await?;
                info!("Database connection established successfully");
                Some(Arc::new(db))
            } else {
                info!("Database is disabled in configuration");
                None
            }
        } else {
            info!("No database configuration found");
            None
        };
        Ok(Self { config, db })
    }

    pub async fn run(&self) -> Result<()> {
        // Preload Type IDs from all configured files
        info!("Preloading Type IDs from configured files...");
        if let Err(e) = preload_type_id_files(&self.config.spore_filters) {
            error!("Failed to preload Type IDs: {}", e);
            // Continue execution, as this is not fatal
        }

        // Parse the WebSocket URL
        let url = Url::parse(&self.config.websocket.url)?;

        // Create shared state for responses
        let state = Arc::new(Mutex::new(SharedState::new()));

        // Create task to manage WebSocket connection with automatic reconnection
        let ws_config = self.config.websocket.clone();
        let url_clone = url.clone();
        let state_clone = state.clone();
        let ckb_config = self.config.ckb.clone();
        let db_clone = self.db.clone();
        let config_clone = self.config.clone();

        let connection_manager_handle = tokio::spawn(async move {
            loop {
                info!("Establishing WebSocket connection");
                match ws::connect_with_retry(url_clone.clone(), ws_config.clone()).await {
                    Ok(ws_stream) => {
                        info!("WebSocket connection established");
                        let shutdown = ShutdownChannels::new();
                        let mut shutdown_receiver = shutdown.receiver();
                        let (write, read) = ws_stream.split();
                        let shared_writer = Arc::new(Mutex::new(write));
                        let ckb_client = CkbClient::with_shared_writer(shared_writer.clone());
                        let (task_complete_tx, mut task_complete_rx) = oneshot::channel();
                        let mut task_complete_tx_opt = Some(task_complete_tx);

                        let handler_state = state_clone.clone();
                        let receive_handle = tokio::spawn(async move {
                            if let Err(e) = ws::handle_websocket_messages(read, handler_state).await {
                                error!("WebSocket message handler error: {}", e);
                            }
                            if let Some(tx) = task_complete_tx_opt.take() {
                                let _ = tx.send("ws_handler");
                            }
                        });

                        let ping_writer = shared_writer.clone();
                        let shutdown_sender = shutdown.sender();
                        let mut shutdown_ping = shutdown.receiver();
                        let ping_handle = tokio::spawn(async move {
                            let mut interval =
                                tokio::time::interval(Duration::from_secs(ws_config.ping_interval));
                            loop {
                                tokio::select! {
                                    _ = interval.tick() => {
                                        let mut writer_guard = ping_writer.lock().await;
                                        if let Err(e) = writer_guard.send(Message::Ping(vec![1, 2, 3, 4].into())).await {
                                            error!("Failed to send ping: {}", e);
                                            let _ = shutdown_sender.send(());
                                            break;
                                        }
                                    },
                                    _ = shutdown_ping.recv() => {
                                        info!("Ping task received shutdown");
                                        break;
                                    }
                                }
                            }
                        });

                        let query_state = state_clone.clone();
                        let shutdown_sender = shutdown.sender();
                        let mut shutdown_receiver_clone = shutdown.receiver();
                        let db_clone_for_query = db_clone.clone();
                        let query_handle = tokio::spawn(async move {
                            if let Err(e) = crate::query::run_query_loop(
                                query_state,
                                ckb_client,
                                &config_clone,
                                db_clone_for_query,
                                &mut shutdown_receiver_clone,
                            )
                            .await
                            {
                                error!("Query loop error: {}", e);
                                let _ = shutdown_sender.send(());
                            }
                        });

                        tokio::select! {
                            _ = tokio::signal::ctrl_c() => {
                                info!("Ctrl-C received, shutting down");
                                let _ = shutdown.sender().send(());
                            },
                            res = &mut task_complete_rx => {
                                if let Ok(task_name) = res {
                                    info!("Task `{}` completed, shutting down other tasks", task_name);
                                } else {
                                    info!("A task completed, shutting down other tasks");
                                }
                                let _ = shutdown.sender().send(());
                            }
                        }

                        let _ = tokio::join!(receive_handle, ping_handle, query_handle);
                        info!("All tasks shut down gracefully.");
                    }
                    Err(e) => {
                        error!("Failed to connect WebSocket: {}", e);
                        sleep(Duration::from_secs(ws_config.reconnect_interval)).await;
                    }
                }
            }
        });

        connection_manager_handle.await.context("Connection manager task failed")?;

        Ok(())
    }
} 