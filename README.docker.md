# Lambda Next - Docker Deployment

This document describes how to deploy the Lambda Next system using Docker Compose.

## System Components

The deployment consists of the following components:

1. **PostgreSQL Database** - A single database instance that stores spore data
2. **Lambda Next Mainnet** - A Lambda Next instance configured for the Mainnet network
3. **Lambda Next Testnet** - A Lambda Next instance configured for the Testnet network
4. **Spore API Server** - The API server that provides HTTP access to spore data

## Prerequisites

- Docker and Docker Compose installed on your system
- Git to clone this repository

## Quick Start

1. Clone the repository:
   ```
   git clone <repository-url>
   cd lambda-next
   ```

2. Build and start the services:
   ```
   docker-compose up -d
   ```

3. Monitor the logs:
   ```
   docker-compose logs -f
   ```

## Configuration

### Environment Variables

The Docker Compose setup uses these environment variables:

- `LAMBDA_NEXT__DATABASE__URL` - Database connection string
- `LAMBDA_NEXT__CKB__NETWORK_TYPE` - Network type (mainnet or testnet)

### Configuration Files

- `config/default.toml` - Main configuration file for Lambda Next
- `config/api.toml` - Configuration for the Spore API Server

You can modify these files to customize the deployment.

## Service Details

### Database

- PostgreSQL 16
- Port: 5432 (exposed to host)
- Credentials: postgres/postgres
- Database name: spore_db

### Lambda Next Mainnet & Testnet

- Built from the provided Dockerfile
- Connected to the PostgreSQL database
- Different network configurations (mainnet vs testnet)

### Spore API Server

- Built from Dockerfile.api
- Port: 3000 (exposed to host)
- Connected to the PostgreSQL database
- Access the API at: http://localhost:3000/

## Maintenance

### Scaling

To scale the API server:
```
docker-compose up -d --scale spore-api-server=3
```

### Backup Database

```
docker exec spore-db pg_dump -U postgres -d spore_db > backup.sql
```

### Restore Database

```
cat backup.sql | docker exec -i spore-db psql -U postgres -d spore_db
```

## Troubleshooting

### Check Service Status

```
docker-compose ps
```

### View Logs

```
docker-compose logs -f
docker-compose logs -f db
docker-compose logs -f lambda-mainnet
docker-compose logs -f lambda-testnet
docker-compose logs -f spore-api-server
``` 