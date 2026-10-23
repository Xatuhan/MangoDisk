use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{filesystem::atomic_replace::replace_file, shared::application_paths};

const SCHEMA_VERSION: u32 = 1;
const MAX_ENTRIES: usize = 1024;
const MAX_BYTES: u64 = 1024 * 1024;
const RETENTION_MS: u64 = 365 * 24 * 60 * 60 * 1000;
static WRITER_RUNNING: AtomicBool = AtomicBool::new(false);
static STORE: OnceLock<Mutex<UsageState>> = OnceLock::new();

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Observation {
    identifier: String,
    path: PathBuf,
    timestamp_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UsageDocument {
    schema_version: u32,
    observations: Vec<Observation>,
}

#[derive(Default)]
struct UsageState {
    loaded: bool,
    writable: bool,
    revision: u64,
    persisted_revision: u64,
    observations: HashMap<(String, PathBuf), Observation>,
}

/// Application-scoped foreground evidence. This never records document or window contents.
pub struct ApplicationUsageService;

pub struct ApplicationUsagePersistence {
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Drop for ApplicationUsagePersistence {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl ApplicationUsageService {
    /// Notification callbacks only update bounded memory; the adapter owns the writer lifetime.
    pub fn observe(identifier: String, path: PathBuf, timestamp_ms: u64) {
        if !valid(&identifier, &path, timestamp_ms, now_millis()) {
            return;
        }
        if let Ok(mut state) = store().lock() {
            if insert(
                &mut state,
                Observation {
                    identifier,
                    path,
                    timestamp_ms,
                },
            ) {
                state.revision = state.revision.wrapping_add(1);
                prune(&mut state, now_millis());
            }
        }
    }

    pub fn start_persistence() -> std::io::Result<ApplicationUsagePersistence> {
        if WRITER_RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "application usage writer already running",
            ));
        }
        let (stop, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("application-usage-writer".into())
            .spawn(move || {
                struct WriterGuard;
                impl Drop for WriterGuard {
                    fn drop(&mut self) {
                        WRITER_RUNNING.store(false, Ordering::Release);
                    }
                }
                let _guard = WriterGuard;
                load();
                loop {
                    let stopping = !matches!(
                        receiver.recv_timeout(Duration::from_secs(30)),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    );
                    flush();
                    if stopping {
                        break;
                    }
                }
            })
            .inspect_err(|_| WRITER_RUNNING.store(false, Ordering::Release))?;
        Ok(ApplicationUsagePersistence {
            stop,
            worker: Some(worker),
        })
    }

    pub(crate) fn dates() -> HashMap<(String, PathBuf), u64> {
        load();
        store()
            .lock()
            .map(|state| {
                state
                    .observations
                    .iter()
                    .map(|(key, value)| (key.clone(), value.timestamp_ms))
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn store() -> &'static Mutex<UsageState> {
    STORE.get_or_init(|| Mutex::new(UsageState::default()))
}
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default()
}
fn path() -> Option<PathBuf> {
    application_paths()
        .ok()
        .map(|paths| paths.data_directory().join("application-usage.json"))
}

fn valid(identifier: &str, path: &Path, timestamp: u64, now: u64) -> bool {
    !identifier.is_empty()
        && identifier.len() <= 255
        && !identifier.chars().any(char::is_control)
        && path.is_absolute()
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        && timestamp > 0
        && timestamp <= now
        && timestamp >= now.saturating_sub(RETENTION_MS)
}

fn insert(state: &mut UsageState, observation: Observation) -> bool {
    let key = (observation.identifier.clone(), observation.path.clone());
    if state
        .observations
        .get(&key)
        .is_some_and(|existing| existing.timestamp_ms >= observation.timestamp_ms)
    {
        return false;
    }
    state.observations.insert(key, observation);
    true
}

fn prune(state: &mut UsageState, now: u64) {
    state
        .observations
        .retain(|_, value| valid(&value.identifier, &value.path, value.timestamp_ms, now));
    while state.observations.len() > MAX_ENTRIES {
        let Some(key) = state
            .observations
            .iter()
            .min_by_key(|(_, value)| value.timestamp_ms)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        state.observations.remove(&key);
    }
}

fn read_document(path: &Path, now: u64) -> Result<Vec<Observation>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_BYTES {
        return Err("unsupported file type or size".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("document exceeds read limit".into());
    }
    let document: UsageDocument =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if document.schema_version != SCHEMA_VERSION || document.observations.len() > MAX_ENTRIES {
        return Err("unsupported application usage schema or entry count".into());
    }
    Ok(document
        .observations
        .into_iter()
        .filter(|value| valid(&value.identifier, &value.path, value.timestamp_ms, now))
        .collect())
}

fn load() {
    if store().lock().map_or(true, |state| state.loaded) {
        return;
    }
    let Some(path) = path() else {
        return;
    };
    // Loading is performed by the writer or a scan worker, never by native UI notifications.
    let loaded = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        _ => read_document(&path, now_millis()),
    };
    let Ok(mut state) = store().lock() else {
        return;
    };
    if state.loaded {
        return;
    }
    state.loaded = true;
    match loaded {
            Ok(observations) => {
                for observation in observations { insert(&mut state, observation); }
                prune(&mut state, now_millis());
                state.writable = true;
            }
            Err(error) => log::warn!("application_usage_load_failed path={} stage=read native_error={} outcome=preserved_read_only", mangodisk_platform::diagnostics::text(&path.display()), mangodisk_platform::diagnostics::text(&error)),
    }
}

fn flush() {
    let Some(path) = path() else {
        return;
    };
    let Some((observations, revision)) = store().lock().ok().and_then(|state| {
        (state.writable && state.revision != state.persisted_revision).then(|| {
            (
                state.observations.values().cloned().collect::<Vec<_>>(),
                state.revision,
            )
        })
    }) else {
        return;
    };
    if let Err(error) = write_document(&path, observations) {
        log::warn!("application_usage_save_failed path={} stage=atomic_replace native_error={} outcome=memory_retained", mangodisk_platform::diagnostics::text(&path.display()), mangodisk_platform::diagnostics::text(&error));
    } else if let Ok(mut state) = store().lock() {
        state.persisted_revision = revision;
    }
}

fn write_document(path: &Path, observations: Vec<Observation>) -> Result<(), String> {
    let parent = path.parent().ok_or("missing parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(&UsageDocument {
        schema_version: SCHEMA_VERSION,
        observations,
    })
    .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("application usage document exceeds write limit".into());
    }
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    let temporary = file.into_temp_path();
    replace_file(&temporary, path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_load_preserves_newer_in_memory_observations_and_separate_copies() {
        let root = tempfile::tempdir().unwrap();
        let mut state = UsageState::default();
        let mut item = Observation {
            identifier: "com.example.App".into(),
            path: root.path().join("first/App.app"),
            timestamp_ms: 20,
        };
        assert!(insert(&mut state, item.clone()));
        item.timestamp_ms = 10;
        assert!(!insert(&mut state, item.clone()));
        item.path = root.path().join("second/App.app");
        assert!(insert(&mut state, item));
        assert_eq!(state.observations.len(), 2);
    }

    #[test]
    fn usage_document_round_trip_and_unknown_schema_preservation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("usage.json");
        let now = now_millis();
        write_document(
            &path,
            vec![Observation {
                identifier: "com.example.App".into(),
                path: root.path().join("App.app"),
                timestamp_ms: now,
            }],
        )
        .unwrap();
        assert_eq!(read_document(&path, now).unwrap().len(), 1);
        let bytes = br#"{"schemaVersion":99,"observations":[]}"#;
        fs::write(&path, bytes).unwrap();
        assert!(read_document(&path, now).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
