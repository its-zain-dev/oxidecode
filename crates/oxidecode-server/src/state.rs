use anyhow::{Context, Result};
use oxidecode_core::auth;
use oxidecode_core::config::LoadedConfig;
use oxidecode_core::event::EventBus;
use oxidecode_core::paths::Paths;
use oxidecode_core::store::Store;
use oxidecode_provider::ProviderClient;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tokio::sync::{oneshot, watch};

#[derive(Debug, Clone)]
pub struct PermissionAnswer {
    pub allow: bool,
    pub always: bool,
    pub message: Option<String>,
}

pub struct Runtime {
    pub abort: watch::Sender<bool>,
    pub message_id: String,
    pub started: i64,
}

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    pub paths: Paths,
    pub store: Store,
    pub bus: EventBus,
    pub client: ProviderClient,
    pub password: String,
    pub default_directory: PathBuf,
    pub default_agent: String,
    pub started: Instant,
    pub version: String,
    configs: RwLock<HashMap<PathBuf, Arc<LoadedConfig>>>,
    runtimes: Mutex<HashMap<String, Runtime>>,
    approvals: Mutex<HashMap<String, oneshot::Sender<PermissionAnswer>>>,
    pending: Mutex<HashMap<String, VecDeque<String>>>,
    pair_tokens: Mutex<HashMap<String, i64>>,
    pair_counter: AtomicU64,
    pub hostname: String,
    pub port: AtomicU16,
}

impl AppState {
    pub fn new(
        paths: Paths,
        store: Store,
        bus: EventBus,
        password: String,
        default_directory: PathBuf,
        port: u16,
    ) -> Result<Self> {
        paths.ensure().context("creating data directories")?;
        let hostname = hostname();
        Ok(Self {
            inner: Arc::new(Inner {
                paths,
                store,
                bus,
                client: ProviderClient::new(),
                password,
                default_directory,
                default_agent: "build".to_string(),
                started: Instant::now(),
                version: oxidecode_core::API_VERSION.to_string(),
                configs: RwLock::new(HashMap::new()),
                runtimes: Mutex::new(HashMap::new()),
                approvals: Mutex::new(HashMap::new()),
                pending: Mutex::new(HashMap::new()),
                pair_tokens: Mutex::new(HashMap::new()),
                pair_counter: AtomicU64::new(1),
                hostname,
                port: AtomicU16::new(port),
            }),
        })
    }

    pub fn paths(&self) -> &Paths {
        &self.inner.paths
    }

    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    pub fn bus(&self) -> &EventBus {
        &self.inner.bus
    }

    pub fn client(&self) -> &ProviderClient {
        &self.inner.client
    }

    pub fn password(&self) -> &str {
        &self.inner.password
    }

    pub fn verify(&self, candidate: &str) -> bool {
        auth::secret_eq(candidate, &self.inner.password)
    }

    pub fn default_directory(&self) -> PathBuf {
        self.inner.default_directory.clone()
    }

    pub fn default_agent(&self) -> String {
        self.inner.default_agent.clone()
    }

    pub fn hostname(&self) -> &str {
        &self.inner.hostname
    }

    pub fn port(&self) -> u16 {
        self.inner.port.load(Ordering::Relaxed)
    }

    /// Records the port the listener actually bound to (relevant when the
    /// caller asked for port 0).
    pub fn set_port(&self, port: u16) {
        self.inner.port.store(port, Ordering::Relaxed);
    }

    pub fn version(&self) -> &str {
        &self.inner.version
    }

    pub fn started(&self) -> Instant {
        self.inner.started
    }

