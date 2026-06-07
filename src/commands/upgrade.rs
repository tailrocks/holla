use anyhow::Result;
use crate::tui::{TaskDef, run_parallel_tasks, run_tasks};
use crate::probe::Probe;

pub async fn run_all() -> Result<()> {
    let probe = Probe::run();
    let mut tasks = Vec::new();
    if probe.mise {
        tasks.push(TaskDef::new("mise upgrade", "mise", &["upgrade"]));
    }
    if probe.amp {
        tasks.push(TaskDef::new("amp update", "amp", &["update"]));
    }
    if probe.brew {
        tasks.push(TaskDef::new("brew upgrade", "brew", &["upgrade"]));
        tasks.push(TaskDef::new("brew cask upgrade", "brew", &["upgrade", "--cask", "--greedy"]));
    }
    run_parallel_tasks(tasks).await
}

pub async fn run_brew() -> Result<()> {
    run_tasks(vec![
        TaskDef::new("brew update", "brew", &["update"]),
        TaskDef::new("brew upgrade", "brew", &["upgrade"]),
        TaskDef::new("brew cleanup", "brew", &["cleanup"]),
    ])
    .await
}

pub async fn run_brew_casks() -> Result<()> {
    run_tasks(vec![
        TaskDef::new("brew update", "brew", &["update"]),
        TaskDef::new("brew upgrade casks", "brew", &["upgrade", "--cask", "--greedy"]),
        TaskDef::new("brew cleanup", "brew", &["cleanup"]),
    ])
    .await
}

pub async fn run_mise() -> Result<()> {
    run_tasks(vec![TaskDef::new("mise upgrade", "mise", &["upgrade"])]).await
}

pub async fn run_amp() -> Result<()> {
    run_tasks(vec![TaskDef::new("amp update", "amp", &["update"])]).await
}
