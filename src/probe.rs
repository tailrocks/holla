use std::path::Path;

use which::which;

#[derive(Debug, Clone)]
pub struct MiseTask {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct Probe {
    // system tools
    pub git: bool,
    pub docker: bool,
    pub brew: bool,
    pub gradle: bool,
    pub mise: bool,
    pub amp: bool,
    pub omz: bool,
    pub idea: bool,

    // current folder context
    pub in_git_repo: bool,
    #[expect(dead_code)]
    pub has_mise_toml: bool,
    pub has_docker_compose: bool,
    pub has_gradle_build: bool,
    pub has_idea_dir: bool,
    pub mise_tasks: Vec<MiseTask>,

    // parent folder context
    pub parent_git_repos: Vec<String>,
}

impl Probe {
    pub fn run() -> Self {
        let git = which("git").is_ok();
        let docker = which("docker").is_ok();
        let brew = which("brew").is_ok();
        let gradle = which("gradle").is_ok();
        let mise = which("mise").is_ok();
        let amp = which("amp").is_ok();
        let omz = which("omz").is_ok();
        let idea = which("idea").is_ok();

        let in_git_repo = Path::new(".git").exists();
        let has_mise_toml = Path::new("mise.toml").exists() || Path::new(".mise.toml").exists();
        let has_docker_compose = Path::new("docker-compose.yml").exists()
            || Path::new("docker-compose.yaml").exists()
            || Path::new("compose.yml").exists()
            || Path::new("compose.yaml").exists();
        let has_gradle_build =
            Path::new("build.gradle").exists() || Path::new("build.gradle.kts").exists();
        let has_idea_dir = Path::new(".idea").exists();

        let mise_tasks = if mise && has_mise_toml {
            discover_mise_tasks()
        } else {
            vec![]
        };

        let parent_git_repos = discover_parent_git_repos();

        Self {
            git,
            docker,
            brew,
            gradle,
            mise,
            amp,
            omz,
            idea,
            in_git_repo,
            has_mise_toml,
            has_docker_compose,
            has_gradle_build,
            has_idea_dir,
            mise_tasks,
            parent_git_repos,
        }
    }
}

#[cfg(test)]
impl Probe {
    pub(crate) fn empty() -> Self {
        Self {
            git: false,
            docker: false,
            brew: false,
            gradle: false,
            mise: false,
            amp: false,
            omz: false,
            idea: false,
            in_git_repo: false,
            has_mise_toml: false,
            has_docker_compose: false,
            has_gradle_build: false,
            has_idea_dir: false,
            mise_tasks: vec![],
            parent_git_repos: vec![],
        }
    }
}

fn discover_mise_tasks() -> Vec<MiseTask> {
    let Ok(out) = std::process::Command::new("mise")
        .args(["tasks", "ls", "--no-header"])
        .output()
    else {
        return vec![];
    };

    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_mise_tasks(&stdout)
}

fn parse_mise_tasks(stdout: &str) -> Vec<MiseTask> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let mut parts = line.splitn(2, char::is_whitespace);
            let name = parts.next()?.trim().to_owned();
            let description = parts
                .next()
                .unwrap_or("")
                .trim()
                .trim_start_matches('#')
                .trim()
                .to_owned();
            if name.is_empty() {
                None
            } else {
                Some(MiseTask { name, description })
            }
        })
        .collect()
}

fn discover_parent_git_repos() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("..") else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| e.path().join(".git").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mise_task_name_and_description() {
        let tasks = parse_mise_tasks("build  # Build the app\n");

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "build");
        assert_eq!(tasks[0].description, "Build the app");
    }

    #[test]
    fn parses_mise_task_without_description() {
        let tasks = parse_mise_tasks("test\n");

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "test");
        assert!(tasks[0].description.is_empty());
    }

    #[test]
    fn skips_blank_mise_task_lines_and_strips_description_marker() {
        let tasks = parse_mise_tasks("\n  \nrelease    #   Publish artifacts\n");

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "release");
        assert_eq!(tasks[0].description, "Publish artifacts");
    }
}
