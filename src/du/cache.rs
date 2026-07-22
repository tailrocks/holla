use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::{NodeId, ScanTree};

const SCHEMA_VERSION: u8 = 3;
const TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct CachedSize {
    pub on_disk: u64,
    pub apparent: u64,
    pub entry_count: u64,
    pub scanned_at: u64,
    pub root_mtime: u64,
}

#[derive(Clone, Debug)]
pub struct SizeSnapshot {
    entries: Vec<(PathBuf, CachedSize)>,
}

#[derive(Clone, Debug, Default)]
pub struct SizeBaseline {
    mtimes: HashMap<PathBuf, u64>,
}

impl SizeBaseline {
    pub fn capture(root: &Path) -> Self {
        Self::capture_cancellable(root, &AtomicBool::new(false))
    }

    pub fn capture_cancellable(root: &Path, cancel: &AtomicBool) -> Self {
        let mut baseline = Self::default();
        baseline.capture_path(root, 0, cancel);
        baseline
    }

    fn capture_path(&mut self, path: &Path, depth: usize, cancel: &AtomicBool) {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        if let Some(mtime) = modified_nanos(path) {
            self.mtimes.insert(path.to_path_buf(), mtime);
        }
        if depth >= 2 {
            return;
        }
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            if cancel.load(Ordering::Acquire) {
                break;
            }
            self.capture_path(&entry.path(), depth + 1, cancel);
        }
    }

    fn unchanged_mtime(&self, path: &Path) -> Option<u64> {
        let before = self.mtimes.get(path).copied()?;
        (modified_nanos(path)? == before).then_some(before)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SizeCache {
    v: u8,
    #[serde(default)]
    entries: HashMap<PathBuf, CachedSize>,
}

impl Default for SizeCache {
    fn default() -> Self {
        Self {
            v: SCHEMA_VERSION,
            entries: HashMap::new(),
        }
    }
}

impl SizeCache {
    pub fn load() -> Self {
        cache_path()
            .as_deref()
            .map(Self::load_from)
            .unwrap_or_default()
    }

    fn load_from(path: &Path) -> Self {
        let Ok(bytes) = fs::read(path) else {
            return Self::default();
        };
        let Ok(cache) = serde_json::from_slice::<Self>(&bytes) else {
            return Self::default();
        };
        if cache.v == SCHEMA_VERSION {
            cache
        } else {
            Self::default()
        }
    }

    pub fn valid(&self, path: &Path, now: SystemTime) -> Option<&CachedSize> {
        let entry = self.entries.get(path)?;
        if !entry_is_valid(path, entry, now) {
            return None;
        }
        Some(entry)
    }

    pub fn valid_below(&self, root: &Path, now: SystemTime) -> Vec<(PathBuf, CachedSize)> {
        if self.valid(root, now).is_none() {
            return Vec::new();
        }
        let mut entries = self
            .entries
            .iter()
            .filter(|(path, _)| *path == root || path.starts_with(root))
            .filter_map(|(path, _)| {
                self.valid(path, now)
                    .cloned()
                    .map(|entry| (path.clone(), entry))
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }

    pub fn capture(&mut self, root: &Path, tree: &ScanTree, scanned_at: SystemTime) {
        self.capture_snapshot(snapshot(
            root,
            tree,
            scanned_at,
            &SizeBaseline::capture(root),
        ));
    }

    pub fn capture_snapshot(&mut self, snapshot: SizeSnapshot) {
        for (path, entry) in snapshot.entries {
            self.entries.insert(path, entry);
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let Some(path) = cache_path() else {
            return Ok(());
        };
        self.save_to(&path)
    }

    fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))?;
        lock.lock()?;

        let mut merged = Self::load_from(path);
        let now = SystemTime::now();
        merged
            .entries
            .retain(|entry_path, entry| entry_is_valid(entry_path, entry, now));
        for (path, entry) in self
            .entries
            .iter()
            .filter(|(entry_path, entry)| entry_is_valid(entry_path, entry, now))
        {
            let replace = merged
                .entries
                .get(path)
                .is_none_or(|current| entry.scanned_at > current.scanned_at);
            if replace {
                merged.entries.insert(path.clone(), entry.clone());
            }
        }
        let bytes = serde_json::to_vec_pretty(&merged).map_err(io::Error::other)?;
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)
    }
}

