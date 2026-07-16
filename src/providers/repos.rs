use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    probe::Probe,
    providers::Provider,
};

pub struct ReposProvider;

impl Provider for ReposProvider {
    fn id(&self) -> &'static str {
        "repos"
    }

    fn scan(&self) -> Option<GroupSpec> {
        group(&Probe::repositories())
    }
}

pub(super) fn group(probe: &Probe) -> Option<GroupSpec> {
    if !probe.git || probe.child_git_repos.len() <= 1 {
        return None;
    }
    let repo_list = probe.child_git_repos.join(", ");
    let pull_repos = probe.child_git_repos.clone();
    let push_repos = probe.child_git_repos.clone();
    let status_repos = probe.child_git_repos.clone();
    let remote_repos = probe.child_git_repos.clone();
    Some(GroupSpec {
        id: "repos".into(),
        title: "Repos in this folder".into(),
        actions: vec![
            ActionSpec::new(
                "git.pull-all",
                "git: pull all repos",
                format!("Pull {} repos in parallel", probe.child_git_repos.len()),
                format!("Repos: {repo_list}\n\n$ git pull (parallel)"),
                &["repository", "sync"],
                Danger::Mutating,
                move || {
                    let repos = pull_repos.clone();
                    Box::pin(async move { crate::commands::git::pull_all(&repos).await })
                },
            ),
            ActionSpec::new(
                "git.push-all",
                "git: push all repos",
                format!("Push {} repos in parallel", probe.child_git_repos.len()),
                format!("Repos: {repo_list}\n\n$ git push (parallel)"),
                &["repository", "publish"],
                Danger::Mutating,
                move || {
                    let repos = push_repos.clone();
                    Box::pin(async move { crate::commands::git::push_all(&repos).await })
                },
            ),
            ActionSpec::new(
                "git.status-all",
                "git: status all repos",
                "Show status of all repos",
                format!("Repos: {repo_list}\n\n$ git status --short"),
                &["repository", "inspect"],
                Danger::Safe,
                move || {
                    let repos = status_repos.clone();
                    Box::pin(async move { crate::commands::git::status_all(&repos).await })
                },
            ),
            ActionSpec::new(
                "git.push-all-remotes",
                "git: push all remotes",
                "Push every repo to origin + gitlab",
                format!("Repos: {repo_list}\n\n$ git push origin\n$ git push gitlab"),
                &["repository", "publish", "mirror"],
                Danger::Mutating,
                move || {
                    let repos = remote_repos.clone();
                    Box::pin(async move { crate::commands::git::push_all_remotes(&repos).await })
                },
            ),
        ],
    })
}
