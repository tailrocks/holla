use fff_search::{
    FFFMode, FilePicker, FilePickerOptions, FuzzySearchOptions, MixedItemRef, PaginationArgs,
    QueryParser, SharedFilePicker, SharedFrecency,
};
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};
use std::{
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const ROOT_POLL: Duration = Duration::from_millis(25);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHit {
    pub path: PathBuf,
    pub relative_path: String,
    pub score: i32,
    pub match_byte_offsets: Vec<(u32, u32)>,
}

struct IndexedRoot {
    root: PathBuf,
    picker: SharedFilePicker,
}

pub struct FileIndex {
    roots: Arc<RwLock<Vec<IndexedRoot>>>,
    complete: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl FileIndex {
    #[must_use]
    pub fn build(roots: Vec<PathBuf>) -> Self {
        let roots = safe_scan_roots(&roots);
        let indexed_roots = Arc::new(RwLock::new(Vec::new()));
        let complete = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = Some(spawn_indexer(
            roots,
            Arc::clone(&indexed_roots),
            Arc::clone(&complete),
            Arc::clone(&cancel),
        ));
        Self {
            roots: indexed_roots,
            complete,
            cancel,
            worker,
        }
    }

    #[must_use]
    pub fn indexed_count(&self) -> usize {
        self.roots
            .read()
            .expect("FFF roots read lock")
            .iter()
            .filter_map(|root| root.picker.read().ok())
            .filter_map(|guard| guard.as_ref().map(FilePicker::get_scan_progress))
            .map(|progress| progress.scanned_files_count)
            .sum()
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn query(&self, query: &str, limit: usize) -> Vec<FileHit> {
        if query.trim().is_empty() || limit == 0 {
            return Vec::new();
        }
        let parsed = QueryParser::default().parse(query);
        let roots = self.roots.read().expect("FFF roots read lock");
        let mut hits = Vec::new();
        for indexed_root in roots.iter() {
            let Ok(guard) = indexed_root.picker.read() else {
                continue;
            };
            let Some(picker) = guard.as_ref() else {
                continue;
            };
            let results = picker.fuzzy_search_mixed(
                &parsed,
                None,
                FuzzySearchOptions {
                    max_threads: 0,
                    pagination: PaginationArgs { offset: 0, limit },
                    ..Default::default()
                },
            );
            for (item, score) in results.items.into_iter().zip(results.scores) {
                let relative_path = match item {
                    MixedItemRef::File(file) => file.relative_path(picker),
                    MixedItemRef::Dir(directory) => directory.relative_path(picker),
                };
                let path = indexed_root.root.join(&relative_path);
                hits.push((
                    filename_rank(&path, query),
                    FileHit {
                        path,
                        match_byte_offsets: highlight_offsets(query, &relative_path),
                        relative_path,
                        score: score.total,
                    },
                ));
            }
        }
        hits.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.score.cmp(&left.1.score))
                .then_with(|| left.1.path.cmp(&right.1.path))
        });
        hits.dedup_by(|left, right| left.1.path == right.1.path);
        hits.truncate(limit);
        hits.into_iter().map(|(_, hit)| hit).collect()
    }
}

impl Drop for FileIndex {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Ok(roots) = self.roots.read() {
            for root in roots.iter() {
                root.picker.cancel();
            }
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn spawn_indexer(
    roots: Vec<PathBuf>,
    indexed_roots: Arc<RwLock<Vec<IndexedRoot>>>,
    complete: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("holla-fff-index".into())
        .spawn(move || {
            for root in roots {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                let picker = SharedFilePicker::default();
                let frecency = SharedFrecency::default();
                let options = FilePickerOptions {
                    base_path: root.to_string_lossy().into_owned(),
                    mode: FFFMode::Ai,
                    watch: false,
                    follow_symlinks: false,
                    enable_home_dir_scanning: true,
                    enable_fs_root_scanning: false,
                    ..Default::default()
                };
                if FilePicker::new_with_shared_state(picker.clone(), frecency, options).is_err() {
                    continue;
                }
                indexed_roots
                    .write()
                    .expect("FFF roots write lock")
                    .push(IndexedRoot {
                        root,
                        picker: picker.clone(),
                    });
                loop {
                    if cancel.load(Ordering::Acquire) {
                        picker.cancel();
                    }
                    let progress = picker
                        .read()
                        .ok()
                        .and_then(|guard| guard.as_ref().map(FilePicker::get_scan_progress));
                    if let Some(progress) = progress
                        && !progress.is_scanning
                    {
                        break;
                    }
                    thread::sleep(ROOT_POLL);
                }
            }
            complete.store(true, Ordering::Release);
        })
        .expect("spawn FFF indexer")
}

fn safe_scan_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut safe = Vec::new();
    for root in roots {
        let library = root.join("Library");
        if !library.is_dir() {
            safe.push(root.clone());
            continue;
        }
        let Ok(entries) = std::fs::read_dir(root) else {
            safe.push(root.clone());
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == library {
                if let Ok(library_entries) = std::fs::read_dir(&library) {
                    safe.extend(library_entries.flatten().filter_map(|entry| {
                        let name = entry.file_name();
                        (!name.to_string_lossy().starts_with("Mobile Documents"))
                            .then(|| entry.path())
                    }));
                }
            } else {
                safe.push(path);
            }
        }
    }
    safe.sort();
    safe.dedup();
    safe
}

fn highlight_offsets(query: &str, relative_path: &str) -> Vec<(u32, u32)> {
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut utf32_buf = Vec::new();
    let mut indices = Vec::new();
    if pattern
        .indices(
            Utf32Str::new(relative_path, &mut utf32_buf),
            &mut matcher,
            &mut indices,
        )
        .is_none()
    {
        return Vec::new();
    }
    indices.sort_unstable();
    indices.dedup();
    let chars = relative_path.char_indices().collect::<Vec<_>>();
    indices
        .into_iter()
        .filter_map(|index| {
            let index = usize::try_from(index).ok()?;
            let (start, character) = chars.get(index).copied()?;
            Some((
                u32::try_from(start).ok()?,
                u32::try_from(start + character.len_utf8()).ok()?,
            ))
        })
        .collect()
}

fn filename_rank(path: &std::path::Path, query: &str) -> u8 {
    let query = query.trim().to_lowercase();
    let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
        return 0;
    };
    let filename = filename.to_lowercase();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if filename == query || stem == query {
        2
    } else if filename.contains(&query) {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Instant};
    use tempfile::tempdir;

