use crate::{
    cleanup::{DeleteMode, DeletePlan, DeleteReport, execute},
    du::{NodeId, NodeState, ScanEvent, ScanHandle, ScanOptions, ScanTree, SortKey, scan},
};
use crossterm::event::{self, Event, KeyEventKind};
use humansize::{DECIMAL, format_size};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    text::{Line, Text},
    widgets::{Paragraph, Wrap},
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, mpsc},
    time::Duration,
};
use termrock::{
    input::KeyCode,
    interaction::{ModalStack, Outcome},
    layout::centered_rect,
    style::{Role, Theme},
    widgets::{
        Action as DialogAction, Backdrop, ChoiceDialog, ChoiceDialogState, DetailCapability,
        DetailRow, DetailTableState, Dialog, Hint, HintBar, MessageDialog, Panel, PanelEmphasis,
        Progress, ProgressKind, StatusBar, StatusBarState, StatusSlot, TextInput, TextInputOutcome,
        TextInputState, Tree, TreeNode, TreeNodeStatus, TreeOutcome, TreeState, Validation,
    },
};

pub const FOLD_SET: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "build",
    "dist",
    ".venv",
    "venv",
    "__pycache__",
    "DerivedData",
    "Pods",
    ".gradle",
    ".next",
    ".turbo",
    ".cache",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderSlot {
    Root,
    Scan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteChoice {
    Cancel,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitChoice {
    Stay,
    Leave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportRow {
    Removed,
    Failed,
    Skipped,
    Freed,
    Log,
}

struct ConfirmItem {
    path: PathBuf,
    size: u64,
}

enum AnalyzerModal {
    Confirm {
        items: Vec<ConfirmItem>,
        total: u64,
        mode: DeleteMode,
        state: ChoiceDialogState<DeleteChoice>,
    },
    Deleting {
        report: mpsc::Receiver<DeleteReport>,
        mode: DeleteMode,
    },
    QuitRunning {
        state: ChoiceDialogState<QuitChoice>,
    },
    Report {
        report: DeleteReport,
        mode: DeleteMode,
        state: DetailTableState<ReportRow>,
    },
}

enum ModalEffect {
    None,
    Close,
    OpenQuit,
    Start(DeletePlan),
    ExitAfterDelete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmTransition {
    None,
    Cancel,
    Delete(DeleteMode),
}

fn handle_confirm_input(
    mode: &mut DeleteMode,
    state: &mut ChoiceDialogState<DeleteChoice>,
    key: termrock::input::KeyEvent,
) -> ConfirmTransition {
    if key.code == KeyCode::Char('p') {
        *mode = match mode {
            DeleteMode::Trash => DeleteMode::Permanent,
            DeleteMode::Permanent => DeleteMode::Trash,
        };
        return ConfirmTransition::None;
    }
    let actions = [
        DialogAction {
            id: DeleteChoice::Cancel,
            label: "Cancel",
            enabled: true,
            style: None,
        },
        DialogAction {
            id: DeleteChoice::Delete,
            label: "Delete",
            enabled: true,
            style: None,
        },
    ];
    match state.handle_key(&actions, key) {
        Outcome::Activated(DeleteChoice::Cancel) | Outcome::Cancelled => ConfirmTransition::Cancel,
        Outcome::Activated(DeleteChoice::Delete) => ConfirmTransition::Delete(*mode),
        Outcome::Ignored | Outcome::Changed => ConfirmTransition::None,
    }
}

fn requests_quit_while_deleting(key: termrock::input::KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
}

fn new_delete_confirmation(items: Vec<ConfirmItem>, total: u64) -> AnalyzerModal {
    AnalyzerModal::Confirm {
        items,
        total,
        mode: DeleteMode::Trash,
        state: ChoiceDialogState::new(Some(DeleteChoice::Cancel)),
    }
}

const ANALYZER_HINTS: &[Hint<'static>] = &[
    Hint {
        chord: "↑↓←→",
        label: "navigate",
        priority: 5,
        visible: true,
    },
    Hint {
        chord: "space",
        label: "select",
        priority: 5,
        visible: true,
    },
    Hint {
        chord: "f",
        label: "fold",
        priority: 4,
        visible: true,
    },
    Hint {
        chord: "s",
        label: "size",
        priority: 3,
        visible: true,
    },
    Hint {
        chord: "r",
        label: "rescan",
        priority: 2,
        visible: true,
    },
    Hint {
        chord: "d",
        label: "delete",
        priority: 5,
        visible: true,
    },
    Hint {
        chord: "q",
        label: "back",
        priority: 5,
        visible: true,
    },
];

pub async fn run(root: PathBuf) -> anyhow::Result<()> {
    let theme = Theme::tailrocks_phosphor();
    let mut handle = scan(ScanOptions::new(&root));
    let mut expanded = HashSet::from([NodeId(0)]);
    let mut folding = true;
    let mut sort = SortKey::OnDisk;
    let mut tree_state = TreeState::new(Some(NodeId(0)));
    tree_state.enable_multi_select();
    let mut status_state = StatusBarState::default();
    let mut scanning = true;
    let mut bytes_seen = 0_u64;
    let mut inaccessible = 0_u64;
    let mut tick = 0_u64;
    let mut modals = ModalStack::new();
    let mut exit_after_delete = false;
    let root_label = root.display().to_string();

    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;

    'screen: loop {
        let completed_report = match modals.current_mut() {
            Some(AnalyzerModal::Deleting { report, mode }) => match report.try_recv() {
                Ok(report) => Some((report, *mode)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    let mut report = DeleteReport::default();
                    report
                        .failed
                        .push((root.clone(), "cleanup worker stopped".into()));
                    Some((report, *mode))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            },
            _ => None,
        };
        if let Some((report, mode)) = completed_report {
            if exit_after_delete {
                break 'screen;
            }
            handle.cancel.store(true, Ordering::Release);
            handle = scan(ScanOptions::new(&root));
            expanded = HashSet::from([NodeId(0)]);
            tree_state = TreeState::new(Some(NodeId(0)));
            tree_state.enable_multi_select();
            scanning = true;
            bytes_seen = 0;
            inaccessible = 0;
            modals.open(AnalyzerModal::Report {
                report,
                mode,
                state: DetailTableState::default(),
            });
        }
        drain_scan_events(&handle, &mut scanning, &mut bytes_seen, &mut inaccessible);
        let (rows, root_bytes, selected_count, selected_bytes) = {
            let tree = handle.tree.read().expect("disk scan tree lock");
            let checked = tree_state
                .selection()
                .map(|selection| selection.checked())
                .unwrap_or_default();
            let (effective, selected_bytes) = effective_selection(&tree, checked);
            (
                project_tree(&tree, &expanded, folding, sort),
                tree.node(tree.root()).on_disk,
                effective.len(),
                selected_bytes,
            )
        };
        if tree_state.selected().is_none() {
            tree_state.select(rows.first().map(|row| row.id));
        }
        let scan_bytes = bytes_seen.max(root_bytes);
        let scan_copy = if scanning {
            format!(
                "scanning · {} · {inaccessible} unreadable",
                format_size(scan_bytes, DECIMAL)
            )
        } else {
            format!(
                "complete · {} · {inaccessible} unreadable",
                format_size(root_bytes, DECIMAL)
            )
        };
        let selection_copy = format!(
            "{selected_count} items selected — {} reclaimable · {} · folding {}",
            format_size(selected_bytes, DECIMAL),
            match sort {
                SortKey::OnDisk => "on-disk",
                SortKey::Apparent => "apparent",
                SortKey::Name => "name",
            },
            if folding { "on" } else { "off" }
        );
        let honesty_copy = if inaccessible == 0 {
            "sizes are on-disk; APFS clones may overcount; purgeable space not included".to_owned()
        } else {
            format!(
                "{inaccessible} items unreadable — grant Full Disk Access for a complete picture; APFS clones may overcount; purgeable space not included"
            )
        };

        terminal.draw(|frame| {
            let [
                header_area,
                progress_area,
                tree_area,
                selection_area,
                honesty_area,
                hints_area,
            ] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(frame.area());
            let left = [StatusSlot {
                id: HeaderSlot::Root,
                content: &root_label,
                priority: 2,
                min_width: 8,
                enabled: true,
                style: theme.style(Role::Accent),
                hover_style: None,
            }];
            let right = [StatusSlot {
                id: HeaderSlot::Scan,
                content: &scan_copy,
                priority: 1,
                min_width: 12,
                enabled: true,
                style: theme.style(Role::TextMuted),
                hover_style: None,
            }];
            frame.render_stateful_widget(
                &StatusBar {
                    left: &left,
                    right: &right,
                    theme: &theme,
                    alpha: 1.0,
                },
                header_area,
                &mut status_state,
            );
            if scanning {
                let label = format!("{} scanned", format_size(scan_bytes, DECIMAL));
                frame.render_widget(
                    &Progress {
                        kind: ProgressKind::Indeterminate { tick },
                        label: Some(&label),
                        theme: &theme,
                    },
                    progress_area,
                );
            } else {
                frame.render_widget(
                    Paragraph::new("Scan complete").style(theme.style(Role::Success)),
                    progress_area,
                );
            }
            let panel = Panel::new(&theme)
                .title(" Disk usage ")
                .emphasis(PanelEmphasis::Focused);
            let inner = panel.inner(tree_area);
            frame.render_widget(&panel, tree_area);
            frame.render_stateful_widget(
                &Tree {
                    nodes: &rows,
                    theme: &theme,
                },
                inner,
                &mut tree_state,
            );
            frame.render_widget(
                Paragraph::new(selection_copy.as_str()).style(theme.style(Role::Text)),
                selection_area,
            );
            frame.render_widget(
                Paragraph::new(honesty_copy.as_str())
                    .style(theme.style(Role::TextMuted))
                    .wrap(Wrap { trim: false }),
                honesty_area,
            );
            frame.render_widget(
                &HintBar {
                    hints: ANALYZER_HINTS,
                    separator: " · ",
                    theme: &theme,
                },
                hints_area,
            );
            render_modal(frame, &mut modals, &theme, tick, exit_after_delete);
        })?;

        tick = tick.wrapping_add(1);
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let key = termrock::input::KeyEvent::from(key);
            if modals.is_open() {
                let effect = match modals.current_mut().expect("open modal") {
                    AnalyzerModal::Confirm {
                        items, mode, state, ..
                    } => match handle_confirm_input(mode, state, key) {
                        ConfirmTransition::None => ModalEffect::None,
                        ConfirmTransition::Cancel => ModalEffect::Close,
                        ConfirmTransition::Delete(mode) => ModalEffect::Start(DeletePlan {
                            items: items.iter().map(|item| item.path.clone()).collect(),
                            mode,
                            dry_run: false,
                        }),
                    },
                    AnalyzerModal::Deleting { .. } => {
                        if requests_quit_while_deleting(key) {
                            ModalEffect::OpenQuit
                        } else {
                            ModalEffect::None
                        }
                    }
                    AnalyzerModal::QuitRunning { state } => {
                        let actions = [
                            DialogAction {
                                id: QuitChoice::Stay,
                                label: "Stay",
                                enabled: true,
                                style: None,
                            },
                            DialogAction {
                                id: QuitChoice::Leave,
                                label: "Leave when finished",
                                enabled: true,
                                style: None,
                            },
                        ];
                        match state.handle_key(&actions, key) {
                            Outcome::Activated(QuitChoice::Stay) | Outcome::Cancelled => {
                                ModalEffect::Close
                            }
                            Outcome::Activated(QuitChoice::Leave) => ModalEffect::ExitAfterDelete,
                            Outcome::Ignored | Outcome::Changed => ModalEffect::None,
                        }
                    }
                    AnalyzerModal::Report { .. } => {
                        if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter) {
                            ModalEffect::Close
                        } else {
                            ModalEffect::None
                        }
                    }
                };
                match effect {
                    ModalEffect::None => {}
                    ModalEffect::Close => modals.pop(),
                    ModalEffect::OpenQuit => {
                        modals.open_sub(AnalyzerModal::QuitRunning {
                            state: ChoiceDialogState::new(Some(QuitChoice::Stay)),
                        });
                    }
                    ModalEffect::Start(plan) => {
                        let mode = plan.mode;
                        let (sender, report) = mpsc::channel();
                        tokio::task::spawn_blocking(move || {
                            let _ = sender.send(execute(&plan));
                        });
                        modals.open(AnalyzerModal::Deleting { report, mode });
                    }
                    ModalEffect::ExitAfterDelete => {
                        exit_after_delete = true;
                        modals.pop();
                    }
                }
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break 'screen,
                KeyCode::Char('d') | KeyCode::Backspace => {
                    let items = selected_items(&handle, &tree_state, &root);
                    if !items.is_empty() {
                        let total = items
                            .iter()
                            .fold(0_u64, |sum, item| sum.saturating_add(item.size));
                        modals.open(new_delete_confirmation(items, total));
                    }
                }
                KeyCode::Char('f') => folding = !folding,
                KeyCode::Char('s') => {
                    sort = match sort {
                        SortKey::OnDisk => SortKey::Apparent,
                        SortKey::Apparent | SortKey::Name => SortKey::OnDisk,
                    };
                }
                KeyCode::Char('r') => {
                    handle.cancel.store(true, Ordering::Release);
                    handle = scan(ScanOptions::new(&root));
                    expanded = HashSet::from([NodeId(0)]);
                    tree_state = TreeState::new(Some(NodeId(0)));
                    tree_state.enable_multi_select();
                    scanning = true;
                    bytes_seen = 0;
                    inaccessible = 0;
                }
                _ => match tree_state.handle_key(&rows, key) {
                    TreeOutcome::Toggle(id) | TreeOutcome::Activated(id) => {
                        if !expanded.remove(&id) {
                            expanded.insert(id);
                        }
                    }
                    TreeOutcome::Ignored
                    | TreeOutcome::SelectionChanged(_)
                    | TreeOutcome::CheckToggled(_) => {}
                },
            }
        }
    }

    handle.cancel.store(true, Ordering::Release);
    drop(terminal);
    session.restore()?;
    Ok(())
}

pub async fn prompt_path() -> anyhow::Result<Option<PathBuf>> {
    let theme = Theme::tailrocks_phosphor();
    let initial = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let mut input = TextInputState::new(initial);
    let mut error_message: Option<String> = None;
    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;

    let selected = loop {
        terminal.draw(|frame| {
            frame.render_widget(&Backdrop::default(), frame.area());
            let area = centered_rect(76, 7, frame.area());
            let panel = Panel::new(&theme)
                .title(" Analyze a path ")
                .emphasis(PanelEmphasis::Focused);
            let inner = panel.inner(area);
            frame.render_widget(&panel, area);
            let [copy_area, input_area, error_area, hints_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(inner);
            frame.render_widget(Paragraph::new("Enter an absolute existing path"), copy_area);
            frame.render_stateful_widget(
                &TextInput {
                    label: "Path",
                    placeholder: "/Users/name",
                    validation: error_message
                        .as_deref()
                        .map_or(Validation::Valid, Validation::Invalid),
                    theme: &theme,
                },
                input_area,
                &mut input,
            );
            if let Some(error) = error_message.as_deref() {
                frame.render_widget(
                    Paragraph::new(error).style(theme.style(Role::Danger)),
                    error_area,
                );
            }
            frame.render_widget(
                &HintBar {
                    hints: &[
                        Hint {
                            chord: "enter",
                            label: "analyze",
                            priority: 1,
                            visible: true,
                        },
                        Hint {
                            chord: "esc",
                            label: "cancel",
                            priority: 1,
                            visible: true,
                        },
                    ],
                    separator: " · ",
                    theme: &theme,
                },
                hints_area,
            );
        })?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match input.handle_key(termrock::input::KeyEvent::from(key)) {
                TextInputOutcome::Submitted(value) => {
                    let path = PathBuf::from(value.trim());
                    if !path.is_absolute() {
                        error_message = Some("Path must be absolute".into());
                    } else if !path.exists() {
                        error_message = Some("Path does not exist".into());
                    } else {
                        break Some(path);
                    }
                }
                TextInputOutcome::Cancelled => break None,
                TextInputOutcome::Changed => error_message = None,
                TextInputOutcome::Ignored => {}
            }
        }
    };

    drop(terminal);
    session.restore()?;
    Ok(selected)
}

fn drain_scan_events(
    handle: &ScanHandle,
    scanning: &mut bool,
    bytes_seen: &mut u64,
    inaccessible: &mut u64,
) {
    while let Ok(event) = handle.events.try_recv() {
        match event {
            ScanEvent::DirErrored { .. } => *inaccessible = inaccessible.saturating_add(1),
            ScanEvent::Progress {
                bytes_seen: seen, ..
            } => *bytes_seen = seen,
            ScanEvent::Finished {
                inaccessible: final_inaccessible,
                ..
            } => {
                *inaccessible = final_inaccessible;
                *scanning = false;
            }
            ScanEvent::DirAdded { .. } | ScanEvent::SizesUpdated => {}
        }
    }
}

fn render_modal(
    frame: &mut ratatui::Frame<'_>,
    modals: &mut ModalStack<AnalyzerModal>,
    theme: &Theme,
    tick: u64,
    exit_after_delete: bool,
) {
    let Some(modal) = modals.current_mut() else {
        return;
    };
    frame.render_widget(&Backdrop::default(), frame.area());
    match modal {
        AnalyzerModal::Confirm {
            items,
            total,
            mode,
            state,
        } => {
            let mut body = vec![
                Line::styled(
                    format!(
                        "{} items — {} {}",
                        items.len(),
                        format_size(*total, DECIMAL),
                        if *mode == DeleteMode::Trash {
                            "reclaimable after emptying Trash"
                        } else {
                            "will be freed"
                        }
                    ),
                    theme.style(Role::Text),
                ),
                Line::styled(
                    match mode {
                        DeleteMode::Trash => "Mode: Move to Trash",
                        DeleteMode::Permanent => "Mode: PERMANENT DELETE",
                    },
                    theme.style(if *mode == DeleteMode::Trash {
                        Role::Success
                    } else {
                        Role::Danger
                    }),
                ),
                Line::raw(""),
            ];
            body.extend(items.iter().take(10).map(|item| {
                Line::styled(
                    format!(
                        "{}  {}",
                        format_size(item.size, DECIMAL),
                        item.path.display()
                    ),
                    theme.style(Role::TextMuted),
                )
            }));
            if items.len() > 10 {
                body.push(Line::styled(
                    format!("…and {} more", items.len() - 10),
                    theme.style(Role::TextMuted),
                ));
            }
            if *mode == DeleteMode::Permanent {
                body.push(Line::styled(
                    "Permanent deletion cannot be undone.",
                    theme.style(Role::Danger),
                ));
            } else {
                body.push(Line::styled(
                    "Press p to switch to permanent deletion.",
                    theme.style(Role::Warning),
                ));
            }
            let actions = [
                DialogAction {
                    id: DeleteChoice::Cancel,
                    label: "Cancel",
                    enabled: true,
                    style: None,
                },
                DialogAction {
                    id: DeleteChoice::Delete,
                    label: "Delete",
                    enabled: true,
                    style: Some(theme.style(Role::Danger)),
                },
            ];
            let area = centered_rect(76, 18, frame.area());
            frame.render_stateful_widget(
                &ChoiceDialog {
                    dialog: Dialog {
                        title: "Confirm cleanup",
                        body: Text::from(body),
                        style: theme.style(Role::Text),
                        theme,
                        emphasis: PanelEmphasis::Focused,
                    },
                    actions: &actions,
                    gap: "  ",
                },
                area,
                state,
            );
        }
        AnalyzerModal::Deleting { .. } => {
            let area = centered_rect(58, 7, frame.area());
            frame.render_widget(
                &Dialog {
                    title: "Cleanup in progress",
                    body: Text::from(if exit_after_delete {
                        "Finishing safely; holla will exit afterward."
                    } else {
                        "Moving selected items. This operation cannot be interrupted."
                    }),
                    style: theme.style(Role::Text),
                    theme,
                    emphasis: PanelEmphasis::Focused,
                },
                area,
            );
            frame.render_widget(
                &Progress {
                    kind: ProgressKind::Indeterminate { tick },
                    label: Some("Cleaning up"),
                    theme,
                },
                ratatui::layout::Rect::new(
                    area.x.saturating_add(2),
                    area.bottom().saturating_sub(2),
                    area.width.saturating_sub(4),
                    1,
                ),
            );
        }
        AnalyzerModal::QuitRunning { state } => {
            let actions = [
                DialogAction {
                    id: QuitChoice::Stay,
                    label: "Stay",
                    enabled: true,
                    style: None,
                },
                DialogAction {
                    id: QuitChoice::Leave,
                    label: "Leave when finished",
                    enabled: true,
                    style: Some(theme.style(Role::Warning)),
                },
            ];
            let area = centered_rect(64, 8, frame.area());
            frame.render_stateful_widget(
                &ChoiceDialog {
                    dialog: Dialog {
                        title: "Cleanup still running",
                        body: Text::from(
                            "Deletion cannot be interrupted. Leave automatically after it finishes?",
                        ),
                        style: theme.style(Role::Text),
                        theme,
                        emphasis: PanelEmphasis::Focused,
                    },
                    actions: &actions,
                    gap: "  ",
                },
                area,
                state,
            );
        }
        AnalyzerModal::Report {
            report,
            mode,
            state,
        } => {
            let removed = report.removed.len().to_string();
            let failed = report.failed.len().to_string();
            let skipped = report.skipped.len().to_string();
            let freed = format_size(
                report
                    .removed
                    .iter()
                    .fold(0_u64, |total, (_, size)| total.saturating_add(*size)),
                DECIMAL,
            );
            let freed_copy = if *mode == DeleteMode::Trash {
                format!("{freed} after emptying Trash")
            } else {
                freed
            };
            let log = if report.log_errors.is_empty() {
                "recorded".to_owned()
            } else {
                format!("{} errors", report.log_errors.len())
            };
            let details = [
                DetailRow {
                    id: ReportRow::Removed,
                    label: "Removed",
                    value: &removed,
                    href: None,
                    capability: DetailCapability::None,
                    emphasis: true,
                    style: Some(theme.style(Role::Success)),
                },
                DetailRow {
                    id: ReportRow::Failed,
                    label: "Failed",
                    value: &failed,
                    href: None,
                    capability: DetailCapability::None,
                    emphasis: !report.failed.is_empty(),
                    style: (!report.failed.is_empty()).then(|| theme.style(Role::Danger)),
                },
                DetailRow {
                    id: ReportRow::Skipped,
                    label: "Skipped",
                    value: &skipped,
                    href: None,
                    capability: DetailCapability::None,
                    emphasis: !report.skipped.is_empty(),
                    style: None,
                },
                DetailRow {
                    id: ReportRow::Freed,
                    label: if *mode == DeleteMode::Trash {
                        "Reclaimable"
                    } else {
                        "Freed"
                    },
                    value: &freed_copy,
                    href: None,
                    capability: DetailCapability::None,
                    emphasis: true,
                    style: Some(theme.style(Role::Accent)),
                },
                DetailRow {
                    id: ReportRow::Log,
                    label: "Ops log",
                    value: &log,
                    href: None,
                    capability: DetailCapability::None,
                    emphasis: false,
                    style: None,
                },
            ];
            let area = centered_rect(60, 12, frame.area());
            frame.render_stateful_widget(
                &MessageDialog {
                    dialog: Dialog {
                        title: "Cleanup report",
                        body: Text::from(if *mode == DeleteMode::Trash {
                            "Moved to Trash. Empty Trash to reclaim space. Press Esc or Enter."
                        } else {
                            "Cleanup finished. Press Esc or Enter to return."
                        }),
                        style: theme.style(Role::Text),
                        theme,
                        emphasis: PanelEmphasis::Focused,
                    },
                    details: &details,
                    label_width: 12,
                    wrap: false,
                    theme,
                },
                area,
                state,
            );
        }
    }
}

pub fn effective_selection(tree: &ScanTree, checked: &[NodeId]) -> (Vec<NodeId>, u64) {
    let checked_set: HashSet<_> = checked.iter().copied().collect();
    let effective: Vec<_> = checked
        .iter()
        .copied()
        .filter(|id| tree.nodes().get(id.0 as usize).is_some())
        .filter(|id| {
            let mut parent = tree.node(*id).parent;
            while let Some(ancestor) = parent {
                if checked_set.contains(&ancestor) {
                    return false;
                }
                parent = tree.node(ancestor).parent;
            }
            true
        })
        .collect();
    let bytes = effective.iter().fold(0_u64, |total, id| {
        total.saturating_add(tree.node(*id).on_disk)
    });
    (effective, bytes)
}

fn selected_items(handle: &ScanHandle, state: &TreeState<NodeId>, root: &Path) -> Vec<ConfirmItem> {
    let tree = handle.tree.read().expect("disk scan tree lock");
    let checked = state
        .selection()
        .map(|selection| selection.checked())
        .unwrap_or_default();
    effective_selection(&tree, checked)
        .0
        .into_iter()
        .map(|id| ConfirmItem {
            path: node_path(&tree, root, id),
            size: tree.node(id).on_disk,
        })
        .collect()
}

fn node_path(tree: &ScanTree, root: &Path, id: NodeId) -> PathBuf {
    if id == tree.root() {
        return root.to_path_buf();
    }
    let mut names = Vec::new();
    let mut current = Some(id);
    while let Some(node_id) = current {
        if node_id == tree.root() {
            break;
        }
        let node = tree.node(node_id);
        names.push(node.name.clone());
        current = node.parent;
    }
    let mut path = root.to_path_buf();
    for name in names.into_iter().rev() {
        path.push(name);
    }
    path
}

fn project_tree(
    tree: &ScanTree,
    expanded: &HashSet<NodeId>,
    folding: bool,
    sort: SortKey,
) -> Vec<TreeNode<'static, NodeId>> {
    let mut rows = Vec::new();
    let mut pending = vec![(tree.root(), 0_u16)];
    while let Some((id, depth)) = pending.pop() {
        let node = tree.node(id);
        let name = node.name.to_string_lossy();
        let folded = folding && node.is_dir && FOLD_SET.contains(&name.as_ref());
        let branch = node.is_dir
            && !folded
            && (!node.children.is_empty() || node.state == NodeState::Scanning);
        let is_expanded = branch && expanded.contains(&id);
        let label = if folded {
            format!("{name} (folded)")
        } else {
            name.into_owned()
        };
        let percentage = node.parent.and_then(|parent| {
            let total = tree.node(parent).on_disk;
            (total > 0).then(|| (node.on_disk as f64 / total as f64) * 100.0)
        });
        let trailing = match percentage {
            Some(percentage) => format!(
                "{} · {:.0}%",
                format_size(node.on_disk, DECIMAL),
                percentage
            ),
            None => format_size(node.on_disk, DECIMAL),
        };
        rows.push(TreeNode {
            id,
            label: Line::from(label),
            trailing: Some(Line::from(trailing)),
            depth,
            branch,
            expanded: is_expanded,
            enabled: true,
            status: match node.state {
                NodeState::Scanning => TreeNodeStatus::Loading,
                NodeState::Done => TreeNodeStatus::Ready,
                NodeState::Errored(_) => TreeNodeStatus::Error,
            },
        });
        if is_expanded {
            let children = tree.sorted_children(id, sort);
            pending.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|child| (child, depth.saturating_add(1))),
            );
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::du::{NodeState, ScanTree};
    use termrock::widgets::TreeNodeStatus;

    struct Fixture {
        tree: ScanTree,
        root: NodeId,
        parent: NodeId,
        child: NodeId,
        sibling: NodeId,
    }

    fn fixture() -> Fixture {
        let mut tree = ScanTree::new("root".into(), true);
        let root = tree.root();
        let parent = tree.add_dir(root, "parent".into());
        let child = tree.add_dir(parent, "child".into());
        let sibling = tree.add_dir(root, "sibling".into());
        tree.add_sizes(child, 20, 40, 1);
        tree.add_sizes(sibling, 10, 80, 1);
        Fixture {
            tree,
            root,
            parent,
            child,
            sibling,
        }
    }

    #[test]
    fn effective_selection_empty_is_zero() {
        let fixture = fixture();
        assert_eq!(effective_selection(&fixture.tree, &[]), (vec![], 0));
    }

    #[test]
    fn effective_selection_sums_disjoint_nodes() {
        let fixture = fixture();
        assert_eq!(
            effective_selection(&fixture.tree, &[fixture.child, fixture.sibling]),
            (vec![fixture.child, fixture.sibling], 30)
        );
    }

    #[test]
    fn effective_selection_deduplicates_nested_parent_first() {
        let fixture = fixture();
        assert_eq!(
            effective_selection(&fixture.tree, &[fixture.parent, fixture.child]),
            (vec![fixture.parent], 20)
        );
    }

    #[test]
    fn effective_selection_deduplicates_nested_child_first() {
        let fixture = fixture();
        assert_eq!(
            effective_selection(&fixture.tree, &[fixture.child, fixture.parent]),
            (vec![fixture.parent], 20)
        );
    }

    #[test]
    fn node_paths_reconstruct_without_canonicalizing() {
        let fixture = fixture();
        assert_eq!(
            node_path(&fixture.tree, Path::new("/tmp/root"), fixture.child),
            PathBuf::from("/tmp/root/parent/child")
        );
        assert_eq!(
            node_path(&fixture.tree, Path::new("/tmp/root"), fixture.root),
            PathBuf::from("/tmp/root")
        );
    }

    #[test]
    fn delete_confirmation_focuses_cancel() {
        let AnalyzerModal::Confirm { mode, state, .. } = new_delete_confirmation(vec![], 0) else {
            panic!("confirmation modal")
        };
        assert_eq!(mode, DeleteMode::Trash);
        assert_eq!(state.focused, Some(DeleteChoice::Cancel));
    }

    fn key(code: KeyCode) -> termrock::input::KeyEvent {
        termrock::input::KeyEvent::new(code, termrock::input::KeyModifiers::NONE)
    }

    #[test]
    fn delete_confirmation_escape_cancels_without_activation() {
        let mut mode = DeleteMode::Trash;
        let mut state = ChoiceDialogState::new(Some(DeleteChoice::Cancel));
        assert_eq!(
            handle_confirm_input(&mut mode, &mut state, key(KeyCode::Esc)),
            ConfirmTransition::Cancel
        );
    }

    #[test]
    fn permanent_delete_requires_p_then_explicit_delete_activation() {
        let mut mode = DeleteMode::Trash;
        let mut state = ChoiceDialogState::new(Some(DeleteChoice::Cancel));
        assert_eq!(
            handle_confirm_input(&mut mode, &mut state, key(KeyCode::Char('p'))),
            ConfirmTransition::None
        );
        assert_eq!(mode, DeleteMode::Permanent);
        assert_eq!(
            handle_confirm_input(&mut mode, &mut state, key(KeyCode::Right)),
            ConfirmTransition::None
        );
        assert_eq!(
            handle_confirm_input(&mut mode, &mut state, key(KeyCode::Enter)),
            ConfirmTransition::Delete(DeleteMode::Permanent)
        );
    }

    #[test]
    fn deleting_q_requests_confirmation_instead_of_immediate_exit() {
        assert!(requests_quit_while_deleting(key(KeyCode::Char('q'))));
        assert!(requests_quit_while_deleting(key(KeyCode::Esc)));
        assert!(!requests_quit_while_deleting(key(KeyCode::Enter)));
    }

    #[test]
    fn projection_starts_with_root_and_first_level() {
        let fixture = fixture();
        let rows = project_tree(
            &fixture.tree,
            &HashSet::from([fixture.root]),
            false,
            SortKey::OnDisk,
        );
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            [fixture.root, fixture.parent, fixture.sibling]
        );
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[1].depth, 1);
    }

    #[test]
    fn projection_sorts_by_requested_size() {
        let fixture = fixture();
        let expanded = HashSet::from([fixture.root]);
        let on_disk = project_tree(&fixture.tree, &expanded, false, SortKey::OnDisk);
        let apparent = project_tree(&fixture.tree, &expanded, false, SortKey::Apparent);
        assert_eq!(on_disk[1].id, fixture.parent);
        assert_eq!(apparent[1].id, fixture.sibling);
    }

    #[test]
    fn folded_directory_is_selectable_but_not_expandable() {
        let mut tree = ScanTree::new("root".into(), true);
        let root = tree.root();
        let folded = tree.add_dir(root, "node_modules".into());
        tree.add_dir(folded, "package".into());
        let rows = project_tree(&tree, &HashSet::from([root, folded]), true, SortKey::OnDisk);
        let row = rows.iter().find(|row| row.id == folded).unwrap();
        assert!(row.enabled);
        assert!(!row.branch);
        assert!(!row.expanded);
        assert!(row.label.to_string().contains("(folded)"));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn folding_can_be_disabled() {
        let mut tree = ScanTree::new("root".into(), true);
        let root = tree.root();
        let folded = tree.add_dir(root, "target".into());
        let child = tree.add_dir(folded, "debug".into());
        let rows = project_tree(
            &tree,
            &HashSet::from([root, folded]),
            false,
            SortKey::OnDisk,
        );
        assert!(rows.iter().any(|row| row.id == child));
    }

    #[test]
    fn scanning_and_error_states_map_to_tree_status() {
        let fixture = fixture();
        assert_eq!(fixture.tree.node(fixture.parent).state, NodeState::Scanning);
        let rows = project_tree(
            &fixture.tree,
            &HashSet::from([fixture.root]),
            false,
            SortKey::OnDisk,
        );
        assert_eq!(
            rows.iter()
                .find(|row| row.id == fixture.parent)
                .unwrap()
                .status,
            TreeNodeStatus::Loading
        );
    }

    #[test]
    fn trailing_metadata_has_size_and_parent_percentage() {
        let fixture = fixture();
        let rows = project_tree(
            &fixture.tree,
            &HashSet::from([fixture.root]),
            false,
            SortKey::OnDisk,
        );
        let trailing = rows
            .iter()
            .find(|row| row.id == fixture.parent)
            .unwrap()
            .trailing
            .as_ref()
            .unwrap()
            .to_string();
        assert!(trailing.contains("20 B"));
        assert!(trailing.contains("67%"));
    }
}
