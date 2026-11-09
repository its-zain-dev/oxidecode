use std::env;
use std::path::{Path, PathBuf};

/// XDG-aware application paths.
///
/// OxideCode keeps its own state under `oxidecode/` so it can coexist with the
/// official OpenCode installation, while still reading OpenCode's configuration
/// (and, when asked, its service password file).
#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
    pub tmp: PathBuf,
    pub opencode_config: PathBuf,
    pub opencode_data: PathBuf,
}

fn env_path(key: &str) -> Option<PathBuf> {
    env::var_os(key).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

impl Paths {
    pub fn discover() -> Self {
        let home = env_path("HOME").unwrap_or_else(|| PathBuf::from("/"));
        let config = env_path("XDG_CONFIG_HOME")
            .unwrap_or_else(|| home.join(".config"))
            .join("oxidecode");
        let data = env_path("XDG_DATA_HOME")
            .unwrap_or_else(|| home.join(".local/share"))
            .join("oxidecode");
        let state = env_path("XDG_STATE_HOME")
            .unwrap_or_else(|| home.join(".local/state"))
            .join("oxidecode");
        let cache = env_path("XDG_CACHE_HOME")
            .unwrap_or_else(|| home.join(".cache"))
            .join("oxidecode");
        let tmp = env_path("TMPDIR")
            .or_else(|| {
                let termux = PathBuf::from("/data/data/com.termux/files/usr/tmp");
                if termux.is_dir() {
                    Some(termux)
                } else {
                    None
                }
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("oxidecode");
        let opencode_config = env_path("XDG_CONFIG_HOME")
            .unwrap_or_else(|| home.join(".config"))
            .join("opencode");
        let opencode_data = env_path("XDG_DATA_HOME")
            .unwrap_or_else(|| home.join(".local/share"))
            .join("opencode");
        Self {
            home,
            config,
            data,
            state,
            cache,
            tmp,
            opencode_config,
            opencode_data,
        }
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [&self.config, &self.data, &self.state, &self.cache, &self.tmp] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn database(&self) -> PathBuf {
        self.data.join("oxidecode.db")
    }

    pub fn opencode_database(&self) -> PathBuf {
        env_path("OPENCODE_DB").unwrap_or_else(|| self.opencode_data.join("opencode.db"))
    }

    /// Candidate `auth.json` credential files from lowest to highest precedence.
    pub fn auth_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for dir in [
            &self.opencode_config,
            &self.opencode_data,
            &self.config,
            &self.data,
        ] {
            let candidate = dir.join("auth.json");
            if candidate.is_file() {
                out.push(candidate);
            }
        }
        out
    }

    pub fn service_file(&self) -> PathBuf {
        self.config.join("service.json")
    }

    pub fn log_file(&self) -> PathBuf {
        self.state.join("oxidecode.log")
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        self.data.join("worktrees")
    }

    /// The global OpenCode configuration file, if present.
    pub fn opencode_global_config(&self) -> Option<PathBuf> {
        for name in ["opencode.jsonc", "opencode.json"] {
            let path = self.opencode_config.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
        None
    }

    pub fn is_within(path: &Path, root: &Path) -> bool {
        path.starts_with(root)
    }
}
