use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use super::is_old_enough;

const ARTIFACT_NAMES: &[&str] = &[
    "node_modules",
    "target",
    "build",
    "dist",
    ".venv",
    "venv",
    ".next",
    ".turbo",
    ".gradle",
    "DerivedData",
    "Pods",
    "__pycache__",
];
const INDICATORS: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    ".git",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub name: OsString,
    pub eligible: bool,
}

pub fn classify_artifact(
    dir_name: &OsStr,
    siblings: &[OsString],
    mtime: SystemTime,
    now: SystemTime,
) -> Option<Artifact> {
    ARTIFACT_NAMES
        .iter()
        .any(|candidate| dir_name == OsStr::new(candidate))
        .then_some(())?;
    siblings
        .iter()
        .any(|sibling| {
            INDICATORS
                .iter()
                .any(|indicator| sibling == OsStr::new(indicator))
        })
        .then_some(())?;
    Some(Artifact {
        name: dir_name.to_owned(),
        eligible: is_old_enough(mtime, Some(7), now),
    })
}

pub fn default_project_roots(home: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let projects = home.join("Projects");
    if projects.is_dir() {
        roots.push(projects);
    }
    if let Some(parent) = cwd.parent()
        && parent.is_dir()
        && !roots.iter().any(|root| root == parent)
    {
        roots.push(parent.to_path_buf());
    }
    roots
}

pub fn collect_artifacts(roots: &[PathBuf], now: SystemTime) -> Vec<(PathBuf, Artifact)> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        let mut pending = vec![(root.clone(), 0_usize)];
        while let Some((directory, depth)) = pending.pop() {
            if depth >= 6 || !seen.insert(directory.clone()) {
                continue;
            }
            let Ok(read_dir) = fs::read_dir(&directory) else {
                continue;
            };
            let entries: Vec<_> = read_dir.flatten().collect();
            let siblings: Vec<_> = entries.iter().map(|entry| entry.file_name()).collect();
            for entry in entries {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() || file_type.is_symlink() {
                    continue;
                }
                let mtime = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(now);
                if let Some(artifact) = classify_artifact(&entry.file_name(), &siblings, mtime, now)
                {
                    found.push((path, artifact));
                } else {
                    pending.push((path, depth + 1));
                }
            }
        }
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Duration};
    use tempfile::tempdir;

    fn old(now: SystemTime) -> SystemTime {
        now - Duration::from_secs(8 * 86_400)
    }

    #[test]
    fn classifier_requires_known_name_and_project_indicator() {
        let now = SystemTime::now();
        assert!(
            classify_artifact(OsStr::new("target"), &["Cargo.toml".into()], old(now), now)
                .is_some()
        );
        assert!(
            classify_artifact(OsStr::new("target"), &["notes.txt".into()], old(now), now).is_none()
        );
        assert!(
            classify_artifact(
                OsStr::new("downloads"),
                &["Cargo.toml".into()],
                old(now),
                now
            )
            .is_none()
        );
    }

    #[test]
    fn classifier_keeps_recent_artifact_visible_but_ineligible() {
        let now = SystemTime::now();
        let artifact =
            classify_artifact(OsStr::new("build"), &["pom.xml".into()], now, now).unwrap();
        assert!(!artifact.eligible);
    }

    #[test]
    fn scanner_finds_real_project_and_ignores_decoy() {
        let fixture = tempdir().unwrap();
        let real = fixture.path().join("real");
        let decoy = fixture.path().join("decoy");
        fs::create_dir_all(real.join("node_modules/nested/target")).unwrap();
        fs::create_dir_all(decoy.join("node_modules")).unwrap();
        fs::write(real.join("package.json"), b"{}").unwrap();
        let found = collect_artifacts(&[fixture.path().to_path_buf()], SystemTime::now());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, real.join("node_modules"));
    }

    #[test]
    fn scanner_deduplicates_nested_artifacts() {
        let fixture = tempdir().unwrap();
        fs::create_dir_all(fixture.path().join("project/target/child/node_modules")).unwrap();
        fs::write(fixture.path().join("project/Cargo.toml"), b"").unwrap();
        fs::write(
            fixture.path().join("project/target/child/package.json"),
            b"{}",
        )
        .unwrap();
        let found = collect_artifacts(&[fixture.path().to_path_buf()], SystemTime::now());
        assert_eq!(found.len(), 1);
        assert!(found[0].0.ends_with("project/target"));
    }
}
