use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::cleanup::{DeleteMode, DeletePlan, execute};

pub fn discover(
    root: &Path,
    directory_names: &[&str],
    file_extensions: &[&str],
    max_depth: usize,
) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    while let Some((directory, depth)) = pending.pop() {
        if depth >= max_depth {
            continue;
        }
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            let name = entry.file_name();
            if metadata.is_dir() {
                if name == "node_modules" {
                    continue;
                }
                if directory_names.iter().any(|candidate| name == *candidate) {
                    found.push(path);
                } else {
                    pending.push((path, depth + 1));
                }
            } else if path.extension().is_some_and(|extension| {
                file_extensions
                    .iter()
                    .any(|candidate| extension == *candidate)
            }) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

pub async fn move_to_trash(items: Vec<PathBuf>) -> anyhow::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let report = tokio::task::spawn_blocking(move || {
        execute(&DeletePlan {
            items,
            mode: DeleteMode::Trash,
            dry_run: false,
        })
    })
    .await?;
    if report.failed.is_empty() && report.skipped.is_empty() {
        return Ok(());
    }
    let details = report
        .failed
        .iter()
        .chain(&report.skipped)
        .take(5)
        .map(|(path, reason)| format!("{}: {reason}", path.display()))
        .collect::<Vec<_>>()
        .join("; ");
    anyhow::bail!(
        "cleanup incomplete: {} failed, {} skipped: {details}",
        report.failed.len(),
        report.skipped.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    #[test]
    fn discovers_gradle_candidates_with_depth_cap_and_nested_dedup() {
        let fixture = tempdir().unwrap();
        fs::create_dir_all(fixture.path().join("app/build/nested/build")).unwrap();
        fs::create_dir_all(fixture.path().join("app/.gradle")).unwrap();
        fs::create_dir_all(fixture.path().join("a/b/c/d/e/build")).unwrap();
        let found = discover(fixture.path(), &[".gradle", "build"], &[], 5);
        assert_eq!(
            found,
            [
                fixture.path().join("app/.gradle"),
                fixture.path().join("app/build")
            ]
        );
    }

    #[test]
    fn discovers_idea_and_iml_without_entering_node_modules() {
        let fixture = tempdir().unwrap();
        fs::create_dir_all(fixture.path().join("project/.idea")).unwrap();
        fs::create_dir_all(fixture.path().join("project/node_modules/pkg")).unwrap();
        fs::write(fixture.path().join("project/module.iml"), b"").unwrap();
        fs::write(
            fixture.path().join("project/node_modules/pkg/hidden.iml"),
            b"",
        )
        .unwrap();
        let found = discover(fixture.path(), &[".idea"], &["iml"], 5);
        assert_eq!(
            found,
            [
                fixture.path().join("project/.idea"),
                fixture.path().join("project/module.iml")
            ]
        );
    }

    #[test]
    fn walker_never_follows_directory_symlinks() {
        let fixture = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::create_dir(outside.path().join("build")).unwrap();
        symlink(outside.path(), fixture.path().join("linked")).unwrap();
        assert!(discover(fixture.path(), &["build"], &[], 5).is_empty());
    }
}
