use std::{
    ffi::OsString,
    fs::Metadata,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    time::{Duration, Instant},
};

use dashmap::DashMap;
use jwalk::{Parallelism, WalkDirGeneric};

use super::{ErrKind, NodeId, ScanEvent, ScanOptions, ScanTree, hardlinks::Hardlinks, platform};

#[derive(Debug, Default)]
struct EntryState {
    node_id: Option<NodeId>,
    raw: Option<RawEntry>,
    metadata_error: Option<ErrKind>,
}

#[derive(Debug)]
struct RawEntry {
    name: OsString,
    file_type: std::fs::FileType,
    on_disk: u64,
    apparent: u64,
    nlink: u64,
    dev: u64,
    ino: u64,
    is_dataless: bool,
}

struct SharedScan {
    tree: Arc<RwLock<ScanTree>>,
    directories: DashMap<PathBuf, NodeId>,
    hardlinks: Hardlinks,
    events: SyncSender<ScanEvent>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    last_sizes_emit_ms: AtomicU64,
    bytes_seen: AtomicU64,
}

pub(crate) fn run(
    options: ScanOptions,
    tree: Arc<RwLock<ScanTree>>,
    cancel: Arc<AtomicBool>,
    events: SyncSender<ScanEvent>,
) {
    let started = Instant::now();
    let root_id = tree.read().expect("scan tree poisoned").root();
    if platform::init_scan_thread().is_err() {
        record_directory_error(&tree, &events, root_id, options.root, ErrKind::Io);
        finish(&tree, &events, started);
        return;
    }

    let workers = options.workers.max(1);
    let (init_tx, init_rx) = sync_channel(workers);
    let pool = match rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .start_handler(move |_| {
            let _ = init_tx.send(platform::init_scan_thread());
        })
        .build()
    {
        Ok(pool) => Arc::new(pool),
        Err(_) => {
            record_directory_error(&tree, &events, root_id, options.root, ErrKind::Io);
            finish(&tree, &events, started);
            return;
        }
    };
    let workers_ready = (0..workers).all(|_| {
        init_rx
            .recv_timeout(Duration::from_secs(2))
            .is_ok_and(|result| result.is_ok())
    });
    if !workers_ready {
        record_directory_error(&tree, &events, root_id, options.root, ErrKind::Io);
        finish(&tree, &events, started);
        return;
    }

    let shared = Arc::new(SharedScan {
        tree,
        directories: DashMap::from_iter([(options.root.clone(), root_id)]),
        hardlinks: Hardlinks::default(),
        events,
        cancel,
        started,
        last_sizes_emit_ms: AtomicU64::new(0),
        bytes_seen: AtomicU64::new(0),
    });
    let callback_shared = Arc::clone(&shared);
    let callback_options = options.clone();
    let walker = WalkDirGeneric::<((), EntryState)>::new(&options.root)
        .follow_links(false)
        .skip_hidden(!options.follow_hidden)
        .parallelism(Parallelism::RayonExistingPool {
            pool,
            busy_timeout: Some(Duration::from_secs(1)),
        })
        .process_read_dir(move |depth, parent_path, _, entries| {
            process_entries(
                depth,
                parent_path,
                entries,
                &callback_options,
                &callback_shared,
            );
        });

    let mut open_directories: Vec<(usize, NodeId)> = Vec::new();
    let mut dirs_done = 0_u64;
    for result in walker {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        let depth = result
            .as_ref()
            .map_or_else(|error| error.depth(), |entry| entry.depth());
        complete_directories(&mut open_directories, depth, &mut dirs_done, &shared);
        match result {
            Ok(entry) => {
                if let Some(raw) = entry.client_state.raw.as_ref() {
                    debug_assert_eq!(raw.name, entry.file_name());
                }
                if let Some(error) = entry
                    .read_children
                    .as_ref()
                    .and_then(jwalk::ReadChildren::error)
                    && let Some(id) = entry.client_state.node_id
                {
                    record_directory_error(
                        &shared.tree,
                        &shared.events,
                        id,
                        entry.path(),
                        classify_jwalk_error(error),
                    );
                }
                if entry.file_type().is_dir()
                    && let Some(id) = entry.client_state.node_id
                {
                    open_directories.push((entry.depth(), id));
                }
            }
            Err(error) => {
                let path = error
                    .path()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| options.root.clone());
                let id = shared
                    .directories
                    .get(&path)
                    .map(|entry| *entry)
                    .unwrap_or(root_id);
                record_directory_error(
                    &shared.tree,
                    &shared.events,
                    id,
                    path,
                    classify_jwalk_error(&error),
                );
            }
        }
    }
    if !shared.cancel.load(Ordering::Relaxed) {
        complete_directories(&mut open_directories, 0, &mut dirs_done, &shared);
    }
    let _ = shared.events.send(ScanEvent::SizesUpdated);
    finish(&shared.tree, &shared.events, started);
}