pub fn snapshot(
    root: &Path,
    tree: &ScanTree,
    scanned_at: SystemTime,
    baseline: &SizeBaseline,
) -> SizeSnapshot {
    let mut entries = Vec::new();
    let scanned_at = epoch_nanos(scanned_at);
    for (index, node) in tree.nodes().iter().enumerate() {
        let id = NodeId(u32::try_from(index).expect("scan tree exceeded u32 nodes"));
        if node_depth(tree, id) > 2 {
            continue;
        }
        let path = node_path(tree, root, id);
        let Some(root_mtime) = baseline.unchanged_mtime(&path) else {
            continue;
        };
        entries.push((
            path,
            CachedSize {
                on_disk: node.on_disk,
                apparent: node.apparent,
                entry_count: node.entry_count,
                scanned_at,
                root_mtime,
            },
        ));
    }
    SizeSnapshot { entries }
}

fn cache_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .map(|cache| cache.join("holla/sizes.json"))
}

fn epoch_nanos(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn entry_is_valid(path: &Path, entry: &CachedSize, now: SystemTime) -> bool {
    let age = now
        .duration_since(UNIX_EPOCH + Duration::from_nanos(entry.scanned_at))
        .unwrap_or_default();
    age <= TTL && modified_nanos(path) == Some(entry.root_mtime)
}

fn modified_nanos(path: &Path) -> Option<u64> {
    let duration = fs::symlink_metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?;
    u64::try_from(duration.as_nanos()).ok()
}

fn node_depth(tree: &ScanTree, mut id: NodeId) -> usize {
    let mut depth = 0;
    while let Some(parent) = tree.node(id).parent {
        depth += 1;
        id = parent;
    }
    depth
}

fn node_path(tree: &ScanTree, root: &Path, id: NodeId) -> PathBuf {
    let mut names = Vec::new();
    let mut current = Some(id);
    while let Some(node_id) = current {
        if node_id == tree.root() {
            break;
        }
        let node = tree.node(node_id);
        names.push(node.name.clone());
        current = node.parent;
    }
    let mut path = root.to_path_buf();
    for name in names.into_iter().rev() {
        path.push(name);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture_tree(root: &Path) -> ScanTree {
        fs::create_dir_all(root.join("one/two/three")).unwrap();
        fs::write(root.join("one/file"), b"cache").unwrap();
        let mut tree = ScanTree::new(root.file_name().unwrap().to_owned(), true);
        let one = tree.add_dir(tree.root(), "one".into());
        let two = tree.add_dir(one, "two".into());
        let three = tree.add_dir(two, "three".into());
        tree.add_sizes(three, 4_096, 5, 1);
        tree.finish_scanning_nodes();
        tree
    }

    #[test]
    fn round_trip_preserves_versioned_entries() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("root");
        let tree = fixture_tree(&root);
        let path = fixture.path().join("sizes.json");
        let mut cache = SizeCache::default();
        cache.capture(&root, &tree, SystemTime::now());
        cache.save_to(&path).unwrap();

        let loaded = SizeCache::load_from(&path);
        assert_eq!(loaded.entries, cache.entries);
        let json: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(json["v"], SCHEMA_VERSION);
    }

    #[test]
    fn expired_entry_is_invalid() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("root");
        let tree = fixture_tree(&root);
        let scanned = SystemTime::now() - TTL - Duration::from_secs(1);
        let mut cache = SizeCache::default();
        cache.capture(&root, &tree, scanned);
        assert!(cache.valid(&root, SystemTime::now()).is_none());
    }

    #[test]
    fn mtime_mismatch_invalidates_entry() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("root");
        let tree = fixture_tree(&root);
        let now = SystemTime::now();
        let mut cache = SizeCache::default();
        cache.capture(&root, &tree, now);
        cache.entries.get_mut(&root).unwrap().root_mtime = 0;
        assert!(cache.valid(&root, now).is_none());
    }

    #[test]
    fn corrupt_and_unknown_files_load_empty() {
        let fixture = tempdir().unwrap();
        let path = fixture.path().join("sizes.json");
        fs::write(&path, b"garbage").unwrap();
        assert!(SizeCache::load_from(&path).entries.is_empty());
        fs::write(&path, br#"{"v":2,"entries":{}}"#).unwrap();
        assert!(SizeCache::load_from(&path).entries.is_empty());
    }

    #[test]
    fn capture_is_bounded_to_root_plus_two_levels() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("root");
        let tree = fixture_tree(&root);
        let mut cache = SizeCache::default();
        cache.capture(&root, &tree, SystemTime::now());

        assert!(cache.entries.contains_key(&root));
        assert!(cache.entries.contains_key(&root.join("one")));
        assert!(cache.entries.contains_key(&root.join("one/two")));
        assert!(!cache.entries.contains_key(&root.join("one/two/three")));
    }

    #[test]
    fn cancelled_baseline_does_no_filesystem_work() {
        let fixture = tempdir().unwrap();
        let cancel = AtomicBool::new(true);

        let baseline = SizeBaseline::capture_cancellable(fixture.path(), &cancel);

        assert!(baseline.mtimes.is_empty());
    }

    #[test]
    fn save_merges_entries_from_independent_scans() {
        let fixture = tempdir().unwrap();
        let path = fixture.path().join("sizes.json");
        let first_root = fixture.path().join("first");
        let second_root = fixture.path().join("second");
        let mut first = SizeCache::default();
        first.capture(&first_root, &fixture_tree(&first_root), SystemTime::now());
        first.save_to(&path).unwrap();
        let mut second = SizeCache::default();
        second.capture(&second_root, &fixture_tree(&second_root), SystemTime::now());
        second.save_to(&path).unwrap();

        let merged = SizeCache::load_from(&path);
        assert!(merged.entries.contains_key(&first_root));
        assert!(merged.entries.contains_key(&second_root));
    }

    #[test]
    fn delayed_older_writer_cannot_replace_newer_entry() {
        let fixture = tempdir().unwrap();
        let path = fixture.path().join("sizes.json");
        let root = fixture.path().join("root");
        let tree = fixture_tree(&root);
        let mut newer = SizeCache::default();
        let now = SystemTime::now();
        newer.capture(&root, &tree, now);
        newer.save_to(&path).unwrap();
        let mut older = SizeCache::default();
        older.capture(&root, &tree, now - Duration::from_secs(1));
        older.save_to(&path).unwrap();

        assert_eq!(
            SizeCache::load_from(&path)
                .entries
                .get(&root)
                .unwrap()
                .scanned_at,
            epoch_nanos(now)
        );
    }

    #[test]
    fn save_prunes_expired_and_missing_entries() {
        let fixture = tempdir().unwrap();
        let path = fixture.path().join("sizes.json");
        let root = fixture.path().join("root");
        let tree = fixture_tree(&root);
        let mut stale = SizeCache::default();
        stale.capture(
            &root,
            &tree,
            SystemTime::now() - TTL - Duration::from_secs(1),
        );
        let bytes = serde_json::to_vec(&stale).unwrap();
        fs::write(&path, bytes).unwrap();

        SizeCache::default().save_to(&path).unwrap();

        assert!(SizeCache::load_from(&path).entries.is_empty());
    }

    #[test]
    fn snapshot_omits_paths_changed_during_scan() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("root");
        let tree = fixture_tree(&root);
        let baseline = SizeBaseline::capture(&root);
        let before = modified_nanos(&root).expect("root mtime");

        // Advance wall clock past 1s mtime granularity used by some filesystems.
        std::thread::sleep(Duration::from_millis(1100));
        fs::write(root.join("changed-after-scan"), b"changed").unwrap();

        let after = modified_nanos(&root).expect("root mtime after write");
        assert_ne!(
            before, after,
            "filesystem did not advance directory mtime after create"
        );

        let snapshot = snapshot(&root, &tree, SystemTime::now(), &baseline);
        assert!(!snapshot.entries.iter().any(|(path, _)| path == &root));
    }
}
