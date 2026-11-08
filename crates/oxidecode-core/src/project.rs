use std::path::{Path, PathBuf};

/// Resolves the project root used for IDs, worktrees and instructions.
pub fn root_for(directory: &Path) -> PathBuf {
    let canonical = directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    crate::vcs::git_root(&canonical).unwrap_or(canonical)
}

pub fn display_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("workspace")
        .to_string()
}
