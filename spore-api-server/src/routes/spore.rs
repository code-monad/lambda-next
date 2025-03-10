use axum::{
    extract::{Path, Query, State},
    Json,
};
use lambda_next::db::DbSporeData;
use tracing::debug;

use crate::{
    network::NetworkType, 
    routes::{ApiError, QueryParams},
    routes::cluster::EnhancedSporeData,
    AppState,
};

/// Get spore by ID
pub async fn get_by_id(
    Path(id): Path<String>,
    Query(params): Query<QueryParams>,
    State(state): State<AppState>,
) -> Result<Json<EnhancedSporeData>, ApiError> {
    debug!("Getting spore: {}", id);
    
    // Parse network type if provided
    let network_type = if let Some(network) = &params.network {
        match NetworkType::parse(network) {
            Some(net) => Some(net.as_db_str()),
            None => return Err(ApiError::InvalidNetwork(network.clone())),
        }
    } else {
        None
    };
    
    // Query the database
    let spores = state.db
        .get_spore_by_id_with_network(
            &id,
            network_type,
        )
        .await?;
    
    // Check if any spore was found
    let spore = spores.into_iter().next()
        .ok_or_else(|| ApiError::NotFound(format!("Spore with ID '{}' not found", id)))?;
    
    // Convert to enhanced spore data with parsed render_output
    let enhanced_spore = EnhancedSporeData::from(spore);
    
    // Return the spore directly
    Ok(Json(enhanced_spore))
} 