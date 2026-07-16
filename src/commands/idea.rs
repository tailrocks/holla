use crate::commands::cleanup_paths;

pub async fn clean() -> anyhow::Result<()> {
    let root = std::env::current_dir()?;
    let items = cleanup_paths::discover(&root, &[".idea"], &["iml"], 5);
    cleanup_paths::move_to_trash(items).await
}
