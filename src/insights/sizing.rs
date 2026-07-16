use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime},
};

use crate::du::{ScanEvent, ScanOptions, scan};

use super::{
    InsightSpec, Probe, Safety, collect_artifacts, default_project_roots, expand_roots_with_xdg,
    is_old_enough,
};

#[derive(Debug, Clone)]
pub struct Candidate {
    pub insight_id: &'static str,
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
    pub eligible: bool,
    pub safety: Safety,
}

#[derive(Debug)]
pub enum SizeEvent {
    Candidate(Candidate),
    Finished,
}

pub struct SizeHandle {
    pub events: mpsc::Receiver<SizeEvent>,
    pub cancel: Arc<AtomicBool>,
}

impl Drop for SizeHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

pub fn size(spec: &'static InsightSpec) -> SizeHandle {
    let (sender, events) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name(format!("holla-insight-{}", spec.id))
        .spawn(move || {
            let Some(probe) = Probe::current() else {
                let _ = sender.send(SizeEvent::Finished);
                return;
            };
            let now = SystemTime::now();
            let candidates = enumerate(spec, &probe, now);
            for (path, modified, eligible) in candidates {
                if worker_cancel.load(Ordering::Acquire) {
                    break;
                }
                let Some(bytes) = scan_size(&path, &worker_cancel) else {
                    break;
                };
                if sender
                    .send(SizeEvent::Candidate(Candidate {
                        insight_id: spec.id,
                        path,
                        size: bytes,
                        modified,
                        eligible,
                        safety: spec.safety,
                    }))
                    .is_err()
                {
                    break;
                }
            }
            let _ = sender.send(SizeEvent::Finished);
        })
        .expect("failed to spawn insight sizing worker");
    SizeHandle { events, cancel }
}

fn enumerate(
    spec: &InsightSpec,
    probe: &Probe,
    now: SystemTime,
) -> Vec<(PathBuf, SystemTime, bool)> {
    if spec.id == "docker.data" {
        return Vec::new();
    }
    if spec.id == "project.artifacts" {
        let cwd = std::env::current_dir().unwrap_or_else(|_| probe.home.clone());
        return collect_artifacts(&default_project_roots(&probe.home, &cwd), now)
            .into_iter()
            .map(|(path, artifact)| {
                let modified = modified(&path).unwrap_or(now);
                (path, modified, artifact.eligible)
            })
            .collect();
    }
    resolved_roots(spec, probe)
        .into_iter()
        .flat_map(|root| enumerate_children(&root))
        .map(|(path, modified)| {
            let eligible = is_old_enough(modified, spec.min_age_days, now);
            (path, modified, eligible)
        })
        .collect()
}

fn resolved_roots(spec: &InsightSpec, probe: &Probe) -> Vec<PathBuf> {
    if spec.id == "pnpm.store"
        && let Ok(output) = std::process::Command::new("pnpm")
            .args(["store", "path"])
            .output()
        && output.status.success()
        && let Ok(path) = String::from_utf8(output.stdout)
    {
        let path = PathBuf::from(path.trim());
        if allowed_pnpm_store(&path, &probe.home) {
            return vec![path];
        }
    }
    expand_roots_with_xdg(spec, &probe.home, probe.xdg_cache_home.as_deref())
}

fn allowed_pnpm_store(path: &Path, home: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Ok(path) = fs::canonicalize(path) else {
        return false;
    };
    let Ok(home) = fs::canonicalize(home) else {
        return false;
    };
    [
        home.join("Library/pnpm/store"),
        home.join(".local/share/pnpm/store"),
        home.join(".pnpm-store"),
    ]
    .iter()
    .any(|root| path.starts_with(root))
}

fn enumerate_children(root: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return Vec::new();
    };
    if !metadata.is_dir() {
        return vec![(
            root.to_path_buf(),
            metadata.modified().unwrap_or(SystemTime::now()),
        )];
    }
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).ok()?;
            (!metadata.file_type().is_symlink())
                .then(|| (path, metadata.modified().unwrap_or(SystemTime::now())))
        })
        .collect();
    paths.sort_by(|left, right| left.0.cmp(&right.0));
    paths
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::symlink_metadata(path).ok()?.modified().ok()
}

fn scan_size(path: &Path, cancel: &AtomicBool) -> Option<u64> {
    let handle = scan(ScanOptions::new(path));
    loop {
        if cancel.load(Ordering::Acquire) {
            handle.cancel.store(true, Ordering::Release);
            return None;
        }
        match handle.events.recv_timeout(Duration::from_millis(50)) {
            Ok(ScanEvent::Finished { .. }) => {
                let tree = handle.tree.read().expect("insight scan tree");
                return Some(tree.node(tree.root()).on_disk);
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn enumeration_keeps_recent_children_disabled_instead_of_hiding() {
        let fixture = tempdir().unwrap();
        fs::write(fixture.path().join("recent"), b"x").unwrap();
        let entries = enumerate_children(fixture.path());
        assert_eq!(entries.len(), 1);
        assert!(entries[0].0.ends_with("recent"));
    }

    #[test]
    fn pnpm_store_accepts_only_known_store_roots_below_home() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("Library/pnpm/store/v10");
        fs::create_dir_all(&store).unwrap();
        let documents = fixture.path().join("Documents");
        fs::create_dir(&documents).unwrap();

        assert!(allowed_pnpm_store(&store, fixture.path()));
        assert!(!allowed_pnpm_store(&documents, fixture.path()));
        assert!(!allowed_pnpm_store(
            Path::new("relative/store"),
            fixture.path()
        ));
    }

    #[test]
    fn scan_size_uses_disk_engine() {
        let fixture = tempdir().unwrap();
        fs::write(fixture.path().join("data"), vec![1; 8_192]).unwrap();
        let cancel = AtomicBool::new(false);
        assert!(scan_size(fixture.path(), &cancel).unwrap() > 0);
    }

    #[test]
    fn cancelled_scan_returns_none() {
        let fixture = tempdir().unwrap();
        let cancel = AtomicBool::new(true);
        assert_eq!(scan_size(fixture.path(), &cancel), None);
    }

    #[test]
    #[ignore = "manual machine-dependent insight sizing smoke"]
    fn manual_size_insights_smoke() {
        for insight in super::super::REGISTRY {
            let handle = size(insight);
            while let Ok(event) = handle.events.recv_timeout(Duration::from_secs(30)) {
                match event {
                    SizeEvent::Candidate(candidate) => eprintln!(
                        "{} {} {}",
                        candidate.insight_id,
                        candidate.size,
                        candidate.path.display()
                    ),
                    SizeEvent::Finished => break,
                }
            }
        }
    }
}
