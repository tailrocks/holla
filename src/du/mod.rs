mod hardlinks;
mod platform;
mod tree;
mod walker;

use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{Arc, RwLock, atomic::AtomicBool, mpsc},
    time::Duration,
};

pub use platform::should_skip;
pub use tree::{ErrKind, Node, NodeId, NodeState, ScanError, ScanTree, SortKey};

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub root: PathBuf,
    pub follow_hidden: bool,
    pub skip_paths: Vec<PathBuf>,
    pub workers: usize,
}

impl ScanOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            skip_paths: platform::default_skip_paths(&root),
            root,
            follow_hidden: true,
            workers: platform::default_workers(),
        }
    }
}

#[derive(Debug)]
pub enum ScanEvent {
    DirAdded {
        id: NodeId,
        parent: NodeId,
        name: OsString,
    },
    SizesUpdated,
    DirErrored {
        id: NodeId,
        kind: ErrKind,
    },
    Progress {
        dirs_done: u64,
        bytes_seen: u64,
    },
    Finished {
        duration: Duration,
        inaccessible: u64,
    },
}

pub struct ScanHandle {
    pub events: mpsc::Receiver<ScanEvent>,
    pub cancel: Arc<AtomicBool>,
    pub tree: Arc<RwLock<ScanTree>>,
}

