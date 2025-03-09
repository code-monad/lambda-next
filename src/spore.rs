use molecule::prelude::*;
use serde_json::Value;
use spore_types::{NativeNFTData, generated::spore::SporeData};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use tracing::{debug, error, info, trace, warn};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use lazy_static::lazy_static;

// Add imports for CKB address handling
use ckb_jsonrpc_types::Script;
use ckb_types::{
    core::ScriptHashType,
    packed,
    prelude::*,
    H256,
};
use ckb_sdk::{
    Address,
    AddressPayload,
    NetworkType,
};

use crate::config::{SporeFilterConfig, CkbConfig};
use crate::utils::{AppError, Result};
use std::str::FromStr;
use crate::db::{DbSporeData, ServerDecodeResult};

/// Represents a Spore cell with its decoded data
#[derive(Debug, Clone)]
pub struct SporeCell {
    /// The raw cell data from CKB
    pub raw_cell: Value,
    /// The decoded Spore data
    pub spore_data: Option<NativeNFTData>,
    /// The cluster ID if present
    pub cluster_id: Option<String>,
    /// The Type ID of the cell
    pub type_id: Option<String>,
    /// The network type to use for address formatting
    pub network_type: NetworkType,
}

/// Cache for decoded Spore cells to prevent repeated decoding
pub struct SporeCache {
    /// Cached Spore data by Type ID
    cache: HashMap<String, NativeNFTData>,
}

impl SporeCache {
    /// Create a new empty cache
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Get cached Spore data if available
    pub fn get(&self, type_id: &str) -> Option<&NativeNFTData> {
        self.cache.get(type_id)
    }

    /// Store decoded Spore data in the cache
    pub fn insert(&mut self, type_id: String, data: NativeNFTData) {
        self.cache.insert(type_id, data);
    }

    /// Get the number of entries in the cache
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Remove an entry from the cache
    pub fn remove(&mut self, type_id: &str) -> Option<NativeNFTData> {
        self.cache.remove(type_id)
    }
}

// Global cache instance
lazy_static! {
    static ref SPORE_CACHE: Arc<StdMutex<SporeCache>> = Arc::new(StdMutex::new(SporeCache::new()));
}

// Global cache for Type IDs loaded from files
lazy_static! {
    static ref TYPE_ID_FILE_CACHE: StdMutex<HashMap<String, Vec<String>>> = StdMutex::new(HashMap::new());
}

/// Extract the Type ID from a cell
fn extract_type_id(cell: &Value) -> Option<String> {
    cell.get("output")
        .and_then(|output| output.get("type"))
        .and_then(|type_script| type_script.get("args"))
        .and_then(|args| args.as_str())
        .map(|args_str| {
            // Take the first 32 bytes (64 hex characters + 0x prefix)
            let args_str = args_str.trim_start_matches("0x");
            if args_str.len() >= 64 {
                format!("0x{}", &args_str[0..64])
            } else {
                format!("0x{}", args_str)
            }
        })
}

/// Decodes the molecule-encoded SporeData from a cell, using cache when available
pub fn decode_spore_data(cell: &Value) -> Result<NativeNFTData> {
    // Extract Type ID for cache lookup
    let type_id = extract_type_id(cell)
        .ok_or_else(|| AppError::SporeError("Cell has no Type ID for cache lookup".to_string()))?;

    // Try to get from cache first
    let cache = SPORE_CACHE.lock().unwrap();
    if let Some(cached_data) = cache.get(&type_id) {
        debug!("Cache hit for Type ID {}", type_id);
        return Ok(cached_data.clone());
    }
    drop(cache); // Release the lock before decoding

    // Get data field from the cell
    let data_hex = cell
        .get("output_data")
        .and_then(|data| data.as_str())
        .ok_or_else(|| AppError::SporeError("Cell has no data field".to_string()))?;

    // Remove 0x prefix if present
    let data_hex = data_hex.trim_start_matches("0x");

    // Convert hex to bytes
    let data = hex::decode(data_hex).map_err(|e| {
        error!("Failed to decode hex data: {}", e);
        AppError::SporeError(format!("Failed to decode hex data: {}", e))
    })?;

    // Parse SporeData from bytes
    let spore_data = SporeData::from_compatible_slice(&data).map_err(|e| {
        error!("Failed to parse SporeData: {}", e);
        AppError::SporeError(format!("Failed to parse SporeData: {}", e))
    })?;

    // Convert to native representation (manually)
    let native_data = NativeNFTData {
        content_type: String::from_utf8_lossy(spore_data.content_type().unpack()).to_string(),
        content: spore_data.content().unpack().to_vec(),
        cluster_id: spore_data
            .cluster_id()
            .to_opt()
            .map(|id| id.unpack().to_vec()),
    };

    // Store in cache using a clone of the type_id to avoid moving it
    let mut cache = SPORE_CACHE.lock().unwrap();
    cache.insert(type_id.clone(), native_data.clone());
    debug!(
        "Added cell to cache with Type ID {}, total cached: {}",
        type_id,
        cache.len()
    );

    Ok(native_data)
}

