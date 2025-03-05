use futures::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, connect_async_tls_with_config, tungstenite::protocol::Message};
use url::Url;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use tokio::sync::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use tracing::{info, warn, error, debug, trace, Level, instrument};
use tracing_subscriber::{EnvFilter};

mod config;
mod ckb;

use config::Config;
use ckb::{CkbClient, CkbError};

#[derive(Debug)]
enum AppError {
    WebSocketError(tokio_tungstenite::tungstenite::Error),
    ConfigError(Box<dyn Error + Send + Sync>),
    UrlParseError(url::ParseError),
    InvalidScheme(String),
    CkbError(ckb::CkbError),
    MessageHandlingError(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::WebSocketError(e) => write!(f, "WebSocket error: {}", e),
            AppError::ConfigError(e) => write!(f, "Configuration error: {}", e),
            AppError::UrlParseError(e) => write!(f, "URL parse error: {}", e),
            AppError::InvalidScheme(s) => write!(f, "Invalid URL scheme: {}", s),
            AppError::CkbError(e) => write!(f, "CKB error: {}", e),
            AppError::MessageHandlingError(e) => write!(f, "Message handling error: {}", e),
        }
    }
}

impl Error for AppError {}

impl From<ckb::CkbError> for AppError {
    fn from(error: ckb::CkbError) -> Self {
        AppError::CkbError(error)
    }
}

type Result<T> = std::result::Result<T, AppError>;

// Shared state for responses
struct SharedState {
    responses: HashMap<u64, Value>,
}

// Connect to a WebSocket server
#[instrument(fields(url = %url))]
async fn connect_to_websocket(url: Url) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>> {
    info!("Connecting to WebSocket server at {}", url);
    
    match url.scheme() {
        "ws" | "wss" => {
            // Both secure and non-secure connections are handled automatically by connect_async
            let (ws_stream, _) = connect_async(url.as_str())
                .await
                .map_err(|e| {
                    error!("WebSocket connection error: {}", e);
                    AppError::WebSocketError(e)
                })?;
                
            info!("WebSocket connection established");
            Ok(ws_stream)
        },
        s => {
            let error = AppError::InvalidScheme(s.to_string());
            error!("Invalid URL scheme: {}", s);
            Err(error)
        }
    }
}

// Handle incoming WebSocket messages and store responses
#[instrument(skip(read, state))]
async fn handle_websocket_messages(
    mut read: futures::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>,
    state: Arc<Mutex<SharedState>>,
) -> Result<()> {
    info!("Starting WebSocket message handler");
    
    while let Some(message) = read.next().await {
        match message {
            Ok(msg) => {
                if let Message::Text(text) = msg {
                    debug!("Received WebSocket message: {}", text);
                    
                    // Parse the JSON-RPC response
                    match serde_json::from_str::<Value>(&text) {
                        Ok(json) => {
                            // Check if it's a JSON-RPC response with an ID
                            if let Some(id) = json.get("id").and_then(|id| id.as_u64()) {
                                info!(id = %id, "Received response for request");
                                
                                // Store the response in the shared state
                                let mut state = state.lock().await;
                                state.responses.insert(id, json.clone());
                            }
                            
                            // Process cells from the response if it contains the result field
                            if json.get("result").is_some() && json.get("method").is_none() {
                                if let Some(result) = json.get("result") {
                                    if let Some(objects) = result.get("objects") {
                                        if let Some(objects_array) = objects.as_array() {
                                            info!(count = %objects_array.len(), "Found cells in response");
                                        }
                                    }
                                }
                            }
                        },
                        Err(e) => {
                            warn!("Error parsing JSON message: {}", e);
                        }
                    }
                }
            },
            Err(e) => {
                error!("Error receiving WebSocket message: {}", e);
                return Err(AppError::WebSocketError(e));
            }
        }
    }
    
    info!("WebSocket message handler completed");
    Ok(())
}

