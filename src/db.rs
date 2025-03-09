use sqlx::{postgres::PgPoolOptions, Pool, Postgres};
use serde::{Serialize, Deserialize};
use serde_json::Value;
use std::error::Error;
use tracing::{info, debug, trace};


/// Database configuration parameters
#[derive(Debug, Clone, Deserialize)]
pub struct DbConfig {
    pub url: String,
    pub max_connections: u32,
}

/// Server-side decode result
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ServerDecodeResult {
    pub render_output: String,
    pub dob_content: Value,
}

/// Spore data structure for storing in database
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DbSporeData {
    pub id: String,
    pub cluster_id: String,
    pub content_type: String,
    pub content: String,
    pub tx_hash: String,
    pub index: u32,
    pub owner: String,
    pub capacity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dob_decode_output: Option<ServerDecodeResult>,
}

/// PostgreSQL database interface for Spore data
pub struct SporeDb {
    pool: Pool<Postgres>,
}

impl SporeDb {
    /// Create a new database connection pool
    pub async fn new(config: &DbConfig) -> Result<Self, Box<dyn Error + Send + Sync>> {
        debug!("Connecting to PostgreSQL at {}", config.url);
        
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .connect(&config.url)
            .await?;
            
        info!("Connected to PostgreSQL database");
        
        // Initialize database schema
        Self::init_schema(&pool).await?;
        
        Ok(Self { pool })
    }
    