/// Extract cluster ID from a spore NFT data
pub fn extract_cluster_id(spore_data: &NativeNFTData) -> Option<String> {
    spore_data.cluster_id.as_ref().map(|id| {
        // Convert bytes to hex string
        format!("0x{}", hex::encode(id))
    })
}

/// Loads Type IDs from a file, supporting two formats:
/// 1. Plain text format: One Type ID per line, lines starting with # are comments
/// 2. JSON array format: Objects with typeInfo.args containing Type IDs
fn load_type_ids_from_file(file_path: &str) -> std::result::Result<Vec<String>, Box<dyn std::error::Error>> {
    // Check if we already have this file in cache
    {
        let cache = TYPE_ID_FILE_CACHE.lock().unwrap();
        if let Some(type_ids) = cache.get(file_path) {
            info!("Using cached Type IDs for file '{}' ({} entries)", file_path, type_ids.len());
            return Ok(type_ids.clone());
        }
    }
    
    // Not in cache, load from file
    info!("Loading Type IDs from file '{}' (first time)", file_path);
    let path = Path::new(file_path);
    let file = File::open(path)?;
    
    let type_ids = if file_path.to_lowercase().ends_with(".json") {
        info!("Detected JSON format for Type IDs file: {}", file_path);
        load_type_ids_from_json(file)?
    } else {
        // Use the existing plain text format parser
        load_type_ids_from_text(file)?
    };
    
    // Add to cache for future use
    {
        let mut cache = TYPE_ID_FILE_CACHE.lock().unwrap();
        cache.insert(file_path.to_string(), type_ids.clone());
        info!("Cached {} Type IDs from file '{}'", type_ids.len(), file_path);
    }
    
    Ok(type_ids)
}

/// Loads Type IDs from a plain text file (one per line, # for comments)
fn load_type_ids_from_text(file: File) -> std::result::Result<Vec<String>, Box<dyn std::error::Error>> {
    let reader = BufReader::new(file);
    let mut type_ids = Vec::new();
    
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        
        // Skip empty lines and comments
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            type_ids.push(trimmed.to_string());
        }
    }
    
    Ok(type_ids)
}

/// Loads Type IDs from a JSON file
/// Expects either an array of Type IDs as strings, or
/// an array of objects with typeInfo.args containing the Type IDs
fn load_type_ids_from_json(file: File) -> std::result::Result<Vec<String>, Box<dyn std::error::Error>> {
    let reader = BufReader::new(file);
    let json: Value = serde_json::from_reader(reader)?;
    
    let mut type_ids = Vec::new();
    
    // Check if it's a JSON array
    if let Some(array) = json.as_array() {
        for item in array {
            if let Some(type_id) = extract_type_id_from_json(item) {
                type_ids.push(type_id);
            }
        }
    } else if let Some(type_id) = extract_type_id_from_json(&json) {
        // Single object case
        type_ids.push(type_id);
    } else {
        return Err("Invalid JSON format: expected array of objects with typeInfo.args".into());
    }
    
    info!("Loaded {} Type IDs from JSON file", type_ids.len());
    Ok(type_ids)
}

