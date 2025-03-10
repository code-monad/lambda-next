use axum::{
    extract::{Path, Query, State},
    Json,
};
use lambda_next::db::{DbSporeData, ServerDecodeResult};
use tracing::{debug, warn};
use serde_json::Value;
use serde::{Serialize, Serializer, ser::SerializeStruct};

use crate::{
    network::NetworkType, 
    routes::{ApiError, QueryParams},
    AppState,
};

/// Enhanced Spore Data with render_output as JSON
#[derive(Debug)]
pub struct EnhancedSporeData {
    data: DbSporeData,
    parsed_json: Option<Value>,
}

impl From<DbSporeData> for EnhancedSporeData {
    fn from(spore: DbSporeData) -> Self {
        // If there's DOB output, try to parse the render_output as JSON
        let parsed_json = if let Some(ref dob_output) = spore.dob_decode_output {
            match serde_json::from_str::<Value>(&dob_output.render_output) {
                Ok(json) => Some(json),
                Err(e) => {
                    warn!("Failed to parse render_output as JSON: {}", e);
                    None
                }
            }
        } else {
            None
        };
        
        Self {
            data: spore,
            parsed_json,
        }
    }
}

// Custom serializer to modify the output format
impl Serialize for EnhancedSporeData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Create a base structure with all fields from the original data
        let mut state = serializer.serialize_struct("EnhancedSporeData", 10)?;
        
        // Serialize all base fields
        state.serialize_field("id", &self.data.id)?;
        state.serialize_field("cluster_id", &self.data.cluster_id)?;
        state.serialize_field("content_type", &self.data.content_type)?;
        state.serialize_field("content", &self.data.content)?;
        state.serialize_field("tx_hash", &self.data.tx_hash)?;
        state.serialize_field("index", &self.data.index)?;
        state.serialize_field("owner", &self.data.owner)?;
        state.serialize_field("capacity", &self.data.capacity)?;
        state.serialize_field("network_type", &self.data.network_type)?;
        
        // If dob_decode_output exists, create a modified version with JSON render_output
        if let Some(ref dob_output) = self.data.dob_decode_output {
            let modified_dob = CustomDobOutput {
                render_output: self.parsed_json.clone().unwrap_or_else(|| {
                    // If parsing failed, use the original string as Value
                    Value::String(dob_output.render_output.clone())
                }),
                dob_content: dob_output.dob_content.clone(),
            };
            state.serialize_field("dob_decode_output", &modified_dob)?;
        }
        
        state.end()
    }
}

// Helper struct for serializing the modified DOB output
#[derive(Serialize)]
struct CustomDobOutput {
    render_output: Value,
    dob_content: Value,
}

/// Get all spores by cluster ID
pub async fn get_all_by_cluster(
    Path(cluster_id): Path<String>,
    Query(params): Query<QueryParams>,
    State(state): State<AppState>,
) -> Result<Json<Vec<EnhancedSporeData>>, ApiError> {
    debug!("Getting spores for cluster: {}", cluster_id);
    
    // Parse network type if provided
    let network_type = if let Some(network) = &params.network {
        match NetworkType::parse(network) {
            Some(net) => Some(net.as_db_str()),
            None => return Err(ApiError::InvalidNetwork(network.clone())),
        }
    } else {
        None
    };
    
    // Query the database - using a high limit value since pagination is removed
    let spores = state.db
        .get_spores_by_cluster_with_network(
            &cluster_id,
            500, // Higher limit since no pagination
            0,   // No offset
            network_type,
        )
        .await?;
    
    // Convert DbSporeData to EnhancedSporeData
    let enhanced_spores = spores.into_iter()
        .map(EnhancedSporeData::from)
        .collect();
    
    // Return the results as a direct array
    Ok(Json(enhanced_spores))
} 