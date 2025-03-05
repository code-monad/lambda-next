use ckb_jsonrpc_types::{
    CellOutput, OutPoint, Script, Uint32, Capacity,
};
use serde_json::{json, Value};
use futures::SinkExt;
use tokio_tungstenite::tungstenite::protocol::Message;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use tracing::{info, warn, error, debug, trace, instrument, field};

#[derive(Debug)]
pub enum CkbError {
    JsonRpcError(String),
    ParseError(String),
    WebSocketError(String),
}

impl fmt::Display for CkbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CkbError::JsonRpcError(e) => write!(f, "CKB JSON-RPC error: {}", e),
            CkbError::ParseError(e) => write!(f, "Parse error: {}", e),
            CkbError::WebSocketError(e) => write!(f, "WebSocket error: {}", e),
        }
    }
}

impl Error for CkbError {}

pub type Result<T> = std::result::Result<T, CkbError>;

// Enum for script types
pub enum ScriptType {
    Lock,
    Type,
}

impl ToString for ScriptType {
    fn to_string(&self) -> String {
        match self {
            ScriptType::Lock => "lock".to_string(),
            ScriptType::Type => "type".to_string(),
        }
    }
}

// Enum for outputs validator
pub enum OutputsValidator {
    Default,
    PassAll,
}

impl ToString for OutputsValidator {
    fn to_string(&self) -> String {
        match self {
            OutputsValidator::Default => "default".to_string(),
            OutputsValidator::PassAll => "passall".to_string(),
        }
    }
}

// CKB client that uses WebSocket
#[derive(Clone)]
pub struct CkbClient {
    writer: Arc<Mutex<WsWriter>>,
    id_counter: Arc<Mutex<u64>>,
}

// Type alias for the WebSocket writer
type WsWriter = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, 
    Message
>;

impl CkbClient {
    /// Create a new CKB client with a direct writer
    pub fn new(writer: WsWriter) -> Self {
        Self { 
            writer: Arc::new(Mutex::new(writer)),
            id_counter: Arc::new(Mutex::new(1)),
        }
    }
    
    /// Create a new CKB client with an already shared writer
    pub fn with_shared_writer(writer: Arc<Mutex<WsWriter>>) -> Self {
        Self {
            writer,
            id_counter: Arc::new(Mutex::new(1)),
        }
    }

    async fn get_next_id(&self) -> u64 {
        // We still increment the counter for debugging/logging purposes
        let mut counter = self.id_counter.lock().await;
        let id = *counter;
        *counter += 1;
        
        // Generate a UUID v4 (random)
        let uuid = Uuid::new_v4();
        
        // Convert the first 8 bytes of the UUID to a u64
        // This maintains the uniqueness while providing a numeric ID
        let uuid_bytes = uuid.as_bytes();
        let mut uuid_u64 = 0u64;
        
        // Combine the first 8 bytes into a u64
        for i in 0..8 {
            uuid_u64 = (uuid_u64 << 8) | (uuid_bytes[i] as u64);
        }
        
        // Make sure the ID is never zero (JSON-RPC requires non-zero IDs)
        if uuid_u64 == 0 {
            uuid_u64 = 1;
        }
        
        debug!("Generated request ID from UUID: {} ({})", uuid_u64, uuid);
        uuid_u64
    }

    /// Search for cells based on type script code hash
    #[instrument(skip(self), fields(code_hash = %code_hash, hash_type = %hash_type, limit = %limit))]
    pub async fn get_cells_by_type_script_code_hash(
        &self,
        code_hash: &str,
        hash_type: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Value> {
        if let Some(cursor_value) = &cursor {
            debug!(cursor = %cursor_value, "Continuing query with cursor");
        } else {
            info!("Starting new cell query");
        }
        
        // Create search key using script type enum
        let search_key = json!({
            "script": {
                "code_hash": code_hash,
                "hash_type": hash_type,
                "args": "0x"
            },
            "script_type": ScriptType::Type.to_string(),
        });

        // Prepare the JSON-RPC request parameters
        let params = json!([
            search_key,
            "asc", // Order::Asc
            Uint32::from(limit),
            cursor,
            OutputsValidator::Default.to_string()
        ]);

        // Execute RPC call
        debug!("Executing get_cells RPC call");
        self.call_rpc("get_cells", params).await
    }

    /// Make a WebSocket RPC call
    #[instrument(skip(self, params), fields(method = %method, id = field::Empty))]
    pub async fn call_rpc(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.get_next_id().await;
        tracing::Span::current().record("id", &id);
        
        let request_body = json!({
            "id": id,
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        // Convert to string
        let request_str = request_body.to_string();
        
        debug!("Sending JSON-RPC request: {}", request_str);
        
        // Send the request through the WebSocket
        let mut writer = self.writer.lock().await;
        writer.send(Message::Text(request_str.into()))
            .await
            .map_err(|e| {
                error!("WebSocket send error: {}", e);
                CkbError::WebSocketError(e.to_string())
            })?;

        // Note: The response will be handled by the websocket message handler
        info!("JSON-RPC request sent with ID: {}", id);
        Ok(json!({"id": id}))
    }

    // Helper function to parse cells from get_cells response
    #[instrument(skip(self, response))]
    pub fn parse_cells_response(&self, response: &Value) -> Result<(Vec<Value>, Option<String>)> {
        debug!("Parsing get_cells response");
        
        // Extract objects and next cursor from the response
        let objects = response
            .get("result")
            .and_then(|r| r.get("objects"))
            .and_then(|o| o.as_array())
            .ok_or_else(|| {
                let err = CkbError::ParseError("Invalid response format: missing 'objects' array".into());
                error!("Parse error: {}", err);
                err
            })?
            .clone();
        
        let last_cursor = response
            .get("result")
            .and_then(|r| r.get("last_cursor"))
            .and_then(|c| c.as_str())
            .map(String::from);
        
        if let Some(cursor) = &last_cursor {
            debug!(cells_count = %objects.len(), cursor = %cursor, "Found cells with next cursor");
        } else {
            debug!(cells_count = %objects.len(), "Found cells (no more pages)");
        }

        Ok((objects, last_cursor))
    }
} 