/// Extracts a Type ID from a JSON object
/// Supports two formats:
/// 1. Direct string (the Type ID itself)
/// 2. Object with typeInfo.args field containing the Type ID
fn extract_type_id_from_json(item: &Value) -> Option<String> {
    // Case 1: If item is a string, use it directly
    if let Some(type_id) = item.as_str() {
        return Some(type_id.to_string());
    }
    
    
    
    // Case 2: Look for sporeId directly
    if let Some(spore_id) = item.get("sporeId") {
        if let Some(type_id) = spore_id.as_str() {
            return Some(type_id.to_string());
        }
    }

    // Case 3: Look for typeInfo.args structure
    if let Some(type_info) = item.get("typeInfo") {
        if let Some(args) = type_info.get("args") {
            if let Some(type_id) = args.as_str() {
                return Some(type_id.to_string());
            }
        }
    }
    
    None
}

/// Filter cells based on configured criteria
pub fn apply_filter(cells: Vec<Value>, filter: &SporeFilterConfig, network_type: NetworkType) -> Vec<SporeCell> {
    let cells_len = cells.len();
    debug!("Applying filter '{}' to {} cells", filter.name, cells_len);

    // Extract Type IDs from the filter
    let mut type_ids = filter.type_ids.clone();

    // Add Type IDs from file if configured
    if filter.filter_by_type_ids && filter.type_ids_file.is_some() {
        let file_path = filter.type_ids_file.as_ref().unwrap();
        if !file_path.is_empty() {
            match load_type_ids_from_file(file_path) {
                Ok(file_type_ids) => {
                    debug!("Adding {} Type IDs from file '{}'", file_type_ids.len(), file_path);
                    type_ids.extend(file_type_ids);
                }
                Err(e) => {
                    error!("Failed to load Type IDs from file '{}': {}", file_path, e);
                }
            }
        }
    }

    // Create a set for faster lookup of Type IDs
    let type_id_set: std::collections::HashSet<String> = if filter.filter_by_type_ids {
        type_ids.into_iter().collect()
    } else {
        std::collections::HashSet::new()
    };

    // Create a set for excluded cluster IDs for faster lookup
    let exclude_cluster_id_set: std::collections::HashSet<String> = 
        filter.exclude_cluster_ids.iter().cloned().collect();

    let cluster_id_filter = if filter.filter_by_cluster {
        Some(filter.cluster_id.clone())
    } else {
        None
    };

    let mut filtered_cells = Vec::new();

    // Process each cell
    for cell in cells {
        // Extract the Type ID from the cell
        let type_id = match extract_type_id(&cell) {
            Some(id) => id,
            None => {
                trace!("Cell has no Type ID, skipping");
                continue;
            }
        };

        // Check if this cell's Type ID is in our filter set
        let type_id_matches = if filter.filter_by_type_ids {
            type_id_set.contains(&type_id)
        } else {
            true
        };

        // Skip if Type ID doesn't match
        if !type_id_matches {
            continue;
        }

        // Decode the cell data
        let spore_data = if filter.skip_decoding {
            None
        } else {
            match decode_spore_data(&cell) {
                Ok(data) => Some(data),
                Err(e) => {
                    debug!("Failed to decode cell with Type ID {}: {}", type_id, e);
                    None
                }
            }
        };

        // Extract cluster ID from the data
        let cluster_id = match &spore_data {
            Some(data) => extract_cluster_id(data),
            None => None,
        };

        // Check if this cell's cluster ID is in the exclude list
        if let Some(cell_cluster_id) = &cluster_id {
            if !exclude_cluster_id_set.is_empty() && exclude_cluster_id_set.contains(cell_cluster_id) {
                trace!("Skipping cell with excluded cluster ID: {}", cell_cluster_id);
                continue;
            }
        }

        // Check if this cell's cluster ID matches our filter
        let cluster_id_matches = if let Some(filter_cluster_id) = &cluster_id_filter {
            if let Some(cell_cluster_id) = &cluster_id {
                cell_cluster_id == filter_cluster_id
            } else {
                false
            }
        } else {
            true
        };

        // Skip if cluster ID doesn't match
        if !cluster_id_matches {
            continue;
        }

        // Add the matching cell to our result set
        filtered_cells.push(SporeCell {
            raw_cell: cell,
            spore_data,
            cluster_id,
            type_id: Some(type_id),
            network_type,
        });
    }

    if !filtered_cells.is_empty() {
        trace!(
            "Filter '{}' selected {} cells out of {}",
            filter.name,
            filtered_cells.len(),
            cells_len
        );
    }

    filtered_cells
}

