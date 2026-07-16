use std::{path::PathBuf, time::UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    providers::{Provider, run_argv},
};

const LIMIT: usize = 30;
const CACHE_VERSION: u8 = 1;
const CACHE_TTL_SECONDS: u64 = 300;

pub struct BrewServicesProvider;

impl Provider for BrewServicesProvider {
    fn id(&self) -> &'static str {
        "brew-services"
    }

    fn scan(&self) -> Option<GroupSpec> {
        which::which("brew").ok()?;
        if let Some(services) = load_cache() {
            return group(services);
        }
        let output = std::process::Command::new("brew")
            .args(["services", "list", "--json"])
            .output()
            .ok()?;
        output.status.success().then_some(())?;
        let services = parse_services(&output.stdout)?;
        save_cache(&services);
        group(services)
    }
}

#[derive(Deserialize, Serialize)]
struct Cache {
    version: u8,
    fetched_at: u64,
    services: Vec<String>,
}

fn cache_path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("holla/brew-services-v1.json"))
}

fn load_cache() -> Option<Vec<String>> {
    let contents = std::fs::read_to_string(cache_path()?).ok()?;
    parse_cache(&contents, unix_now())
}

fn parse_cache(contents: &str, now: u64) -> Option<Vec<String>> {
    let cache: Cache = serde_json::from_str(contents).ok()?;
    (cache.version == CACHE_VERSION && now.saturating_sub(cache.fetched_at) <= CACHE_TTL_SECONDS)
        .then_some(cache.services)
}

fn save_cache(services: &[String]) {
    let Some(path) = cache_path() else { return };
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let cache = Cache {
        version: CACHE_VERSION,
        fetched_at: unix_now(),
        services: services.to_vec(),
    };
    let Ok(contents) = serde_json::to_vec(&cache) else {
        return;
    };
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if std::fs::write(&temporary, contents).is_ok() {
        let _ = std::fs::rename(&temporary, path);
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parse_services(output: &[u8]) -> Option<Vec<String>> {
    let value: Value = serde_json::from_slice(output).ok()?;
    let entries = value
        .as_array()
        .or_else(|| value.get("services").and_then(Value::as_array))?;
    let mut names = entries
        .iter()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    Some(names)
}

fn group(services: Vec<String>) -> Option<GroupSpec> {
    let mut actions = Vec::new();
    for service in services {
        for verb in ["start", "stop", "restart"] {
            let run_service = service.clone();
            actions.push(ActionSpec::new(
                format!("brew.service.{service}.{verb}"),
                format!("brew: {verb} {service}"),
                format!("{verb} Homebrew service `{service}`"),
                format!("$ brew services {verb} {service}"),
                &["brew", "homebrew", "services", "start", "stop", "restart"],
                Danger::Mutating,
                move || {
                    Box::pin(run_argv(
                        "brew".into(),
                        vec!["services".into(), verb.into(), run_service.clone()],
                    ))
                },
            ));
        }
    }
    let total = actions.len();
    if total == 0 {
        return None;
    }
    actions.truncate(LIMIT);
    Some(GroupSpec {
        id: "brew-services".into(),
        title: if total > LIMIT {
            format!("Brew services ({LIMIT} of {total})")
        } else {
            "Brew services".into()
        },
        actions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_brew_service_array() {
        let names = parse_services(
            br#"[{"name":"postgresql@17","status":"started"},{"name":"redis","status":"none"}]"#,
        )
        .unwrap();
        assert_eq!(names, ["postgresql@17", "redis"]);
    }

    #[test]
    fn accepts_wrapped_schema_and_skips_bad_entries() {
        let names =
            parse_services(br#"{"services":[{"name":"redis"},{"name":4},{"status":"started"}]}"#)
                .unwrap();
        assert_eq!(names, ["redis"]);
    }

    #[test]
    fn malformed_or_empty_service_output_contributes_nothing() {
        assert!(parse_services(b"{").is_none());
        assert!(group(parse_services(b"[]").unwrap()).is_none());
    }

    #[test]
    fn actions_are_mutating_and_use_exact_argv_preview() {
        let group = group(vec!["redis".into()]).unwrap();
        assert_eq!(group.actions.len(), 3);
        assert!(
            group
                .actions
                .iter()
                .all(|action| action.danger == Danger::Mutating)
        );
        assert_eq!(group.actions[2].preview, "$ brew services restart redis");
    }

    #[test]
    fn action_cap_is_enforced_and_visible() {
        let group = group((0..11).map(|index| format!("service-{index}")).collect()).unwrap();
        assert_eq!(group.actions.len(), LIMIT);
        assert_eq!(group.title, "Brew services (30 of 33)");
    }

    #[test]
    fn fresh_cache_avoids_slow_brew_enumeration() {
        let contents = r#"{"version":1,"fetched_at":100,"services":["redis"]}"#;
        assert_eq!(parse_cache(contents, 400).unwrap(), ["redis"]);
    }

    #[test]
    fn stale_wrong_version_and_corrupt_caches_refresh() {
        assert!(parse_cache(r#"{"version":1,"fetched_at":100,"services":[]}"#, 401).is_none());
        assert!(parse_cache(r#"{"version":2,"fetched_at":100,"services":[]}"#, 100).is_none());
        assert!(parse_cache("{", 100).is_none());
    }
}
