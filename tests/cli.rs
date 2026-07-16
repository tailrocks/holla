use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn command(directory: &TempDir) -> Command {
    let config = directory.path().join("config");
    let cache = directory.path().join("cache");
    let mut command = cargo_bin_cmd!("holla");
    command
        .current_dir(directory.path())
        .env("HOME", directory.path())
        .env("XDG_CONFIG_HOME", &config)
        .env("XDG_CACHE_HOME", &cache);
    command
}

fn write_global(directory: &TempDir, contents: &str) {
    let root = directory.path().join("config/holla");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("actions.toml"), contents).unwrap();
}

#[test]
fn list_json_has_versioned_schema() {
    let directory = TempDir::new().unwrap();
    let output = command(&directory)
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["v"], 1);
    assert!(value["actions"].is_array());
    let first = &value["actions"][0];
    for key in ["id", "label", "group", "danger"] {
        assert!(first.get(key).is_some(), "missing {key}");
    }
}

#[test]
fn unknown_action_exits_two() {
    let directory = TempDir::new().unwrap();
    command(&directory)
        .args(["run", "missing.action"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown action"));
}

#[test]
fn destructive_action_without_yes_exits_three() {
    let directory = TempDir::new().unwrap();
    write_global(
        &directory,
        "[[action]]\nid='test.destroy'\nlabel='Destroy'\ncommand=['true']\ndanger='destructive'\n",
    );
    command(&directory)
        .args(["run", "test.destroy"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("requires --yes"));
}

#[test]
fn user_action_streams_plain_output() {
    let directory = TempDir::new().unwrap();
    write_global(
        &directory,
        "[[action]]\nid='test.echo'\nlabel='Echo'\ncommand=['/bin/echo','hello-headless']\ndanger='safe'\n",
    );
    command(&directory)
        .args(["run", "test.echo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello-headless"));
}

#[test]
fn doctor_reports_registry_and_config() {
    let directory = TempDir::new().unwrap();
    command(&directory)
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("registry:"))
        .stdout(predicate::str::contains("config: ok"));
}

#[test]
fn project_trust_persists_and_file_change_reprompts() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join(".holla.toml");
    let action = "[[action]]\nid='project.echo'\nlabel='Project echo'\ncommand=['/bin/echo','trusted']\ndanger='safe'\n";
    fs::write(&path, action).unwrap();
    command(&directory)
        .args(["run", "project.echo"])
        .assert()
        .code(3);
    command(&directory)
        .args(["run", "project.echo", "--yes"])
        .assert()
        .success();
    command(&directory)
        .args(["run", "project.echo"])
        .assert()
        .success();
    fs::write(&path, format!("{action}# changed\n")).unwrap();
    command(&directory)
        .args(["run", "project.echo"])
        .assert()
        .code(3);
}
