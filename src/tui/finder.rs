use anyhow::{Context, ensure};
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    text::{Line, Span, Text},
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use termrock::{
    input::KeyCode,
    interaction::Outcome,
    keymap::{KeyBinding, KeyChord, Keymap, Visibility},
    layout::centered_rect,
    osc::{ClipboardSelection, ClipboardWrite, encode_clipboard},
    style::{Density, DesignTokens, Role, Theme},
    widgets::{
        Action as DialogAction, Backdrop, ChoiceDialog, ChoiceDialogState, Dialog, List, ListRow,
        ListState, Panel, PanelEmphasis, RowRole, TextInput, TextInputOutcome, TextInputState,
        Validation, render_hint_bar,
    },
};
use tokio::process::Command;
use unicode_segmentation::UnicodeSegmentation;

use crate::{find::FileHit, find::FileIndex, tui::analyzer};

const RESULT_LIMIT: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FinderAction {
    Reveal,
    Open,
    Copy,
    Analyze,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingAction {
    path: PathBuf,
    state: ChoiceDialogState<FinderAction>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FinderKey {
    Navigate,
    Select,
    Quit,
}

static FINDER_BINDINGS: &[KeyBinding<FinderKey>] = &[
    KeyBinding::borrowed(
        &[KeyChord::plain(KeyCode::Up), KeyChord::plain(KeyCode::Down)],
        FinderKey::Navigate,
        Some("navigate"),
        Visibility::Shown,
        Some("↑↓"),
    ),
    KeyBinding::borrowed(
        &[KeyChord::plain(KeyCode::Enter)],
        FinderKey::Select,
        Some("actions"),
        Visibility::Shown,
        Some("⏎"),
    ),
    KeyBinding::borrowed(
        &[KeyChord::plain(KeyCode::Esc)],
        FinderKey::Quit,
        Some("clear/quit"),
        Visibility::Shown,
        Some("esc"),
    ),
];
static FINDER_KEYMAP: Keymap<FinderKey> = Keymap::from_static(FINDER_BINDINGS);

pub async fn run() -> anyhow::Result<()> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    let index = FileIndex::build(vec![home.clone()]);
    let theme = Theme::tailrocks_phosphor();
    let tokens = DesignTokens::new(theme.clone(), Density::default());
    let mut input = TextInputState::new("").with_allow_empty(true);
    let mut list_state = ListState::<PathBuf>::new(None);
    let mut pending: Option<PendingAction> = None;
    let actions = action_choices();
    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;

    let selected = loop {
        let hits = index.query(input.value(), RESULT_LIMIT);
        let rows = result_rows(&hits, &theme);
        if !rows
            .iter()
            .any(|row| list_state.selected() == Some(&row.id))
        {
            list_state.select(rows.first().map(|row| row.id.clone()));
        }
        terminal.draw(|frame| {
            let [search_area, list_area, status_area, footer_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(frame.area());
            frame.render_stateful_widget(
                &TextInput::new("Find", &theme)
                    .placeholder("Type a file or folder name…")
                    .validation(Validation::Valid),
                search_area,
                &mut input,
            );
            let panel = Panel::new(&tokens)
                .title(" Files and folders ")
                .emphasis(PanelEmphasis::Focused);
            let inner = panel.inner(list_area);
            frame.render_widget(&panel, list_area);
            frame.render_stateful_widget(&List::new(&rows, &tokens), inner, &mut list_state);
            let status = if index.is_complete() {
                format!(
                    "{} indexed · {} results · {}",
                    index.indexed_count(),
                    hits.len(),
                    home.display()
                )
            } else {
                format!(
                    "indexing… {} files · {} results · {}",
                    index.indexed_count(),
                    hits.len(),
                    home.display()
                )
            };
            frame.render_widget(
                ratatui::widgets::Paragraph::new(Line::styled(
                    status,
                    theme.style(Role::TextMuted),
                )),
                status_area,
            );
            render_hint_bar(frame, footer_area, &FINDER_KEYMAP.hint_spans(), &theme);

            if let Some(pending) = pending.as_mut() {
                frame.render_widget(Backdrop::default(), frame.area());
                let body = Text::from(vec![
                    Line::styled(pending.path.display().to_string(), theme.style(Role::Text)),
                    Line::raw(""),
                    Line::styled("Choose what holla should do.", theme.style(Role::TextMuted)),
                ]);
                frame.render_stateful_widget(
                    &ChoiceDialog::new(
                        Dialog::new("File action", body, &theme)
                            .style(theme.style(Role::Text))
                            .emphasis(PanelEmphasis::Focused),
                        &actions,
                    )
                    .gap("  "),
                    centered_rect(78, 10, frame.area()),
                    &mut pending.state,
                );
            }
        })?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let key = termrock::input::KeyEvent::from(key);
            if let Some(dialog) = pending.as_mut() {
                match dialog.state.handle_key(&actions, key) {
                    Outcome::Activated(FinderAction::Cancel) | Outcome::Cancelled => pending = None,
                    Outcome::Activated(action) => break Some((dialog.path.clone(), action)),
                    Outcome::Ignored | Outcome::Changed => {}
                    _ => {}
                }
                continue;
            }
            if key.code == KeyCode::Esc {
                if input.value().is_empty() {
                    break None;
                }
                input = TextInputState::new("").with_allow_empty(true);
                list_state.select(None);
                continue;
            }
            if matches!(
                key.code,
                KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
            ) {
                if input.handle_key(key) == TextInputOutcome::Changed {
                    list_state.select(None);
                }
                continue;
            }
            match list_state.handle_key(&rows, key) {
                Outcome::Activated(path) => {
                    pending = Some(PendingAction {
                        path,
                        state: ChoiceDialogState::new(Some(FinderAction::Open)),
                    });
                }
                Outcome::Cancelled => break None,
                Outcome::Ignored | Outcome::Changed => {}
                _ => {}
            }
        }
    };

    drop(terminal);
    session.restore()?;
    drop(index);
    if let Some((path, action)) = selected {
        execute(path, action).await?;
    }
    Ok(())
}

fn action_choices() -> [DialogAction<'static, FinderAction>; 5] {
    [
        DialogAction {
            id: FinderAction::Reveal,
            label: "Reveal",
            enabled: true,
            style: None,
        },
        DialogAction {
            id: FinderAction::Open,
            label: "Open",
            enabled: true,
            style: None,
        },
        DialogAction {
            id: FinderAction::Copy,
            label: "Copy path",
            enabled: true,
            style: None,
        },
        DialogAction {
            id: FinderAction::Analyze,
            label: "Analyze size",
            enabled: true,
            style: None,
        },
        DialogAction {
            id: FinderAction::Cancel,
            label: "Cancel",
            enabled: true,
            style: None,
        },
    ]
}

async fn execute(path: PathBuf, action: FinderAction) -> anyhow::Result<()> {
    match action {
        FinderAction::Reveal => run_open(&path, true).await,
        FinderAction::Open => run_open(&path, false).await,
        FinderAction::Copy => copy_path(&path),
        FinderAction::Analyze => {
            let root = if path.is_dir() {
                path
            } else {
                path.parent().unwrap_or(Path::new(".")).to_path_buf()
            };
            analyzer::run(root).await
        }
        FinderAction::Cancel => Ok(()),
    }
}

async fn run_open(path: &Path, reveal: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "macos")]
    if reveal {
        command.arg("-R");
    }
    #[cfg(target_os = "macos")]
    let (program, status) = ("open", command.arg(path).status().await?);

    #[cfg(target_os = "linux")]
    let (program, status) = (
        "xdg-open",
        Command::new("xdg-open")
            .arg(open_target(path, reveal))
            .status()
            .await?,
    );

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    anyhow::bail!("opening files is unsupported on this platform");

    ensure!(status.success(), "{program} exited with {status}");
    Ok(())
}

