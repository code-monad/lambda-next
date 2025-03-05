use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};
use tracing::{debug, error, info, instrument, trace};

use crate::ckb::CkbClient;
use crate::utils::{Result, SharedState, wait_for_response};

/// Query for cells using WebSocket with proper request tracking and paging
#[instrument(skip(client, state), fields(code_hash = %type_script_code_hash, hash_type = %hash_type, limit = %limit))]
pub async fn query_cells_ws(
    client: CkbClient,
    type_script_code_hash: String,
    hash_type: String,
    limit: u32,
    state: Arc<Mutex<SharedState>>,
) -> Result<()> {
    // Start timing the query cycle
    let query_start_time = Instant::now();

    info!("Starting cell query operation");
    let mut cursor: Option<String> = None;
    let mut total_cells = 0;
    let mut page_count = 0;

    loop {
        info!(cursor = ?cursor, "Querying for cells");

        // Make the RPC call
        let request = client
            .get_cells_by_type_script_code_hash(
                &type_script_code_hash,
                &hash_type,
                limit,
                cursor.as_deref(),
            )
            .await?;

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
                return Err(e.into());
            }
        };

        // no cells means we're done
        if cells.is_empty() {
            info!("No cells found in response, ending query");
            break;
        }

        // Increment page counter
        page_count += 1;

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
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Calculate and log the total query time
    let query_duration = query_start_time.elapsed();

    info!(
        total_cells = %total_cells,
        pages = %page_count,
        duration_ms = %query_duration.as_millis(),
        duration_secs = %query_duration.as_secs_f64(),
        cells_per_second = %(if query_duration.as_secs_f64() > 0.0 { total_cells as f64 / query_duration.as_secs_f64() } else { 0.0 }),
        "Cell query operation completed"
    );

    Ok(())
}
