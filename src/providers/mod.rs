mod current_folder;
mod disk;
mod docker;
mod find;
mod gradle;
mod insights;
mod repos;
mod system;
mod user;

use crate::model::GroupSpec;
#[cfg(test)]
use crate::probe::Probe;
use std::sync::mpsc;

pub trait Provider: Send {
    fn id(&self) -> &'static str;
    fn scan(&self) -> Option<GroupSpec>;
}

pub fn all_providers() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(find::FindProvider),
        Box::new(disk::DiskProvider),
        Box::new(current_folder::CurrentFolderProvider),
        Box::new(repos::ReposProvider),
        Box::new(system::SystemProvider),
        Box::new(docker::DockerProvider),
        Box::new(gradle::GradleProvider),
        Box::new(insights::InsightsProvider),
    ]
}

pub enum ScanEvent {
    Group {
        provider_index: usize,
        provider_id: &'static str,
        group: GroupSpec,
    },
    Warning(String),
    Finished,
}

pub struct RegistryReport {
    pub groups: Vec<GroupSpec>,
    pub warnings: Vec<String>,
}

pub fn scan_all() -> RegistryReport {
    let receiver = spawn_scans();
    let mut groups = Vec::new();
    let mut warnings = Vec::new();
    while let Ok(event) = receiver.recv() {
        match event {
            ScanEvent::Group {
                provider_index,
                group,
                ..
            } => groups.push((provider_index, group)),
            ScanEvent::Warning(warning) => warnings.push(warning),
            ScanEvent::Finished => break,
        }
    }
    groups.sort_by_key(|(index, _)| *index);
    let mut seen = std::collections::HashSet::new();
    for (_, group) in &mut groups {
        group.actions.retain(|action| {
            if seen.insert(action.id.clone()) {
                true
            } else {
                warnings.push(format!(
                    "action id `{}` collides with an earlier provider; earlier action wins",
                    action.id
                ));
                false
            }
        });
    }
    RegistryReport {
        groups: groups.into_iter().map(|(_, group)| group).collect(),
        warnings,
    }
}

pub fn spawn_scans() -> mpsc::Receiver<ScanEvent> {
    let (tx, rx) = mpsc::channel();
    let (user_providers, errors) = user::load();
    for error in errors {
        let _ = tx.send(ScanEvent::Warning(error.to_string()));
    }
    let mut providers = all_providers();
    providers.extend(user_providers);
    let workers: Vec<_> = providers
        .into_iter()
        .enumerate()
        .map(|(provider_index, provider)| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let provider_id = provider.id();
                if let Some(group) = provider.scan() {
                    let _ = tx.send(ScanEvent::Group {
                        provider_index,
                        provider_id,
                        group,
                    });
                }
            })
        })
        .collect();
    std::thread::spawn(move || {
        for worker in workers {
            let _ = worker.join();
        }
        let _ = tx.send(ScanEvent::Finished);
    });
    rx
}

#[cfg(test)]
pub fn groups_from_probe(probe: &Probe) -> Vec<GroupSpec> {
    [
        Some(find::group()),
        Some(disk::group()),
        current_folder::group(probe),
        repos::group(probe),
        system::group(probe),
        docker::group(probe),
        gradle::group(probe),
        Some(insights::group(&[])),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub(crate) async fn run_shell(command: &str) -> anyhow::Result<()> {
    use crate::tui::{TaskDef, run_tasks};
    run_tasks(vec![TaskDef {
        label: command.to_owned(),
        program: "sh".into(),
        args: vec!["-c".into(), command.to_owned()],
        working_directory: None,
    }])
    .await
}
