use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    probe::Probe,
    providers::Provider,
};

pub struct DockerProvider;

impl Provider for DockerProvider {
    fn id(&self) -> &'static str {
        "docker"
    }

    fn scan(&self) -> Option<GroupSpec> {
        let probe = Probe::docker();
        let preview = docker_size_preview();
        group_with_preview(&probe, preview.as_deref())
    }
}

#[cfg(test)]
pub(super) fn group(probe: &Probe) -> Option<GroupSpec> {
    group_with_preview(probe, None)
}

fn group_with_preview(probe: &Probe, size_preview: Option<&str>) -> Option<GroupSpec> {
    probe.docker.then(|| GroupSpec {
        id: "system".into(),
        title: "System".into(),
        actions: vec![
            ActionSpec::new(
                "docker.stop-all",
                "docker: stop all containers",
                "Stop and remove all running containers",
                "$ docker ps -qa | xargs docker stop\n$ docker ps -qa | xargs docker rm",
                &["cleanup", "containers", "remove"],
                Danger::Destructive,
                || Box::pin(crate::commands::docker::stop_all()),
            ),
            ActionSpec::new(
                "docker.clean-all",
                "docker: clean everything",
                "Stop/remove containers, force-remove all images, prune networks/system/volumes (matches legacy docker_clean_all)",
                "$ docker ps -qa | xargs docker stop\n$ docker ps -qa | xargs docker rm\n$ docker rmi --force $(docker images -qa)\n$ docker network rm ...\n$ docker system prune --force\n$ docker volume prune --force",
                &["cleanup", "prune", "images", "volumes", "networks"],
                Danger::Destructive,
                || Box::pin(crate::commands::docker::clean()),
            ),
            ActionSpec::new(
                "docker.builder-prune",
                "docker: prune builder cache",
                "Review Docker disk accounting, then remove unused builder cache",
                size_preview.map_or_else(
                    || "$ docker system df\n$ docker builder prune -f".to_owned(),
                    |output| format!("$ docker system df\n{output}\n$ docker builder prune -f"),
                ),
                &["cleanup", "docker", "builder", "cache", "disk"],
                Danger::Destructive,
                || Box::pin(crate::commands::docker::builder_prune()),
            ),
        ],
    })
}

fn docker_size_preview() -> Option<String> {
    let output = std::process::Command::new("docker")
        .args(["system", "df"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|output| !output.is_empty())
}
