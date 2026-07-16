use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    providers::{Provider, run_argv},
};

const LIMIT: usize = 30;

pub struct GitHygieneProvider;

impl Provider for GitHygieneProvider {
    fn id(&self) -> &'static str {
        "git-hygiene"
    }

    fn scan(&self) -> Option<GroupSpec> {
        which::which("git").ok()?;
        command(&["rev-parse", "--is-inside-work-tree"])
            .filter(|output| output.trim() == "true")?;

        let local = parse_branches(&command(&[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
        ])?);
        let default = choose_default(
            command(&[
                "symbolic-ref",
                "--quiet",
                "--short",
                "refs/remotes/origin/HEAD",
            ])
            .as_deref(),
            &local,
        )?;
        let current = command(&["symbolic-ref", "--quiet", "--short", "HEAD"]);
        let merged_output = command(&[
            "branch",
            "--format=%(refname:short)",
            "--merged",
            &default.revision,
        ])?;
        let merged = eligible_merged(
            &merged_output,
            &default.name,
            current.as_deref().map(str::trim),
        );
        group(merged)
    }
}

struct DefaultBranch {
    revision: String,
    name: String,
}

fn choose_default(remote_head: Option<&str>, local: &[String]) -> Option<DefaultBranch> {
    if let Some(remote) = remote_head.map(str::trim).filter(|value| !value.is_empty()) {
        let name = remote
            .strip_prefix("refs/remotes/origin/")
            .or_else(|| remote.strip_prefix("origin/"))
            .unwrap_or(remote)
            .to_owned();
        return Some(DefaultBranch {
            revision: remote.to_owned(),
            name,
        });
    }
    ["main", "master"]
        .into_iter()
        .find(|candidate| local.iter().any(|branch| branch == candidate))
        .map(|branch| DefaultBranch {
            revision: branch.to_owned(),
            name: branch.to_owned(),
        })
}

fn command(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn parse_branches(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|line| line.trim().trim_start_matches(['*', '+']).trim().to_owned())
        .filter(|branch| !branch.is_empty())
        .collect()
}

fn eligible_merged(output: &str, default: &str, current: Option<&str>) -> Vec<String> {
    let mut branches = parse_branches(output)
        .into_iter()
        .filter(|branch| branch != default && current.is_none_or(|current| branch != current))
        .collect::<Vec<_>>();
    branches.sort();
    branches.dedup();
    branches
}

fn group(mut branches: Vec<String>) -> Option<GroupSpec> {
    let total = branches.len();
    branches.truncate(LIMIT);
    let mut actions = vec![
        action(
            "git.fetch-prune",
            "git: fetch and prune",
            "Fetch remote updates and remove stale remote-tracking refs",
            &["fetch", "--prune"],
            Danger::Mutating,
        ),
        action(
            "git.gc",
            "git: garbage collect",
            "Optimize the local Git object database",
            &["gc"],
            Danger::Mutating,
        ),
    ];
    if !branches.is_empty() {
        let preview = format!("$ git branch -d -- {}", branches.join(" "));
        let run_branches = branches.clone();
        actions.insert(
            0,
            ActionSpec::new(
                "git.delete-merged",
                "git: delete merged branches",
                format!(
                    "Delete {} local branches already merged into the default branch",
                    branches.len()
                ),
                preview,
                &["git", "branches", "delete", "cleanup", "merged"],
                Danger::Destructive,
                move || {
                    let mut args = vec!["branch".into(), "-d".into(), "--".into()];
                    args.extend(run_branches.clone());
                    Box::pin(run_argv("git".into(), args))
                },
            ),
        );
    }
    Some(GroupSpec {
        id: "git-hygiene".into(),
        title: if total > LIMIT {
            format!("Git hygiene ({LIMIT} of {total} branches)")
        } else {
            "Git hygiene".into()
        },
        actions,
    })
}

fn action(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    args: &'static [&'static str],
    danger: Danger,
) -> ActionSpec {
    ActionSpec::new(
        id,
        label,
        description,
        format!("$ git {}", args.join(" ")),
        &["git", "repository", "cleanup", "branches"],
        danger,
        move || {
            Box::pin(run_argv(
                "git".into(),
                args.iter().map(|arg| (*arg).to_owned()).collect(),
            ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_head_wins_and_is_normalized() {
        let default = choose_default(Some("origin/trunk\n"), &["main".into()]).unwrap();
        assert_eq!(default.revision, "origin/trunk");
        assert_eq!(default.name, "trunk");
    }

    #[test]
    fn default_falls_back_to_main_then_master() {
        assert_eq!(
            choose_default(None, &["master".into(), "main".into()])
                .unwrap()
                .name,
            "main"
        );
        assert_eq!(
            choose_default(None, &["master".into()]).unwrap().name,
            "master"
        );
        assert!(choose_default(None, &["feature".into()]).is_none());
    }

    #[test]
    fn merged_parser_excludes_current_and_default() {
        let branches = eligible_merged(
            "* main\n+ other-worktree\nfeature\nold\n",
            "main",
            Some("feature"),
        );
        assert_eq!(branches, ["old", "other-worktree"]);
    }

    #[test]
    fn delete_action_is_destructive_and_lists_exact_branches() {
        let group = group(vec!["feature-a".into(), "fix-b".into()]).unwrap();
        let delete = &group.actions[0];
        assert_eq!(delete.danger, Danger::Destructive);
        assert_eq!(delete.preview, "$ git branch -d -- feature-a fix-b");
    }

    #[test]
    fn merged_branch_cap_is_visible() {
        let group = group((0..31).map(|index| format!("branch-{index}")).collect()).unwrap();
        assert_eq!(group.title, "Git hygiene (30 of 31 branches)");
        assert!(group.actions[0].preview.split_whitespace().count() <= 35);
    }

    #[test]
    fn no_merged_branches_still_offers_safe_hygiene_actions() {
        let group = group(Vec::new()).unwrap();
        let ids = group
            .actions
            .iter()
            .map(|action| action.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["git.fetch-prune", "git.gc"]);
    }
}
