use crate::tui::{TaskDef, run_tasks};

pub async fn stop_all() -> anyhow::Result<()> {
    let containers = list_containers().await?;
    if containers.is_empty() {
        return Ok(());
    }
    let mut stop_args = vec!["-c".to_string()];
    stop_args.push(format!("docker stop {}", containers.join(" ")));

    let mut rm_args = vec!["-c".to_string()];
    rm_args.push(format!("docker rm {}", containers.join(" ")));

    run_tasks(vec![
        TaskDef {
            label: "Stopping containers".into(),
            program: "sh".into(),
            args: stop_args,
        },
        TaskDef {
            label: "Removing containers".into(),
            program: "sh".into(),
            args: rm_args,
        },
    ])
    .await
}

pub async fn clean() -> anyhow::Result<()> {
    stop_all().await?;

    // Match legacy docker_clean_all exactly: after containers, force-remove all images,
    // then prune networks/system/volumes. Use shell for the rmi/images step to match
    // the original `docker rmi --force $(docker images -qa)` behavior (with 2>/dev/null tolerance).
    run_tasks(vec![
        TaskDef {
            label: "Removing images".into(),
            program: "sh".into(),
            args: vec![
                "-c".into(),
                "docker rmi --force $(docker images -qa) 2>/dev/null || true".into(),
            ],
        },
        TaskDef::new(
            "Pruning networks",
            "docker",
            &["network", "prune", "--force"],
        ),
        TaskDef::new("Pruning system", "docker", &["system", "prune", "--force"]),
        TaskDef::new("Pruning volumes", "docker", &["volume", "prune", "--force"]),
    ])
    .await
}

pub async fn builder_prune() -> anyhow::Result<()> {
    run_tasks(vec![TaskDef::new(
        "Pruning Docker builder cache",
        "docker",
        &["builder", "prune", "-f"],
    )])
    .await
}

async fn list_containers() -> anyhow::Result<Vec<String>> {
    let out = tokio::process::Command::new("docker")
        .args(["ps", "-qa"])
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect())
}