    /// Initialize database schema if not exists
    async fn init_schema(pool: &Pool<Postgres>) -> Result<(), sqlx::Error> {
        info!("Initializing database schema");
        
        // Execute each statement separately
        // 1. Create the spores table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS spores (
                id TEXT PRIMARY KEY,
                cluster_id TEXT NOT NULL,
                content_type TEXT NOT NULL,
                content TEXT NOT NULL,
                tx_hash TEXT NOT NULL,
                index INTEGER NOT NULL,
                owner TEXT NOT NULL,
                capacity TEXT NOT NULL,
                render_output TEXT,
                dob_content JSONB
            )
            "#,
        )
        .execute(pool)
        .await?;
        
        // 2. Create index on cluster_id
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_spores_cluster_id ON spores(cluster_id)
            "#,
        )
        .execute(pool)
        .await?;
        
        // 3. Create index on owner
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_spores_owner ON spores(owner)
            "#,
        )
        .execute(pool)
        .await?;
        
        // 4. Create index on content_type
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_spores_content_type ON spores(content_type)
            "#,
        )
        .execute(pool)
        .await?;
        
        info!("Database schema setup complete");
        Ok(())
    }

    /// Insert or update a spore
    pub async fn upsert_spore(&self, spore: &DbSporeData) -> Result<(), sqlx::Error> {
        debug!("Upserting spore with ID: {}", spore.id);
        
        // Check if the record already exists and has DOB data
        let existing_spore = self.get_spore_by_id(&spore.id).await?;
        
        // Convert the dob_content to a JSON value if it exists
        let (render_output, dob_content) = match &spore.dob_decode_output {
            Some(decode) => {
                trace!("DOB decode output available for ID {}", spore.id);
                (Some(&decode.render_output), Some(&decode.dob_content))
            },
            None => {
                // If no new DOB data but we have existing DOB data, preserve it
                if let Some(existing) = &existing_spore {
                    if let Some(existing_dob) = &existing.dob_decode_output {
                        trace!("Preserving existing DOB decode output for ID {}", spore.id);
                        (Some(&existing_dob.render_output), Some(&existing_dob.dob_content))
                    } else {
                        trace!("No DOB decode output for ID {}", spore.id);
                        (None, None)
                    }
                } else {
                    trace!("No DOB decode output for ID {}", spore.id);
                    (None, None)
                }
            },
        };
        
        trace!("Database insert/update for ID {}", spore.id);

        sqlx::query(
            r#"
            INSERT INTO spores 
            (id, cluster_id, content_type, content, tx_hash, index, owner, capacity, render_output, dob_content)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (id) DO UPDATE SET
                cluster_id = EXCLUDED.cluster_id,
                content_type = EXCLUDED.content_type,
                content = EXCLUDED.content,
                tx_hash = EXCLUDED.tx_hash,
                index = EXCLUDED.index,
                owner = EXCLUDED.owner,
                capacity = EXCLUDED.capacity,
                render_output = COALESCE(EXCLUDED.render_output, spores.render_output),
                dob_content = COALESCE(EXCLUDED.dob_content, spores.dob_content)
            "#,
        )
        .bind(&spore.id)
        .bind(&spore.cluster_id)
        .bind(&spore.content_type)
        .bind(&spore.content)
        .bind(&spore.tx_hash)
        .bind(spore.index as i32)
        .bind(&spore.owner)
        .bind(&spore.capacity)
        .bind(render_output)
        .bind(dob_content)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Get spore by ID
    pub async fn get_spore_by_id(&self, id: &str) -> Result<Option<DbSporeData>, sqlx::Error> {
        debug!("Getting spore with ID: {}", id);
        
        let row: Option<(String, String, String, String, String, i32, String, String, Option<String>, Option<serde_json::Value>)> = 
            sqlx::query_as(
                r#"
                SELECT id, cluster_id, content_type, content, tx_hash, index, owner, capacity, render_output, dob_content
                FROM spores
                WHERE id = $1
                "#,
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        
        Ok(row.map(|(id, cluster_id, content_type, content, tx_hash, index, owner, capacity, render_output, dob_content)| {
            let dob_decode_output = match (render_output, dob_content) {
                (Some(render), Some(content)) => Some(ServerDecodeResult {
                    render_output: render,
                    dob_content: content,
                }),
                _ => None,
            };
            
            DbSporeData {
                id,
                cluster_id,
                content_type,
                content,
                tx_hash,
                index: index as u32,
                owner,
                capacity: capacity.to_string(),
                dob_decode_output,
            }
        }))
    }
    
    /// Get spores by cluster ID
    pub async fn get_spores_by_cluster(&self, cluster_id: &str, limit: i64, offset: i64) -> Result<Vec<DbSporeData>, sqlx::Error> {
        debug!("Getting spores with cluster ID: {}", cluster_id);
        
        let rows: Vec<(String, String, String, String, String, i32, String, String, Option<String>, Option<serde_json::Value>)> = 
            sqlx::query_as(
                r#"
                SELECT id, cluster_id, content_type, content, tx_hash, index, owner, capacity, render_output, dob_content
                FROM spores
                WHERE cluster_id = $1
                ORDER BY id
                LIMIT $2 OFFSET $3
                "#,
            )
            .bind(cluster_id)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        
        Ok(rows.into_iter().map(|(id, cluster_id, content_type, content, tx_hash, index, owner, capacity, render_output, dob_content)| {
            let dob_decode_output = match (render_output, dob_content) {
                (Some(render), Some(content)) => Some(ServerDecodeResult {
                    render_output: render,
                    dob_content: content,
                }),
                _ => None,
            };
            
            DbSporeData {
                id,
                cluster_id,
                content_type,
                content,
                tx_hash,
                index: index as u32,
                owner,
                capacity,
                dob_decode_output,
            }
        }).collect())
    }
    
    /// Get spores by owner
    pub async fn get_spores_by_owner(&self, owner: &str, limit: i64, offset: i64) -> Result<Vec<DbSporeData>, sqlx::Error> {
        debug!("Getting spores with owner: {}", owner);
        
        let rows: Vec<(String, String, String, String, String, i32, String, String, Option<String>, Option<serde_json::Value>)> = 
            sqlx::query_as(
                r#"
                SELECT id, cluster_id, content_type, content, tx_hash, index, owner, capacity, render_output, dob_content
                FROM spores
                WHERE owner = $1
                ORDER BY id
                LIMIT $2 OFFSET $3
                "#,
            )
            .bind(owner)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        
        Ok(rows.into_iter().map(|(id, cluster_id, content_type, content, tx_hash, index, owner, capacity, render_output, dob_content)| {
            let dob_decode_output = match (render_output, dob_content) {
                (Some(render), Some(content)) => Some(ServerDecodeResult {
                    render_output: render,
                    dob_content: content,
                }),
                _ => None,
            };
            
            DbSporeData {
                id,
                cluster_id,
                content_type,
                content,
                tx_hash,
                index: index as u32,
                owner,
                capacity,
                dob_decode_output,
            }
        }).collect())
    }
} 