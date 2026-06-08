
pub async fn run(task: &str) -> Result<()> {
    run_tasks(vec![TaskDef::new(
        format!("mise run {task}"),
        "mise",
        &["run", task],
    )])
    .await
}
