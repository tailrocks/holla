use std::path::Path;

use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    providers::{Provider, run_argv},
};

pub struct CargoProvider;

impl Provider for CargoProvider {
    fn id(&self) -> &'static str {
        "cargo-project"
    }

    fn scan(&self) -> Option<GroupSpec> {
        (Path::new("Cargo.toml").is_file() && which::which("cargo").is_ok()).then(group)
    }
}

fn group() -> GroupSpec {
    GroupSpec {
        id: "cargo-project".into(),
        title: "Cargo".into(),
        actions: vec![
            action(
                "build",
                "Build the Rust project",
                &["build"],
                Danger::Mutating,
            ),
            action("test", "Run Rust tests", &["test"], Danger::Mutating),
            action(
                "clippy",
                "Lint every Rust target",
                &["clippy", "--all-targets", "--all-features"],
                Danger::Mutating,
            ),
            action(
                "clean",
                "Delete Cargo build artifacts from target/",
                &["clean"],
                Danger::Destructive,
            ),
        ],
    }
}

fn action(
    name: &'static str,
    description: &'static str,
    args: &'static [&'static str],
    danger: Danger,
) -> ActionSpec {
    ActionSpec::new(
        format!("cargo.{name}"),
        format!("cargo: {name}"),
        description,
        format!("$ cargo {}", args.join(" ")),
        &["cargo", "rust", "build", "test", "lint", "cleanup"],
        danger,
        move || {
            Box::pin(run_argv(
                "cargo".into(),
                args.iter().map(|arg| (*arg).to_owned()).collect(),
            ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_clean_is_destructive_with_exact_preview() {
        let group = group();
        let clean = group
            .actions
            .iter()
            .find(|action| action.id == "cargo.clean")
            .unwrap();
        assert_eq!(clean.danger, Danger::Destructive);
        assert_eq!(clean.preview, "$ cargo clean");
    }

    #[test]
    fn cargo_static_actions_are_complete() {
        let group = group();
        let ids = group
            .actions
            .iter()
            .map(|action| action.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            ["cargo.build", "cargo.test", "cargo.clippy", "cargo.clean"]
        );
    }
}
