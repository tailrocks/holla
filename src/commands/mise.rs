use crate::tui::{TaskDef, run_tasks};

pub async fn run(task: &str) -> anyhow::Result<()> {
    run_tasks(vec![TaskDef::new(
        format!("mise run {task}"),
        "mise",
        &["run", task],
    )])
    .await
}