    /// Loads (and caches) configuration for a directory.
    pub fn config_for(&self, directory: &Path) -> Arc<LoadedConfig> {
        let canonical = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());
        if let Some(cached) = self
            .inner
            .configs
            .read()
            .ok()
            .and_then(|cache| cache.get(&canonical).cloned())
        {
            return cached;
        }
        let loaded = Arc::new(LoadedConfig::load(&self.inner.paths, &canonical));
        if let Ok(mut cache) = self.inner.configs.write() {
            cache.insert(canonical, loaded.clone());
        }
        loaded
    }

    pub fn invalidate_config(&self, directory: &Path) {
        if let Ok(mut cache) = self.inner.configs.write() {
            cache.remove(directory);
        }
    }

    /// Resolves the requested location directory, defaulting to the server cwd.
    pub fn location(&self, requested: Option<&str>) -> PathBuf {
        match requested {
            Some(value) if !value.trim().is_empty() => {
                let path = PathBuf::from(value);
                if path.is_absolute() {
                    path
                } else {
                    self.inner.default_directory.join(path)
                }
            }
            _ => self.inner.default_directory.clone(),
        }
    }

    pub fn register_runtime(&self, session_id: &str, message_id: &str) -> watch::Receiver<bool> {
        let (sender, receiver) = watch::channel(false);
        if let Ok(mut runtimes) = self.inner.runtimes.lock() {
            runtimes.insert(
                session_id.to_string(),
                Runtime {
                    abort: sender,
                    message_id: message_id.to_string(),
                    started: oxidecode_core::ids::now_ms(),
                },
            );
        }
        receiver
    }

    pub fn finish_runtime(&self, session_id: &str) {
        if let Ok(mut runtimes) = self.inner.runtimes.lock() {
            runtimes.remove(session_id);
        }
        // A prompt that arrived while the session was busy starts the next turn.
        let next = self
            .inner
            .pending
            .lock()
            .ok()
            .and_then(|mut queues| queues.get_mut(session_id).and_then(|queue| queue.pop_front()));
        if next.is_some() {
            let state = self.clone();
            let session = session_id.to_string();
            tokio::spawn(async move {
                crate::runner::spawn(state, session);
            });
        }
    }

    /// Stores a prompt that arrived while the session was running.
    pub fn queue_prompt(&self, session_id: &str, message_id: &str) {
        if let Ok(mut queues) = self.inner.pending.lock() {
            queues
                .entry(session_id.to_string())
                .or_default()
                .push_back(message_id.to_string());
        }
    }

    pub fn pending_count(&self, session_id: &str) -> usize {
        self.inner
            .pending
            .lock()
            .ok()
            .and_then(|queues| queues.get(session_id).map(VecDeque::len))
            .unwrap_or(0)
    }

    pub fn running_sessions(&self) -> Vec<(String, String, i64)> {
        self.inner
            .runtimes
            .lock()
            .map(|runtimes| {
                runtimes
                    .iter()
                    .map(|(id, runtime)| {
                        (id.clone(), runtime.message_id.clone(), runtime.started)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn is_running(&self, session_id: &str) -> bool {
        self.inner
            .runtimes
            .lock()
            .map(|runtimes| runtimes.contains_key(session_id))
            .unwrap_or(false)
    }

    pub fn interrupt(&self, session_id: &str) -> bool {
        let manager = match self.inner.runtimes.lock() {
            Ok(manager) => manager,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(runtime) = manager.get(session_id) {
            let _ = runtime.abort.send(true);
            true
        } else {
            false
        }
    }

    pub fn register_approval(&self, id: &str) -> oneshot::Receiver<PermissionAnswer> {
        let (sender, receiver) = oneshot::channel();
        if let Ok(mut approvals) = self.inner.approvals.lock() {
            approvals.insert(id.to_string(), sender);
        }
        receiver
    }

    pub fn resolve_approval(&self, id: &str, answer: PermissionAnswer) -> bool {
        let sender = self
            .inner
            .approvals
            .lock()
            .ok()
            .and_then(|mut approvals| approvals.remove(id));
        match sender {
            Some(sender) => sender.send(answer).is_ok(),
            None => false,
        }
    }

    pub fn cancel_approval(&self, id: &str) {
        if let Ok(mut approvals) = self.inner.approvals.lock() {
            approvals.remove(id);
        }
    }

    /// Creates a single-use pairing token (10 minute lifetime).
    pub fn create_pair_token(&self) -> String {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let token: String = (0..32)
            .map(|_| {
                const ALPHABET: &[u8] =
                    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
                ALPHABET[rng.gen_range(0..ALPHABET.len())] as char
            })
            .collect();
        if let Ok(mut tokens) = self.inner.pair_tokens.lock() {
            let now = oxidecode_core::ids::now_ms();
            tokens.retain(|_, expiry| *expiry > now);
            tokens.insert(token.clone(), now + 10 * 60 * 1000);
        }
        self.inner.pair_counter.fetch_add(1, Ordering::Relaxed);
        token
    }

    pub fn consume_pair_token(&self, token: &str) -> bool {
        let now = oxidecode_core::ids::now_ms();
        self.inner
            .pair_tokens
            .lock()
            .map(|mut tokens| {
                matches!(tokens.remove(token), Some(expiry) if expiry > now)
            })
            .unwrap_or(false)
    }
}

fn hostname() -> String {
    if let Ok(value) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_string())
}
