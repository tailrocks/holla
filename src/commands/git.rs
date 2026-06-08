
pub async fn pull_all() -> Result<()> {
    let tasks = find_git_repos()
        .into_iter()
        .map(|repo| TaskDef::new(format!("git pull — {repo}"), "git", &["-C", &repo, "pull"]))
        .collect();
    run_parallel_tasks(tasks).await
}

pub async fn push_all() -> Result<()> {
    let tasks = find_git_repos()
        .into_iter()
        .map(|repo| TaskDef::new(format!("git push — {repo}"), "git", &["-C", &repo, "push"]))
        .collect();
    run_parallel_tasks(tasks).await
}

pub async fn status_all() -> Result<()> {
    let tasks = find_git_repos()
        .into_iter()
        .map(|repo| TaskDef::new(format!("status — {repo}"), "git", &["-C", &repo, "status", "--short"]))
        .collect();
    run_tasks(tasks).await
}

pub async fn push_all_remotes() -> Result<()> {
    let repos = find_git_repos();
    let tasks = repos
        .into_iter()
        .flat_map(|repo| {
            vec![
                TaskDef::new(format!("origin — {repo}"), "git", &["-C", &repo, "push", "origin"]),
                TaskDef::new(format!("gitlab — {repo}"), "git", &["-C", &repo, "push", "gitlab"]),
            ]
        })
        .collect();
    run_parallel_tasks(tasks).await
}

fn find_git_repos() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(".") else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| e.path().join(".git").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}
