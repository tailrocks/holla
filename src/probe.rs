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
        let idea = which("idea").is_ok();

        let in_git_repo = Path::new(".git").exists();
        let has_mise_toml =
            Path::new("mise.toml").exists() || Path::new(".mise.toml").exists();
        let has_docker_compose =
            Path::new("docker-compose.yml").exists()
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

fn discover_mise_tasks() -> Vec<MiseTask> {
    let Ok(out) = std::process::Command::new("mise")
        .args(["tasks", "ls", "--no-header"])
        .output()
    else {
        return vec![];
    };

    let stdout = String::from_utf8_lossy(&out.stdout);
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
            if name.is_empty() { None } else { Some(MiseTask { name, description }) }
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
