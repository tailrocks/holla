
pub async fn stop_all() -> Result<()> {
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

pub async fn clean() -> Result<()> {
    stop_all().await?;
    run_tasks(vec![
        TaskDef::new("Pruning networks", "docker", &["network", "prune", "--force"]),
        TaskDef::new("Pruning system", "docker", &["system", "prune", "--force"]),
        TaskDef::new("Pruning volumes", "docker", &["volume", "prune", "--force"]),
    ])
    .await
}

async fn list_containers() -> Result<Vec<String>> {
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
