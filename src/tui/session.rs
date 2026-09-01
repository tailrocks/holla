use crossterm::event::EventStream;
use ratatui::backend::CrosstermBackend;
use termrock::input::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Launcher,
    Browser,
}

impl Mode {
    pub(crate) const fn toggle(self) -> Self {
        match self {
            Self::Launcher => Self::Browser,
            Self::Browser => Self::Launcher,
        }
    }
}

pub(crate) fn is_mode_toggle(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('o') && key.modifiers == KeyModifiers::CONTROL
}

pub async fn run() -> anyhow::Result<()> {
    let theme = termrock::style::DesignSystem::phosphor();
    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;
    let mut events = EventStream::new();
    let result = crate::tui::menu::run_with_session(&mut terminal, &mut events, &theme).await;

    drop(terminal);
    let restore_result = session.restore();
    let exit = result?;
    restore_result?;
    crate::tui::menu::execute(exit).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl_o() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_o_is_the_bidirectional_mode_toggle() {
        assert!(is_mode_toggle(ctrl_o()));
        assert_eq!(Mode::Launcher.toggle(), Mode::Browser);
        assert_eq!(Mode::Browser.toggle(), Mode::Launcher);
    }

    #[test]
    fn other_keys_do_not_toggle_mode() {
        assert!(!is_mode_toggle(KeyEvent::new(
            KeyCode::Char('o'),
            KeyModifiers::NONE,
        )));
        assert!(!is_mode_toggle(KeyEvent::new(
            KeyCode::Char('p'),
            KeyModifiers::CONTROL,
        )));
    }

    #[test]
    fn toggling_does_not_replace_view_state() {
        let mut mode = Mode::Launcher;
        let launcher_search = String::from("cargo");
        let browser_path = String::from("/tmp/project");

        mode = mode.toggle();
        mode = mode.toggle();

        assert_eq!(mode, Mode::Launcher);
        assert_eq!(launcher_search, "cargo");
        assert_eq!(browser_path, "/tmp/project");
    }
}
