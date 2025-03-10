# Spore API Server

An HTTP API server for querying Spore data from the database.

## Features

* RESTful API for querying spores by cluster ID and owner address
* Filter by network type (mainnet, testnet, etc.) with support for aliases
* Pagination support
* Separate endpoints for DOB and regular spores

## API Endpoints

### Cluster Queries

* `GET /cluster/:cluster_id/all?limit=100&offset=0&network=mainnet`
  * Get all spores by cluster ID with pagination
  * Network is optional and supports aliases

### Address Queries

* `GET /address/:address/dob/all?limit=100&offset=0&network=mainnet`
  * Get all DOB spores for a given address
  * Includes DOB rendering data

* `GET /address/:address/spore/all?limit=100&offset=0&network=mainnet`
  * Get all spores for a given address
  * Excludes DOB rendering data for efficiency

## Network Aliases

* Mainnet: `main`, `mainnet`, `mirana` 
* Testnet: `test`, `testnet`, `pudge`, `meepo`
* Devnet: `dev`, `devnet`, `local`

All aliases are case-insensitive.

## Running the Server

```bash
cargo run --bin spore-api-server -- -c config/api.toml
```

## Configuration

See the example configuration in `config/api.toml`:

```toml
[api]
address = "0.0.0.0"  # Listen on all interfaces
port = 8080          # Port to listen on

[database]
enabled = true
url = "postgres://postgres:postgres@localhost/spore_db"
max_connections = 5
``` 