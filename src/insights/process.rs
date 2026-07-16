use std::process::Command;

pub fn is_process_running(name: &str) -> bool {
    let exact = Command::new("pgrep")
        .args(["-x", name])
        .status()
        .is_ok_and(|status| status.success());
    if exact {
        return true;
    }
    // macOS truncates the name used by pgrep. `ps ucomm` exposes that same
    // stable kernel name, so compare its basename without fuzzy substrings.
    Command::new("ps")
        .args(["-axo", "ucomm="])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| process_list_contains(&output.stdout, name))
}

fn process_list_contains(output: &[u8], name: &str) -> bool {
    String::from_utf8_lossy(output).lines().any(|command| {
        std::path::Path::new(command.trim())
            .file_name()
            .is_some_and(|reported| reported == name)
    })
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