#[cfg(any(test, target_os = "linux"))]
fn open_target(path: &Path, reveal: bool) -> &Path {
    if reveal {
        path.parent().unwrap_or(path)
    } else {
        path
    }
}

fn copy_path(path: &Path) -> anyhow::Result<()> {
    let text = path.to_string_lossy();
    let encoded = encode_clipboard(ClipboardWrite {
        selection: ClipboardSelection::Clipboard,
        text: &text,
    });
    ensure!(!encoded.is_empty(), "path exceeds OSC 52 clipboard limit");
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&encoded)?;
    stdout.flush()?;
    Ok(())
}

fn result_rows(hits: &[FileHit], theme: &Theme) -> Vec<ListRow<'static, PathBuf>> {
    hits.iter()
        .map(|hit| {
            let relative = hit.relative_path.trim_end_matches('/');
            let filename = hit
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(relative);
            let filename_start = relative.rfind(filename).unwrap_or(0);
            let parent = relative[..filename_start].trim_end_matches('/');
            let mut label = highlighted_bytes(
                filename,
                filename_start,
                &hit.match_byte_offsets,
                Role::Text,
                theme,
            );
            if !parent.is_empty() {
                label
                    .spans
                    .push(Span::styled("  ", theme.style(Role::TextMuted)));
                label.spans.extend(
                    highlighted_bytes(parent, 0, &hit.match_byte_offsets, Role::TextMuted, theme)
                        .spans,
                );
            }
            ListRow {
                id: hit.path.clone(),
                label,
                leading: None,
                secondary: None,
                badge: None,
                shortcut: None,
                trailing: None,
                role: RowRole::Item,
                enabled: true,
                loading: false,
            }
        })
        .collect()
}

