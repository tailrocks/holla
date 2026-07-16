use std::path::Path;

use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    providers::{Provider, run_argv},
};

const LIMIT: usize = 30;

pub struct JustProvider;

impl Provider for JustProvider {
    fn id(&self) -> &'static str {
        "just"
    }

    fn scan(&self) -> Option<GroupSpec> {
        if which::which("just").is_err()
            || !["justfile", "Justfile", ".justfile"]
                .iter()
                .any(|name| Path::new(name).is_file())
        {
            return None;
        }
        let output = std::process::Command::new("just")
            .arg("--summary")
            .output()
            .ok()?;
        output.status.success().then_some(())?;
        group(parse_summary(&String::from_utf8_lossy(&output.stdout)))
    }
}

fn parse_summary(output: &str) -> Vec<String> {
    let mut recipes = output
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    recipes.sort();
    recipes.dedup();
    recipes
}

fn group(recipes: Vec<String>) -> Option<GroupSpec> {
    let total = recipes.len();
    if total == 0 {
        return None;
    }
    Some(GroupSpec {
        id: "just".into(),
        title: title("Just", total),
        actions: recipes
            .into_iter()
            .take(LIMIT)
            .map(|name| {
                let run_name = name.clone();
                ActionSpec::new(
                    format!("just.recipe.{name}"),
                    format!("just: {name}"),
                    format!("Run just recipe `{name}`"),
                    format!("$ just {name}"),
                    &["just", "recipe", "task", "test", "build"],
                    Danger::Mutating,
                    move || Box::pin(run_argv("just".into(), vec![run_name.clone()])),
                )
            })
            .collect(),
    })
}

fn title(label: &str, total: usize) -> String {
    if total > LIMIT {
        format!("{label} ({LIMIT} of {total})")
    } else {
        label.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_summary_names_deterministically() {
        assert_eq!(
            parse_summary("test build\nrelease\n"),
            ["build", "release", "test"]
        );
    }

    #[test]
    fn empty_summary_contributes_nothing() {
        assert!(group(parse_summary(" \n")).is_none());
    }

    #[test]
    fn cap_is_visible() {
        let group = group((0..31).map(|index| format!("r{index}")).collect()).unwrap();
        assert_eq!(group.actions.len(), LIMIT);
        assert_eq!(group.title, "Just (30 of 31)");
    }
}
