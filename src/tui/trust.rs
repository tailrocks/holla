use crossterm::event::{self, Event, KeyEventKind};
use ratatui::{
    backend::CrosstermBackend,
    text::{Line, Text},
};
use std::sync::atomic::{AtomicBool, Ordering};
use termrock::{
    interaction::Outcome,
    layout::centered_rect,
    style::{Role, Theme},
    widgets::{Action, Backdrop, ChoiceDialog, ChoiceDialogState, Dialog, PanelEmphasis},
};

static ASSUME_TRUST: AtomicBool = AtomicBool::new(false);

pub fn set_assume_trust(enabled: bool) {
    ASSUME_TRUST.store(enabled, Ordering::Release);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrustChoice {
    Cancel,
    Trust,
}

pub fn assumed() -> bool {
    ASSUME_TRUST.load(Ordering::Acquire)
}

pub fn confirm(argv: &[String]) -> anyhow::Result<bool> {
    let theme = Theme::tailrocks_phosphor();
    let actions = [
        Action {
            id: TrustChoice::Cancel,
            label: "Cancel",
            enabled: true,
            style: None,
        },
        Action {
            id: TrustChoice::Trust,
            label: "Trust and run",
            enabled: true,
            style: Some(theme.style(Role::Warning)),
        },
    ];
    let mut state = ChoiceDialogState::new(Some(TrustChoice::Cancel));
    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;
    let trusted = loop {
        terminal.draw(|frame| {
            frame.render_widget(Backdrop::default(), frame.area());
            let mut lines = vec![
                Line::styled(
                    "This project file has not been reviewed.",
                    theme.style(Role::Warning),
                ),
                Line::styled("Exact argv:", theme.style(Role::TextMuted)),
            ];
            lines.extend(argv.iter().enumerate().map(|(index, argument)| {
                Line::styled(
                    format!("argv[{index}] = {argument:?}"),
                    theme.style(Role::Text),
                )
            }));
            frame.render_stateful_widget(
                &ChoiceDialog::new(
                    Dialog::new("Trust project action", Text::from(lines), &theme)
                        .style(theme.style(Role::Text))
                        .emphasis(PanelEmphasis::Focused),
                    &actions,
                )
                .gap("  "),
                centered_rect(78, 16, frame.area()),
                &mut state,
            );
        })?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match state.handle_key(&actions, termrock::input::KeyEvent::from(key)) {
                Outcome::Activated(TrustChoice::Trust) => break true,
                Outcome::Activated(TrustChoice::Cancel) | Outcome::Cancelled => break false,
                _ => {}
            }
        }
    };
    drop(terminal);
    session.restore()?;
    Ok(trusted)
}
