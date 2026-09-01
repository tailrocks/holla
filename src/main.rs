pub mod cleanup;
mod commands;
mod config;
pub mod du;
mod find;
mod frecency;
pub mod insights;
mod model;
mod probe;
mod providers;
mod search;
mod tui;

use clap::{Parser, Subcommand};
use serde::Serialize;
use std::{path::PathBuf, process::ExitCode, time::Instant};

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("holla: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Parser)]
#[command(name = "holla", version = env!("HOLLA_VERSION"), about = "Adaptive dev environment CLI")]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Subcommand)]
enum CliCommand {
    /// Browse folders and preview files.
    Browse {
        /// Directory to browse. Defaults to the current directory.
        path: Option<PathBuf>,
    },
    /// List detected actions. JSON is a stable `{"v":1,"actions":[...]}` envelope.
    List {
        /// Emit the versioned machine-readable schema.
        #[arg(long)]
        json: bool,
    },
    /// Run one detected action without opening the launcher.
    Run {
        action_id: String,
        /// Confirm destructive, explicitly-confirmed, or untrusted project actions.
        #[arg(long)]
        yes: bool,
    },
    /// Report probes, registry timing, and configuration health.
    Doctor,
}

#[derive(Serialize)]
struct ListEnvelope {
    v: u8,
    actions: Vec<ListAction>,
}

#[derive(Serialize)]
struct ListAction {
    id: String,
    label: String,
    group: String,
    danger: &'static str,
}

async fn run(cli: Cli) -> anyhow::Result<u8> {
    let Some(command) = cli.command else {
        tui::menu::run().await?;
        return Ok(0);
    };
    let command = match command {
        CliCommand::Browse { path } => {
            let path = path.unwrap_or(std::env::current_dir()?);
            tui::browser::run_at(path).await?;
            return Ok(0);
        }
        command => command,
    };
    let started = Instant::now();
    let registry = providers::scan_all();
    let elapsed = started.elapsed();
    match command {
        CliCommand::Browse { .. } => unreachable!("browse handled before provider scan"),
        CliCommand::List { json } => {
            let actions = list_actions(&registry.groups);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&ListEnvelope { v: 1, actions })?
                );
            } else {
                for action in actions {
                    println!(
                        "{:<34} {:<13} {:<20} {}",
                        action.id, action.danger, action.group, action.label
                    );
                }
            }
            print_warnings(&registry.warnings);
            Ok(if registry.warnings.is_empty() { 0 } else { 4 })
        }
        CliCommand::Run { action_id, yes } => {
            if !registry.warnings.is_empty() {
                print_warnings(&registry.warnings);
                return Ok(4);
            }
            let action = registry
                .groups
                .iter()
                .flat_map(|group| &group.actions)
                .find(|action| action.id == action_id);
            let Some(action) = action else {
                eprintln!("holla: unknown action `{action_id}`");
                return Ok(2);
            };
            if (action.danger == model::Danger::Destructive
                || action.confirm
                || action.trust_required)
                && !yes
            {
                eprintln!("holla: action `{action_id}` requires --yes");
                return Ok(3);
            }
            tui::app::set_headless(true);
            tui::trust::set_assume_trust(yes);
            let result = (action.run)().await;
            tui::trust::set_assume_trust(false);
            tui::app::set_headless(false);
            if let Err(error) = result {
                eprintln!("holla: action `{action_id}` failed: {error:#}");
                return Ok(1);
            }
            Ok(0)
        }
        CliCommand::Doctor => {
            println!(
                "registry: {} groups in {:.2?}",
                registry.groups.len(),
                elapsed
            );
            println!(
                "actions: {}",
                registry
                    .groups
                    .iter()
                    .map(|group| group.actions.len())
                    .sum::<usize>()
            );
            for group in &registry.groups {
                println!(
                    "detected: {} ({} actions)",
                    group.title,
                    group.actions.len()
                );
            }
            let global = config::global_actions_path();
            println!(
                "global config: {}",
                global
                    .as_deref()
                    .map_or_else(|| "unavailable".into(), describe_config)
            );
            println!(
                "project config: {}",
                describe_config(std::path::Path::new(".holla.toml"))
            );
            if registry.warnings.is_empty() {
                println!("config: ok");
                Ok(0)
            } else {
                print_warnings(&registry.warnings);
                Ok(4)
            }
        }
    }
}

fn list_actions(groups: &[model::GroupSpec]) -> Vec<ListAction> {
    groups
        .iter()
        .flat_map(|group| {
            group.actions.iter().map(|action| ListAction {
                id: action.id.clone(),
                label: action.label.clone(),
                group: group.title.clone(),
                danger: match action.danger {
                    model::Danger::Safe => "safe",
                    model::Danger::Mutating => "mutating",
                    model::Danger::Destructive => "destructive",
                },
            })
        })
        .collect()
}

fn print_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("holla: config warning: {warning}");
    }
}

fn describe_config(path: &std::path::Path) -> String {
    format!(
        "{} ({})",
        path.display(),
        if path.is_file() {
            "loaded"
        } else {
            "not found"
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browse_accepts_omitted_path() {
        let cli = Cli::try_parse_from(["holla", "browse"]).expect("browse should parse");

        assert!(matches!(
            cli.command,
            Some(CliCommand::Browse { path: None })
        ));
    }

    #[test]
    fn browse_accepts_explicit_path() {
        let cli = Cli::try_parse_from(["holla", "browse", "/tmp"])
            .expect("browse with a path should parse");

        assert!(matches!(
            cli.command,
            Some(CliCommand::Browse { path: Some(path) })
                if path.as_path() == std::path::Path::new("/tmp")
        ));
    }
}
