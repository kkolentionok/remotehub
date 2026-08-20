//! `NoteStore` implementation backed by SQLite.
//!
//! Plain CRUD over the `notes` table (free-form notes). Runtime queries
//! (bind + `try_get`) like `host_store`/`snippet_store`, so no sqlx-offline
//! data is needed for the table.

use async_trait::async_trait;
use sqlx::Row;
use tracing::instrument;

use rh_core::{Note, NoteId, NoteStore, StorageError};

use crate::db::Db;
use crate::host_store::{map_err, parse_datetime};

#[derive(Debug, Clone)]
pub struct SqliteNoteStore {
    db: Db,
}

impl SqliteNoteStore {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl NoteStore for SqliteNoteStore {
    #[instrument(level = "debug", skip(self))]
    async fn list(&self) -> Result<Vec<Note>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, title, body, pinned, created_at, updated_at FROM notes
             ORDER BY pinned DESC, updated_at DESC",
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(map_err)?;
        rows.iter().map(row_to_note).collect()
    }

    #[instrument(level = "debug", skip(self, note), fields(note_id = %note.id))]
    async fn create(&self, note: &Note) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO notes (id, title, body, pinned, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(note.id.as_str())
        .bind(&note.title)
        .bind(&note.body)
        .bind(i64::from(note.pinned))
        .bind(note.created_at.to_rfc3339())
        .bind(note.updated_at.to_rfc3339())
        .execute(self.db.pool())
        .await
        .map_err(map_err)?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self, note), fields(note_id = %note.id))]
    async fn update(&self, note: &Note) -> Result<(), StorageError> {
        let result = sqlx::query("UPDATE notes SET title = ?, body = ?, updated_at = ? WHERE id = ?")
            .bind(&note.title)
            .bind(&note.body)
            .bind(note.updated_at.to_rfc3339())
            .bind(note.id.as_str())
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;
        if result.rows_affected() == 0 {
            return Err(StorageError::Backend(format!(
                "note {} not found for update",
                note.id.as_str()
            )));
        }
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(note_id = %id))]
    async fn set_pinned(&self, id: &NoteId, pinned: bool) -> Result<(), StorageError> {
        sqlx::query("UPDATE notes SET pinned = ?, updated_at = ? WHERE id = ?")
            .bind(i64::from(pinned))
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(id.as_str())
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(note_id = %id))]
    async fn delete(&self, id: &NoteId) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM notes WHERE id = ?")
            .bind(id.as_str())
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;
        Ok(())
    }
}

fn row_to_note(row: &sqlx::sqlite::SqliteRow) -> Result<Note, StorageError> {
    let id: String = row
        .try_get("id")
        .map_err(|e| StorageError::Backend(format!("read id: {e}")))?;
    let title: String = row
        .try_get("title")
        .map_err(|e| StorageError::Backend(format!("read title: {e}")))?;
    let body: String = row
        .try_get("body")
        .map_err(|e| StorageError::Backend(format!("read body: {e}")))?;
    let pinned: i64 = row
        .try_get("pinned")
        .map_err(|e| StorageError::Backend(format!("read pinned: {e}")))?;
    let created_at_s: String = row
        .try_get("created_at")
        .map_err(|e| StorageError::Backend(format!("read created_at: {e}")))?;
    let updated_at_s: String = row
        .try_get("updated_at")
        .map_err(|e| StorageError::Backend(format!("read updated_at: {e}")))?;

    Ok(Note {
        id: NoteId::from_raw(id),
        title,
        body,
        pinned: pinned != 0,
        created_at: parse_datetime("notes.created_at", &created_at_s)?,
        updated_at: parse_datetime("notes.updated_at", &updated_at_s)?,
    })
}
