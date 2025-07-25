# Spore Protocol Lambda Integration

A Rust-based service that integrates with the Spore Protocol on CKB (Nervos Network) using WebSocket connections. This service allows for efficient filtering and processing of Spore cells.

## Features

- **WebSocket Connection**: Maintains a persistent WebSocket connection to a CKB node
- **Efficient Cell Querying**: Handles paginated queries to retrieve cells
- **Filtering Capabilities**: 
  - Filter by Cluster ID
  - Filter by Type ID (inline or from file)
  - Combine multiple filters
- **TypeID-based Caching**: Improved performance through caching decoded cells by Type ID
- **External Type ID Lists**: Load Type IDs from separate files for easier management
- **Performance Optimizations**:
  - Type ID files are loaded once during startup and cached in memory
  - Particularly beneficial for large JSON files (e.g., NervapeInfo.json)

## Getting Started

### Prerequisites

- Rust and Cargo (latest stable version recommended)
- Access to a CKB node with WebSocket support

### Installation

1. Clone the repository
2. Build the project:
   ```
   cargo build --release
   ```

### Configuration

The service uses TOML configuration files located in the `config` directory:

- `default.toml`: Default configuration values
- `local.toml`: Local overrides (not committed to repository)

Example configuration:

```toml
[websocket]
url = "ws://127.0.0.1:8114"
reconnect_interval = 5000  # milliseconds
ping_interval = 30000      # milliseconds

[ckb]
type_script_code_hash = "0x..."
type_script_hash_type = "type"
query_limit = 100
query_interval_secs = 10

[[spore_filters]]
name = "Default Cluster Filter"
enabled = true
filter_by_cluster = true
cluster_id = "0x..."
filter_by_type_ids = false
type_ids = []
type_ids_file = ""
skip_decoding = false

[[spore_filters]]
name = "Nervape Collection Filter"      
enabled = true                      
filter_by_cluster = false            
cluster_id = ""                      
filter_by_type_ids = true
type_ids = []  
type_ids_file = "config/NervapeInfo.json"
skip_decoding = false
```

### Type IDs File Feature

When dealing with a large number of Type IDs, it can be cumbersome to include them all directly in your TOML configuration file. This project supports loading Type IDs from external files using the `type_ids_file` option.

#### Supported File Formats:

1. **Plain Text**: One Type ID per line, with comment support (lines starting with #)
2. **JSON Arrays**: Simple arrays of Type ID strings
3. **Collection Metadata**: JSON files with Type IDs in typeInfo.args fields (e.g., Nervape format)

#### Performance Optimization:

For efficiency, all Type ID files are loaded only once during application startup and cached in memory. This optimization is particularly beneficial for large JSON files like collection metadata that would be expensive to parse repeatedly.

## Documentation

For more detailed information, see the documentation in the `docs` directory:

- [Usage Guide](docs/USAGE.md)
- [Development Guide](docs/DEVELOPMENT.md)

## License

This project is licensed under the MIT License - see the LICENSE file for details. 