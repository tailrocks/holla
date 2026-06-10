use crate::probe::Probe;
use crate::tui::{TaskDef, run_parallel_tasks, run_tasks};

pub async fn run_all() -> anyhow::Result<()> {
    let probe = Probe::run();
    let mut tasks = Vec::new();
    if probe.omz {
        tasks.push(TaskDef::new("oh-my-zsh upgrade", "omz", &["update"]));
    }
    if probe.mise {
        tasks.push(TaskDef::new("mise upgrade", "mise", &["upgrade"]));
    }
    if probe.amp {
        tasks.push(TaskDef::new("amp update", "amp", &["update"]));
    }
    if probe.brew {
        tasks.push(TaskDef::new(
            "brew upgrade",
            "sh",
            &["-c", "brew update && brew upgrade --greedy --yes && brew cleanup && brew autoremove && brew doctor"],
        ));
    }
    run_parallel_tasks(tasks).await
}

pub async fn run_brew() -> anyhow::Result<()> {
    run_tasks(vec![
        TaskDef::new("brew update", "brew", &["update"]),
        TaskDef::new("brew upgrade", "brew", &["upgrade", "--greedy", "--yes"]),
        TaskDef::new("brew cleanup", "brew", &["cleanup"]),
        TaskDef::new("brew autoremove", "brew", &["autoremove"]),
        TaskDef::new("brew doctor", "brew", &["doctor"]),
    ])
    .await
}

pub async fn run_brew_casks() -> anyhow::Result<()> {
    run_tasks(vec![
        TaskDef::new("brew update", "brew", &["update"]),
        TaskDef::new(
            "brew upgrade casks",
            "brew",
            &["upgrade", "--cask", "--greedy", "--yes"],
        ),
        TaskDef::new("brew cleanup", "brew", &["cleanup"]),
        TaskDef::new("brew autoremove", "brew", &["autoremove"]),
        TaskDef::new("brew doctor", "brew", &["doctor"]),
    ])
    .await
}

pub async fn run_mise() -> anyhow::Result<()> {
    run_tasks(vec![TaskDef::new("mise upgrade", "mise", &["upgrade"])]).await
}

pub async fn run_amp() -> anyhow::Result<()> {
    run_tasks(vec![TaskDef::new("amp update", "amp", &["update"])]).await
}

pub async fn run_omz() -> anyhow::Result<()> {
    run_tasks(vec![TaskDef::new("oh-my-zsh upgrade", "omz", &["update"])]).await
}