// Query for cells using WebSocket with proper request tracking and paging
#[instrument(skip(client, state), fields(code_hash = %type_script_code_hash, hash_type = %hash_type, limit = %limit))]
async fn query_cells_ws(
    client: CkbClient, 
    type_script_code_hash: String, 
    hash_type: String, 
    limit: u32,
    state: Arc<Mutex<SharedState>>,
) -> Result<()> {
    info!("Starting cell query operation");
    let mut cursor: Option<String> = None;
    let mut total_cells = 0;
    
    loop {
        info!(cursor = ?cursor, "Querying for cells");
        
        // Make the RPC call
        let request = client.get_cells_by_type_script_code_hash(
            &type_script_code_hash,
            &hash_type,
            limit,
            cursor.as_deref(),
        ).await?;
        
        // Get the request ID
        let request_id = request.get("id").and_then(|id| id.as_u64()).unwrap_or(0);
        info!(id = %request_id, "Request sent, waiting for response");
        
        // Wait for the response with matching ID
        let response = wait_for_response(request_id, state.clone()).await?;
        
        // Process the response - extract cells and cursor for next page
        let (cells, next_cursor) = match client.parse_cells_response(&response) {
            Ok((cells, cursor)) => (cells, cursor),
            Err(e) => {
                error!("Failed to parse cells response: {}", e);
                return Err(AppError::CkbError(e));
            }
        };
        
        // no cells means we're done
        if cells.is_empty() {
            info!("No cells found in response, ending query");
            break;
        }

        // Process the cells
        debug!(cells_count = %cells.len(), "Processing cells");
        for (idx, cell) in cells.iter().enumerate() {
            trace!(cell_idx = %idx, cell = ?cell, "Processing cell");
            total_cells += 1;
        }
        
        info!(page_count = %cells.len(), total_count = %total_cells, "Processed batch of cells");
        
        // If there's no next cursor, we're done
        match next_cursor {
            Some(next) => {
                debug!(next_cursor = %next, "Continuing with next page");
                cursor = Some(next);
            }
            None => {
                info!(total_cells = %total_cells, "Query complete, no more pages");
                break;
            }
        }
        
        // Small delay to prevent flooding
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    
    info!(total_cells = %total_cells, "Cell query operation completed");
    Ok(())
}

// Helper function to wait for a response with a specific ID
#[instrument(skip(state), fields(request_id = %request_id))]
async fn wait_for_response(request_id: u64, state: Arc<Mutex<SharedState>>) -> Result<Value> {
    debug!("Waiting for response");
    
    // Maximum time to wait for a response
    let timeout = tokio::time::Duration::from_secs(30);
    let start = tokio::time::Instant::now();
    
    while start.elapsed() < timeout {
        // Check if we have a response for this ID
        let response = {
            let mut state_guard = state.lock().await;
            state_guard.responses.remove(&request_id)
        };
        
        if let Some(response) = response {
            debug!("Response received within {} ms", start.elapsed().as_millis());
            return Ok(response);
        }
        
        // Wait a bit before checking again
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    
    let timeout_error = AppError::MessageHandlingError(format!(
        "Timeout waiting for response to request ID: {}", 
        request_id
    ));
    error!("Response timeout after {} ms: {}", timeout.as_millis(), timeout_error);
    Err(timeout_error)
}

#[tokio::main]
#[instrument]
async fn main() -> Result<()> {
    // Set up tracing with console output only
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(Level::INFO.into()))
        .with_ansi(true) // Use colors in terminal output
        .init();
    
    info!("Starting lambda-next application");
    
    // Load configuration
    let config = Config::load().map_err(|e| {
        error!("Failed to load configuration: {}", e);
        AppError::ConfigError(e)
    })?;
    
    // Connect to the WebSocket server
    let url = Url::parse(&config.websocket.url)
        .map_err(|e| {
            error!("Failed to parse WebSocket URL: {}", e);
            AppError::UrlParseError(e)
        })?;
    
    let ws_stream = connect_to_websocket(url).await?;
    info!("WebSocket connection established");
    
    // Create shared state for responses
    let state = Arc::new(Mutex::new(SharedState {
        responses: HashMap::new(),
    }));
    
    // Split the WebSocket stream
    let (write, read) = ws_stream.split();
    
    // Create CKB client with the WebSocket writer
    let ckb_client = CkbClient::new(write);
    
    // Spawn message handler task
    let state_clone = state.clone();
    info!("Starting WebSocket message handler task");
    let receive_handle = tokio::spawn(async move {
        handle_websocket_messages(read, state_clone).await
    });
    
    // Get the query interval from config
    let query_interval = tokio::time::Duration::from_secs(config.ckb.query_interval_secs);
    info!(interval_secs = %config.ckb.query_interval_secs, "Query interval configured");
    
    // Loop forever, running queries at the specified interval
    let ckb_config = config.ckb.clone();
    let state_clone = state.clone();
    let query_loop_handle = tokio::spawn(async move {
        // Small delay to ensure WebSocket connection is ready
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        
        loop {
            info!("Starting query cycle");
            
            // Run a complete query
            match query_cells_ws(
                ckb_client.clone(),
                ckb_config.type_script_code_hash.clone(),
                ckb_config.type_script_hash_type.clone(), 
                ckb_config.query_limit,
                state_clone.clone()
            ).await {
                Ok(()) => {
                    info!("Query cycle completed successfully");
                },
                Err(e) => {
                    error!("Query cycle failed: {}", e);
                    // We don't exit on error, just continue with the next cycle
                }
            }
            
            // Wait for the configured interval before starting the next query
            info!(sleep_secs = %query_interval.as_secs(), "Sleeping until next query cycle");
            tokio::time::sleep(query_interval).await;
        }
    });
    
    // Create a future that completes when CTRL+C is pressed
    let ctrl_c = tokio::signal::ctrl_c();
    
    // Wait for the query loop, the WebSocket connection to close, or CTRL+C
    info!("Waiting for tasks to complete or shutdown signal");
    tokio::select! {
        _ = query_loop_handle => {
            error!("Query loop exited unexpectedly");
            return Err(AppError::MessageHandlingError("Query loop exited unexpectedly".into()));
        }
        receive_result = receive_handle => {
            match receive_result {
                Ok(Ok(())) => {
                    info!("WebSocket connection closed gracefully");
                }
                Ok(Err(e)) => {
                    warn!("WebSocket handler exited with error: {}", e);
                    return Err(e);
                }
                Err(e) => {
                    error!("WebSocket handler task panicked: {}", e);
                    return Err(AppError::MessageHandlingError(format!("Task panic: {}", e)));
                }
            }
        }
        _ = ctrl_c => {
            info!("Received shutdown signal, gracefully terminating");
        }
    }
    
    info!("Application shutting down");
    Ok(())
}