use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};
use tokio::time::{self, Duration, Instant};
use tracing::{debug, error, info, instrument, trace, warn};
use anyhow::Result;
use ckb_jsonrpc_types::Script;
use ckb_types::H256;
use hex;

use crate::ckb::CkbClient;
use crate::config::Config;
use crate::spore::{self, process_cells_with_filters};
use crate::utils::{SharedState, wait_for_response};
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
            process_cells_with_filters(cells.as_slice(), &config.spore_filters, &config.ckb, db_ref).await
                .map_err(|e| anyhow::anyhow!("Spore error: {}", e))?;
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

#[instrument(skip_all)]
pub async fn run_query_loop(
    state: Arc<Mutex<SharedState>>,
    ckb_client: CkbClient,
    config: &Config,
    db: Option<Arc<SporeDb>>,
    shutdown_receiver: &mut broadcast::Receiver<()>,
) -> Result<()> {
    let mut interval = time::interval(Duration::from_secs(config.ckb.query_interval_secs));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let filters = &config.spore_filters;
                for filter in filters {
                    if !filter.enabled {
                        continue;
                    }
                    info!("Running query for filter: {}", filter.name);
                    let script = Script::new(
                        H256::from_slice(&hex::decode(config.ckb.type_script_code_hash.trim_start_matches("0x")).unwrap()).unwrap(),
                        config.ckb.type_script_hash_type.as_str().into(),
                        None,
                    );
                    let cells = ckb_client.get_cells_by_type_script(&script, config.ckb.query_limit).await?;
                    let processed_cells = spore::apply_filter(cells, filter, config.ckb.network_type.clone().into());
                    spore::process_spore_cells(processed_cells, db.as_deref()).await?;
                }
            }
            _ = shutdown_receiver.recv() => {
                warn!("Query loop received shutdown signal, terminating.");
                break;
            }
        }
    }
    Ok(())
}
