use crate::{
    config::{self, ConfigAction, ConfigDanger, ConfigError, ConfigOrigin, TrustStore},
    model::{ActionSpec, Danger, GroupSpec},
    providers::Provider,
    tui::{TaskDef, run_tasks, trust},
};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::Mutex,
};

pub struct UserProvider {
    group: Mutex<Option<GroupSpec>>,
}

impl Provider for UserProvider {
    fn id(&self) -> &'static str {
        "user"
    }

    fn scan(&self) -> Option<GroupSpec> {
        self.group.lock().expect("user group lock").take()
    }
}

pub fn load() -> (Vec<Box<dyn Provider>>, Vec<ConfigError>) {
    let mut seen = builtin_ids();
    let mut actions = Vec::new();
    let mut errors = Vec::new();
    if let Some(path) = config::global_actions_path() {
        let report = config::load_actions(&path, ConfigOrigin::Global, &seen);
        seen.extend(report.actions.iter().map(|action| action.id.clone()));
        actions.extend(report.actions);
        errors.extend(report.errors);
    }
    let project_path = PathBuf::from(".holla.toml");
    let report = config::load_actions(&project_path, ConfigOrigin::Project, &seen);
    actions.extend(report.actions);
    errors.extend(report.errors);

    let trust_path = config::trust_store_path();
    let trusted = trust_path
        .as_deref()
        .map(TrustStore::load)
        .unwrap_or_default();
    let mut grouped = BTreeMap::<String, Vec<ActionSpec>>::new();
    for action in actions {
        let title = action.group.clone();
        grouped
            .entry(title)
            .or_default()
            .push(to_action(action, &trusted, trust_path.clone()));
    }
    let providers = grouped
        .into_iter()
        .map(|(title, actions)| {
            let id = if title == "Current folder" {
                "current-folder".to_owned()
            } else {
                format!("user.{}", title.to_lowercase().replace(' ', "-"))
            };
            Box::new(UserProvider {
                group: Mutex::new(Some(GroupSpec { id, title, actions })),
            }) as Box<dyn Provider>
        })
        .collect();
    (providers, errors)
}

fn to_action(
    action: ConfigAction,
    trusted: &TrustStore,
    trust_path: Option<PathBuf>,
) -> ActionSpec {
    let needs_trust = action
        .project_hash
        .as_deref()
        .is_some_and(|hash| !trusted.contains(hash));
    let label = if needs_trust {
        format!("{}  ⚠ unreviewed", action.label)
    } else {
        action.label.clone()
    };
    let preview = action
        .command
        .iter()
        .enumerate()
        .map(|(index, argument)| format!("argv[{index}] = {argument:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    let danger = match action.danger {
        ConfigDanger::Safe => Danger::Safe,
        ConfigDanger::Mutating => Danger::Mutating,
        ConfigDanger::Destructive => Danger::Destructive,
    };
    let id = action.id.clone();
    let description = action.description.clone();
    let keywords = action.keywords.clone();
    let command = action.command.clone();
    let directory = action.working_directory.clone();
    let project_hash = action.project_hash.clone();
    ActionSpec::new(id, label, description, preview, &[], danger, move || {
        let command = command.clone();
        let directory = directory.clone();
        let project_hash = project_hash.clone();
        let trust_path = trust_path.clone();
        Box::pin(async move {
            if let Some(hash) = project_hash {
                let mut store = trust_path
                    .as_deref()
                    .map(TrustStore::load)
                    .unwrap_or_default();
                if !store.contains(&hash) {
                    let accepted = if trust::assumed() {
                        true
                    } else {
                        let argv = command.clone();
                        tokio::task::spawn_blocking(move || trust::confirm(&argv)).await??
                    };
                    if !accepted {
                        return Ok(());
                    }
                    store.accept(hash);
                    if let Some(path) = trust_path.as_deref() {
                        store.save(path)?;
                    }
                }
            }
            let (program, args) = command.split_first().expect("validated argv");
            run_tasks(vec![TaskDef {
                label: program.clone(),
                program: program.clone(),
                args: args.to_vec(),
                working_directory: Some(directory),
            }])
            .await
        })
    })
    .with_confirmation(action.confirm)
    .with_trust_required(needs_trust)
    .with_keywords(keywords)
}

fn builtin_ids() -> HashSet<String> {
    [
        "find.files",
        "disk.overview",
        "disk.scan-here",
        "disk.scan-custom",
        "git.pull",
        "git.push",
        "git.status",
        "gradle.clean",
        "gradle.build",
        "gradle.test",
        "compose.up",
        "compose.down",
        "compose.logs",
        "idea.clean",
        "docker.stop-all",
        "docker.clean-all",
        "docker.builder-prune",
        "upgrade.all",
        "upgrade.brew-packages",
        "upgrade.brew-casks",
        "upgrade.mise",
        "upgrade.amp",
        "upgrade.oh-my-zsh",
        "insights.review-all",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(origin: ConfigOrigin, hash: Option<&str>) -> ConfigAction {
        ConfigAction {
            id: "test.action".into(),
            label: "Test action".into(),
            description: String::new(),
            command: vec!["true".into()],
            danger: ConfigDanger::Safe,
            keywords: vec!["test".into()],
            group: "Custom".into(),
            confirm: false,
            origin,
            working_directory: PathBuf::from("."),
            project_hash: hash.map(str::to_owned),
        }
    }

    #[test]
    fn untrusted_project_action_has_badge_and_gate() {
        let result = to_action(
            action(ConfigOrigin::Project, Some("hash")),
            &TrustStore::default(),
            None,
        );
        assert!(result.label.contains("⚠ unreviewed"));
        assert!(result.trust_required);
    }

    #[test]
    fn global_action_is_implicitly_trusted() {
        let result = to_action(
            action(ConfigOrigin::Global, None),
            &TrustStore::default(),
            None,
        );
        assert_eq!(result.label, "Test action");
        assert!(!result.trust_required);
        assert_eq!(result.keywords, ["test"]);
    }
}
