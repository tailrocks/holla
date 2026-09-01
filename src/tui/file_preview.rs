use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::Path,
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use termrock::widgets::FilePreview;

const MAX_PREVIEW_BYTES: usize = 256 * 1024;
const MAX_PREVIEW_LINES: usize = 2_000;
const MAX_LINE_CHARS: usize = 4_096;
const BYTE_TRUNCATION_MARKER: &str = "[preview truncated at 256 KiB]";
const LINE_TRUNCATION_MARKER: &str = "[remaining lines truncated]";
const LONG_LINE_MARKER: &str = " … [line truncated]";

/// Loads a bounded, terminal-safe preview for a local filesystem path.
///
/// Runtime failures are returned inside [`FilePreview::error`] so a missing,
/// unreadable, binary, or unsupported selection cannot terminate the browser.
#[must_use]
pub fn load(path: &Path) -> FilePreview {
    let link_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return error_preview(path, "unable to inspect", &error),
    };

    let symlink_target = if link_metadata.file_type().is_symlink() {
        match fs::read_link(path) {
            Ok(target) => Some(sanitize(&target.to_string_lossy())),
            Err(error) => return error_preview(path, "unable to read symlink", &error),
        }
    } else {
        None
    };

    let metadata = if symlink_target.is_some() {
        match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => return error_preview(path, "unable to resolve symlink", &error),
        }
    } else {
        link_metadata
    };

    if metadata.is_dir() {
        return directory_preview(path, symlink_target.as_deref());
    }
    if !metadata.is_file() {
        return unavailable_preview(path, "special file preview refused");
    }

    file_preview(path, symlink_target.as_deref())
}

fn file_preview(path: &Path, symlink_target: Option<&str>) -> FilePreview {
    let file = match open_for_preview(path) {
        Ok(file) => file,
        Err(error) => return error_preview(path, "unable to open", &error),
    };
    // Path metadata can race with replacement; descriptor metadata identifies what will be read.
    let metadata = match file.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return unavailable_preview(path, "special file preview refused"),
        Err(error) => return error_preview(path, "unable to inspect opened file", &error),
    };

    let capacity = usize::try_from(metadata.len())
        .unwrap_or(MAX_PREVIEW_BYTES)
        .min(MAX_PREVIEW_BYTES);
    let mut bytes = Vec::with_capacity(capacity);
    let mut reader = file.take(MAX_PREVIEW_BYTES as u64);
    if let Err(error) = reader.read_to_end(&mut bytes) {
        return error_preview(path, "unable to read", &error);
    }

    let truncated = reader
        .get_ref()
        .metadata()
        .map_or(metadata.len() > bytes.len() as u64, |current| {
            current.len() > bytes.len() as u64
        });

    if bytes.contains(&0) {
        return unavailable_preview(path, "binary file preview unavailable");
    }

    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            match std::str::from_utf8(&bytes[..error.valid_up_to()]) {
                Ok(prefix) => prefix,
                Err(_) => {
                    return unavailable_preview(path, "binary file preview unavailable");
                }
            }
        }
        Err(_) => return unavailable_preview(path, "binary file preview unavailable"),
    };

    let mut lines = project_lines(text);
    if lines.is_empty() {
        lines.push("(empty file)".to_owned());
    }
    if let Some(target) = symlink_target {
        lines.insert(0, String::new());
        lines.insert(0, format!("Symlink → {target}"));
    }
    if truncated {
        lines.push(BYTE_TRUNCATION_MARKER.to_owned());
    }

    FilePreview::text(title(path), lines)
}

fn open_for_preview(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);

    #[cfg(unix)]
    {
        // Keep a path swap to a FIFO or device from blocking either open or read.
        options.custom_flags(libc::O_NONBLOCK);
    }

    options.open(path)
}

