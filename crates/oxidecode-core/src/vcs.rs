use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Branch {
    pub current: String,
    #[serde(default)]
    pub default: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStatus {
    pub file: String,
    pub additions: u64,
    pub deletions: u64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub file: String,
    pub patch: String,
    pub additions: u64,
    pub deletions: u64,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more: Option<bool>,
}

pub fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn git_root(directory: &Path) -> Option<PathBuf> {
    if let Some(root) = git(directory, &["rev-parse", "--show-toplevel"]) {
        let root = root.trim();
        if !root.is_empty() {
            return Some(PathBuf::from(root));
        }
    }
    let mut current = Some(directory);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

pub fn is_git(directory: &Path) -> bool {
    git(directory, &["rev-parse", "--is-inside-work-tree"])
        .map(|value| value.trim() == "true")
        .unwrap_or(false)
}

pub fn branch(directory: &Path) -> Branch {
    let current = git(directory, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "HEAD".to_string());

    let default = git(directory, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .map(|value| value.trim().trim_start_matches("origin/").to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            for candidate in ["main", "master"] {
                if git(directory, &["rev-parse", "--verify", candidate]).is_some() {
                    return Some(candidate.to_string());
                }
            }
            None
        })
        .unwrap_or_else(|| current.clone());

    Branch { current, default }
}

pub fn branch_list(directory: &Path) -> Vec<String> {
    git(directory, &["branch", "--format=%(refname:short)"])
        .map(|raw| {
            raw.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn diff(directory: &Path, mode: &str) -> Vec<FileDiff> {
    let prefix_args: Vec<&str> = match mode {
        "branch" => vec!["diff", "--no-color", "--no-ext-diff", "HEAD"],
        "committed" => vec!["diff", "--no-color", "--no-ext-diff", "HEAD~1"],
        _ => vec!["diff", "--no-color", "--no-ext-diff"],
    };

    let numstat = git(directory, &[&prefix_args[..], &["--numstat"]].concat())
        .unwrap_or_default();
    let mut files: Vec<FileDiff> = Vec::new();
    for line in numstat.lines() {
        let mut parts = line.split('\t');
        let additions = parts.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
        let deletions = parts.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
        let file = parts.next().unwrap_or("").to_string();
        if file.is_empty() {
            continue;
        }
        let patch = diff_for_file(directory, &prefix_args, &file);
        files.push(FileDiff {
            status: if patch.contains("new file mode") {
                "added".to_string()
            } else if patch.contains("deleted file mode") {
                "deleted".to_string()
            } else {
                "modified".to_string()
            },
            file,
            patch,
            additions,
            deletions,
            more: None,
        });
    }

    // Untracked files do not appear in `git diff`; include them as additions.
    if let Some(raw) = git(directory, &prefix_args) {
        let _ = raw;
    }
    if let Some(raw) = git(directory, &["ls-files", "--others", "--exclude-standard"]) {
        for file in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let patch = std::fs::read_to_string(directory.join(file))
                .map(|content| {
                    let mut patch = String::new();
                    patch.push_str(&format!("--- /dev/null\n+++ b/{file}\n"));
                    for line in content.lines() {
                        patch.push('+');
                        patch.push_str(line);
                        patch.push('\n');
                    }
                    patch
                })
                .unwrap_or_default();
            let additions = patch.lines().filter(|line| line.starts_with('+')).count() as u64;
            files.push(FileDiff {
                file: file.to_string(),
                patch,
                additions,
                deletions: 0,
                status: "added".to_string(),
                more: None,
            });
        }
    }
    files
}

fn diff_for_file(directory: &Path, prefix_args: &[&str], file: &str) -> String {
    let mut args: Vec<&str> = prefix_args.to_vec();
    args.push("--");
    args.push(file);
    git(directory, &args).unwrap_or_default()
}

pub fn status_files(directory: &Path) -> Vec<FileStatus> {
    let mut files = Vec::new();
    if let Some(raw) = git(directory, &["status", "--porcelain=v1", "-z"]) {
        let mut iter = raw.split('\0').filter(|entry| !entry.is_empty());
        while let Some(entry) = iter.next() {
            if entry.len() < 4 {
                continue;
            }
            let code = &entry[..2];
            let file = entry[3..].to_string();
            let status = if code.contains('D') {
                "deleted"
            } else if code.contains('A') || code == "??" {
                "added"
            } else {
                "modified"
            };
            files.push(FileStatus {
                file,
                additions: 0,
                deletions: 0,
                status: status.to_string(),
            });
        }
    }
    files
}

pub fn init(directory: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(directory)
        .arg("init")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