fn process_entries(
    depth: Option<usize>,
    parent_path: &Path,
    entries: &mut Vec<jwalk::Result<jwalk::DirEntry<((), EntryState)>>>,
    options: &ScanOptions,
    shared: &SharedScan,
) {
    if shared.cancel.load(Ordering::Relaxed) {
        entries.clear();
        return;
    }
    entries.retain_mut(|result| {
        if shared.cancel.load(Ordering::Relaxed) {
            return false;
        }
        let Ok(entry) = result else {
            return true;
        };
        let path = entry.path();
        if platform::should_skip(&path, options) {
            return false;
        }
        let is_root = depth.is_none();
        let node_id = if is_root {
            shared.tree.read().expect("scan tree poisoned").root()
        } else {
            let Some(parent) = shared.directories.get(parent_path).map(|entry| *entry) else {
                return false;
            };
            let id = if entry.file_type().is_dir() {
                shared
                    .tree
                    .write()
                    .expect("scan tree poisoned")
                    .add_dir(parent, entry.file_name().to_owned())
            } else {
                shared
                    .tree
                    .write()
                    .expect("scan tree poisoned")
                    .add_file(parent, entry.file_name().to_owned())
            };
            if entry.file_type().is_dir() {
                shared.directories.insert(path.clone(), id);
                let _ = shared.events.send(ScanEvent::DirAdded {
                    id,
                    parent,
                    name: entry.file_name().to_owned(),
                });
            }
            id
        };
        entry.client_state.node_id = Some(node_id);
        match entry.metadata() {
            Ok(metadata) => {
                let raw = raw_entry(entry.file_name().to_owned(), entry.file_type(), &metadata);
                if is_root {
                    shared
                        .tree
                        .write()
                        .expect("scan tree poisoned")
                        .set_root_is_dir(raw.file_type.is_dir());
                }
                let count_size = raw.file_type.is_dir()
                    || shared
                        .hardlinks
                        .should_count_size(raw.nlink, raw.dev, raw.ino);
                let (on_disk, apparent) = if count_size {
                    (raw.on_disk, raw.apparent)
                } else {
                    (0, 0)
                };
                shared.tree.write().expect("scan tree poisoned").add_sizes(
                    node_id,
                    on_disk,
                    apparent,
                    u64::from(!raw.file_type.is_dir()),
                );
                shared.bytes_seen.fetch_add(on_disk, Ordering::Relaxed);
                if raw.is_dataless {
                    record_node_error(
                        &shared.tree,
                        &shared.events,
                        node_id,
                        path,
                        ErrKind::Dataless,
                        raw.file_type.is_dir(),
                    );
                    if raw.file_type.is_dir() {
                        entry.read_children = None;
                    }
                }
                entry.client_state.raw = Some(raw);
            }
            Err(error) => {
                let kind = classify_jwalk_error(&error);
                if !entry.file_type().is_dir() {
                    shared
                        .tree
                        .write()
                        .expect("scan tree poisoned")
                        .add_sizes(node_id, 0, 0, 1);
                }
                record_node_error(
                    &shared.tree,
                    &shared.events,
                    node_id,
                    path,
                    kind,
                    entry.file_type().is_dir(),
                );
                entry.client_state.metadata_error = Some(kind);
                if entry.file_type().is_dir() {
                    entry.read_children = None;
                }
            }
        }
        maybe_emit_sizes(shared);
        true
    });
}

