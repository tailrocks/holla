use crate::tui::{TaskDef, run_parallel_tasks, run_tasks};

pub async fn pull_all(repos: &[String]) -> anyhow::Result<()> {
    let tasks = repos
        .iter()
        .map(|repo| TaskDef::new(format!("git pull — {repo}"), "git", &["-C", repo, "pull"]))
        .collect();
    run_parallel_tasks(tasks).await
}

pub async fn push_all(repos: &[String]) -> anyhow::Result<()> {
    let tasks = repos
        .iter()
        .map(|repo| TaskDef::new(format!("git push — {repo}"), "git", &["-C", repo, "push"]))
        .collect();
    run_parallel_tasks(tasks).await
}

pub async fn status_all(repos: &[String]) -> anyhow::Result<()> {
    let tasks = repos
        .iter()
        .map(|repo| {
            TaskDef::new(
                format!("status — {repo}"),
                "git",
                &["-C", repo, "status", "--short"],
            )
        })
        .collect();
    run_tasks(tasks).await
}

pub async fn push_all_remotes(repos: &[String]) -> anyhow::Result<()> {
    let mut tasks = Vec::new();
    for repo in repos {
        let has_gitlab = tokio::process::Command::new("git")
            .args(["-C", repo, "remote", "get-url", "gitlab"])
            .output()
            .await
            .is_ok_and(|output| output.status.success());

        let origin_label = if has_gitlab {
            format!("push origin — {repo}")
        } else {
            format!("push origin — {repo} (no gitlab remote)")
        };
        tasks.push(TaskDef::new(
            origin_label,
            "git",
            &["-C", repo, "push", "origin"],
        ));
        if has_gitlab {
            tasks.push(TaskDef::new(
                format!("push gitlab — {repo}"),
                "git",
                &["-C", repo, "push", "gitlab"],
            ));
        }
    }
    run_parallel_tasks(tasks).await
}
