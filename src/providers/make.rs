use std::{collections::HashSet, path::Path};

use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    providers::{Provider, run_argv},
};

const LIMIT: usize = 30;

pub struct MakeProvider;

impl Provider for MakeProvider {
    fn id(&self) -> &'static str {
        "make"
    }

    fn scan(&self) -> Option<GroupSpec> {
        if which::which("make").is_err() || !Path::new("Makefile").is_file() {
            return None;
        }
        let contents = std::fs::read_to_string("Makefile").ok()?;
        group(parse_targets(&contents))
    }
}

fn parse_targets(contents: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for line in contents.lines() {
        if line.starts_with(char::is_whitespace) || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((left, right)) = line.split_once(':') else {
            continue;
        };
        if right.trim_start().starts_with('=') {
            continue;
        }
        for target in left.split_whitespace() {
            if valid_target(target) && seen.insert(target.to_owned()) {
                targets.push(target.to_owned());
            }
        }
    }
    targets
}

fn valid_target(target: &str) -> bool {
    !target.starts_with('.')
        && !target.contains(['%', '$'])
        && !target.is_empty()
        && target
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn group(targets: Vec<String>) -> Option<GroupSpec> {
    let total = targets.len();
    if total == 0 {
        return None;
    }
    Some(GroupSpec {
        id: "make".into(),
        title: if total > LIMIT {
            format!("Make ({LIMIT} of {total})")
        } else {
            "Make".into()
        },
        actions: targets
            .into_iter()
            .take(LIMIT)
            .map(|name| {
                let run_name = name.clone();
                ActionSpec::new(
                    format!("make.target.{name}"),
                    format!("make: {name}"),
                    format!("Run documented Make target `{name}`"),
                    format!("$ make {name}"),
                    &["make", "target", "task", "test", "build"],
                    Danger::Mutating,
                    move || Box::pin(run_argv("make".into(), vec![run_name.clone()])),
                )
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
.PHONY: build clean
build: src/main.c
clean:
	rm -rf out
%.o: %.c
$(GENERATED):
internal/path:
release test: build
VAR := value
"#;

    #[test]
    fn parses_only_conservative_documented_targets() {
        assert_eq!(
            parse_targets(FIXTURE),
            ["build", "clean", "release", "test"]
        );
    }

    #[test]
    fn ignores_recipes_comments_specials_patterns_and_variables() {
        let targets = parse_targets(FIXTURE);
        for rejected in [
            ".PHONY",
            "%.o",
            "$(GENERATED)",
            "internal/path",
            "rm",
            "VAR",
        ] {
            assert!(!targets.iter().any(|target| target == rejected));
        }
    }

    #[test]
    fn duplicate_declarations_keep_first_position() {
        assert_eq!(
            parse_targets("test:\nbuild:\ntest: build\n"),
            ["test", "build"]
        );
    }

    #[test]
    fn cap_is_enforced_and_visible() {
        let group = group((0..31).map(|index| format!("t{index}")).collect()).unwrap();
        assert_eq!(group.actions.len(), LIMIT);
        assert_eq!(group.title, "Make (30 of 31)");
    }
}
