mod commands;
mod probe;
mod tui;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let probe = probe::Probe::run();
    let menu = tui::menu::Menu::build(&probe);
    tui::menu::run(menu).await
}