/// Process all cells using all enabled filters
pub async fn process_cells_with_filters(
    cells: &[Value],
    filters: &[SporeFilterConfig],
    ckb_config: &CkbConfig,
    db: Option<&Arc<crate::db::SporeDb>>,
) -> Result<()> {
    if cells.is_empty() {
        info!("No cells to process");
        return Ok(());
    }

    // Get only enabled filters
    let enabled_filters: Vec<_> = filters.iter().filter(|f| f.enabled).collect();
    if enabled_filters.is_empty() {
        info!("No enabled filters found, skipping processing");
        return Ok(());
    }

    debug!(
        "Processing {} cells with {} enabled filters",
        cells.len(),
        enabled_filters.len()
    );

    // Get the configured network type
    let network_type: NetworkType = ckb_config.network_type.clone().into();
    debug!("Using network type: {:?}", network_type);

    // Process each filter independently
    for filter in enabled_filters {
        debug!("Applying filter: {}", filter.name);

        // Create a fresh copy of the cells for each filter
        // This avoids the borrow after move issue
        let cells_for_filter = cells.to_vec();

        // Apply the filter
        let filtered_cells = apply_filter(cells_for_filter, filter, network_type);

        if !filtered_cells.is_empty() {
            trace!(
                "Filter '{}' selected {} cells for processing",
                filter.name,
                filtered_cells.len()
            );

            // Process the filtered cells
            process_spore_cells(filtered_cells, db).await?;
        } else {
            debug!("No cells matched filter: {}", filter.name);
        }
    }

    Ok(())
}

/// Process a single spore cell
///
/// This function is a placeholder that should be customized based on specific requirements.
/// The user should implement this function to perform their desired operations on each spore cell.
pub async fn process_single_spore(cell: &SporeCell, db: Option<&Arc<crate::db::SporeDb>>) -> Result<()> {
    debug!("Processing spore cell: {}", cell.type_id.as_deref().unwrap_or("None"));
    trace!("Spore data: {:?}", cell.spore_data);
    trace!("Cluster ID: {:?}", cell.cluster_id);
    trace!("Type ID: {:?}", cell.type_id);
    trace!("Raw cell: {:?}", cell.raw_cell);
    trace!("--------------------------------");

    // Convert to SporeData and store in database if DB is configured
    if let Some(db) = db {
        if let Some(spore_data) = cell.to_spore_data() {
            debug!("Storing spore in database: {}", spore_data.id);
            if let Err(e) = db.upsert_spore(&spore_data).await {
                error!("Failed to store spore in database: {}", e);
            }
        } else {
            error!("Could not convert spore cell to database format");
        }
    }

    Ok(())
}

/// Process a list of spore cells
///
/// This function is a placeholder for user-defined processing logic
pub async fn process_spore_cells(cells: Vec<SporeCell>, db: Option<&Arc<crate::db::SporeDb>>) -> Result<()> {
    info!("Processing {} spore cells", cells.len());

    for (index, cell) in cells.iter().enumerate() {
        match &cell.spore_data {
            Some(data) => {
                debug!(
                    "Cell {}: Type ID: {}, Content Type: {}, Cluster ID: {}",
                    index,
                    cell.type_id.as_deref().unwrap_or("None"),
                    data.content_type,
                    cell.cluster_id.as_deref().unwrap_or("None")
                );

                // Process the individual cell
                if let Err(e) = process_single_spore(cell, db).await {
                    warn!("Error processing spore cell {}: {}", index, e);
                    // Continue processing other cells
                }
            }
            None => {
                if let Some(type_id) = &cell.type_id {
                    warn!(
                        "Cell {} with Type ID {} could not be decoded as a valid spore",
                        index, type_id
                    );
                } else {
                    warn!("Cell {} could not be decoded as a valid spore", index);
                }
            }
        }
    }

    Ok(())
}

/// Initialize the Type ID cache by preloading all files specified in filter configurations
pub fn preload_type_id_files(filters: &[SporeFilterConfig]) -> Result<()> {
    for filter in filters {
        if let Some(file_path) = &filter.type_ids_file {
            if !file_path.is_empty() {
                match load_type_ids_from_file(file_path) {
                    Ok(type_ids) => {
                        info!("Preloaded {} Type IDs from file '{}'", type_ids.len(), file_path);
                    }
                    Err(err) => {
                        error!("Failed to preload Type IDs from file '{}': {}", file_path, err);
                        // Convert the box error to AppError
                        return Err(AppError::SporeError(format!("Failed to load Type IDs from {}: {}", file_path, err)));
                    }
                }
            }
        }
    }
    Ok(())
}

