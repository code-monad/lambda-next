use futures::StreamExt;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep, timeout};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, instrument, warn};
use url::Url;

use crate::config::WebSocketConfig;
use crate::utils::{AppError, Result, SharedState};

/// Check if an error is an "unexpected EOF" which is common for WebSocket disconnections
pub fn is_eof_error(err: &tokio_tungstenite::tungstenite::Error) -> bool {
    let err_str = err.to_string();
    err_str.contains("unexpected EOF")
        || err_str.contains("connection closed normally")
        || err_str.contains("protocol error")
        || err_str.contains("Connection reset")
}

/// Connect to a WebSocket server with automatic reconnection
#[instrument(skip(config), fields(url = %url))]
pub async fn connect_with_retry(
    url: Url,
    config: WebSocketConfig,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    let mut retry_interval = Duration::from_secs(config.reconnect_interval);
    let mut attempts = 0;

    loop {
        attempts += 1;
        info!(
            "Connecting to WebSocket server at {} (attempt {})",
            url, attempts
        );

        match connect_to_websocket(&url).await {
            Ok(ws_stream) => {
                info!(
                    "WebSocket connection established after {} attempts",
                    attempts
                );
                return Ok(ws_stream);
            }
            Err(e) => {
                if attempts > 5 {
                    error!("Failed to connect after {} attempts: {}", attempts, e);
                    return Err(e);
                }

                warn!(
                    "Connection attempt {} failed: {}. Retrying in {} seconds",
                    attempts,
                    e,
                    retry_interval.as_secs()
                );

                sleep(retry_interval).await;

                // Exponential backoff with a cap
                retry_interval = std::cmp::min(
                    retry_interval * 2,
                    Duration::from_secs(60), // Maximum retry interval of 60 seconds
                );
            }
        }
    }
}

/// Connect to a WebSocket server
#[instrument(fields(url = %url))]
pub async fn connect_to_websocket(
    url: &Url,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    match url.scheme() {
        "ws" | "wss" => {
            // Both secure and non-secure connections are handled automatically by connect_async
            let connect_future = connect_async(url.as_str());

            // Add a timeout to the connection attempt
            match timeout(Duration::from_secs(10), connect_future).await {
                Ok(result) => match result {
                    Ok((ws_stream, _)) => {
                        info!("WebSocket connection established");
                        Ok(ws_stream)
                    }
                    Err(e) => {
                        error!("WebSocket connection error: {}", e);
                        Err(AppError::WebSocketError(e))
                    }
                },
                Err(_) => {
                    let error_msg = format!("Connection to {} timed out after 10 seconds", url);
                    error!("{}", error_msg);
                    Err(AppError::MessageHandlingError(error_msg))
                }
            }
        }
        s => {
            let error = AppError::InvalidScheme(s.to_string());
            error!("Invalid URL scheme: {}", s);
            Err(error)
        }
    }
}

/// Handle incoming WebSocket messages and store responses
#[instrument(skip(read, state))]
pub async fn handle_websocket_messages(
    mut read: futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    state: Arc<Mutex<SharedState>>,
) -> Result<()> {
    info!("Starting WebSocket message handler");

    while let Some(message_result) = read.next().await {
        match message_result {
            Ok(msg) => {
                match msg {
                    Message::Text(text) => {
                        debug!("Received text message: {}", text);

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
                            }
                            Err(e) => {
                                warn!("Error parsing JSON message: {}", e);
                                // Continue processing other messages, don't exit on parse error
                            }
                        }
                    }
                    Message::Binary(data) => {
                        debug!("Received binary message of {} bytes", data.len());
                    }
                    Message::Ping(_) => {
                        // The WebSocket client should automatically respond with a pong
                    }
                    Message::Pong(_) => {
                        debug!("Received pong response");
                    }
                    Message::Close(frame) => {
                        info!("Received close frame: {:?}", frame);
                        return Ok(());
                    }
                    Message::Frame(_) => {
                        // low-level frame, don't need to handle it
                    }
                }
            }
            Err(e) => {
                if is_eof_error(&e) {
                    info!("WebSocket connection closed: {}", e);
                    // This is a normal disconnection, we should return Ok to signal
                    // that it was a clean shutdown rather than an error
                    return Ok(());
                } else {
                    error!("Error receiving WebSocket message: {}", e);
                    // Return the error for other types of failures
                    return Err(AppError::WebSocketError(e));
                }
            }
        }
    }

    info!("WebSocket connection closed");
    Ok(())
}
