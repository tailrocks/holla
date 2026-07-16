use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    probe::Probe,
    providers::{Provider, run_argv},
};

pub struct CurrentFolderProvider;

impl Provider for CurrentFolderProvider {
    fn id(&self) -> &'static str {
        "current-folder"
    }

    fn scan(&self) -> Option<GroupSpec> {
        group(&Probe::current_folder())
    }
}

pub(super) fn group(probe: &Probe) -> Option<GroupSpec> {
    let mut actions = Vec::new();

    for task in &probe.mise_tasks {
        let name = task.name.clone();
        let description = if task.description.is_empty() {
            format!("Run mise task `{name}`")
        } else {
            task.description.clone()
        };
        let run_name = name.clone();
        actions.push(ActionSpec::new(
            format!("mise.task.{name}"),
            format!("mise: {name}"),
            description,
            format!("$ mise run {name}"),
            &["task", "script"],
            Danger::Mutating,
            move || {
                let name = run_name.clone();
                Box::pin(async move { crate::commands::mise::run(&name).await })
            },
        ));
    }

    if probe.in_git_repo && probe.git {
        actions.extend([
            shell_action(
                "git.pull",
                "git: pull",
                "Pull latest changes for this repository",
                "$ git pull",
                &["repository", "sync"],
                Danger::Mutating,
                ("git", &["pull"]),
            ),
            shell_action(
                "git.push",
                "git: push",
                "Push commits to remote",
                "$ git push",
                &["repository", "publish"],
                Danger::Mutating,
                ("git", &["push"]),
            ),
            shell_action(
                "git.status",
                "git: status",
                "Show working tree status",
                "$ git status",
                &["repository", "inspect"],
                Danger::Safe,
                ("git", &["status"]),
            ),
        ]);
    }

    if probe.has_gradle_build && probe.gradle {
        actions.extend([
            shell_action(
                "gradle.clean",
                "gradle: clean",
                "Clean build output",
                "$ gradle clean",
                &["cleanup", "build"],
                Danger::Mutating,
                ("gradle", &["clean"]),
            ),
            shell_action(
                "gradle.build",
                "gradle: build",
                "Build the project",
                "$ gradle build",
                &["compile"],
                Danger::Mutating,
                ("gradle", &["build"]),
            ),
            shell_action(
                "gradle.test",
                "gradle: test",
                "Run tests",
                "$ gradle test",
                &["verify"],
                Danger::Mutating,
                ("gradle", &["test"]),
            ),
        ]);
    }

    if probe.has_docker_compose && probe.docker {
        actions.extend([
            shell_action(
                "compose.up",
                "compose: up",
                "Start services in background",
                "$ docker compose up -d",
                &["docker", "services", "start"],
                Danger::Mutating,
                ("docker", &["compose", "up", "-d"]),
            ),
            shell_action(
                "compose.down",
                "compose: down",
                "Stop and remove containers",
                "$ docker compose down",
                &["docker", "services", "stop"],
                Danger::Mutating,
                ("docker", &["compose", "down"]),
            ),
            shell_action(
                "compose.logs",
                "compose: logs (follow)",
                "Follow service logs until cancelled",
                "$ docker compose logs -f",
                &["docker", "services", "tail"],
                Danger::Safe,
                ("docker", &["compose", "logs", "-f"]),
            ),
        ]);
    }

    if probe.has_idea_dir || probe.idea {
        actions.push(ActionSpec::new(
            "idea.clean",
            "idea: clean",
            "Remove .idea dirs and *.iml files",
            "Enumerate .idea directories and *.iml files to depth 5, then move them to Trash through holla's validated cleanup core",
            &["cleanup", "jetbrains", "intellij"],
            Danger::Destructive,
            || Box::pin(crate::commands::idea::clean()),
        ));
    }

    (!actions.is_empty()).then_some(GroupSpec {
        id: "current-folder".into(),
        title: "Current folder".into(),
        actions,
    })
}

fn shell_action(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    preview: &'static str,
    keywords: &'static [&'static str],
    danger: Danger,
    command: (&'static str, &'static [&'static str]),
) -> ActionSpec {
    let (program, args) = command;
    ActionSpec::new(
        id,
        label,
        description,
        preview,
        keywords,
        danger,
        move || {
            Box::pin(run_argv(
                program.to_owned(),
                args.iter().map(|arg| (*arg).to_owned()).collect(),
            ))
        },
    )
}
