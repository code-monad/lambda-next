use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::error;

pub mod cluster;
pub mod spore;

/// Query parameters for API requests
#[derive(Deserialize, Default)]
pub struct QueryParams {
    pub network: Option<String>,
}

/// API Error types
#[derive(Error, Debug)]
pub enum ApiError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    
    #[error("Invalid network type: {0}")]
    InvalidNetwork(String),
    
    #[error("Not found: {0}")]
    NotFound(String),
    
    #[error("JSON parsing error: {0}")]
    JsonParsingError(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            ApiError::Database(err) => {
                error!("Database error: {}", err);
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Database error: {}", err))
            }
            ApiError::InvalidNetwork(network) => (
                StatusCode::BAD_REQUEST,
                format!("Invalid network type: {}", network),
            ),
            ApiError::NotFound(resource) => (
                StatusCode::NOT_FOUND,
                resource,
            ),
            ApiError::JsonParsingError(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to parse JSON: {}", err),
            ),
        };

        // Return a JSON response with the error message
        let body = Json(serde_json::json!({
            "error": error_message
        }));

        (status, body).into_response()
    }
} 