impl SporeCell {
    /// Convert SporeCell to SporeData for database storage
    pub fn to_spore_data(&self) -> Option<DbSporeData> {
        // Log the starting point
        debug!("Starting to_spore_data conversion");
        
        // Check if type_id exists
        let type_id = match self.type_id.as_ref() {
            Some(id) => id,
            None => {
                error!("Missing type_id in SporeCell");
                return None;
            }
        };
        debug!("Processing cell with type_id: {}", type_id);
        
        let raw_cell = &self.raw_cell;
        
        // Extract required fields from the raw cell
        debug!("Extracting output field");
        let output = match raw_cell.get("output") {
            Some(o) => o,
            None => {
                error!("Missing 'output' field in cell: {:?}", raw_cell);
                return None;
            }
        };
        
        // Get out_point for tx_hash and index
        debug!("Extracting out_point field");
        let out_point = match raw_cell.get("out_point") {
            Some(op) => op,
            None => {
                error!("Missing 'out_point' field in cell: {:?}", raw_cell);
                return None;
            }
        };
        
        debug!("Extracting tx_hash from out_point");
        let tx_hash = match out_point.get("tx_hash") {
            Some(hash) => match hash.as_str() {
                Some(s) => s.to_string(),
                None => {
                    error!("tx_hash is not a string: {:?}", hash);
                    return None;
                }
            },
            None => {
                error!("Missing 'tx_hash' in out_point: {:?}", out_point);
                return None;
            }
        };
        debug!("Extracted tx_hash: {}", tx_hash);
        
        // Index is in out_point.index, not tx_index
        debug!("Extracting index from out_point");
        let index_str = match out_point.get("index") {
            Some(idx) => match idx.as_str() {
                Some(s) => s,
                None => {
                    error!("index is not a string: {:?}", idx);
                    return None;
                }
            },
            None => {
                error!("Missing 'index' in out_point: {:?}", out_point);
                return None;
            }
        };
        
        debug!("Parsing index from hex string: {}", index_str);
        let index = match u32::from_str_radix(index_str.trim_start_matches("0x"), 16) {
            Ok(i) => i,
            Err(e) => {
                error!("Failed to parse index '{}' as hex: {}", index_str, e);
                return None;
            }
        };
        debug!("Parsed index: {}", index);
        
        // Get capacity
        debug!("Extracting capacity from output");
        let capacity_str = match output.get("capacity") {
            Some(cap) => match cap.as_str() {
                Some(s) => s,
                None => {
                    error!("capacity is not a string: {:?}", cap);
                    return None;
                }
            },
            None => {
                error!("Missing 'capacity' in output: {:?}", output);
                return None;
            }
        };
        
        debug!("Parsing capacity from hex string: {}", capacity_str);
        let capacity = capacity_str.to_string();
        
        // Get lock script for owner
        debug!("Extracting lock script from output");
        let lock = match output.get("lock") {
            Some(l) => l,
            None => {
                error!("Missing 'lock' in output: {:?}", output);
                return None;
            }
        };
        
        debug!("Creating CKB address from lock script");
        
        // Extract code_hash from lock script
        let code_hash = match lock.get("code_hash") {
            Some(ch) => match ch.as_str() {
                Some(s) => s,
                None => {
                    error!("code_hash is not a string: {:?}", ch);
                    return None;
                }
            },
            None => {
                error!("Missing 'code_hash' in lock: {:?}", lock);
                return None;
            }
        };
        
        // Extract hash_type from lock script
        let hash_type_str = match lock.get("hash_type") {
            Some(ht) => match ht.as_str() {
                Some(s) => s,
                None => {
                    error!("hash_type is not a string: {:?}", ht);
                    return None;
                }
            },
            None => {
                error!("Missing 'hash_type' in lock: {:?}", lock);
                return None;
            }
        };
        
        // Convert hash_type string to ScriptHashType
        let hash_type = match hash_type_str {
            "type" => ScriptHashType::Type,
            "data" => ScriptHashType::Data,
            "data1" => ScriptHashType::Data1,
            "data2" => ScriptHashType::Data2,
            _ => {
                error!("Invalid hash_type: {}", hash_type_str);
                return None;
            }
        };
        
        // Extract args from lock script
        let args = match lock.get("args") {
            Some(a) => match a.as_str() {
                Some(s) => s.trim_start_matches("0x"),
                None => {
                    error!("args is not a string: {:?}", a);
                    return None;
                }
            },
            None => {
                error!("Missing 'args' in lock: {:?}", lock);
                return None;
            }
        };
        
        // Parse code_hash as H256
        let code_hash_bytes = match H256::from_str(code_hash.trim_start_matches("0x")) {
            Ok(ch) => ch,
            Err(e) => {
                error!("Failed to parse code_hash '{}': {}", code_hash, e);
                return None;
            }
        };
        
        // Parse args as bytes
        let args_bytes = match hex::decode(args) {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("Failed to decode args '{}' as hex: {}", args, e);
                return None;
            }
        };
        