impl Drop for ScanHandle {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

pub fn scan(options: ScanOptions) -> ScanHandle {
    let root_name = options
        .root
        .file_name()
        .unwrap_or_else(|| options.root.as_os_str())
        .to_owned();
    let tree = Arc::new(RwLock::new(ScanTree::new(root_name, true)));
    let cancel = Arc::new(AtomicBool::new(false));
    // Bounded delivery applies backpressure if a consumer stops draining a
    // huge scan; coalescible updates use `try_send` inside the walker.
    let (events, receiver) = mpsc::sync_channel(4_096);
    let worker_tree = Arc::clone(&tree);
    let worker_cancel = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name("holla-du-scan".into())
        .spawn(move || walker::run(options, worker_tree, worker_cancel, events))
        .expect("failed to spawn disk scan coordinator");
    ScanHandle {
        events: receiver,
        cancel,
        tree,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::Write,
        path::Path,
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    use tempfile::tempdir;

    fn wait_for_finish(handle: &ScanHandle) -> (Duration, u64) {
        loop {
            if let ScanEvent::Finished {
                duration,
                inaccessible,
            } = handle
                .events
                .recv_timeout(Duration::from_secs(10))
                .expect("scan event")
            {
                return (duration, inaccessible);
            }
        }
    }

    #[cfg(unix)]
    fn metadata_sizes(path: &Path) -> (u64, u64) {
        use std::os::unix::fs::MetadataExt;

        let metadata = fs::symlink_metadata(path).expect("metadata");
        (metadata.blocks() * 512, metadata.size())
    }

    #[cfg(not(unix))]
    fn metadata_sizes(path: &Path) -> (u64, u64) {
        let size = fs::symlink_metadata(path).expect("metadata").len();
        (size, size)
    }

    #[test]
    fn fixture_scan_counts_apparent_bytes_and_entries() {
        let fixture = tempdir().expect("fixture");
        fs::create_dir(fixture.path().join("nested")).expect("nested");
        fs::write(fixture.path().join("one"), vec![1_u8; 17]).expect("one");
        fs::write(fixture.path().join("nested/two"), vec![2_u8; 33]).expect("two");
        let expected = [
            fixture.path().to_path_buf(),
            fixture.path().join("nested"),
            fixture.path().join("one"),
            fixture.path().join("nested/two"),
        ]
        .iter()
        .map(|path| metadata_sizes(path))
        .fold((0_u64, 0_u64), |total, size| {
            (total.0 + size.0, total.1 + size.1)
        });

        let handle = scan(ScanOptions::new(fixture.path()));
        let (_, inaccessible) = wait_for_finish(&handle);
        let tree = handle.tree.read().expect("tree");
        let root = tree.node(tree.root());

        assert_eq!(inaccessible, 0);
        assert_eq!(root.entry_count, 2);
        assert_eq!((root.on_disk, root.apparent), expected);
        assert_eq!(root.state, NodeState::Done);
        assert!(
            tree.nodes().iter().any(|node| !node.is_dir),
            "files must be arena nodes for the analyzer UI"
        );
    }

    #[test]
    fn hidden_files_are_optional() {
        let fixture = tempdir().expect("fixture");
        fs::write(fixture.path().join("visible"), b"a").expect("visible");
        fs::write(fixture.path().join(".hidden"), b"b").expect("hidden");
        let mut options = ScanOptions::new(fixture.path());
        options.follow_hidden = false;

        let handle = scan(options);
        wait_for_finish(&handle);

        assert_eq!(
            handle
                .tree
                .read()
                .expect("tree")
                .node(NodeId(0))
                .entry_count,
            1
        );
    }

    #[test]
    fn hardlinks_count_as_entries_but_only_one_size() {
        let fixture = tempdir().expect("fixture");
        let original = fixture.path().join("original");
        fs::write(&original, vec![7_u8; 8_192]).expect("original");
        fs::hard_link(&original, fixture.path().join("linked")).expect("hard link");
        let metadata = fs::metadata(&original).expect("metadata");
        #[cfg(unix)]
        let expected_on_disk = {
            use std::os::unix::fs::MetadataExt;
            metadata.blocks() * 512
        };
        #[cfg(not(unix))]
        let expected_on_disk = metadata.len();

        let handle = scan(ScanOptions::new(fixture.path()));
        wait_for_finish(&handle);
        let tree = handle.tree.read().expect("tree");
        let root = tree.node(tree.root());

        assert_eq!(root.entry_count, 2);
        assert_eq!(
            root.apparent - fs::metadata(fixture.path()).unwrap().len(),
            8_192
        );
        assert_eq!(
            root.on_disk - {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    fs::metadata(fixture.path()).unwrap().blocks() * 512
                }
                #[cfg(not(unix))]
                {
                    fs::metadata(fixture.path()).unwrap().len()
                }
            },
            expected_on_disk
        );
    }

    #[test]
    fn skipped_subtree_is_not_counted() {
        let fixture = tempdir().expect("fixture");
        let skipped = fixture.path().join("skip");
        fs::create_dir(&skipped).expect("skip dir");
        fs::write(skipped.join("large"), vec![0_u8; 4_096]).expect("large");
        fs::write(fixture.path().join("keep"), b"x").expect("keep");
        let mut options = ScanOptions::new(fixture.path());
        options.skip_paths = vec![skipped];

        let handle = scan(options);
        wait_for_finish(&handle);

        assert_eq!(
            handle
                .tree
                .read()
                .expect("tree")
                .node(NodeId(0))
                .entry_count,
            1
        );
    }

    #[test]
    fn cancellation_finishes_promptly_with_partial_tree() {
        let fixture = tempdir().expect("fixture");
        for index in 0..2_000 {
            fs::create_dir(fixture.path().join(format!("dir-{index}"))).expect("dir");
        }
        let mut options = ScanOptions::new(fixture.path());
        options.workers = 2;
        let handle = scan(options);
        loop {
            match handle
                .events
                .recv_timeout(Duration::from_secs(10))
                .expect("first scan event")
            {
                ScanEvent::Progress { .. } | ScanEvent::DirAdded { .. } => break,
                ScanEvent::Finished { .. } => panic!("scan completed before cancellation"),
                _ => {}
            }
        }
        let started = Instant::now();
        handle.cancel.store(true, Ordering::Relaxed);

        wait_for_finish(&handle);

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            handle.tree.read().expect("tree").nodes().len() < 2_001,
            "cancelled scan unexpectedly completed every directory"
        );
    }

