use std::path::PathBuf;

#[cfg(any(target_os = "macos", test))]
use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    time::Duration,
};

#[cfg(test)]
use std::fs;

#[cfg(any(target_os = "macos", test))]
use futures::{StreamExt, stream};
#[cfg(any(target_os = "macos", test))]
use tokio::{process::Command, time::timeout};

#[cfg(any(target_os = "macos", test))]
const QUERY: &str = "kMDItemFSSize >= 104857600";
#[cfg(any(target_os = "macos", test))]
const LIMIT: usize = 50;
#[cfg(target_os = "macos")]
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopFile {
    pub path: PathBuf,
    pub on_disk: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TopFiles {
    Available(Vec<TopFile>),
    Unavailable,
}

#[cfg(target_os = "macos")]
pub async fn discover() -> TopFiles {
    discover_with("mdfind", &[], TIMEOUT).await
}

#[cfg(not(target_os = "macos"))]
pub async fn discover() -> TopFiles {
    TopFiles::Unavailable
}

#[cfg(any(target_os = "macos", test))]
async fn discover_with(
    program: impl AsRef<OsStr>,
    prefix_args: &[&OsStr],
    deadline: Duration,
) -> TopFiles {
    let mut command = Command::new(program);
    command
        .args(prefix_args)
        .arg("-0")
        .arg(QUERY)
        .kill_on_drop(true);
    let result = timeout(deadline, async move {
        let output = command.output().await.ok()?;
        if !output.status.success() {
            return None;
        }
        let paths = parse_paths(&output.stdout);
        Some(rank_paths_with_stat(paths).await)
    })
    .await;
    match result {
        Ok(Some(files)) => TopFiles::Available(files),
        Ok(None) => TopFiles::Unavailable,
        Err(_) => TopFiles::Unavailable,
    }
}

#[cfg(any(target_os = "macos", test))]
fn parse_paths(output: &[u8]) -> Vec<PathBuf> {
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(os_string(path)))
        .collect()
}

#[cfg(all(unix, any(target_os = "macos", test)))]
fn os_string(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec())
}

#[cfg(all(not(unix), any(target_os = "macos", test)))]
fn os_string(bytes: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(any(target_os = "macos", test))]
async fn rank_paths_with_stat(paths: Vec<PathBuf>) -> Vec<TopFile> {
    let mut seen = HashSet::new();
    let unique = paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect::<Vec<_>>();
    let results = stream::iter(unique)
        .map(stat_top_file)
        .buffer_unordered(16)
        .collect::<Vec<Option<TopFile>>>()
        .await;
    let mut files = results.into_iter().flatten().collect::<Vec<_>>();
    sort_and_truncate(&mut files);
    files
}

#[cfg(any(target_os = "macos", test))]
async fn stat_top_file(path: PathBuf) -> Option<TopFile> {
    let mut command = Command::new("/usr/bin/stat");
    command.arg("-f").arg("%p:%b").arg(&path).kill_on_drop(true);
    let output = command.output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let value = std::str::from_utf8(&output.stdout).ok()?.trim();
    let (mode, blocks) = value.split_once(':')?;
    let mode = u32::from_str_radix(mode, 8).ok()?;
    if mode & 0o170_000 != 0o100_000 {
        return None;
    }
    Some(TopFile {
        path,
        on_disk: blocks.parse::<u64>().ok()?.saturating_mul(512),
    })
}

#[cfg(test)]
fn rank_paths(paths: Vec<PathBuf>) -> Vec<TopFile> {
    let mut seen = HashSet::new();
    let mut files = paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .filter_map(|path| {
            let metadata = fs::symlink_metadata(&path).ok()?;
            metadata.file_type().is_file().then(|| TopFile {
                path,
                on_disk: on_disk_size(&metadata),
            })
        })
        .collect::<Vec<_>>();
    sort_and_truncate(&mut files);
    files
}

#[cfg(any(target_os = "macos", test))]
fn sort_and_truncate(files: &mut Vec<TopFile>) {
    files.sort_by(|left, right| {
        right
            .on_disk
            .cmp(&left.on_disk)
            .then_with(|| left.path.cmp(&right.path))
    });
    files.truncate(LIMIT);
}

#[cfg(unix)]
#[cfg(test)]
fn on_disk_size(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
#[cfg(test)]
fn on_disk_size(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::File,
        io::{self, Write},
    };
    use tempfile::tempdir;

    #[test]
    fn parses_nul_separated_paths_without_losing_spaces() {
        assert_eq!(
            parse_paths(b"/tmp/one\0/tmp/two words\0"),
            [PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two words")]
        );
    }

    #[tokio::test]
    async fn slow_command_times_out_as_unavailable() {
        let args = [OsStr::new("-c"), OsStr::new("sleep 1")];
        let result = discover_with("/bin/sh", &args, Duration::from_millis(20)).await;
        assert_eq!(result, TopFiles::Unavailable);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn killable_stat_phase_returns_exact_regular_files() -> io::Result<()> {
        let fixture = tempdir()?;
        let file = fixture.path().join("large");
        File::create(&file)?.write_all(&vec![1; 8_192])?;

        let files = rank_paths_with_stat(vec![file.clone(), fixture.path().to_path_buf()]).await;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, file);
        assert!(files[0].on_disk > 0);
        Ok(())
    }

    #[test]
    fn ranks_exact_files_descending_and_deduplicates() -> io::Result<()> {
        let fixture = tempdir()?;
        let small = fixture.path().join("small");
        let large = fixture.path().join("large");
        File::create(&small)?.write_all(&vec![1; 1_024])?;
        File::create(&large)?.write_all(&vec![1; 8_192])?;
        let files = rank_paths(vec![small.clone(), large.clone(), small.clone()]);

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, large);
        assert_eq!(files[1].path, small);
        assert!(files[0].on_disk >= files[1].on_disk);
        Ok(())
    }
}
