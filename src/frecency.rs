use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const SCHEMA_VERSION: u8 = 1;
const MAX_USES: usize = 20;
const MAX_AGE_SECS: u64 = 90 * 24 * 60 * 60;
const DECAY_PER_DAY: f64 = 0.0693;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ActionHistory {
    #[serde(default)]
    pub uses: Vec<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FrecencyStore {
    v: u8,
    #[serde(default)]
    pub actions: HashMap<String, ActionHistory>,
    #[serde(default)]
    pub queries: HashMap<String, String>,
    #[serde(skip)]
    pending_uses: Vec<(String, u64)>,
    #[serde(skip)]
    pending_queries: HashMap<String, String>,
}

impl Default for FrecencyStore {
    fn default() -> Self {
        Self {
            v: SCHEMA_VERSION,
            actions: HashMap::new(),
            queries: HashMap::new(),
            pending_uses: Vec::new(),
            pending_queries: HashMap::new(),
        }
    }
}

impl FrecencyStore {
    pub fn load() -> Self {
        if history_disabled() {
            return Self::default();
        }
        cache_path()
            .as_deref()
            .map(Self::load_from)
            .unwrap_or_default()
    }

    fn load_from(path: &Path) -> Self {
        let Ok(bytes) = fs::read(path) else {
            return Self::default();
        };
        let Ok(store) = serde_json::from_slice::<Self>(&bytes) else {
            return Self::default();
        };
        if store.v == SCHEMA_VERSION {
            store
        } else {
            Self::default()
        }
    }

    pub fn record(&mut self, action_id: &str, query: &str, now: u64) {
        let uses = &mut self.actions.entry(action_id.to_owned()).or_default().uses;
        uses.push(now);
        self.pending_uses.push((action_id.to_owned(), now));
        if uses.len() > MAX_USES {
            uses.drain(..uses.len() - MAX_USES);
        }

        let query = normalize_query(query);
        if !query.is_empty() {
            self.queries.insert(query.clone(), action_id.to_owned());
            self.pending_queries.insert(query, action_id.to_owned());
        }
    }

    pub fn score(&self, action_id: &str, now: u64) -> f64 {
        self.actions
            .get(action_id)
            .map(|history| frecency_score(&history.uses, now))
            .unwrap_or_default()
    }

    pub fn remembered_action(&self, query: &str) -> Option<&str> {
        self.queries
            .get(&normalize_query(query))
            .map(String::as_str)
    }

    pub fn save(mut self, now: u64) -> io::Result<()> {
        if history_disabled() {
            return Ok(());
        }
        let Some(path) = cache_path() else {
            return Ok(());
        };
        self.save_to(&path, now)
    }

    fn save_to(&mut self, path: &Path, now: u64) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock_path = path.with_extension("lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        // SAFETY: flock only operates on this valid, owned file descriptor.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }

        let mut merged = Self::load_from(path);
        for (action_id, timestamp) in self.pending_uses.drain(..) {
            let uses = &mut merged.actions.entry(action_id).or_default().uses;
            uses.push(timestamp);
            if uses.len() > MAX_USES {
                uses.drain(..uses.len() - MAX_USES);
            }
        }
        merged.queries.extend(self.pending_queries.drain());
        merged.prune(now);
        let bytes = serde_json::to_vec_pretty(&merged).map_err(io::Error::other)?;
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)?;
        *self = merged;
        Ok(())
    }

    fn prune(&mut self, now: u64) {
        self.actions.retain(|_, history| {
            history
                .uses
                .last()
                .is_some_and(|last| now.saturating_sub(*last) <= MAX_AGE_SECS)
        });
        self.queries
            .retain(|_, action_id| self.actions.contains_key(action_id));
    }
}

