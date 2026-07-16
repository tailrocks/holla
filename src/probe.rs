use std::path::{Path, PathBuf};

use which::which;

#[derive(Debug, Clone)]
pub struct MiseTask {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default)]
pub struct Probe {
    // system tools
    pub git: bool,
    pub docker: bool,
    pub brew: bool,
    pub gradle: bool,
    pub mise: bool,
    pub amp: bool,
    pub omz_dir: Option<PathBuf>,
    pub idea: bool,

    // current folder context
    pub in_git_repo: bool,
    pub has_docker_compose: bool,
    pub has_gradle_build: bool,
    pub has_idea_dir: bool,
    pub mise_tasks: Vec<MiseTask>,

    // repositories immediately inside the current folder
    pub child_git_repos: Vec<String>,
}

impl Probe {
    pub fn run() -> Self {
        let git = which("git").is_ok();
        let docker = which("docker").is_ok();
        let brew = which("brew").is_ok();
        let gradle = which("gradle").is_ok();
        let mise = which("mise").is_ok();
        let amp = which("amp").is_ok();
        let omz_dir = discover_omz_dir();
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

        let child_git_repos = discover_child_git_repos();

        Self {
            git,
            docker,
            brew,
            gradle,
            mise,
            amp,
            omz_dir,
            idea,
            in_git_repo,
            has_docker_compose,
            has_gradle_build,
            has_idea_dir,
            mise_tasks,
            child_git_repos,
        }
    }

    pub fn current_folder() -> Self {
        let git = which("git").is_ok();
        let docker = which("docker").is_ok();
        let gradle = which("gradle").is_ok();
        let mise = which("mise").is_ok();
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
        Self {
            git,
            docker,
            gradle,
            mise,
            idea,
            in_git_repo,
            has_docker_compose,
            has_gradle_build,
            has_idea_dir,
            mise_tasks,
            ..Self::default()
        }
    }

    pub fn repositories() -> Self {
        Self {
            git: which("git").is_ok(),
            child_git_repos: discover_child_git_repos(),
            ..Self::default()
        }
    }

    pub fn system() -> Self {
        Self {
            brew: which("brew").is_ok(),
            mise: which("mise").is_ok(),
            amp: which("amp").is_ok(),
            omz_dir: discover_omz_dir(),
            ..Self::default()
        }
    }

    pub fn docker() -> Self {
        Self {
            docker: which("docker").is_ok(),
            ..Self::default()
        }
    }

    pub fn gradle() -> Self {
        Self {
            gradle: which("gradle").is_ok(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
impl Probe {
    pub(crate) fn empty() -> Self {
        Self::default()
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

fn discover_omz_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ZSH").map(PathBuf::from)
        && path.is_dir()
    {
        return Some(path);
    }

    let path = PathBuf::from(std::env::var_os("HOME")?).join(".oh-my-zsh");
    path.is_dir().then_some(path)
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

fn discover_child_git_repos() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(".") else {
        return vec![];
    };
    let repos = entries
        .flatten()
        .filter(|e| e.path().join(".git").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    sort_repo_names(repos)
}

fn sort_repo_names(mut repos: Vec<String>) -> Vec<String> {
    repos.sort();
    repos
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

    #[test]
    fn sorts_discovered_child_repositories_alphabetically() {
        let repos = sort_repo_names(vec!["zeta".into(), "alpha".into(), "beta".into()]);

        assert_eq!(repos, ["alpha", "beta", "zeta"]);
    }
}
