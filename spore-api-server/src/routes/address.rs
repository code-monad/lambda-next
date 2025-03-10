use axum::{
    extract::{Path, Query, State},
    Json,
};
use lambda_next::db::DbSporeData;
use tracing::debug;

use crate::{
    network::NetworkType, 
    routes::{ApiError, ApiResponse, PaginationParams},
    AppState,
};

/// Get all DOB spores by address
pub async fn get_all_dob_by_address(
    Path(address): Path<String>,
    Query(params): Query<PaginationParams>,
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<DbSporeData>>>, ApiError> {
    debug!("Getting DOB spores for address: {}", address);
    
    // Parse network type if provided
    let network_type = if let Some(network) = &params.network {
        match NetworkType::parse(network) {
            Some(net) => Some(net.as_db_str()),
            None => return Err(ApiError::InvalidNetwork(network.clone().to_owned())),
        }
    } else {
        None
    };
    
    // Query the database with DOB filter and include DOB output
    let spores = state.db
        .get_spores_by_owner_filtered(
            &address,
            params.limit,
            params.offset,
            Some("dob"),
            true, // Include DOB output
            network_type,
        )
        .await?;
    
    // TODO: Get total count for pagination
    let total = spores.len() as i64; // Temporary placeholder
    
    // Return the results
    Ok(Json(ApiResponse::with_pagination(
        spores,
        total,
        params.limit,
        params.offset,
    )))
}

/// Get all standard spores by address (without DOB data)
pub async fn get_all_spore_by_address(
    Path(address): Path<String>,
    Query(params): Query<PaginationParams>,
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<DbSporeData>>>, ApiError> {
    debug!("Getting standard spores for address: {}", address);
    
    // Parse network type if provided
    let network_type = if let Some(network) = &params.network {
        match NetworkType::parse(network) {
            Some(net) => Some(net.as_db_str()),
            None => return Err(ApiError::InvalidNetwork(network.clone().to_owned())),
        }
    } else {
        None
    };
    
    // Query the database, including all content types but excluding DOB output
    let spores = state.db
        .get_spores_by_owner_filtered(
            &address,
            params.limit,
            params.offset,
            None, // All content types
            false, // Exclude DOB output
            network_type,
        )
        .await?;
    
    // TODO: Get total count for pagination
    let total = spores.len() as i64; // Temporary placeholder
    
    // Return the results
    Ok(Json(ApiResponse::with_pagination(
        spores,
        total,
        params.limit,
        params.offset,
    )))
} 