fn project_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut source_lines = text.lines();

    for _ in 0..MAX_PREVIEW_LINES {
        let Some(line) = source_lines.next() else {
            return lines;
        };
        lines.push(sanitize_line(line));
    }

    if source_lines.next().is_some() {
        lines.push(LINE_TRUNCATION_MARKER.to_owned());
    }
    lines
}

fn sanitize_line(line: &str) -> String {
    let mut output = String::with_capacity(line.len().min(MAX_LINE_CHARS));
    let mut chars = line.chars();

    for _ in 0..MAX_LINE_CHARS {
        let Some(character) = chars.next() else {
            return output;
        };
        push_sanitized(&mut output, character);
    }

    if chars.next().is_some() {
        output.push_str(LONG_LINE_MARKER);
    }
    output
}

fn sanitize(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        push_sanitized(&mut output, character);
    }
    output
}

fn push_sanitized(output: &mut String, character: char) {
    match character {
        '\t' => output.push_str("    "),
        character if character.is_control() => output.push('\u{fffd}'),
        character => output.push(character),
    }
}

fn directory_preview(path: &Path, symlink_target: Option<&str>) -> FilePreview {
    let mut lines = vec![
        "Type: directory".to_owned(),
        format!("Path: {}", display(path)),
    ];
    if let Some(target) = symlink_target {
        lines.push(format!("Symlink → {target}"));
    }
    FilePreview::text(title(path), lines)
}

fn unavailable_preview(path: &Path, reason: &str) -> FilePreview {
    FilePreview {
        title: title(path),
        lines: Vec::new(),
        error: Some(format!("{}: {reason}", display(path))),
    }
}

fn error_preview(path: &Path, action: &str, error: &std::io::Error) -> FilePreview {
    FilePreview {
        title: title(path),
        lines: Vec::new(),
        error: Some(format!("{}: {action}: {error}", display(path))),
    }
}

fn title(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| display(path), |name| sanitize(&name.to_string_lossy()))
}

