use serde::Serialize;
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    Trash,
    Permanent,
}

#[derive(Debug, Clone)]
pub struct DeletePlan {
    pub items: Vec<PathBuf>,
    pub mode: DeleteMode,
    pub dry_run: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DeleteReport {
    pub removed: Vec<(PathBuf, u64)>,
    pub failed: Vec<(PathBuf, String)>,
    pub skipped: Vec<(PathBuf, String)>,
    pub log_errors: Vec<String>,
}

pub fn validate(path: &Path) -> Result<(), Rejection> {
    if !path.is_absolute() {
        return Err(Rejection("path must be absolute".into()));
    }

    let raw = path.as_os_str().as_bytes();
    if raw.is_empty()
        || raw.windows(2).any(|pair| pair == b"//")
        || (raw.len() > 1 && raw.ends_with(b"/"))
    {
        return Err(Rejection("path contains an empty component".into()));
    }
    if path.components().any(|part| part == Component::ParentDir) {
        return Err(Rejection("path contains a parent component".into()));
    }

    let home = dirs::home_dir();
    if home.as_deref() == Some(path)
        || home
            .as_deref()
            .is_some_and(|home| path == home.join(".Trash"))
    {
        return Err(Rejection("path is a protected user root".into()));
    }

    if path == Path::new("/")
        || is_within(path, "/bin")
        || is_within(path, "/sbin")
        || is_within(path, "/etc")
        || is_within(path, "/System")
        || is_within(path, "/var/db")
        || matches!(path.to_str(), Some("/Library" | "/Applications" | "/Users"))
    {
        return Err(Rejection("path is protected".into()));
    }

    if is_within(path, "/usr") && !is_within(path, "/usr/local") {
        return Err(Rejection(
            "only /usr/local descendants may be deleted".into(),
        ));
    }

    Ok(())
}

pub fn execute(plan: &DeletePlan) -> DeleteReport {
    let log_path = ops_log_path();
    execute_with_log_path(plan, log_path.as_deref())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection(pub String);

fn is_within(path: &Path, root: &str) -> bool {
    path == Path::new(root) || path.starts_with(Path::new(root))
}

fn execute_with_log_path(plan: &DeletePlan, log_path: Option<&Path>) -> DeleteReport {
    let mut report = DeleteReport::default();
    let mut seen = HashSet::new();

    for path in &plan.items {
        if !seen.insert(path.clone()) {
            record(
                &mut report,
                log_path,
                plan,
                path,
                0,
                "skipped",
                Some("duplicate path"),
            );
            report.skipped.push((path.clone(), "duplicate path".into()));
            continue;
        }

        if let Err(rejection) = validate(path) {
            record(
                &mut report,
                log_path,
                plan,
                path,
                0,
                "skipped",
                Some(&rejection.0),
            );
            report.skipped.push((path.clone(), rejection.0));
            continue;
        }

        let size = match apparent_size(path) {
            Ok(size) => size,
            Err(error) => {
                let message = error.to_string();
                record(
                    &mut report,
                    log_path,
                    plan,
                    path,
                    0,
                    "failed",
                    Some(&message),
                );
                report.failed.push((path.clone(), message));
                continue;
            }
        };

        let result = if plan.dry_run {
            Ok(())
        } else {
            match plan.mode {
                DeleteMode::Trash => trash::delete(path).map_err(|error| error.to_string()),
                DeleteMode::Permanent => {
                    permanently_remove(path).map_err(|error| error.to_string())
                }
            }
        };

        match result {
            Ok(()) => {
                let outcome = if plan.dry_run { "dry_run" } else { "removed" };
                record(&mut report, log_path, plan, path, size, outcome, None);
                report.removed.push((path.clone(), size));
            }
            Err(message) => {
                record(
                    &mut report,
                    log_path,
                    plan,
                    path,
                    size,
                    "failed",
                    Some(&message),
                );
                report.failed.push((path.clone(), message));
            }
        }
    }

    report
}

fn permanently_remove(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        fs::remove_file(path)
    } else {
        fs::remove_dir_all(path)
    }
}

fn apparent_size(path: &Path) -> std::io::Result<u64> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(metadata.len());
    }

    fs::read_dir(path)?.try_fold(0_u64, |total, entry| {
        let entry = entry?;
        apparent_size(&entry.path()).map(|size| total.saturating_add(size))
    })
}

#[derive(Serialize)]
struct OpsLogLine<'a> {
    v: u8,
    timestamp_ms: u128,
    mode: &'static str,
    path: String,
    size: u64,
    outcome: &'a str,
    error: Option<&'a str>,
}

