use crate::tui::{TaskDef, run_parallel_tasks, run_tasks};

pub async fn pull_all() -> anyhow::Result<()> {
    let tasks = find_git_repos()
        .into_iter()
        .map(|repo| TaskDef::new(format!("git pull — {repo}"), "git", &["-C", &repo, "pull"]))
        .collect();
    run_parallel_tasks(tasks).await
}

pub async fn push_all() -> anyhow::Result<()> {
    let tasks = find_git_repos()
        .into_iter()
        .map(|repo| TaskDef::new(format!("git push — {repo}"), "git", &["-C", &repo, "push"]))
        .collect();
    run_parallel_tasks(tasks).await
}

pub async fn status_all() -> anyhow::Result<()> {
    let tasks = find_git_repos()
        .into_iter()
        .map(|repo| TaskDef::new(format!("status — {repo}"), "git", &["-C", &repo, "status", "--short"]))
        .collect();
    run_tasks(tasks).await
}

pub async fn push_all_remotes() -> anyhow::Result<()> {
    // Match legacy git_push_origin_gitlab_all exactly:
    // For each immediate git subdir, if it has a 'gitlab' remote, push both;
    // otherwise warn (yellow) and push origin only. Non-repos are already filtered by find_git_repos.
    let repos = find_git_repos();
    let tasks: Vec<_> = repos
        .into_iter()
        .map(|repo| {
            // Shell snippet replicates the legacy per-dir if + colored warnings.
            // We run per-repo so the TUI can show per-task progress/output.
            let script = format!(
                r#"if git -C "{repo}" remote get-url gitlab &>/dev/null; then
  echo -e "\e[1;37mPushing {repo}\e[0m"
  git -C "{repo}" push origin && git -C "{repo}" push gitlab
else
  echo -e "\e[1;33m  {repo} has no 'gitlab' remote, pushing to origin only\e[0m"
  git -C "{repo}" push origin
fi"#
            );
            TaskDef {
                label: format!("push all remotes — {repo}"),
                program: "sh".into(),
                args: vec!["-c".into(), script],
            }
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
