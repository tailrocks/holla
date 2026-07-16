mod commands;
mod model;
mod probe;
mod providers;
mod search;
mod tui;

use clap::Command;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Command::new("holla")
        .version(env!("HOLLA_VERSION"))
        .about("Adaptive dev environment CLI")
        .get_matches();

    tui::menu::run().await
}
