use molecule::prelude::*;
use serde_json::Value;
use spore_types::{NativeNFTData, generated::spore::SporeData};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use lazy_static::lazy_static;

use crate::config::SporeFilterConfig;
use crate::utils::{AppError, Result};

/// Represents a Spore cell with its decoded data
pub struct SporeCell {
    /// The raw cell data from CKB
    pub raw_cell: Value,
    /// The decoded Spore data
    pub spore_data: Option<NativeNFTData>,
    /// The cluster ID if present
    pub cluster_id: Option<String>,
    /// The Type ID of the cell
    pub type_id: Option<String>,
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
    let spore_data = SporeData::from_slice(&data).map_err(|e| {
        error!("Failed to parse SporeData: {}", e);
        AppError::SporeError(format!("Failed to parse SporeData: {}", e))
    })?;

    // Convert to native representation (manually)
    let native_data = NativeNFTData {
        content_type: spore_data.content_type().to_string(),
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

/// Apply a filter to a list of cells based on filter configuration
pub fn apply_filter(cells: Vec<Value>, filter: &SporeFilterConfig) -> Vec<SporeCell> {
    debug!("Applying filter '{}' to {} cells", filter.name, cells.len());
    
    let mut filtered_cells = Vec::new();
    let cells_len = cells.len();

    // Load TypeIDs from file if specified
    let mut all_type_ids = filter.type_ids.clone();
    if let Some(file_path) = &filter.type_ids_file {
        if !file_path.is_empty() {
            match load_type_ids_from_file(file_path) {
                Ok(file_type_ids) => {
                    debug!("Loaded {} Type IDs from file '{}'", file_type_ids.len(), file_path);
                    all_type_ids.extend(file_type_ids);
                }
                Err(err) => {
                    error!("Failed to load Type IDs from file '{}': {}", file_path, err);
                }
            }
        }
    }

    for cell in cells {
        // Extract Type ID first (we always need this for filtering and caching)
        let type_id = match extract_type_id(&cell) {
            Some(id) => id,
            None => {
                // Skip cells without a Type ID - they're not valid Spore cells
                trace!("Cell has no Type ID, skipping");
                continue;
            }
        };
        trace!("Processing cell with Type ID: {}", type_id);

        // Apply Type ID filter if enabled
        if filter.filter_by_type_ids && !all_type_ids.is_empty() {
            if !all_type_ids.contains(&type_id) {
                trace!(
                    "Cell with Type ID {} doesn't match filter, skipping",
                    type_id
                );
                continue;
            }
        }

        // Create a new SporeCell with raw cell data and Type ID
        let mut spore_cell = SporeCell {
            raw_cell: cell.clone(),
            spore_data: None,
            cluster_id: None,
            type_id: Some(type_id.clone()),
        };

        // Skip decoding if requested
        if filter.skip_decoding {
            filtered_cells.push(spore_cell);
            continue;
        }

        // Decode the Spore data
        match decode_spore_data(&cell) {
            Ok(spore_data) => {
                // Extract cluster ID
                let cluster_id = extract_cluster_id(&spore_data);

                // Apply cluster filter if enabled
                if filter.filter_by_cluster && !filter.cluster_id.is_empty() {
                    if let Some(ref id) = cluster_id {
                        if id != &filter.cluster_id {
                            trace!(
                                "Cell with cluster ID {} doesn't match filter {}, skipping",
                                id, filter.cluster_id
                            );
                            continue;
                        }
                    } else {
                        trace!("Cell has no cluster ID, skipping due to cluster filter");
                        continue;
                    }
                }

                // Set spore data and cluster ID on the cell
                spore_cell.spore_data = Some(spore_data);
                spore_cell.cluster_id = cluster_id;
                filtered_cells.push(spore_cell);
            }
            Err(e) => {
                warn!("Failed to decode spore data for Type ID {}: {}", type_id, e);
                // We still track cells that failed to decode if we're not filtering by anything
                if !filter.filter_by_cluster && !filter.filter_by_type_ids {
                    filtered_cells.push(spore_cell);
                }
            }
        }
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

    // Process each filter independently
    for filter in enabled_filters {
        debug!("Applying filter: {}", filter.name);

        // Create a fresh copy of the cells for each filter
        // This avoids the borrow after move issue
        let cells_for_filter = cells.to_vec();

        // Apply the filter
        let filtered_cells = apply_filter(cells_for_filter, filter);

        if !filtered_cells.is_empty() {
            trace!(
                "Filter '{}' selected {} cells for processing",
                filter.name,
                filtered_cells.len()
            );

            // Process the filtered cells
            process_spore_cells(filtered_cells).await?;
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
pub fn process_single_spore(cell: &SporeCell) -> Result<()> {
    // TODO: Implement custom logic for processing a single spore cell
    debug!("Processing spore cell: {}", cell.type_id.as_deref().unwrap_or("None"));
    trace!("Spore data: {:?}", cell.spore_data);
    trace!("Cluster ID: {:?}", cell.cluster_id);
    trace!("Type ID: {:?}", cell.type_id);
    trace!("Raw cell: {:?}", cell.raw_cell);
    trace!("--------------------------------");


    info!("{:?}", cell.raw_cell);


    Ok(())
}

/// Process a list of spore cells
///
/// This function is a placeholder for user-defined processing logic
pub async fn process_spore_cells(cells: Vec<SporeCell>) -> Result<()> {
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
                if let Err(e) = process_single_spore(cell) {
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
