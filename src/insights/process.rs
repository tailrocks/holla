use std::process::Command;

pub fn is_process_running(name: &str) -> bool {
    let exact = Command::new("pgrep")
        .args(["-x", name])
        .status()
        .is_ok_and(|status| status.success());
    if exact {
        return true;
    }
    // macOS can expose a truncated process name to `pgrep -x`. Fall back to
    // the executable basename in the full command while retaining boundaries.
    let escaped = name.chars().fold(String::new(), |mut output, character| {
        if ".^$*+?()[]{}\\|".contains(character) {
            output.push('\\');
        }
        output.push(character);
        output
    });
    let pattern = format!(r"(^|/){escaped}[^/ ]*($| )");
    Command::new("pgrep")
        .args(["-f", &pattern])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detects_test_runner_by_its_reported_process_name() {
        let output = Command::new("ps")
            .args(["-p", &std::process::id().to_string(), "-o", "ucomm="])
            .output()
            .expect("ps must be available");
        let command = String::from_utf8(output.stdout).expect("process command is UTF-8");
        let reported_name = Path::new(command.trim())
            .file_name()
            .expect("process name")
            .to_string_lossy();
        assert!(
            is_process_running(&reported_name),
            "pgrep could not find test process `{reported_name}`"
        );
    }

    #[test]
    fn missing_process_is_not_reported_running() {
        assert!(!is_process_running(
            "holla-process-name-that-does-not-exist"
        ));
    }
}