        // Create owner address
        let owner = Address::new(
            self.network_type,
            AddressPayload::Full {
                hash_type,
                code_hash: code_hash_bytes.pack(),
                args: args_bytes.into(),
            },
            true, // Use short address format
        ).to_string();
        
        debug!("Created owner address: {}", owner);
        
        // Get content type and content
        debug!("Determining content type");
        let content_type = match &self.spore_data {
            Some(spore) => {
                debug!("Using content_type from spore_data: {}", spore.content_type);
                spore.content_type.to_string()
            },
            None => {
                debug!("No spore_data present, using default content type");
                "plain/text".to_string()
            }
        };
        
        debug!("Preparing content");
        let content = match &self.spore_data {
            Some(spore) => {
                let content_bytes = &spore.content;
                debug!("Content size: {} bytes", content_bytes.len());
                
                // For text content, try to convert to string
                if content_type.starts_with("text/") || content_type.contains("json") {
                    match String::from_utf8(content_bytes.clone()) {
                        Ok(s) => {
                            debug!("Converted binary content to UTF-8 string");
                            s
                        },
                        Err(_) => {
                            debug!("Failed to convert binary content to UTF-8, using hex encoding");
                            hex::encode(content_bytes)
                        }
                    }
                } else {
                    // For binary content, use hex encoding
                    debug!("Using hex encoding for binary content");
                    hex::encode(content_bytes)
                }
            },
            None => {
                debug!("No spore_data present, content will be empty");
                String::new()
            }
        };
        
        // Get cluster ID or use empty string
        debug!("Determining cluster_id");
        let cluster_id = match &self.cluster_id {
            Some(id) => {
                debug!("Using cluster_id from cell: {}", id);
                id.clone()
            },
            None => {
                debug!("No cluster_id present, using empty string");
                String::new()
            }
        };
        
        // For DOB (Digital Object Binary) content, prepare decoded output
        debug!("Checking if DOB content needs decoding");
        let dob_decode_output = if content_type.contains("dob") && self.spore_data.is_some() {
            debug!("Processing DOB content");
            let spore = match self.spore_data.as_ref() {
                Some(s) => s,
                None => {
                    error!("Expected spore_data to be present for DOB content");
                    return None;
                }
            };
            
            // This is a placeholder - in a real implementation you'd have custom DOB rendering logic
            // For now, we'll serialize the content as JSON for the DOB rendering
            let render_output = format!("DOB content of type: {}", content_type);
            
            // Create a JSON representation of the DOB content
            let dob_content = serde_json::json!({
                "type": content_type,
                "size": spore.content.len(),
                "hex": hex::encode(&spore.content[..std::cmp::min(100, spore.content.len())]) + "...",
            });
            
            debug!("Created DOB decode output");
            Some(ServerDecodeResult {
                render_output,
                dob_content,
            })
        } else {
            debug!("No DOB decoding needed");
            None
        };
        
        debug!("Successfully created DbSporeData");
        Some(DbSporeData {
            id: type_id.clone(),
            cluster_id,
            content_type,
            content,
            tx_hash,
            index,
            owner,
            capacity,
            dob_decode_output,
        })
    }
}
