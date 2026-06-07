//! `GroupStore` implementation backed by SQLite.
//!
//! Group hierarchy is stored as a parent-pointer tree (`parent_id`
//! column). Two operations need explicit invariant checks because
//! the DDL alone can't express them:
//!
//! 1. **No cycles**: A group cannot be moved under one of its own
//!    descendants. We walk the ancestor chain of the proposed new
//!    parent and refuse if we hit the group being moved.
//! 2. **Self-parenting**: A group cannot be its own parent. Subsumed
//!    by (1) but checked explicitly for a clearer error.
//!
//! Both checks happen inside the same transaction as the UPDATE, so
//! concurrent moves can't race past the validation.

use async_trait::async_trait;
use sqlx::Row;
use tracing::instrument;

use rh_core::{GroupId, GroupStore, HostGroup, StorageError};

use crate::db::Db;
use crate::host_store::{map_err, parse_datetime};

#[derive(Debug, Clone)]
pub struct SqliteGroupStore {
    db: Db,
}

impl SqliteGroupStore {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl GroupStore for SqliteGroupStore {
    #[instrument(level = "debug", skip(self, group), fields(group_id = %group.id))]
    async fn create(&self, group: &HostGroup) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO host_groups (id, name, parent_id, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(group.id.as_str())
        .bind(&group.name)
        .bind(group.parent_id.as_ref().map(|g| g.as_str()))
        .bind(group.created_at.to_rfc3339())
        .execute(self.db.pool())
        .await
        .map_err(map_err)?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(group_id = %id))]
    async fn get(&self, id: &GroupId) -> Result<HostGroup, StorageError> {
        let row = sqlx::query(
            "SELECT id, name, parent_id, created_at FROM host_groups WHERE id = ?",
        )
        .bind(id.as_str())
        .fetch_optional(self.db.pool())
        .await
        .map_err(map_err)?;

        let row = row.ok_or_else(|| {
            StorageError::Backend(format!("group {} not found", id.as_str()))
        })?;
        row_to_group(&row)
    }

    #[instrument(level = "debug", skip(self))]
    async fn list(&self) -> Result<Vec<HostGroup>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, name, parent_id, created_at FROM host_groups
             ORDER BY parent_id NULLS FIRST, name COLLATE NOCASE",
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(map_err)?;

        rows.iter().map(row_to_group).collect()
    }

    #[instrument(level = "debug", skip(self, new_name), fields(group_id = %id))]
    async fn rename(&self, id: &GroupId, new_name: &str) -> Result<(), StorageError> {
        let result = sqlx::query("UPDATE host_groups SET name = ? WHERE id = ?")
            .bind(new_name)
            .bind(id.as_str())
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;

        if result.rows_affected() == 0 {
            return Err(StorageError::Backend(format!(
                "group {} not found for rename",
                id.as_str()
            )));
        }
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(group_id = %id, new_parent = ?new_parent))]
    async fn move_to(
        &self,
        id: &GroupId,
        new_parent: Option<&GroupId>,
    ) -> Result<(), StorageError> {
        // Self-parenting is a special case of cycle.
        if let Some(np) = new_parent {
            if np == id {
                return Err(StorageError::Conflict(
                    "group cannot be its own parent".to_string(),
                ));
            }
        }

        // Wrap in a transaction so concurrent moves can't both win the
        // cycle check and produce a cycle.
        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|e| StorageError::Backend(format!("begin tx: {e}")))?;

        // Walk ancestors of new_parent. If we hit `id`, that's a cycle.
        if let Some(np) = new_parent {
            let mut cursor: Option<String> = Some(np.as_str().to_string());
            // Bound the walk to defend against any pre-existing cycles
            // we might inherit from a corrupted DB.
            for _ in 0..1000 {
                let Some(ref current) = cursor else {
                    break;
                };
                if current == id.as_str() {
                    return Err(StorageError::Conflict(format!(
                        "moving group {} under {} would create a cycle",
                        id.as_str(),
                        np.as_str()
                    )));
                }
                let next: Option<String> = sqlx::query_scalar(
                    "SELECT parent_id FROM host_groups WHERE id = ?",
                )
                .bind(current)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_err)?
                .flatten();
                cursor = next;
            }
            if cursor.is_some() {
                return Err(StorageError::Malformed {
                    entity: "host_groups",
                    reason: "ancestor chain exceeds 1000 — DB likely has a cycle".to_string(),
                });
            }
        }

        let result = sqlx::query("UPDATE host_groups SET parent_id = ? WHERE id = ?")
            .bind(new_parent.map(|g| g.as_str()))
            .bind(id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;

        if result.rows_affected() == 0 {
            return Err(StorageError::Backend(format!(
                "group {} not found for move",
                id.as_str()
            )));
        }

        tx.commit()
            .await
            .map_err(|e| StorageError::Backend(format!("commit: {e}")))?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(group_id = %id))]
    async fn delete(&self, id: &GroupId) -> Result<(), StorageError> {
        // ON DELETE CASCADE on host_groups handles sub-groups.
        // ON DELETE SET NULL on hosts.group_id moves contained hosts to root.
        let result = sqlx::query("DELETE FROM host_groups WHERE id = ?")
            .bind(id.as_str())
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;

        if result.rows_affected() == 0 {
            return Err(StorageError::Backend(format!(
                "group {} not found for delete",
                id.as_str()
            )));
        }
        Ok(())
    }
}

fn row_to_group(row: &sqlx::sqlite::SqliteRow) -> Result<HostGroup, StorageError> {
    let id: String = row
        .try_get("id")
        .map_err(|e| StorageError::Backend(format!("read id: {e}")))?;
    let name: String = row
        .try_get("name")
        .map_err(|e| StorageError::Backend(format!("read name: {e}")))?;
    let parent_id: Option<String> = row
        .try_get("parent_id")
        .map_err(|e| StorageError::Backend(format!("read parent_id: {e}")))?;
    let created_at_s: String = row
        .try_get("created_at")
        .map_err(|e| StorageError::Backend(format!("read created_at: {e}")))?;

    Ok(HostGroup {
        id: GroupId::from_raw(id),
        name,
        parent_id: parent_id.map(GroupId::from_raw),
        created_at: parse_datetime("host_groups.created_at", &created_at_s)?,
    })
}