fn display(path: &Path) -> String {
    sanitize(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[cfg(unix)]
    fn create_fifo(path: &Path) {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};

        let path_c = CString::new(path.as_os_str().as_bytes()).expect("path has no NUL");
        // SAFETY: `path_c` is a valid, NUL-terminated path and the mode is valid.
        let result = unsafe { libc::mkfifo(path_c.as_ptr(), 0o600) };
        assert_eq!(
            result,
            0,
            "unable to create FIFO: {}",
            std::io::Error::last_os_error()
        );
    }

    #[cfg(unix)]
    fn preview_without_blocking(path: &Path) -> FilePreview {
        use std::{sync::mpsc, time::Duration};

        let path = path.to_owned();
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            sender
                .send(file_preview(&path, None))
                .expect("send preview");
        });
        let preview = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("special-file preview blocked");
        worker.join().expect("preview worker");
        preview
    }

    #[test]
    fn loads_utf8_text() {
        let fixture = tempdir().expect("fixture");
        let path = fixture.path().join("notes.txt");
        fs::write(&path, "hello\n世界\n").expect("write fixture");

        let preview = load(&path);

        assert_eq!(preview.title, "notes.txt");
        assert_eq!(preview.lines, ["hello", "世界"]);
        assert_eq!(preview.error, None);
    }

    #[test]
    fn classifies_nul_and_invalid_utf8_as_binary() {
        let fixture = tempdir().expect("fixture");
        for (name, contents) in [("nul", &[b'a', 0, b'b'][..]), ("invalid", &[0xff][..])] {
            let path = fixture.path().join(name);
            fs::write(&path, contents).expect("write fixture");

            let preview = load(&path);

            assert!(
                preview
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("binary file")),
                "unexpected preview for {name}: {preview:?}"
            );
        }
    }

    #[test]
    fn marks_byte_and_long_line_truncation() {
        let fixture = tempdir().expect("fixture");
        let path = fixture.path().join("large.txt");
        fs::write(&path, vec![b'a'; MAX_PREVIEW_BYTES + 1]).expect("write fixture");

        let preview = load(&path);

        assert!(preview.lines[0].ends_with(LONG_LINE_MARKER));
        assert_eq!(
            preview.lines.last().map(String::as_str),
            Some(BYTE_TRUNCATION_MARKER)
        );
    }

    #[test]
    fn keeps_valid_utf8_when_byte_limit_splits_a_character() {
        let fixture = tempdir().expect("fixture");
        let path = fixture.path().join("utf8.txt");
        let mut contents = vec![b'a'; MAX_PREVIEW_BYTES - 1];
        contents.extend_from_slice("é".as_bytes());
        fs::write(&path, contents).expect("write fixture");

        let preview = load(&path);

        assert_eq!(preview.error, None);
        assert_eq!(
            preview.lines.last().map(String::as_str),
            Some(BYTE_TRUNCATION_MARKER)
        );
    }

    #[test]
    fn sanitizes_ansi_and_control_characters() {
        let fixture = tempdir().expect("fixture");
        let path = fixture.path().join("unsafe.txt");
        fs::write(&path, "\u{1b}[31mred\u{1b}[0m\tbell\u{7}").expect("write fixture");

        let preview = load(&path);

        assert_eq!(preview.lines, ["�[31mred�[0m    bell�"]);
    }

    #[test]
    fn reports_missing_paths_inside_preview() {
        let fixture = tempdir().expect("fixture");
        let path = fixture.path().join("missing.txt");

        let preview = load(&path);

        assert!(
            preview
                .error
                .as_deref()
                .is_some_and(|error| error.contains("unable to inspect"))
        );
    }

    #[test]
    fn projects_directory_metadata() {
        let fixture = tempdir().expect("fixture");

        let preview = load(fixture.path());

        assert_eq!(preview.error, None);
        assert_eq!(preview.lines[0], "Type: directory");
        assert!(preview.lines[1].starts_with("Path: "));
    }

    #[cfg(unix)]
    #[test]
    fn follows_symlinks_and_reports_the_target() {
        use std::os::unix::fs::symlink;

        let fixture = tempdir().expect("fixture");
        let target = fixture.path().join("target.txt");
        let link = fixture.path().join("link.txt");
        fs::write(&target, "target contents").expect("write fixture");
        symlink("target.txt", &link).expect("create symlink");

        let preview = load(&link);

        assert_eq!(preview.error, None);
        assert_eq!(preview.lines[0], "Symlink → target.txt");
        assert_eq!(preview.lines[2], "target contents");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_special_files() {
        let preview = load(Path::new("/dev/null"));

        assert!(
            preview
                .error
                .as_deref()
                .is_some_and(|error| error.contains("special file preview refused"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_to_fifo_without_blocking_after_open() {
        use std::os::unix::fs::symlink;

        let fixture = tempdir().expect("fixture");
        let fifo = fixture.path().join("preview.fifo");
        create_fifo(&fifo);
        let link = fixture.path().join("preview.link");
        symlink("preview.fifo", &link).expect("create symlink");

        let preview = load(&link);

        assert!(
            preview
                .error
                .as_deref()
                .is_some_and(|error| error.contains("special file preview refused"))
        );

        let preview = preview_without_blocking(&link);

        assert!(
            preview
                .error
                .as_deref()
                .is_some_and(|error| error.contains("special file preview refused"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_fifo_without_blocking_after_open() {
        let fixture = tempdir().expect("fixture");
        let fifo = fixture.path().join("preview.fifo");
        create_fifo(&fifo);
        let preview = preview_without_blocking(&fifo);

        assert!(
            preview
                .error
                .as_deref()
                .is_some_and(|error| error.contains("special file preview refused"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_device_after_nonblocking_open() {
        let preview = file_preview(Path::new("/dev/null"), None);

        assert!(
            preview
                .error
                .as_deref()
                .is_some_and(|error| error.contains("special file preview refused"))
        );
    }
}
