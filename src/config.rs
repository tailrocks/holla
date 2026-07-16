use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConfigDanger {
    Safe,
    Mutating,
    Destructive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigOrigin {
    Global,
    Project,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigAction {
    pub id: String,
    pub label: String,
    pub description: String,
    pub command: Vec<String>,
    pub danger: ConfigDanger,
    pub keywords: Vec<String>,
    pub group: String,
    pub confirm: bool,
    pub origin: ConfigOrigin,
    pub working_directory: PathBuf,
    pub project_hash: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub path: PathBuf,
    pub entry: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.entry {
            Some(entry) => write!(
                formatter,
                "{} action[{}]: {}",
                self.path.display(),
                entry,
                self.message
            ),
            None => write!(formatter, "{}: {}", self.path.display(), self.message),
        }
    }
}

#[derive(Debug, Default)]
pub struct ConfigReport {
    pub actions: Vec<ConfigAction>,
    pub errors: Vec<ConfigError>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawAction {
    id: String,
    label: String,
    #[serde(default)]
    description: String,
    command: Vec<String>,
    danger: ConfigDanger,
    #[serde(default)]
    keywords: Vec<String>,
    group: Option<String>,
    #[serde(default)]
    confirm: bool,
}

pub fn parse_actions(
    path: &Path,
    contents: &str,
    origin: ConfigOrigin,
    builtins: &HashSet<String>,
) -> ConfigReport {
    let mut report = ConfigReport::default();
    let value = match toml::from_str::<toml::Value>(contents) {
        Ok(value) => value,
        Err(error) => {
            report.errors.push(ConfigError {
                path: path.to_path_buf(),
                entry: None,
                message: error.to_string(),
            });
            return report;
        }
    };
    let Some(entries) = value.get("action").and_then(toml::Value::as_array) else {
        if value.get("action").is_some() {
            report.errors.push(ConfigError {
                path: path.to_path_buf(),
                entry: None,
                message: "`action` must be an array of tables".into(),
            });
        }
        return report;
    };
    let project_hash = (origin == ConfigOrigin::Project).then(|| sha256(contents.as_bytes()));
    let working_directory = match origin {
        ConfigOrigin::Global => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        ConfigOrigin::Project => path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf(),
    };
    let mut seen = builtins.clone();
    for (entry, value) in entries.iter().cloned().enumerate() {
        let raw = match value.try_into::<RawAction>() {
            Ok(raw) => raw,
            Err(error) => {
                report
                    .errors
                    .push(entry_error(path, entry, error.to_string()));
                continue;
            }
        };
        if !valid_id(&raw.id) {
            report.errors.push(entry_error(
                path,
                entry,
                "id must contain only lowercase ASCII letters, digits, `.`, `_`, or `-`",
            ));
            continue;
        }
        if !seen.insert(raw.id.clone()) {
            report.errors.push(entry_error(
                path,
                entry,
                format!("action id `{}` collides with an existing action", raw.id),
            ));
            continue;
        }
        if raw.label.trim().is_empty() {
            report
                .errors
                .push(entry_error(path, entry, "label must not be empty"));
            continue;
        }
        if raw.command.is_empty() || raw.command[0].trim().is_empty() {
            report.errors.push(entry_error(
                path,
                entry,
                "command must contain a non-empty program followed by argv entries",
            ));
            continue;
        }
        let group = raw.group.unwrap_or_else(|| match origin {
            ConfigOrigin::Global => "Custom".into(),
            ConfigOrigin::Project => "Current folder".into(),
        });
        if group.trim().is_empty() {
            report
                .errors
                .push(entry_error(path, entry, "group must not be empty"));
            continue;
        }
        report.actions.push(ConfigAction {
            id: raw.id,
            label: raw.label,
            description: raw.description,
            command: raw.command,
            danger: raw.danger,
            keywords: raw.keywords,
            group,
            confirm: raw.confirm,
            origin,
            working_directory: working_directory.clone(),
            project_hash: project_hash.clone(),
        });
    }
    report
}

pub fn load_actions(path: &Path, origin: ConfigOrigin, builtins: &HashSet<String>) -> ConfigReport {
    match std::fs::read_to_string(path) {
        Ok(contents) => parse_actions(path, &contents, origin, builtins),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ConfigReport::default(),
        Err(error) => ConfigReport {
            errors: vec![ConfigError {
                path: path.to_path_buf(),
                entry: None,
                message: error.to_string(),
            }],
            ..ConfigReport::default()
        },
    }
}

#[must_use]
pub fn global_actions_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .map(|root| root.join("holla/actions.toml"))
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct TrustFile {
    v: u32,
    hashes: Vec<String>,
}

#[derive(Debug, Default)]
pub struct TrustStore {
    hashes: HashSet<String>,
}

impl TrustStore {
    pub fn load(path: &Path) -> Self {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(file) = serde_json::from_str::<TrustFile>(&contents) else {
            return Self::default();
        };
        if file.v != 1 {
            return Self::default();
        }
        Self {
            hashes: file.hashes.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn contains(&self, hash: &str) -> bool {
        self.hashes.contains(hash)
    }

    pub fn accept(&mut self, hash: String) {
        self.hashes.insert(hash);
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let mut hashes = self.hashes.iter().cloned().collect::<Vec<_>>();
        hashes.sort();
        let contents = serde_json::to_vec_pretty(&TrustFile { v: 1, hashes })?;
        let parent = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&temporary, contents)?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }
}

#[must_use]
pub fn trust_store_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|root| root.join("holla/trusted.json"))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn entry_error(path: &Path, entry: usize, message: impl Into<String>) -> ConfigError {
    ConfigError {
        path: path.to_path_buf(),
        entry: Some(entry),
        message: message.into(),
    }
}

fn sha256(contents: &[u8]) -> String {
    Sha256::digest(contents)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const VALID: &str = r#"
[[action]]
id = "team.deploy-staging"
label = "deploy: staging"
description = "Deploy current branch"
command = ["./scripts/deploy.sh", "staging"]
danger = "mutating"
keywords = ["deploy", "staging"]
confirm = true
"#;

    fn parse(contents: &str) -> ConfigReport {
        parse_actions(
            Path::new("/project/.holla.toml"),
            contents,
            ConfigOrigin::Project,
            &HashSet::new(),
        )
    }

    #[test]
    fn valid_entry_round_trips_through_toml() {
        let report = parse(VALID);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let action = &report.actions[0];
        assert_eq!(action.id, "team.deploy-staging");
        assert_eq!(action.command, ["./scripts/deploy.sh", "staging"]);
        assert_eq!(action.danger, ConfigDanger::Mutating);
        assert!(action.confirm);
        assert_eq!(action.group, "Current folder");
    }

    #[test]
    fn missing_danger_is_an_entry_error() {
        let report = parse("[[action]]\nid='x'\nlabel='X'\ncommand=['true']\n");
        assert!(report.actions.is_empty());
        assert_eq!(report.errors[0].entry, Some(0));
        assert!(report.errors[0].message.contains("danger"));
    }

    #[test]
    fn shell_string_command_is_rejected() {
        let report =
            parse("[[action]]\nid='x'\nlabel='X'\ncommand='rm -rf /'\ndanger='destructive'\n");
        assert!(report.actions.is_empty());
        assert_eq!(report.errors[0].entry, Some(0));
        assert!(report.errors[0].message.contains("sequence"));
    }

    #[test]
    fn bad_id_charset_is_rejected() {
        let report = parse("[[action]]\nid='Bad ID'\nlabel='X'\ncommand=['true']\ndanger='safe'\n");
        assert!(report.actions.is_empty());
        assert!(report.errors[0].message.contains("lowercase ASCII"));
    }

    #[test]
    fn builtin_collision_is_rejected() {
        let builtins = HashSet::from(["git.pull".to_owned()]);
        let report = parse_actions(
            Path::new("actions.toml"),
            "[[action]]\nid='git.pull'\nlabel='X'\ncommand=['true']\ndanger='safe'\n",
            ConfigOrigin::Global,
            &builtins,
        );
        assert!(report.actions.is_empty());
        assert!(report.errors[0].message.contains("collides"));
    }

    #[test]
    fn duplicate_user_id_is_rejected_at_its_index() {
        let report = parse(
            "[[action]]\nid='x'\nlabel='X'\ncommand=['true']\ndanger='safe'\n\
             [[action]]\nid='x'\nlabel='Again'\ncommand=['true']\ndanger='safe'\n",
        );
        assert_eq!(report.actions.len(), 1);
        assert_eq!(report.errors[0].entry, Some(1));
    }

    #[test]
    fn malformed_entry_does_not_drop_valid_sibling() {
        let report = parse(
            "[[action]]\nid='bad'\nlabel='Bad'\ncommand='string'\ndanger='safe'\n\
             [[action]]\nid='good'\nlabel='Good'\ncommand=['true']\ndanger='safe'\n",
        );
        assert_eq!(report.actions.len(), 1);
        assert_eq!(report.actions[0].id, "good");
        assert_eq!(report.errors[0].entry, Some(0));
    }

    #[test]
    fn empty_command_is_rejected() {
        let report = parse("[[action]]\nid='x'\nlabel='X'\ncommand=[]\ndanger='safe'\n");
        assert!(report.actions.is_empty());
        assert!(report.errors[0].message.contains("non-empty program"));
    }

    #[test]
    fn project_hash_changes_when_file_changes() {
        let first = parse(VALID).actions[0].project_hash.clone();
        let second = parse(&format!("{VALID}\n# changed")).actions[0]
            .project_hash
            .clone();
        assert_ne!(first, second);
    }

    #[test]
    fn missing_file_is_zero_config_success() {
        let directory = tempdir().unwrap();
        let report = load_actions(
            &directory.path().join("missing.toml"),
            ConfigOrigin::Global,
            &HashSet::new(),
        );
        assert!(report.actions.is_empty());
        assert!(report.errors.is_empty());
    }

    #[test]
    fn syntax_error_reports_file_without_entry() {
        let report = parse("[[action]");
        assert_eq!(report.errors[0].path, Path::new("/project/.holla.toml"));
        assert_eq!(report.errors[0].entry, None);
    }

    #[test]
    fn unknown_hash_is_untrusted() {
        assert!(!TrustStore::default().contains("unknown"));
    }

    #[test]
    fn accepted_hash_persists() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("trusted.json");
        let mut store = TrustStore::default();
        store.accept("abc".into());
        store.save(&path).unwrap();
        assert!(TrustStore::load(&path).contains("abc"));
    }

    #[test]
    fn changed_hash_requires_fresh_trust() {
        let mut store = TrustStore::default();
        store.accept("before".into());
        assert!(store.contains("before"));
        assert!(!store.contains("after"));
    }

    #[test]
    fn corrupt_or_unknown_trust_store_is_empty() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("trusted.json");
        std::fs::write(&path, "not-json").unwrap();
        assert!(!TrustStore::load(&path).contains("abc"));
        std::fs::write(&path, r#"{"v":2,"hashes":["abc"]}"#).unwrap();
        assert!(!TrustStore::load(&path).contains("abc"));
    }
}
