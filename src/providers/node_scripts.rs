//! Project-local package scripts are treated like mise tasks: running them is
//! an explicit launcher action, and does not use the custom-config trust store.

use std::path::Path;

use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    providers::{Provider, run_argv},
};

const LIMIT: usize = 30;

pub struct NodeScriptsProvider;

impl Provider for NodeScriptsProvider {
    fn id(&self) -> &'static str {
        "node-scripts"
    }

    fn scan(&self) -> Option<GroupSpec> {
        let path = Path::new("package.json");
        let contents = std::fs::read_to_string(path).ok()?;
        let scripts = parse_scripts(&contents)?;
        group(scripts, pick_agent(Path::new(".")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agent {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl Agent {
    const fn program(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }
}

fn pick_agent(root: &Path) -> Agent {
    if root.join("pnpm-lock.yaml").is_file() {
        Agent::Pnpm
    } else if root.join("yarn.lock").is_file() {
        Agent::Yarn
    } else if root.join("bun.lock").is_file() || root.join("bun.lockb").is_file() {
        Agent::Bun
    } else {
        Agent::Npm
    }
}

fn parse_scripts(contents: &str) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(contents).ok()?;
    let mut scripts = value
        .get("scripts")?
        .as_object()?
        .iter()
        .filter(|(_, command)| command.is_string())
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    scripts.sort();
    Some(scripts)
}

fn group(scripts: Vec<String>, agent: Agent) -> Option<GroupSpec> {
    let total = scripts.len();
    if total == 0 {
        return None;
    }
    let program = agent.program();
    let actions = scripts
        .into_iter()
        .take(LIMIT)
        .map(|name| {
            let run_name = name.clone();
            ActionSpec::new(
                format!("node.script.{name}"),
                format!("{program}: {name}"),
                format!("Run package script `{name}`"),
                format!("$ {program} run {name}"),
                &["node", "package", "script", "task", "test", "build"],
                Danger::Mutating,
                move || {
                    Box::pin(run_argv(
                        program.to_owned(),
                        vec!["run".to_owned(), run_name.clone()],
                    ))
                },
            )
        })
        .collect();
    Some(GroupSpec {
        id: "node-scripts".into(),
        title: capped_title("Node scripts", total),
        actions,
    })
}

fn capped_title(label: &str, total: usize) -> String {
    if total > LIMIT {
        format!("{label} ({LIMIT} of {total})")
    } else {
        label.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parses_only_string_scripts_in_name_order() {
        let scripts =
            parse_scripts(r#"{"scripts":{"test":"vitest","bad":false,"build":"vite build"}}"#)
                .unwrap();
        assert_eq!(scripts, ["build", "test"]);
    }

    #[test]
    fn absent_or_empty_scripts_contribute_nothing() {
        assert!(parse_scripts("{}").is_none());
        assert!(group(parse_scripts(r#"{"scripts":{}}"#).unwrap(), Agent::Npm).is_none());
    }

    #[test]
    fn malformed_package_json_is_ignored() {
        assert!(parse_scripts("{").is_none());
    }

    #[test]
    fn lockfiles_select_modern_agent_precedence() {
        for (lockfile, expected) in [
            ("pnpm-lock.yaml", Agent::Pnpm),
            ("yarn.lock", Agent::Yarn),
            ("bun.lock", Agent::Bun),
        ] {
            let root = TempDir::new().unwrap();
            std::fs::write(root.path().join(lockfile), "").unwrap();
            assert_eq!(pick_agent(root.path()), expected);
        }
        let root = TempDir::new().unwrap();
        assert_eq!(pick_agent(root.path()), Agent::Npm);
    }

    #[test]
    fn cap_is_enforced_and_visible() {
        let group = group(
            (0..31).map(|index| format!("task-{index}")).collect(),
            Agent::Npm,
        )
        .unwrap();
        assert_eq!(group.actions.len(), LIMIT);
        assert_eq!(group.title, "Node scripts (30 of 31)");
    }
}