    #[test]
    fn scan_streams_directory_and_progress_events() {
        let fixture = tempdir().expect("fixture");
        fs::create_dir(fixture.path().join("nested")).expect("nested");
        let handle = scan(ScanOptions::new(fixture.path()));
        let mut saw_dir = false;
        let mut saw_progress = false;
        let mut final_dirs_done = 0;
        loop {
            match handle.events.recv_timeout(Duration::from_secs(10)).unwrap() {
                ScanEvent::DirAdded { name, .. } if name == "nested" => saw_dir = true,
                ScanEvent::Progress { dirs_done, .. } => {
                    saw_progress = true;
                    final_dirs_done = final_dirs_done.max(dirs_done);
                }
                ScanEvent::Finished { .. } => break,
                _ => {}
            }
        }
        assert!(saw_dir);
        assert!(saw_progress);
        assert_eq!(final_dirs_done, 2, "root and nested directory completed");
        assert!(
            handle
                .tree
                .read()
                .expect("tree")
                .nodes()
                .iter()
                .all(|node| !node.is_dir || node.state == NodeState::Done)
        );
    }

    #[test]
    fn hundred_files_have_exact_entry_count() {
        let fixture = tempdir().expect("fixture");
        for index in 0..100 {
            let mut file =
                fs::File::create(fixture.path().join(format!("file-{index}"))).expect("file");
            file.write_all(&[0_u8; 13]).expect("write");
        }
        let handle = scan(ScanOptions::new(fixture.path()));
        wait_for_finish(&handle);

        assert_eq!(
            handle
                .tree
                .read()
                .expect("tree")
                .node(NodeId(0))
                .entry_count,
            100
        );
    }

    #[test]
    fn file_root_is_a_leaf_with_its_own_size() {
        let fixture = tempdir().expect("fixture");
        let file = fixture.path().join("single");
        fs::write(&file, vec![3_u8; 257]).expect("single");
        let expected = metadata_sizes(&file);
        let handle = scan(ScanOptions::new(&file));
        wait_for_finish(&handle);
        let tree = handle.tree.read().expect("tree");
        let root = tree.node(tree.root());

        assert!(!root.is_dir);
        assert_eq!(root.entry_count, 1);
        assert_eq!((root.on_disk, root.apparent), expected);
        assert!(root.children.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn directory_symlinks_are_counted_but_never_followed() {
        use std::os::unix::fs::symlink;

        let fixture = tempdir().expect("fixture");
        let target = tempdir().expect("target");
        fs::write(target.path().join("must-not-be-scanned"), b"secret").expect("target file");
        symlink(target.path(), fixture.path().join("linked-directory")).expect("symlink");
        let handle = scan(ScanOptions::new(fixture.path()));
        wait_for_finish(&handle);
        let tree = handle.tree.read().expect("tree");
        let root = tree.node(tree.root());

        assert_eq!(root.entry_count, 1);
        assert_eq!(root.children.len(), 1);
        assert!(!tree.node(root.children[0]).is_dir);
        assert!(tree.node(root.children[0]).children.is_empty());
    }

    #[test]
    #[ignore = "reads the operator's real cache directory"]
    fn manual_scan_home_smoke() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let root = Path::new(&home).join("Library/Caches");
        if !root.is_dir() {
            return;
        }
        let handle = scan(ScanOptions::new(&root));
        wait_for_finish(&handle);
        let tree = handle.tree.read().expect("tree");
        let root_node = tree.node(tree.root());
        eprintln!("holla on-disk bytes: {}", root_node.on_disk);
        for child in tree
            .sorted_children(tree.root(), SortKey::OnDisk)
            .into_iter()
            .take(10)
        {
            let node = tree.node(child);
            eprintln!("{}\t{}", node.on_disk, node.name.to_string_lossy());
        }
        assert!(root_node.on_disk > 0);
    }
}
