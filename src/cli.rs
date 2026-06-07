use clap::Command;
use crate::probe::Probe;
use crate::commands;

pub fn build(probe: &Probe) -> Command {
    let mut cmd = Command::new("holla")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Adaptive dev environment CLI")
        .subcommand_required(false)
        .arg_required_else_help(true);

    if probe.git {
        cmd = cmd.subcommand(commands::git::command());
    }
    if probe.docker {
        cmd = cmd.subcommand(commands::docker::command());
    }
    if probe.gradle {
        cmd = cmd.subcommand(commands::gradle::command());
    }
    // idea cleanup available regardless (just uses find/rm)
    cmd = cmd.subcommand(commands::idea::command());

    // upgrade available if any upgradable tool exists
    if probe.brew || probe.mise || probe.amp {
        cmd = cmd.subcommand(commands::upgrade::command());
    }

    cmd
}
