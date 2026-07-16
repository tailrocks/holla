use std::{collections::HashMap, path::PathBuf, sync::mpsc};

use humansize::{DECIMAL, format_size};
use ratatui::text::{Line, Text};
use termrock::{
    input::{KeyCode, KeyEvent},
    interaction::{ModalStack, Outcome},
    layout::centered_rect,
    style::{Role, Theme},
    widgets::{
        Action as DialogAction, Backdrop, ChoiceDialog, ChoiceDialogState, DetailCapability,
        DetailRow, DetailTableState, Dialog, MessageDialog, PanelEmphasis, Progress, ProgressKind,
    },
};

use crate::{
    cleanup::{DeleteMode, DeletePlan, DeleteReport, execute, execute_skipped, operation_log_path},
    insights::is_process_running,
};

#[derive(Debug, Clone)]
pub struct CleanupItem {
    pub path: PathBuf,
    pub size: u64,
    pub policy: CleanupPolicy,
}

impl CleanupItem {
    pub fn new(path: PathBuf, size: u64) -> Self {
        Self {
            path,
            size,
            policy: CleanupPolicy::default(),
        }
    }

    pub fn with_policy(mut self, policy: CleanupPolicy) -> Self {
        self.policy = policy;
        self
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CleanupPolicy {
    pub skip_if_running: Option<&'static str>,
    pub stop_gradle: bool,
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

enum CleanupModal {
    Confirm {
        items: Vec<CleanupItem>,
        total: u64,
        mode: DeleteMode,
        dry_run: bool,
        state: ChoiceDialogState<DeleteChoice>,
    },
    Deleting {
        report: mpsc::Receiver<DeleteReport>,
        mode: DeleteMode,
        first_path: PathBuf,
    },
    QuitRunning {
        state: ChoiceDialogState<QuitChoice>,
    },
    Report {
        report: DeleteReport,
        mode: DeleteMode,
        state: Box<DetailTableState<ReportRow>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupPoll {
    None,
    Completed,
    Exit,
}

#[derive(Default)]
pub struct CleanupFlow {
    modals: ModalStack<CleanupModal>,
    exit_after_delete: bool,
}

impl CleanupFlow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_open(&self) -> bool {
        self.modals.is_open()
    }

    pub fn open_confirmation(&mut self, items: Vec<CleanupItem>) {
        let items = deduplicate_items(items);
        let total = items
            .iter()
            .fold(0_u64, |sum, item| sum.saturating_add(item.size));
        self.modals.open(CleanupModal::Confirm {
            items,
            total,
            mode: DeleteMode::Trash,
            dry_run: false,
            state: ChoiceDialogState::new(Some(DeleteChoice::Cancel)),
        });
    }

    pub fn poll(&mut self) -> CleanupPoll {
        let completed = match self.modals.current_mut() {
            Some(CleanupModal::Deleting {
                report,
                mode,
                first_path,
            }) => match report.try_recv() {
                Ok(report) => Some((report, *mode)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    let mut report = DeleteReport::default();
                    report
                        .failed
                        .push((first_path.clone(), "cleanup worker stopped".into()));
                    Some((report, *mode))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            },
            _ => None,
        };
        let Some((report, mode)) = completed else {
            return CleanupPoll::None;
        };
        if self.exit_after_delete {
            return CleanupPoll::Exit;
        }
        self.modals.open(CleanupModal::Report {
            report,
            mode,
            state: Box::default(),
        });
        CleanupPoll::Completed
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        enum Effect {
            None,
            Close,
            OpenQuit,
            Start {
                items: Vec<CleanupItem>,
                mode: DeleteMode,
                dry_run: bool,
            },
            ExitAfterDelete,
        }

        let effect = match self.modals.current_mut() {
            Some(CleanupModal::Confirm {
                items,
                mode,
                dry_run,
                state,
                ..
            }) => {
                if key.code == KeyCode::Char('p') {
                    *mode = match mode {
                        DeleteMode::Trash => DeleteMode::Permanent,
                        DeleteMode::Permanent => DeleteMode::Trash,
                    };
                    Effect::None
                } else if key.code == KeyCode::Char('n') {
                    *dry_run = !*dry_run;
                    Effect::None
                } else {
                    match state.handle_key(&delete_actions(), key) {
                        Outcome::Activated(DeleteChoice::Cancel) | Outcome::Cancelled => {
                            Effect::Close
                        }
                        Outcome::Activated(DeleteChoice::Delete) => Effect::Start {
                            items: items.clone(),
                            mode: *mode,
                            dry_run: *dry_run,
                        },
                        Outcome::Ignored | Outcome::Changed => Effect::None,
                        _ => Effect::None,
                    }
                }
            }
            Some(CleanupModal::Deleting { .. }) => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    Effect::OpenQuit
                } else {
                    Effect::None
                }
            }
            Some(CleanupModal::QuitRunning { state }) => {
                match state.handle_key(&quit_actions(), key) {
                    Outcome::Activated(QuitChoice::Stay) | Outcome::Cancelled => Effect::Close,
                    Outcome::Activated(QuitChoice::Leave) => Effect::ExitAfterDelete,
                    Outcome::Ignored | Outcome::Changed => Effect::None,
                    _ => Effect::None,
                }
            }
            Some(CleanupModal::Report { .. }) => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter) {
                    Effect::Close
                } else {
                    Effect::None
                }
            }
            None => Effect::None,
        };

        match effect {
            Effect::None => {}
            Effect::Close => {
                self.modals.pop();
            }
            Effect::OpenQuit => self.modals.open_sub(CleanupModal::QuitRunning {
                state: ChoiceDialogState::new(Some(QuitChoice::Stay)),
            }),
            Effect::Start {
                items,
                mode,
                dry_run,
            } => {
                let first_path = items
                    .first()
                    .map(|item| item.path.clone())
                    .unwrap_or_default();
                let (sender, report) = mpsc::channel();
                tokio::task::spawn_blocking(move || {
                    if items.iter().any(|item| item.policy.stop_gradle) {
                        let _ = std::process::Command::new("gradle").arg("--stop").status();
                    }
                    let mut report = DeleteReport::default();
                    let mut running = HashMap::new();
                    for item in items {
                        let plan = DeletePlan {
                            items: vec![item.path],
                            mode,
                            dry_run,
                        };
                        let blocked = item.policy.skip_if_running.is_some_and(|name| {
                            *running
                                .entry(name)
                                .or_insert_with(|| is_process_running(name))
                        });
                        let item_report = if blocked {
                            execute_skipped(&plan, "App is running")
                        } else {
                            execute(&plan)
                        };
                        report.removed.extend(item_report.removed);
                        report.failed.extend(item_report.failed);
                        report.skipped.extend(item_report.skipped);
                        report.log_errors.extend(item_report.log_errors);
                    }
                    let _ = sender.send(report);
                });
                self.modals.open(CleanupModal::Deleting {
                    report,
                    mode,
                    first_path,
                });
            }
            Effect::ExitAfterDelete => {
                self.exit_after_delete = true;
                self.modals.pop();
            }
        }
    }

    pub fn render(&mut self, frame: &mut ratatui::Frame<'_>, theme: &Theme, tick: u64) {
        let Some(modal) = self.modals.current_mut() else {
            return;
        };
        frame.render_widget(Backdrop::default(), frame.area());
        match modal {
            CleanupModal::Confirm {
                items,
                total,
                mode,
                dry_run,
                state,
                ..
            } => render_confirm(frame, theme, items, *total, *mode, *dry_run, state),
            CleanupModal::Deleting { .. } => {
                render_deleting(frame, theme, tick, self.exit_after_delete)
            }
            CleanupModal::QuitRunning { state } => render_quit(frame, theme, state),
            CleanupModal::Report {
                report,
                mode,
                state,
            } => render_report(frame, theme, report, *mode, state),
        }
    }
}

fn deduplicate_items(mut items: Vec<CleanupItem>) -> Vec<CleanupItem> {
    items.sort_by(|left, right| {
        left.path
            .components()
            .count()
            .cmp(&right.path.components().count())
            .then_with(|| left.path.cmp(&right.path))
    });
    let mut effective: Vec<CleanupItem> = Vec::new();
    for item in items {
        if effective
            .iter()
            .any(|ancestor| item.path == ancestor.path || item.path.starts_with(&ancestor.path))
        {
            continue;
        }
        effective.push(item);
    }
    effective
}

fn delete_actions() -> [DialogAction<'static, DeleteChoice>; 2] {
    [
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
    ]
}

fn quit_actions() -> [DialogAction<'static, QuitChoice>; 2] {
    [
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
    ]
}

fn render_confirm(
    frame: &mut ratatui::Frame<'_>,
    theme: &Theme,
    items: &[CleanupItem],
    total: u64,
    mode: DeleteMode,
    dry_run: bool,
    state: &mut ChoiceDialogState<DeleteChoice>,
) {
    let mut body = vec![
        Line::styled(
            format!(
                "{} items — {} {}",
                items.len(),
                format_size(total, DECIMAL),
                if mode == DeleteMode::Trash {
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
            theme.style(if mode == DeleteMode::Trash {
                Role::Success
            } else {
                Role::Danger
            }),
        ),
        Line::styled(
            format!(
                "Dry run: {} (n toggles)",
                if dry_run { "ON" } else { "off" }
            ),
            theme.style(if dry_run {
                Role::Warning
            } else {
                Role::TextMuted
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
    body.push(Line::styled(
        if mode == DeleteMode::Permanent {
            "Permanent deletion cannot be undone. Press p for Trash."
        } else {
            "Press p to switch to permanent deletion."
        },
        theme.style(if mode == DeleteMode::Permanent {
            Role::Danger
        } else {
            Role::Warning
        }),
    ));
    let mut actions = delete_actions();
    actions[1].style = Some(theme.style(Role::Danger));
    let area = centered_rect(78, 19, frame.area());
    frame.render_stateful_widget(
        &ChoiceDialog::new(
            Dialog::new("Confirm cleanup", Text::from(body), theme)
                .style(theme.style(Role::Text))
                .emphasis(PanelEmphasis::Focused),
            &actions,
        )
        .gap("  "),
        area,
        state,
    );
}

fn render_deleting(
    frame: &mut ratatui::Frame<'_>,
    theme: &Theme,
    tick: u64,
    exit_after_delete: bool,
) {
    let area = centered_rect(58, 7, frame.area());
    frame.render_widget(
        Dialog::new(
            "Cleanup in progress",
            Text::from(if exit_after_delete {
                "Finishing safely; holla will exit afterward."
            } else {
                "Moving selected items. This operation cannot be interrupted."
            }),
            theme,
        )
        .style(theme.style(Role::Text))
        .emphasis(PanelEmphasis::Focused),
        area,
    );
    frame.render_widget(
        Progress::new(ProgressKind::Indeterminate { tick }, theme).label("Cleaning up"),
        ratatui::layout::Rect::new(
            area.x.saturating_add(2),
            area.bottom().saturating_sub(2),
            area.width.saturating_sub(4),
            1,
        ),
    );
}

fn render_quit(
    frame: &mut ratatui::Frame<'_>,
    theme: &Theme,
    state: &mut ChoiceDialogState<QuitChoice>,
) {
    let mut actions = quit_actions();
    actions[1].style = Some(theme.style(Role::Warning));
    let area = centered_rect(64, 8, frame.area());
    frame.render_stateful_widget(
        &ChoiceDialog::new(
            Dialog::new(
                "Cleanup still running",
                Text::from(
                    "Deletion cannot be interrupted. Leave automatically after it finishes?",
                ),
                theme,
            )
            .style(theme.style(Role::Text))
            .emphasis(PanelEmphasis::Focused),
            &actions,
        )
        .gap("  "),
        area,
        state,
    );
}

const REPORT_ISSUE_LIMIT: usize = 6;

fn report_body_lines(report: &DeleteReport, mode: DeleteMode) -> Vec<String> {
    let mut lines = vec![if mode == DeleteMode::Trash {
        "Moved to Trash. Empty Trash to reclaim space. Press Esc or Enter.".to_owned()
    } else {
        "Cleanup finished. Press Esc or Enter to return.".to_owned()
    }];
    let total_issues = report.failed.len() + report.skipped.len();
    if total_issues > 0 {
        lines.push(String::new());
        lines.extend(
            report
                .failed
                .iter()
                .map(|(path, reason)| format!("Failed: {} — {reason}", path.display()))
                .chain(
                    report
                        .skipped
                        .iter()
                        .map(|(path, reason)| format!("Skipped: {} — {reason}", path.display())),
                )
                .take(REPORT_ISSUE_LIMIT),
        );
        if total_issues > REPORT_ISSUE_LIMIT {
            lines.push(format!(
                "…and {} more; see the operation log.",
                total_issues - REPORT_ISSUE_LIMIT
            ));
        }
    }
    lines.push(format!(
        "Full log: {}",
        operation_log_path().map_or_else(
            || "unavailable".to_owned(),
            |path| path.display().to_string()
        )
    ));
    lines
}

fn render_report(
    frame: &mut ratatui::Frame<'_>,
    theme: &Theme,
    report: &DeleteReport,
    mode: DeleteMode,
    state: &mut DetailTableState<ReportRow>,
) {
    let body_lines = report_body_lines(report, mode);
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
    let freed_copy = if mode == DeleteMode::Trash {
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
            label: if mode == DeleteMode::Trash {
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
    let area = centered_rect(
        80,
        u16::try_from(12_usize.saturating_add(body_lines.len().min(9))).unwrap_or(21),
        frame.area(),
    );
    frame.render_stateful_widget(
        &MessageDialog::new(
            Dialog::new(
                "Cleanup report",
                Text::from(
                    body_lines
                        .iter()
                        .map(|line| Line::raw(line.as_str()))
                        .collect::<Vec<_>>(),
                ),
                theme,
            )
            .style(theme.style(Role::Text))
            .emphasis(PanelEmphasis::Focused),
            &details,
            theme,
        )
        .label_width(12)
        .wrap(false),
        area,
        state,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use termrock::input::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn confirmation_defaults_to_trash_cancel_and_live_mode() {
        let mut flow = CleanupFlow::new();
        flow.open_confirmation(Vec::new());
        let Some(CleanupModal::Confirm {
            mode,
            dry_run,
            state,
            ..
        }) = flow.modals.current_mut()
        else {
            panic!("confirmation")
        };
        assert_eq!(*mode, DeleteMode::Trash);
        assert!(!*dry_run);
        assert_eq!(state.focused, Some(DeleteChoice::Cancel));
    }

    #[test]
    fn escape_closes_confirmation_without_deleting() {
        let mut flow = CleanupFlow::new();
        flow.open_confirmation(Vec::new());
        flow.handle_key(key(KeyCode::Esc));
        assert!(!flow.is_open());
    }

    #[test]
    fn permanent_and_dry_run_require_explicit_toggles() {
        let mut flow = CleanupFlow::new();
        flow.open_confirmation(Vec::new());
        flow.handle_key(key(KeyCode::Char('p')));
        flow.handle_key(key(KeyCode::Char('n')));
        let Some(CleanupModal::Confirm { mode, dry_run, .. }) = flow.modals.current_mut() else {
            panic!("confirmation")
        };
        assert_eq!(*mode, DeleteMode::Permanent);
        assert!(*dry_run);
    }

    #[test]
    fn report_names_failures_and_bounds_details() {
        let report = DeleteReport {
            failed: (0..9)
                .map(|index| (PathBuf::from(format!("/tmp/{index}")), "failed".into()))
                .collect(),
            skipped: vec![(PathBuf::from("/tmp/skip"), "App is running".into())],
            ..DeleteReport::default()
        };
        let lines = report_body_lines(&report, DeleteMode::Trash);
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.starts_with("Failed:"))
                .count(),
            REPORT_ISSUE_LIMIT
        );
        assert!(lines.iter().any(|line| line.contains("and 4 more")));
        assert!(
            lines
                .last()
                .is_some_and(|line| line.starts_with("Full log:"))
        );
    }
}