pub fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn normalize_query(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn frecency_score(uses: &[u64], now: u64) -> f64 {
    let raw = uses
        .iter()
        .map(|used| {
            let age_days = now.saturating_sub(*used) as f64 / 86_400.0;
            (-DECAY_PER_DAY * age_days).exp()
        })
        .sum::<f64>();
    if raw > 10.0 {
        10.0 + (raw - 10.0).max(0.0).sqrt()
    } else {
        raw
    }
}

fn history_disabled() -> bool {
    std::env::var_os("HOLLA_NO_HISTORY").is_some_and(|value| value == "1")
}

fn cache_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .map(|cache| cache.join("holla/frecency.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString, sync::Mutex};
    use tempfile::tempdir;

    const DAY: u64 = 86_400;
    const NOW: u64 = 2_000_000_000;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore {
        key: &'static str,
        old: Option<OsString>,
    }

    impl EnvRestore {
        fn set(key: &'static str, value: &Path) -> Self {
            let old = std::env::var_os(key);
            // SAFETY: these tests serialize all mutation of the variables they touch.
            unsafe { std::env::set_var(key, value) };
            Self { key, old }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            // SAFETY: the matching test still holds ENV_LOCK while guards are dropped.
            unsafe {
                match &self.old {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn recent_use_scores_above_old_use() {
        assert!(frecency_score(&[NOW], NOW) > frecency_score(&[NOW - 20 * DAY], NOW));
    }

    #[test]
    fn ten_day_old_use_has_half_weight() {
        assert!((frecency_score(&[NOW - 10 * DAY], NOW) - 0.5).abs() < 0.001);
    }

    #[test]
    fn future_clock_skew_counts_as_current() {
        assert_eq!(frecency_score(&[NOW + DAY], NOW), 1.0);
    }

    #[test]
    fn high_use_counts_are_diminished() {
        let uses = vec![NOW; 20];
        let score = frecency_score(&uses, NOW);
        assert!(score > 10.0);
        assert!(score < 20.0);
    }

    #[test]
    fn many_old_uses_do_not_outrank_one_current_use() {
        let old = vec![NOW - 90 * DAY; 20];
        assert!(frecency_score(&old, NOW) < frecency_score(&[NOW], NOW));
    }

    #[test]
    fn record_keeps_only_last_twenty_uses() {
        let mut store = FrecencyStore::default();
        for timestamp in 0..25 {
            store.record("action", "", timestamp);
        }
        assert_eq!(store.actions["action"].uses, (5..25).collect::<Vec<_>>());
    }

    #[test]
    fn query_memory_is_normalized() {
        let mut store = FrecencyStore::default();
        store.record("action", "  Docker   CLEAN ", NOW);
        assert_eq!(store.remembered_action("docker clean"), Some("action"));
    }

    #[test]
    fn corrupt_file_loads_as_empty_store() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("frecency.json");
        fs::write(&path, b"not json").unwrap();
        let store = FrecencyStore::load_from(&path);
        assert!(store.actions.is_empty());
        assert!(store.queries.is_empty());
    }

    #[test]
    fn unknown_schema_loads_as_empty_store() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("frecency.json");
        fs::write(&path, br#"{"v":2,"actions":{},"queries":{}}"#).unwrap();
        assert!(FrecencyStore::load_from(&path).actions.is_empty());
    }

    #[test]
    fn missing_schema_loads_as_empty_store() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("frecency.json");
        fs::write(&path, br#"{"actions":{"old":{"uses":[1]}},"queries":{}}"#).unwrap();
        assert!(FrecencyStore::load_from(&path).actions.is_empty());
    }

    #[test]
    fn sequential_writers_merge_new_uses_instead_of_losing_updates() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("frecency.json");
        let mut first = FrecencyStore::default();
        let mut second = FrecencyStore::default();
        first.record("action", "one", NOW);
        second.record("action", "two", NOW + 1);

        first.save_to(&path, NOW + 1).unwrap();
        second.save_to(&path, NOW + 1).unwrap();

        let merged = FrecencyStore::load_from(&path);
        assert_eq!(merged.actions["action"].uses, [NOW, NOW + 1]);
        assert_eq!(merged.remembered_action("one"), Some("action"));
        assert_eq!(merged.remembered_action("two"), Some("action"));
    }

    #[test]
    fn save_includes_version_and_prunes_stale_actions_and_queries() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("cache/holla/frecency.json");
        let mut store = FrecencyStore::default();
        store.record("fresh", "keep", NOW);
        store.record("stale", "drop", NOW - MAX_AGE_SECS - 1);
        store.save_to(&path, NOW).unwrap();

        let json: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(json["v"], 1);
        assert!(json["actions"].get("fresh").is_some());
        assert!(json["actions"].get("stale").is_none());
        assert!(json["queries"].get("keep").is_some());
        assert!(json["queries"].get("drop").is_none());
    }

    #[test]
    fn no_history_environment_switch_creates_no_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        let temp = tempdir().unwrap();
        let _cache = EnvRestore::set("XDG_CACHE_HOME", temp.path());
        let _disabled = EnvRestore::set("HOLLA_NO_HISTORY", Path::new("1"));
        let path = temp.path().join("holla/frecency.json");
        let mut store = FrecencyStore::load();
        store.record("action", "query", NOW);

        store.save(NOW).unwrap();

        assert!(!path.exists());
    }
}
