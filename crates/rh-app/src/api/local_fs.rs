//! `fs_*` Tauri commands — browse the LOCAL filesystem for the SFTP
//! left pane. Read-only listing + home resolution. Remote (right pane)
//! is handled separately by the SFTP session commands.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::api::error::{ApiError, ApiResult};

#[derive(Debug, Serialize)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix epoch seconds of last modification, if available.
    pub modified: Option<i64>,
    /// POSIX permission string (hosts only); always None for local.
    pub perms: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FsListResponse {
    /// Absolute, normalized path that was listed.
    pub path: String,
    /// Parent directory, or `None` at a filesystem root.
    pub parent: Option<String>,
    pub entries: Vec<FsEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsListRequest {
    pub path: String,
}

/// Rename an entry in place (same directory), keeping its parent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsRenameRequest {
    pub path: String,
    pub new_name: String,
}

/// Delete a file or directory (directories recursively).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsRemoveRequest {
    pub path: String,
    pub is_dir: bool,
}

/// Create a new directory `name` inside `parent`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsMkdirRequest {
    pub parent: String,
    pub name: String,
}

fn home_dir() -> PathBuf {
    #[cfg(windows)]
    let home = std::env::var("USERPROFILE").ok();
    #[cfg(not(windows))]
    let home = std::env::var("HOME").ok();
    home.filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Strip the Windows verbatim prefix (`\\?\`, `\\?\UNC\`) so paths shown to
/// the user — and used for breadcrumbs — stay clean. read_dir works either way.
fn clean(p: &str) -> String {
    if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = p.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        p.to_string()
    }
}

fn list_dir(dir: &Path) -> ApiResult<FsListResponse> {
    let canonical = dir
        .canonicalize()
        .unwrap_or_else(|_| dir.to_path_buf());

    let read = std::fs::read_dir(&canonical)
        .map_err(|e| ApiError::Internal { message: format!("read_dir {}: {e}", canonical.display()) })?;

    let mut entries: Vec<FsEntry> = Vec::new();
    for item in read.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        let meta = item.metadata().ok();
        let is_dir = meta.as_ref().map(std::fs::Metadata::is_dir).unwrap_or(false);
        let size = meta.as_ref().map(std::fs::Metadata::len).unwrap_or(0);
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        entries.push(FsEntry {
            name,
            path: clean(&item.path().to_string_lossy()),
            is_dir,
            size: if is_dir { 0 } else { size },
            modified,
            perms: None,
        });
    }
    // Directories first, then case-insensitive by name.
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let parent = canonical
        .parent()
        .map(|p| clean(&p.to_string_lossy()));

    Ok(FsListResponse {
        path: clean(&canonical.to_string_lossy()),
        parent,
        entries,
    })
}

/// List available drive roots ("This PC"). Windows enumerates C:..Z:; other
/// platforms have a single root.
#[tauri::command]
#[instrument(level = "debug")]
pub async fn fs_drives() -> ApiResult<FsListResponse> {
    let mut entries: Vec<FsEntry> = Vec::new();
    #[cfg(windows)]
    {
        for c in b'A'..=b'Z' {
            let root = format!("{}:\\", c as char);
            if Path::new(&root).exists() {
                entries.push(FsEntry {
                    name: format!("{}:", c as char),
                    path: root,
                    is_dir: true,
                    size: 0,
                    modified: None,
                    perms: None,
                });
            }
        }
    }
    #[cfg(not(windows))]
    {
        entries.push(FsEntry {
            name: "/".to_string(),
            path: "/".to_string(),
            is_dir: true,
            size: 0,
            modified: None,
            perms: None,
        });
    }
    Ok(FsListResponse {
        path: String::new(),
        parent: None,
        entries,
    })
}

#[tauri::command]
#[instrument(level = "debug")]
pub async fn fs_home() -> ApiResult<FsListResponse> {
    list_dir(&home_dir())
}

#[tauri::command]
#[instrument(level = "debug")]
pub async fn fs_list(req: FsListRequest) -> ApiResult<FsListResponse> {
    list_dir(Path::new(&req.path))
}

#[tauri::command]
#[instrument(level = "debug")]
pub async fn fs_rename(req: FsRenameRequest) -> ApiResult<()> {
    let src = Path::new(&req.path);
    let parent = src.parent().unwrap_or_else(|| Path::new("."));
    let dest = parent.join(&req.new_name);
    std::fs::rename(src, &dest).map_err(|e| ApiError::Internal {
        message: format!("rename: {e}"),
    })?;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug")]
pub async fn fs_remove(req: FsRemoveRequest) -> ApiResult<()> {
    let p = Path::new(&req.path);
    let res = if req.is_dir {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    };
    res.map_err(|e| ApiError::Internal {
        message: format!("remove: {e}"),
    })?;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug")]
pub async fn fs_mkdir(req: FsMkdirRequest) -> ApiResult<()> {
    let dest = Path::new(&req.parent).join(&req.name);
    std::fs::create_dir(&dest).map_err(|e| ApiError::Internal {
        message: format!("mkdir: {e}"),
    })?;
    Ok(())
}
