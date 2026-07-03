//! `SnippetStore` implementation backed by SQLite.
//!
//! Plain CRUD over the `snippets` table (reusable commands). Runtime
//! queries (bind + `try_get`) like `host_store`, so no sqlx-offline data
//! is needed for the new table.

use async_trait::async_trait;
use sqlx::Row;
use tracing::instrument;

use rh_core::{Snippet, SnippetId, SnippetStore, StorageError};

use crate::db::Db;
use crate::host_store::{map_err, parse_datetime};

#[derive(Debug, Clone)]
pub struct SqliteSnippetStore {
    db: Db,
}

impl SqliteSnippetStore {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SnippetStore for SqliteSnippetStore {
    #[instrument(level = "debug", skip(self))]
    async fn list(&self) -> Result<Vec<Snippet>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, name, command, created_at, updated_at FROM snippets
             ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(map_err)?;
        rows.iter().map(row_to_snippet).collect()
    }

    #[instrument(level = "debug", skip(self, snippet), fields(snippet_id = %snippet.id))]
    async fn create(&self, snippet: &Snippet) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO snippets (id, name, command, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(snippet.id.as_str())
        .bind(&snippet.name)
        .bind(&snippet.command)
        .bind(snippet.created_at.to_rfc3339())
        .bind(snippet.updated_at.to_rfc3339())
        .execute(self.db.pool())
        .await
        .map_err(map_err)?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self, snippet), fields(snippet_id = %snippet.id))]
    async fn update(&self, snippet: &Snippet) -> Result<(), StorageError> {
        let result = sqlx::query(
            "UPDATE snippets SET name = ?, command = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&snippet.name)
        .bind(&snippet.command)
        .bind(snippet.updated_at.to_rfc3339())
        .bind(snippet.id.as_str())
        .execute(self.db.pool())
        .await
        .map_err(map_err)?;
        if result.rows_affected() == 0 {
            return Err(StorageError::Backend(format!(
                "snippet {} not found for update",
                snippet.id.as_str()
            )));
        }
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(snippet_id = %id))]
    async fn delete(&self, id: &SnippetId) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM snippets WHERE id = ?")
            .bind(id.as_str())
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;
        Ok(())
    }
}

fn row_to_snippet(row: &sqlx::sqlite::SqliteRow) -> Result<Snippet, StorageError> {
    let id: String = row
        .try_get("id")
        .map_err(|e| StorageError::Backend(format!("read id: {e}")))?;
    let name: String = row
        .try_get("name")
        .map_err(|e| StorageError::Backend(format!("read name: {e}")))?;
    let command: String = row
        .try_get("command")
        .map_err(|e| StorageError::Backend(format!("read command: {e}")))?;
    let created_at_s: String = row
        .try_get("created_at")
        .map_err(|e| StorageError::Backend(format!("read created_at: {e}")))?;
    let updated_at_s: String = row
        .try_get("updated_at")
        .map_err(|e| StorageError::Backend(format!("read updated_at: {e}")))?;

    Ok(Snippet {
        id: SnippetId::from_raw(id),
        name,
        command,
        created_at: parse_datetime("snippets.created_at", &created_at_s)?,
        updated_at: parse_datetime("snippets.updated_at", &updated_at_s)?,
    })
}