fn highlighted_bytes(
    value: &str,
    base: usize,
    ranges: &[(u32, u32)],
    base_role: Role,
    theme: &Theme,
) -> Line<'static> {
    let spans = value
        .grapheme_indices(true)
        .map(|(start, grapheme)| {
            let absolute_start = base + start;
            let absolute_end = absolute_start + grapheme.len();
            let matched = ranges.iter().any(|&(range_start, range_end)| {
                usize::try_from(range_start).is_ok_and(|range_start| {
                    usize::try_from(range_end).is_ok_and(|range_end| {
                        absolute_start < range_end && range_start < absolute_end
                    })
                })
            });
            Span::styled(
                grapheme.to_owned(),
                theme.style(if matched { Role::Accent } else { base_role }),
            )
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_byte_ranges_highlight_whole_graphemes() {
        let theme = Theme::default();
        let line = highlighted_bytes("résumé", 0, &[(1, 3)], Role::Text, &theme);
        assert_eq!(line.spans[1].content, "é");
        assert_eq!(line.spans[1].style, theme.style(Role::Accent));
        assert_eq!(line.spans[0].style, theme.style(Role::Text));
    }

    #[test]
    fn result_row_projects_filename_and_parent_offsets() {
        let theme = Theme::default();
        let hit = FileHit {
            path: PathBuf::from("/tmp/projects/résumé.txt"),
            relative_path: "projects/résumé.txt".into(),
            score: 10,
            match_byte_offsets: vec![(0, 1), (9, 10)],
        };
        let rows = result_rows(&[hit], &theme);
        assert_eq!(rows[0].label.spans[0].content, "r");
        assert_eq!(rows[0].label.spans[0].style, theme.style(Role::Accent));
        let parent_start = rows[0]
            .label
            .spans
            .iter()
            .position(|span| span.content == "p")
            .unwrap();
        assert_eq!(
            rows[0].label.spans[parent_start].style,
            theme.style(Role::Accent)
        );
        assert_eq!(
            rows[0].label.spans[parent_start + 1].style,
            theme.style(Role::TextMuted)
        );
    }

    #[test]
    fn reveal_fallback_targets_parent_directory() {
        let path = Path::new("/tmp/project/file.rs");
        assert_eq!(open_target(path, true), Path::new("/tmp/project"));
        assert_eq!(open_target(path, false), path);
    }
}
