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
        group(&Probe::docker())
    }
}

pub(super) fn group(probe: &Probe) -> Option<GroupSpec> {
    probe.docker.then(|| GroupSpec {
        id: "system",
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
        ],
    })
}
