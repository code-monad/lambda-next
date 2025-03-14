use anyhow::{anyhow, Result};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use lambda_next::db::{DbConfig, DbSporeData, SporeDb};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::{net::SocketAddr, sync::Arc};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{debug, error, info, warn};

mod network;
mod routes;

use network::NetworkType;
use routes::{cluster, spore, address};

/// Spore API server configuration
#[derive(Debug, Deserialize)]
struct ApiConfig {
    /// Server bind address
    address: String,
    /// Server port
    port: u16,
    /// Database configuration from main app
    database: DbConfig,
}

/// Command line arguments
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Configuration file
    #[arg(short, long, default_value = "config/default.toml")]
    config: String,
}

/// Application state shared with handlers
#[derive(Clone)]
struct AppState {
    db: Arc<SporeDb>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();
    info!("Starting Spore API server");

    // Parse command line arguments
    let args = Args::parse();
    info!("Using configuration file: {}", args.config);

    // Load configuration
    let config_str = std::fs::read_to_string(&args.config)
        .map_err(|e| anyhow!("Failed to read config file: {}", e))?;
    
    let config = config::Config::builder()
        .add_source(config::File::from_str(&config_str, config::FileFormat::Toml))
        .build()
        .map_err(|e| anyhow!("Failed to parse config: {}", e))?;
    
    let api_config: ApiConfig = config.get("api")
        .map_err(|e| anyhow!("Missing 'api' section in config! {}", e))?;
    
    // Initialize database connection
    let db = SporeDb::new(&api_config.database)
        .await
        .map_err(|e| anyhow!("Failed to connect to database: {}", e))?;
    
    // Initialize database schema
    db.initialize()
        .await
        .map_err(|e| anyhow!("Failed to initialize database: {}", e))?;
    
    // Create application state
    let state = AppState {
        db: Arc::new(db),
    };
    
    // Create router with routes
    let app = Router::new()
        // Cluster routes
        .route("/cluster/:cluster_id/all", get(cluster::get_all_by_cluster))
        // Spore routes - GET and POST
        .route("/spore/:id", get(spore::get_by_id))
        // Address routes - GET
        .route("/address/:address/all", get(address::get_all_by_address))
        .route("/address/:address/dob/all", get(address::get_all_dob_by_address))
        .route("/address/:address/spore/all", get(address::get_all_spore_by_address))
        .route("/address/:address/spores/all", get(address::get_all_spore_by_address))
        // name aliases
        .route("/address/:address/dob", get(address::get_all_dob_by_address))
        .route("/address/:address/spore", get(address::get_all_spore_by_address))
        .route("/address/:address/spores", get(address::get_all_spore_by_address))
        // Address routes - POST
        .route("/address/:address/all", post(address::post_all_by_address))
        .route("/address/:address/dob/all", post(address::post_all_dob_by_address))
        .route("/address/:address/spore/all", post(address::post_all_spore_by_address))
        .route("/address/:address/spores/all", post(address::post_all_spore_by_address))
        // name aliases
        .route("/address/:address/dob", post(address::post_all_dob_by_address))
        .route("/address/:address/spore", post(address::post_all_spore_by_address))
        .route("/address/:address/spores", post(address::post_all_spore_by_address))
        // Add state and middleware
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive());
    
    // Bind to address and start server
    let addr: SocketAddr = format!("{}:{}", api_config.address, api_config.port)
        .parse()
        .map_err(|e| anyhow!("Invalid server address: {}", e))?;
    
    info!("Listening on {}", addr);
    
    // Updated server binding for axum 0.7
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    
    Ok(())
} 