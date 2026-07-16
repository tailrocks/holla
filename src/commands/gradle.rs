use crate::{
    commands::cleanup_paths,
    tui::{TaskDef, run_tasks},
};

pub async fn clean() -> anyhow::Result<()> {
    let _ = run_tasks(vec![TaskDef::new(
        "Stopping Gradle daemon",
        "gradle",
        &["--stop"],
    )])
    .await;
    let root = std::env::current_dir()?;
    let items = cleanup_paths::discover(&root, &[".gradle", "build"], &[], 5);
    cleanup_paths::move_to_trash(items).await
}
