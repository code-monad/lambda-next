use serde::Deserialize;
use std::error::Error;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub websocket: WebSocketConfig,
    pub ckb: CkbConfig,
    pub spore_filters: Vec<SporeFilterConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WebSocketConfig {
    pub url: String,
    pub reconnect_interval: u64,
    pub ping_interval: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CkbConfig {
    pub type_script_code_hash: String,
    pub type_script_hash_type: String,
    pub query_limit: u32,
    pub query_interval_secs: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SporeFilterConfig {
    pub name: String,
    pub enabled: bool,
    pub filter_by_cluster: bool,
    pub cluster_id: String,
    pub filter_by_type_ids: bool,
    pub type_ids: Vec<String>,
    pub type_ids_file: Option<String>,
    pub skip_decoding: bool,
}

impl Config {
    pub fn load() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let config_builder = config::Config::builder()
            .add_source(config::File::with_name("config/default.toml"))
            .add_source(config::File::with_name("config/local.toml").required(false))
            .add_source(config::Environment::with_prefix("LAMBDA_NEXT").separator("__"));

        let config = config_builder.build()?;
        Ok(config.try_deserialize()?)
    }
}
