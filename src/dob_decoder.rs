use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::error::Error;
use tracing::{debug, error, info, trace, warn};

use crate::db::ServerDecodeResult;

/// Configuration for the DOB decoder service
#[derive(Debug, Deserialize, Clone)]
pub struct DobDecoderConfig {
    pub url: String,
    pub batch_size: usize,
    pub enabled: bool,
}

impl Default for DobDecoderConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:8080".to_string(),
            batch_size: 10,
            enabled: false,
        }
    }
}

/// JSON-RPC request for DOB decoding
#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    params: Vec<Value>,
    id: u64,
}

/// JSON-RPC batch request for DOB decoding
#[derive(Debug, Serialize)]
struct JsonRpcBatchRequest(Vec<JsonRpcRequest>);

/// JSON-RPC response for DOB decoding
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    result: Option<Value>,
    error: Option<JsonRpcError>,
    id: u64,
}

/// JSON-RPC error structure
#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    data: Option<Value>,
}

/// DOB decoder service
pub struct DobDecoder {
    client: Client,
    config: DobDecoderConfig,
}

impl DobDecoder {
    /// Create a new DOB decoder service
    pub fn new(config: DobDecoderConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    /// Decode a batch of DOB spores
    pub async fn decode_batch(&self, contents: Vec<(String, String)>) -> Vec<(String, Result<ServerDecodeResult, Box<dyn Error + Send + Sync>>)> {
        if contents.is_empty() {
            return vec![];
        }

        info!("Decoding {} DOB spores in batch", contents.len());
        
        // Create batch JSON-RPC request
        let batch_requests = contents
            .iter()
            .enumerate()
            .map(|(i, (id, content))| {
                // Remove 0x prefix from type ID if present
                let clean_id = id.trim_start_matches("0x");
                debug!("Creating request for ID: {}, clean ID: {}", id, clean_id);
                JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: "dob_decode".to_string(),
                    params: vec![json!(clean_id)],
                    id: i as u64,
                }
            })
            .collect::<Vec<_>>();

        // Send batch request
        let results = match self.send_batch_request(batch_requests, &contents).await {
            Ok(results) => results,
            Err(e) => {
                error!("Failed to send batch request: {}", e);
                contents
                    .into_iter()
                    .map(|(id, _)| {
                        (id, Err(Box::new(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!("Failed to send batch request: {}", e),
                        )) as Box<dyn Error + Send + Sync>))
                    })
                    .collect()
            }
        };

        results
    }

    async fn send_batch_request(
        &self,
        batch_requests: Vec<JsonRpcRequest>,
        contents: &[(String, String)],
    ) -> Result<Vec<(String, Result<ServerDecodeResult, Box<dyn Error + Send + Sync>>)>, Box<dyn Error + Send + Sync>> {
        debug!("Sending batch request to URL: {}", self.config.url);
        
        // Debug the request body
        let request_body = serde_json::to_string(&batch_requests).unwrap_or_default();
        trace!("Request body (truncated): {}...", if request_body.len() > 500 { &request_body[..500] } else { &request_body });
        
        let response = self.client
            .post(&self.config.url)
            .json(&batch_requests)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            error!("HTTP error: {} - {}", status, error_text);
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("HTTP error: {} - {}", status, error_text),
            )));
        }

        // Get the response body as text first for debugging
        let response_text = response.text().await?;
        debug!("Received response (truncated): {}...", if response_text.len() > 500 { &response_text[..500] } else { &response_text });
        
        // Parse the response
        let responses: Vec<JsonRpcResponse> = match serde_json::from_str(&response_text) {
            Ok(parsed) => parsed,
            Err(e) => {
                error!("Failed to parse JSON-RPC response: {} - Response was: {}", e, response_text);
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to parse JSON-RPC response: {}", e),
                )));
            }
        };
        
        trace!("Parsed {} JSON-RPC responses", responses.len());
        
        // Map responses back to their IDs
        let mut results = Vec::with_capacity(contents.len());
        
        for (i, (id, _)) in contents.iter().enumerate() {
            // Find the corresponding response
            let response_opt = responses.iter().find(|r| r.id == i as u64);
            
            match response_opt {
                Some(response) => {
                    if let Some(error) = &response.error {
                        error!("JSON-RPC error for ID {}: {}", id, error.message);
                        results.push((
                            id.clone(),
                            Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                format!("JSON-RPC error: {}", error.message),
                            )) as Box<dyn Error + Send + Sync>),
                        ));
                    } else if let Some(result) = &response.result {
                        // Parse the result
                        trace!("Processing result for ID {}: {:?}", id, result);
                        match trace_response_processing(result, id) {
                            Ok(server_result) => {
                                debug!("Successfully parsed DOB decode result for ID {}", id);
                                results.push((id.clone(), Ok(server_result)));
                            }
                            Err(e) => {
                                error!("Failed to parse result for ID {}: {}", id, e);
                                results.push((
                                    id.clone(),
                                    Err(Box::new(std::io::Error::new(
                                        std::io::ErrorKind::Other,
                                        format!("Failed to parse result: {}", e),
                                    )) as Box<dyn Error + Send + Sync>),
                                ));
                            }
                        }
                    } else {
                        error!("Missing result and error in response for ID {}", id);
                        results.push((
                            id.clone(),
                            Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "Missing result and error in response",
                            )) as Box<dyn Error + Send + Sync>),
                        ));
                    }
                }
                None => {
                    error!("No response found for request ID {}", id);
                    results.push((
                        id.clone(),
                        Err(Box::new(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            "No response found for request",
                        )) as Box<dyn Error + Send + Sync>),
                    ));
                }
            }
        }
        
        Ok(results)
    }
}

