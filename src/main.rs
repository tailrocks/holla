mod commands;
mod probe;
mod tui;

use clap::Command;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Command::new("holla")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Adaptive dev environment CLI")
        .get_matches();

    let probe = probe::Probe::run();
    let menu = tui::menu::Menu::build(&probe);
    tui::menu::run(menu).await
}
