use super::context::ToolError;
use std::path::{Component, Path, PathBuf};

/// Resolves `input` against `root` and refuses paths that escape it.
pub fn resolve(root: &Path, input: &str) -> Result<PathBuf, ToolError> {
    let path = resolve_raw(root, input);
    let canonical = path.canonicalize().map_err(|error| {
        ToolError::invalid(format!("path {}: {error}", path.display()))
    })?;
    ensure_inside(root, &canonical)?;
    Ok(canonical)
}

/// Like [`resolve`] but tolerates a missing final component (writes).
pub fn resolve_for_write(root: &Path, input: &str) -> Result<PathBuf, ToolError> {
    let path = resolve_raw(root, input);
    if path.exists() {
        let canonical = path.canonicalize().map_err(|error| {
            ToolError::invalid(format!("path {}: {error}", path.display()))
        })?;
        ensure_inside(root, &canonical)?;
        return Ok(canonical);
    }
    let parent = path.parent().unwrap_or(root);
    let parent = parent.canonicalize().map_err(|error| {
        ToolError::invalid(format!("path {}: {error}", parent.display()))
    })?;
    ensure_inside(root, &parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| ToolError::invalid("path has no file name"))?;
    Ok(parent.join(name))
}

fn resolve_raw(root: &Path, input: &str) -> PathBuf {
    let trimmed = input.trim();
    let expanded = if let Some(rest) = trimmed.strip_prefix("~/") {
        dirs_home().join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        root.join(expanded)
    }
}

fn ensure_inside(root: &Path, candidate: &Path) -> Result<(), ToolError> {
    let root = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());
    if candidate.starts_with(&root) {
        return Ok(());
    }
    // Allow references to the parent of a git worktree only when explicitly
    // under the same project root; everything else is an escape.
    Err(ToolError::invalid(format!(
        "path {} is outside the working directory {}",
        candidate.display(),
        root.display()
    )))
}

pub fn home() -> PathBuf {
    dirs_home()
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Lexically normalises `.` and `..` without touching the filesystem.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Formats a path relative to `root` with forward slashes.
pub fn display_relative(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