#[allow(clippy::too_many_arguments)]
fn record(
    report: &mut DeleteReport,
    log_path: Option<&Path>,
    plan: &DeletePlan,
    path: &Path,
    size: u64,
    outcome: &str,
    error: Option<&str>,
) {
    let Some(log_path) = log_path else {
        report.log_errors.push("cache directory unavailable".into());
        return;
    };
    if let Err(log_error) = append_log(
        log_path,
        &OpsLogLine {
            v: 1,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            mode: match plan.mode {
                DeleteMode::Trash => "trash",
                DeleteMode::Permanent => "permanent",
            },
            path: path.to_string_lossy().into_owned(),
            size,
            outcome,
            error,
        },
    ) {
        report.log_errors.push(log_error);
    }
}

fn append_log(path: &Path, line: &OpsLogLine<'_>) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "operation log has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    serde_json::to_writer(&mut file, line).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())
}

#[cfg(not(test))]
fn ops_log_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".cache")))
        .map(|cache| cache.join("holla/ops.log"))
}

#[cfg(test)]
fn ops_log_path() -> Option<PathBuf> {
    Some(std::env::temp_dir().join(format!("holla-{}-ops.log", std::process::id())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};
    use tempfile::TempDir;

    fn home() -> PathBuf {
        dirs::home_dir().expect("test requires home")
    }

    macro_rules! rejected {
        ($name:ident, $path:expr) => {
            #[test]
            fn $name() {
                assert!(validate(Path::new($path)).is_err(), "{}", $path);
            }
        };
    }

    rejected!(rejects_root, "/");
    rejected!(rejects_relative, "tmp/file");
    rejected!(rejects_parent_component, "/tmp/a/../b");
    rejected!(rejects_empty_component, "/Users//someone");
    rejected!(rejects_trailing_empty_component, "/tmp/somewhere/");
    rejected!(rejects_bin, "/bin");
    rejected!(rejects_bin_child, "/bin/tool");
    rejected!(rejects_sbin, "/sbin");
    rejected!(rejects_sbin_child, "/sbin/tool");
    rejected!(rejects_usr, "/usr");
    rejected!(rejects_usr_bin, "/usr/bin/x");
    rejected!(rejects_usr_lib, "/usr/lib/x");
    rejected!(rejects_etc, "/etc");
    rejected!(rejects_etc_child, "/etc/hosts");
    rejected!(rejects_system, "/System");
    rejected!(rejects_system_child, "/System/Library/x");
    rejected!(rejects_library_root, "/Library");
    rejected!(rejects_applications_root, "/Applications");
    rejected!(rejects_users_root, "/Users");
    rejected!(rejects_var_db, "/var/db");
    rejected!(rejects_var_db_child, "/var/db/x");

    #[test]
    fn rejects_home() {
        assert!(validate(&home()).is_err());
    }

    #[test]
    fn rejects_home_trash() {
        assert!(validate(&home().join(".Trash")).is_err());
    }

    #[test]
    fn allows_usr_local_child() {
        assert_eq!(validate(Path::new("/usr/local/foo")), Ok(()));
    }

    #[test]
    fn allows_library_child() {
        assert_eq!(validate(Path::new("/Library/Caches/example")), Ok(()));
    }

    #[test]
    fn allows_applications_child() {
        assert_eq!(validate(Path::new("/Applications/Example.app")), Ok(()));
    }

    #[test]
    fn allows_user_child() {
        assert_eq!(validate(&home().join("Downloads/file")), Ok(()));
    }

    #[test]
    fn allows_nonexistent_absolute_path() {
        assert_eq!(validate(Path::new("/tmp/holla-does-not-exist")), Ok(()));
    }

    #[test]
    fn allows_unicode_and_newlines() {
        assert_eq!(validate(Path::new("/tmp/雪\nfile")), Ok(()));
    }

    #[test]
    fn symlink_validation_does_not_reject_for_protected_target() {
        let temp = TempDir::new().unwrap();
        let link = temp.path().join("root-link");
        symlink("/", &link).unwrap();
        assert_eq!(validate(&link), Ok(()));
    }

    #[test]
    fn permanent_file_removes_and_reports_exact_size() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("file");
        fs::write(&file, b"12345").unwrap();
        let report = execute(&DeletePlan {
            items: vec![file.clone()],
            mode: DeleteMode::Permanent,
            dry_run: false,
        });
        assert_eq!(report.removed, vec![(file.clone(), 5)]);
        assert!(!file.exists());
        assert!(report.failed.is_empty());
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn permanent_directory_removes_recursively() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("dir");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("file"), b"123").unwrap();
        let report = execute(&DeletePlan {
            items: vec![dir.clone()],
            mode: DeleteMode::Permanent,
            dry_run: false,
        });
        assert_eq!(report.removed.len(), 1);
        assert!(!dir.exists());
    }

    #[test]
    fn dry_run_touches_nothing_and_reports_would_remove() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("file");
        fs::write(&file, b"123").unwrap();
        let report = execute(&DeletePlan {
            items: vec![file.clone()],
            mode: DeleteMode::Permanent,
            dry_run: true,
        });
        assert_eq!(report.removed, vec![(file.clone(), 3)]);
        assert!(file.exists());
    }

    #[test]
    fn nonexistent_path_is_failed_not_skipped() {
        let path = PathBuf::from("/tmp/holla-never-created-item");
        let report = execute(&DeletePlan {
            items: vec![path.clone()],
            mode: DeleteMode::Permanent,
            dry_run: false,
        });
        assert!(report.removed.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, path);
    }

    #[test]
    fn rejected_path_is_skipped() {
        let report = execute(&DeletePlan {
            items: vec![PathBuf::from("/")],
            mode: DeleteMode::Permanent,
            dry_run: false,
        });
        assert!(report.removed.is_empty());
        assert!(report.failed.is_empty());
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn permanent_symlink_removes_link_not_target() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        fs::write(&target, b"keep").unwrap();
        symlink(&target, &link).unwrap();
        let report = execute(&DeletePlan {
            items: vec![link.clone()],
            mode: DeleteMode::Permanent,
            dry_run: false,
        });
        assert_eq!(report.removed.len(), 1);
        assert!(!link.exists());
        assert!(target.exists());
    }

    #[test]
    fn duplicate_items_are_skipped_after_first() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("file");
        fs::write(&file, b"x").unwrap();
        let report = execute(&DeletePlan {
            items: vec![file.clone(), file],
            mode: DeleteMode::Permanent,
            dry_run: true,
        });
        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn trash_mode_moves_source_out_of_place() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("trash-me");
        fs::write(&file, b"trash").unwrap();
        let report = execute(&DeletePlan {
            items: vec![file.clone()],
            mode: DeleteMode::Trash,
            dry_run: false,
        });
        assert_eq!(report.removed.len(), 1, "{report:?}");
        assert!(!file.exists());
    }

    #[test]
    fn operation_log_is_versioned_json_lines() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("file");
        let log = temp.path().join("cache/holla/ops.log");
        fs::write(&file, b"abc").unwrap();
        let report = execute_with_log_path(
            &DeletePlan {
                items: vec![file.clone()],
                mode: DeleteMode::Permanent,
                dry_run: true,
            },
            Some(&log),
        );
        assert!(report.log_errors.is_empty());
        let lines = fs::read_to_string(log).unwrap();
        let value: serde_json::Value = serde_json::from_str(lines.trim()).unwrap();
        assert_eq!(value["v"], 1);
        assert_eq!(value["mode"], "permanent");
        assert_eq!(value["path"], file.to_string_lossy().as_ref());
        assert_eq!(value["size"], 3);
        assert_eq!(value["outcome"], "dry_run");
        assert!(value["timestamp_ms"].as_u64().is_some());
    }

    #[test]
    fn operation_log_failure_is_reported_but_does_not_block_deletion() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("file");
        fs::write(&file, b"abc").unwrap();
        let report = execute_with_log_path(
            &DeletePlan {
                items: vec![file.clone()],
                mode: DeleteMode::Permanent,
                dry_run: false,
            },
            Some(temp.path()),
        );
        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.log_errors.len(), 1);
        assert!(!file.exists());
    }

    #[test]
    fn every_item_gets_one_operation_log_line() {
        let temp = TempDir::new().unwrap();
        let valid = temp.path().join("valid");
        let missing = temp.path().join("missing");
        let log = temp.path().join("ops.log");
        fs::write(&valid, b"x").unwrap();
        let report = execute_with_log_path(
            &DeletePlan {
                items: vec![valid, missing, PathBuf::from("/")],
                mode: DeleteMode::Permanent,
                dry_run: true,
            },
            Some(&log),
        );
        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(fs::read_to_string(log).unwrap().lines().count(), 3);
    }
}
