use axum::{
    extract::{Path, Query, State},
    Json,
};
use lambda_next::db::DbSporeData;
use tracing::debug;

use crate::{
    network::NetworkType, 
    routes::{ApiError, QueryParams, ClusterFilter},
    routes::cluster::EnhancedSporeData,
    AppState,
};

/// Get all DOB spores by address
pub async fn get_all_dob_by_address(
    Path(address): Path<String>,
    Query(params): Query<QueryParams>,
    State(state): State<AppState>,
) -> Result<Json<Vec<EnhancedSporeData>>, ApiError> {
    debug!("Getting DOB spores for address: {}", address);
    
    // Parse network type if provided
    let network_type = if let Some(network) = &params.network {
        match NetworkType::parse(network) {
            Some(net) => Some(net.as_db_str()),
            None => return Err(ApiError::InvalidNetwork(network.clone())),
        }
    } else {
        None
    };
    
    // Query the database with DOB filter and include DOB output
    let spores = state.db
        .get_spores_by_owner_filtered(
            &address,
            500, // Higher limit since no pagination
            0,   // No offset
            Some("dob"),
            true, // Include DOB output
            network_type,
        )
        .await?;
    
    // Convert to enhanced spore data
    let enhanced_spores = spores.into_iter()
        .map(EnhancedSporeData::from)
        .collect();
    
    // Return the results as a direct array
    Ok(Json(enhanced_spores))
}

/// Get all standard spores by address (without DOB data)
pub async fn get_all_spore_by_address(
    Path(address): Path<String>,
    Query(params): Query<QueryParams>,
    State(state): State<AppState>,
) -> Result<Json<Vec<EnhancedSporeData>>, ApiError> {
    debug!("Getting standard spores for address: {}", address);
    
    // Parse network type if provided
    let network_type = if let Some(network) = &params.network {
        match NetworkType::parse(network) {
            Some(net) => Some(net.as_db_str()),
            None => return Err(ApiError::InvalidNetwork(network.clone())),
        }
    } else {
        None
    };
    
    // Query the database, including all content types but excluding DOB output
    let spores = state.db
        .get_spores_by_owner_filtered(
            &address,
            500, // Higher limit since no pagination
            0,   // No offset
            None, // All content types
            false, // Exclude DOB output
            network_type,
        )
        .await?;
    
    // Convert to enhanced spore data
    let enhanced_spores = spores.into_iter()
        .map(EnhancedSporeData::from)
        .collect();
    
    // Return the results as a direct array
    Ok(Json(enhanced_spores))
}

/// Get all spores by address (both DOB and regular)
pub async fn get_all_by_address(
    Path(address): Path<String>,
    Query(params): Query<QueryParams>,
    State(state): State<AppState>,
) -> Result<Json<Vec<EnhancedSporeData>>, ApiError> {
    debug!("Getting all spores for address: {}", address);
    
    // Parse network type if provided
    let network_type = if let Some(network) = &params.network {
        match NetworkType::parse(network) {
            Some(net) => Some(net.as_db_str()),
            None => return Err(ApiError::InvalidNetwork(network.clone())),
        }
    } else {
        None
    };
    
    // Query the database, including all content types and DOB output
    let spores = state.db
        .get_spores_by_owner_filtered(
            &address,
            500, // Higher limit since no pagination
            0,   // No offset
            None, // All content types
            true, // Include DOB output
            network_type,
        )
        .await?;
    
    // Convert to enhanced spore data
    let enhanced_spores = spores.into_iter()
        .map(EnhancedSporeData::from)
        .collect();
    
    // Return the results as a direct array
    Ok(Json(enhanced_spores))
}

/// POST endpoint to get all spores by address, filtered by cluster IDs
pub async fn post_all_by_address(
    Path(address): Path<String>,
    Query(params): Query<QueryParams>,
    State(state): State<AppState>,
    Json(cluster_filter): Json<ClusterFilter>,
) -> Result<Json<Vec<EnhancedSporeData>>, ApiError> {
    debug!("Getting all spores for address: {} with cluster filter: {:?}", address, cluster_filter.0);
    
    // Parse network type if provided
    let network_type = if let Some(network) = &params.network {
        match NetworkType::parse(network) {
            Some(net) => Some(net.as_db_str()),
            None => return Err(ApiError::InvalidNetwork(network.clone())),
        }
    } else {
        None
    };
    
    // Query the database with the cluster filter directly
    let spores = state.db
        .get_spores_by_owner_with_clusters(
            &address,
            500, // Higher limit since no pagination
            0,   // No offset
            None, // All content types
            true, // Include DOB output
            network_type,
            &cluster_filter.0, // Pass cluster IDs for filtering at the database level
        )
        .await?;
    
    // Convert to enhanced spore data
    let enhanced_spores = spores.into_iter()
        .map(EnhancedSporeData::from)
        .collect();
    
    // Return the results as a direct array
    Ok(Json(enhanced_spores))
}

/// POST endpoint to get all DOB spores by address, filtered by cluster IDs
pub async fn post_all_dob_by_address(
    Path(address): Path<String>,
    Query(params): Query<QueryParams>,
    State(state): State<AppState>,
    Json(cluster_filter): Json<ClusterFilter>,
) -> Result<Json<Vec<EnhancedSporeData>>, ApiError> {
    debug!("Getting DOB spores for address: {} with cluster filter: {:?}", address, cluster_filter.0);
    
    // Parse network type if provided
    let network_type = if let Some(network) = &params.network {
        match NetworkType::parse(network) {
            Some(net) => Some(net.as_db_str()),
            None => return Err(ApiError::InvalidNetwork(network.clone())),
        }
    } else {
        None
    };
    
    // Query the database with the cluster filter directly
    let spores = state.db
        .get_spores_by_owner_with_clusters(
            &address,
            500, // Higher limit since no pagination
            0,   // No offset
            Some("dob"), // Only DOB content types
            true, // Include DOB output
            network_type,
            &cluster_filter.0, // Pass cluster IDs for filtering at the database level
        )
        .await?;
    
    // Convert to enhanced spore data
    let enhanced_spores = spores.into_iter()
        .map(EnhancedSporeData::from)
        .collect();
    
    // Return the results as a direct array
    Ok(Json(enhanced_spores))
}

/// POST endpoint to get all standard spores by address, filtered by cluster IDs
pub async fn post_all_spore_by_address(
    Path(address): Path<String>,
    Query(params): Query<QueryParams>,
    State(state): State<AppState>,
    Json(cluster_filter): Json<ClusterFilter>,
) -> Result<Json<Vec<EnhancedSporeData>>, ApiError> {
    debug!("Getting standard spores for address: {} with cluster filter: {:?}", address, cluster_filter.0);
    
    // Parse network type if provided
    let network_type = if let Some(network) = &params.network {
        match NetworkType::parse(network) {
            Some(net) => Some(net.as_db_str()),
            None => return Err(ApiError::InvalidNetwork(network.clone())),
        }
    } else {
        None
    };
    
    // Query the database with the cluster filter directly
    let spores = state.db
        .get_spores_by_owner_with_clusters(
            &address,
            500, // Higher limit since no pagination
            0,   // No offset
            None, // All content types
            false, // Exclude DOB output
            network_type,
            &cluster_filter.0, // Pass cluster IDs for filtering at the database level
        )
        .await?;
    
    // Convert to enhanced spore data
    let enhanced_spores = spores.into_iter()
        .map(EnhancedSporeData::from)
        .collect();
    
    // Return the results as a direct array
    Ok(Json(enhanced_spores))
} 