fn maybe_emit_sizes(shared: &SharedScan) {
    let now = shared.started.elapsed().as_millis() as u64;
    let previous = shared.last_sizes_emit_ms.load(Ordering::Relaxed);
    if now.saturating_sub(previous) >= 100
        && shared
            .last_sizes_emit_ms
            .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        let _ = shared.events.try_send(ScanEvent::SizesUpdated);
    }
}

fn complete_directories(
    open: &mut Vec<(usize, NodeId)>,
    next_depth: usize,
    dirs_done: &mut u64,
    shared: &SharedScan,
) {
    while open.last().is_some_and(|(depth, _)| *depth >= next_depth) {
        let (_, id) = open.pop().expect("checked nonempty");
        shared
            .tree
            .write()
            .expect("scan tree poisoned")
            .mark_done(id);
        *dirs_done = dirs_done.saturating_add(1);
        let _ = shared.events.try_send(ScanEvent::Progress {
            dirs_done: *dirs_done,
            bytes_seen: shared.bytes_seen.load(Ordering::Relaxed),
        });
    }
}

fn finish(tree: &Arc<RwLock<ScanTree>>, events: &SyncSender<ScanEvent>, started: Instant) {
    let inaccessible = tree.read().expect("scan tree poisoned").errors().len() as u64;
    let _ = events.send(ScanEvent::Finished {
        duration: started.elapsed(),
        inaccessible,
    });
}

fn record_directory_error(
    tree: &Arc<RwLock<ScanTree>>,
    events: &SyncSender<ScanEvent>,
    id: NodeId,
    path: PathBuf,
    kind: ErrKind,
) {
    tree.write()
        .expect("scan tree poisoned")
        .record_error(id, path, kind);
    let _ = events.send(ScanEvent::DirErrored { id, kind });
}

fn record_node_error(
    tree: &Arc<RwLock<ScanTree>>,
    events: &SyncSender<ScanEvent>,
    id: NodeId,
    path: PathBuf,
    kind: ErrKind,
    is_dir: bool,
) {
    tree.write()
        .expect("scan tree poisoned")
        .record_error(id, path, kind);
    if is_dir {
        let _ = events.send(ScanEvent::DirErrored { id, kind });
    }
}

fn classify_jwalk_error(error: &jwalk::Error) -> ErrKind {
    error.io_error().map_or(ErrKind::Io, classify_io_error)
}

fn classify_io_error(error: &std::io::Error) -> ErrKind {
    if error.raw_os_error() == Some(libc::EDEADLK) {
        ErrKind::Dataless
    } else {
        match error.kind() {
            std::io::ErrorKind::PermissionDenied => ErrKind::PermissionDenied,
            std::io::ErrorKind::NotFound => ErrKind::NotFound,
            _ => ErrKind::Io,
        }
    }
}

#[cfg(unix)]
fn raw_entry(name: OsString, file_type: std::fs::FileType, metadata: &Metadata) -> RawEntry {
    use std::os::unix::fs::MetadataExt;

    RawEntry {
        name,
        file_type,
        on_disk: metadata.blocks().saturating_mul(512),
        apparent: metadata.size(),
        nlink: metadata.nlink(),
        dev: metadata.dev(),
        ino: metadata.ino(),
        is_dataless: platform::is_dataless(metadata),
    }
}

#[cfg(not(unix))]
fn raw_entry(name: OsString, file_type: std::fs::FileType, metadata: &Metadata) -> RawEntry {
    RawEntry {
        name,
        file_type,
        on_disk: metadata.len(),
        apparent: metadata.len(),
        nlink: 1,
        dev: 0,
        ino: 0,
        is_dataless: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_errors_map_to_stable_ui_kinds() {
        assert_eq!(
            classify_io_error(&std::io::Error::from_raw_os_error(libc::EACCES)),
            ErrKind::PermissionDenied
        );
        assert_eq!(
            classify_io_error(&std::io::Error::from_raw_os_error(libc::EDEADLK)),
            ErrKind::Dataless
        );
        assert_eq!(
            classify_io_error(&std::io::Error::from(std::io::ErrorKind::NotFound)),
            ErrKind::NotFound
        );
    }
}