/// Trace all steps of handling the DOB decode response, to help debug issues
fn trace_response_processing(value: &Value, id: &str) -> Result<ServerDecodeResult, Box<dyn Error + Send + Sync>> {
    trace!("Processing DOB decode response for ID: {}", id);
    trace!("Response value type: {}", if value.is_string() { "string" } else if value.is_object() { "object" } else { "other" });
    
    // If it's a string, try to parse it as JSON
    if value.is_string() {
        let s = value.as_str().unwrap();
        trace!("String response (truncated): {}...", if s.len() > 100 { &s[..100] } else { s });
        
        match serde_json::from_str::<Value>(s) {
            Ok(parsed) => {
                trace!("Parsed string as JSON");
                
                // Check for different field naming patterns
                let has_render_output = parsed.get("render_output").is_some();
                let has_renderOutput = parsed.get("renderOutput").is_some();
                let has_dob_content = parsed.get("dob_content").is_some();
                let has_dobContent = parsed.get("dobContent").is_some();
                
                trace!("Field detection: render_output: {}, renderOutput: {}, dob_content: {}, dobContent: {}", 
                     has_render_output, has_renderOutput, has_dob_content, has_dobContent);
                
                // Extract fields with fallbacks
                let render_output = parsed.get("render_output")
                    .or_else(|| parsed.get("renderOutput"))
                    .map(|v| match v.as_str() {
                        Some(s) => s.to_string(),
                        None => v.to_string()
                    });
                
                let dob_content = parsed.get("dob_content")
                    .or_else(|| parsed.get("dobContent"))
                    .cloned();
                
                if let (Some(ro), Some(dc)) = (render_output, dob_content) {
                    debug!("Extracted DOB fields successfully for ID {}", id);
                    Ok(ServerDecodeResult {
                        render_output: ro,
                        dob_content: dc,
                    })
                } else {
                    error!("Missing required fields in parsed response for ID {}", id);
                    Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Missing required fields in parsed response",
                    )))
                }
            },
            Err(e) => {
                error!("Failed to parse string as JSON for ID {}: {}", id, e);
                Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to parse string as JSON: {}", e),
                )))
            }
        }
    } else if value.is_object() {
        // Direct object handling
        trace!("Processing object response");
        
        // Check for different field naming patterns
        let has_render_output = value.get("render_output").is_some();
        let has_renderOutput = value.get("renderOutput").is_some();
        let has_dob_content = value.get("dob_content").is_some();
        let has_dobContent = value.get("dobContent").is_some();
        
        trace!("Field detection: render_output: {}, renderOutput: {}, dob_content: {}, dobContent: {}", 
             has_render_output, has_renderOutput, has_dob_content, has_dobContent);
        
        // Extract fields with fallbacks
        let render_output = value.get("render_output")
            .or_else(|| value.get("renderOutput"))
            .map(|v| match v.as_str() {
                Some(s) => s.to_string(),
                None => v.to_string()
            });
        
        let dob_content = value.get("dob_content")
            .or_else(|| value.get("dobContent"))
            .cloned();
        
        if let (Some(ro), Some(dc)) = (render_output, dob_content) {
            debug!("Extracted DOB fields successfully for ID {}", id);
            Ok(ServerDecodeResult {
                render_output: ro,
                dob_content: dc,
            })
        } else {
            error!("Missing required fields in object response for ID {}", id);
            Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Missing required fields in object response",
            )))
        }
    } else {
        error!("Unexpected value type in response for ID {}", id);
        Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Unexpected value type in response",
        )))
    }
}

/// Split a list into batches of the specified size
pub fn batch_items<T>(items: Vec<T>, batch_size: usize) -> Vec<Vec<T>> {
    items
        .into_iter()
        .fold(Vec::new(), |mut result, item| {
            if let Some(last) = result.last_mut() {
                if last.len() < batch_size {
                    last.push(item);
                    return result;
                }
            }
            result.push(vec![item]);
            result
        })
} 