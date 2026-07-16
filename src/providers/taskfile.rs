use std::path::Path;

use serde::Deserialize;

use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    providers::{Provider, run_argv},
};

const LIMIT: usize = 30;

pub struct TaskfileProvider;

impl Provider for TaskfileProvider {
    fn id(&self) -> &'static str {
        "taskfile"
    }

    fn scan(&self) -> Option<GroupSpec> {
        if which::which("task").is_err()
            || !["Taskfile.yml", "Taskfile.yaml"]
                .iter()
                .any(|name| Path::new(name).is_file())
        {
            return None;
        }
        let output = std::process::Command::new("task")
            .args(["--list", "--json"])
            .output()
            .ok()?;
        output.status.success().then_some(())?;
        group(parse_tasks(&output.stdout)?)
    }
}

#[derive(Deserialize)]
struct Listing {
    tasks: Vec<ListedTask>,
}

#[derive(Deserialize)]
struct ListedTask {
    name: String,
}

fn parse_tasks(output: &[u8]) -> Option<Vec<String>> {
    let listing: Listing = serde_json::from_slice(output).ok()?;
    let mut tasks = listing
        .tasks
        .into_iter()
        .map(|task| task.name)
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    tasks.sort();
    tasks.dedup();
    Some(tasks)
}

fn group(tasks: Vec<String>) -> Option<GroupSpec> {
    let total = tasks.len();
    if total == 0 {
        return None;
    }
    Some(GroupSpec {
        id: "taskfile".into(),
        title: if total > LIMIT {
            format!("Taskfile ({LIMIT} of {total})")
        } else {
            "Taskfile".into()
        },
        actions: tasks
            .into_iter()
            .take(LIMIT)
            .map(|name| {
                let run_name = name.clone();
                ActionSpec::new(
                    format!("taskfile.task.{name}"),
                    format!("task: {name}"),
                    format!("Run Taskfile task `{name}`"),
                    format!("$ task {name}"),
                    &["task", "taskfile", "test", "build"],
                    Danger::Mutating,
                    move || Box::pin(run_argv("task".into(), vec![run_name.clone()])),
                )
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_task_json_schema() {
        let tasks = parse_tasks(
            br#"{"tasks":[{"name":"test","task":"test","desc":"Run tests"},{"name":"build"}],"location":"/tmp/Taskfile.yml"}"#,
        )
        .unwrap();
        assert_eq!(tasks, ["build", "test"]);
    }

    #[test]
    fn malformed_or_empty_list_contributes_nothing() {
        assert!(parse_tasks(b"[]").is_none());
        assert!(group(parse_tasks(br#"{"tasks":[]}"#).unwrap()).is_none());
    }

    #[test]
    fn cap_is_enforced_and_visible() {
        let group = group((0..31).map(|index| format!("t{index}")).collect()).unwrap();
        assert_eq!(group.actions.len(), LIMIT);
        assert_eq!(group.title, "Taskfile (30 of 31)");
    }
}
