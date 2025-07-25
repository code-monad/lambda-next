use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tracing::{debug, error, instrument};

use crate::ckb::CkbError;

// Shared state for responses
pub struct SharedState {
    pub responses: HashMap<u64, Value>,
}

impl SharedState {
    pub fn new() -> Self {
        Self {
            responses: HashMap::new(),
        }
    }
}

/// Helper function to wait for a response with a specific ID
#[instrument(skip(state), fields(request_id = %request_id))]
pub async fn wait_for_response(request_id: u64, state: Arc<Mutex<SharedState>>) -> Result<Value> {
    debug!("Waiting for response");

    // Maximum time to wait for a response
    let timeout = Duration::from_secs(30);
    let start = tokio::time::Instant::now();

    while start.elapsed() < timeout {
        // Check if we have a response for this ID
        let response = {
            let mut state_guard = state.lock().await;
            state_guard.responses.remove(&request_id)
        };

        if let Some(response) = response {
            debug!(
                "Response received within {} ms",
                start.elapsed().as_millis()
            );
            return Ok(response);
        }

        // Wait a bit before checking again
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let timeout_error = anyhow!(
        "Timeout waiting for response to request ID: {}",
        request_id
    );
    error!(
        "Response timeout after {} ms: {}",
        timeout.as_millis(),
        timeout_error
    );
    Err(timeout_error)
}
