use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};
use tracing::{debug, error, info, instrument, trace, warn};

use crate::ckb::CkbClient;
use crate::config::Config;
use crate::spore::process_cells_with_filters;
use crate::utils::{Result, SharedState, wait_for_response};
use crate::db::SporeDb;

/// Query for cells using WebSocket with proper request tracking and paging
#[instrument(skip(client, state, config, db), fields(code_hash = %type_script_code_hash, hash_type = %hash_type, limit = %limit))]
pub async fn query_cells_ws(
    client: CkbClient,
    type_script_code_hash: String,
    hash_type: String,
    limit: u32,
    state: Arc<Mutex<SharedState>>,
    config: Config,
    db: Option<Arc<SporeDb>>,
) -> Result<()> {
    // Start timing the query cycle
    let query_start_time = Instant::now();

    info!("Starting cell query operation");
    let mut cursor: Option<String> = None;
    let mut total_cells = 0;
    let mut page_count = 0;

    loop {
        // Start timing this specific page query
        let page_start_time = Instant::now();
        debug!(
            cursor = cursor.as_deref().unwrap_or("null"),
            "Querying cells with cursor"
        );

        // Send request to get cells by type script
        let response = client
            .get_cells_by_type_script_code_hash(
                &type_script_code_hash,
                &hash_type,
                limit,
                cursor.as_deref(),
            )
            .await?;
            
        // Extract the request ID from the response
        let request_id = response
            .get("id")
            .and_then(|id| id.as_u64())
            .expect("Response should contain a request ID");

        // Wait for the response with our request ID
        let response = wait_for_response(request_id, state.clone()).await?;

        // Parse the cells from the response
        let (cells, next_cursor) = client.parse_cells_response(&response)?;
        let cells_count = cells.len();
        page_count += 1;
        total_cells += cells_count;

        let page_duration = page_start_time.elapsed();
        debug!(
            page = page_count,
            cells_count = cells_count,
            total_cells = total_cells,
            duration_ms = %page_duration.as_millis(),
            "Retrieved page of cells"
        );

        // Process the cells
        if !cells.is_empty() {
            debug!("Processing {} cells", cells.len());
            
            // Apply filters and process cells with database connection
            let db_ref = db.as_ref();
            process_cells_with_filters(&cells, &config.spore_filters, &config.ckb, db_ref).await?;
        }

        // Check if we have more pages
        cursor = next_cursor;
        if cursor.is_none() || cells_count == 0 {
            break;
        }
    }

    let total_duration = query_start_time.elapsed();
    info!(
        total_cells = total_cells,
        pages = page_count,
        total_duration_ms = %total_duration.as_millis(),
        cells_per_second = %(if total_duration.as_secs_f64() > 0.0 { total_cells as f64 / total_duration.as_secs_f64() } else { 0.0 }) as u64,
        "Query operation completed"
    );

    Ok(())
}