    fn wait(index: &FileIndex) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !index.is_complete() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(index.is_complete());
    }

    #[test]
    fn fixture_files_and_folders_are_searchable() {
        let fixture = tempdir().unwrap();
        fs::create_dir_all(fixture.path().join("projects/alpha")).unwrap();
        fs::write(fixture.path().join("projects/alpha/readme.md"), b"test").unwrap();
        let index = FileIndex::build(vec![fixture.path().to_path_buf()]);
        wait(&index);

        assert_eq!(
            index.query("readme", 10)[0].path,
            fixture.path().join("projects/alpha/readme.md")
        );
        assert!(
            index
                .query("alpha", 10)
                .iter()
                .any(|hit| hit.path.ends_with("alpha"))
        );
    }

    #[test]
    fn filename_match_beats_parent_only_match() {
        let fixture = tempdir().unwrap();
        fs::create_dir_all(fixture.path().join("needle-parent")).unwrap();
        fs::write(fixture.path().join("needle.txt"), b"top").unwrap();
        fs::write(fixture.path().join("needle-parent/other.txt"), b"lower").unwrap();
        let index = FileIndex::build(vec![fixture.path().to_path_buf()]);
        wait(&index);

        assert_eq!(
            index.query("needle", 10)[0].path,
            fixture.path().join("needle.txt")
        );
    }

    #[test]
    fn query_limit_and_empty_query_are_enforced() {
        let fixture = tempdir().unwrap();
        for name in ["alpha", "alpine", "alphabet"] {
            fs::write(fixture.path().join(name), b"test").unwrap();
        }
        let index = FileIndex::build(vec![fixture.path().to_path_buf()]);
        wait(&index);

        assert_eq!(index.query("alp", 2).len(), 2);
        assert!(index.query("", 10).is_empty());
    }

    #[test]
    fn hidden_and_gitignored_paths_are_excluded() {
        let fixture = tempdir().unwrap();
        fs::write(fixture.path().join(".ignore"), b"ignored.txt\n").unwrap();
        fs::write(fixture.path().join("ignored.txt"), b"ignored").unwrap();
        fs::write(fixture.path().join("visible.txt"), b"visible").unwrap();
        fs::write(fixture.path().join(".hidden.txt"), b"hidden").unwrap();
        let index = FileIndex::build(vec![fixture.path().to_path_buf()]);
        wait(&index);

        assert!(
            index
                .query("visible", 10)
                .iter()
                .any(|hit| hit.path.ends_with("visible.txt"))
        );
        assert!(index.query("ignored", 10).is_empty());
        assert!(index.query("hidden", 10).is_empty());
    }

    #[test]
    fn mobile_documents_is_split_out_before_fff_scans() {
        let fixture = tempdir().unwrap();
        let library = fixture.path().join("Library");
        fs::create_dir_all(library.join("Mobile Documents.test")).unwrap();
        fs::create_dir_all(library.join("Caches")).unwrap();

        let roots = safe_scan_roots(&[fixture.path().to_path_buf()]);

        assert!(roots.contains(&library.join("Caches")));
        assert!(
            !roots
                .iter()
                .any(|root| root.starts_with(library.join("Mobile Documents.test")))
        );
        assert!(!roots.contains(&library));
    }

    #[test]
    fn cancellation_stops_sequential_root_indexing() {
        let fixture = tempdir().unwrap();
        for index in 0..20 {
            fs::create_dir_all(fixture.path().join(format!("root-{index}"))).unwrap();
        }
        let mut index = FileIndex::build(
            fs::read_dir(fixture.path())
                .unwrap()
                .flatten()
                .map(|entry| entry.path())
                .collect(),
        );
        index.cancel.store(true, Ordering::Release);
        index.worker.take().unwrap().join().unwrap();

        assert!(index.is_complete());
    }

    #[test]
    fn highlight_offsets_are_utf8_byte_ranges() {
        assert_eq!(
            highlight_offsets("rés", "résumé.txt"),
            [(0, 1), (1, 3), (3, 4)]
        );
    }
}
