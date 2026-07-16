use std::{collections::HashSet, path::PathBuf, sync::mpsc, time::Duration};

use crossterm::event::{self, Event, KeyEventKind};
use humansize::{DECIMAL, format_size};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    text::{Line, Span},
    widgets::Paragraph,
};
use termrock::{
    input::KeyCode,
    interaction::Outcome,
    style::{Role, Theme},
    widgets::{
        Hint, HintBar, List, ListRow, ListState, Panel, PanelEmphasis, Progress, ProgressKind,
        RowRole, StatusBar, StatusBarState, StatusSlot,
    },
};

use crate::{
    insights::{self, Candidate, InsightSpec, Safety, SizeEvent, SizeHandle},
    tui::cleanup_flow::{CleanupFlow, CleanupItem, CleanupPolicy, CleanupPoll},
};

const MAX_SIZERS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderSlot {
    Title,
    State,
}

struct InsightView {
    spec: &'static InsightSpec,
    candidates: Vec<Candidate>,
    finished: bool,
}

struct ActiveSizer {
    view: usize,
    handle: SizeHandle,
}

const HINTS: &[Hint<'static>] = &[
    Hint {
        chord: "↑↓",
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
        chord: "enter",
        label: "inspect",
        priority: 4,
        visible: true,
    },
    Hint {
        chord: "d",
        label: "clean",
        priority: 5,
        visible: true,
    },
    Hint {
        chord: "esc",
        label: "back",
        priority: 5,
        visible: true,
    },
];

pub async fn run(filter: Option<&'static str>) -> anyhow::Result<()> {
    let probe = insights::Probe::current()
        .ok_or_else(|| anyhow::anyhow!("home directory is unavailable"))?;
    let specs: Vec<_> = insights::REGISTRY
        .iter()
        .filter(|spec| spec.id != "docker.data")
        .filter(|spec| filter.is_none_or(|id| spec.id == id))
        .filter(|spec| insights::detect(spec, &probe))
        .collect();
    if specs.is_empty() {
        return Ok(());
    }
    let mut views: Vec<_> = specs
        .into_iter()
        .map(|spec| InsightView {
            spec,
            candidates: Vec::new(),
            finished: false,
        })
        .collect();
    let mut next_sizer = 0;
    let mut active = Vec::new();
    fill_sizers(&views, &mut active, &mut next_sizer);

    let theme = Theme::tailrocks_phosphor();
    let mut overview_state = ListState::new(views.first().map(|view| view.spec.id));
    overview_state.enable_multi_select();
    let mut detail_state = ListState::<PathBuf>::new(None);
    detail_state.enable_multi_select();
    let mut preselected_overview = HashSet::new();
    let mut detail: Option<&'static str> = filter;
    let mut status_state = StatusBarState::default();
    let mut cleanup = CleanupFlow::new();
    let mut tick = 0_u64;

    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;

    'screen: loop {
        pump_sizers(&mut views, &mut active, &mut next_sizer);
        preselect_safe_overview(&views, &mut overview_state, &mut preselected_overview);
        match cleanup.poll() {
            CleanupPoll::Exit => break 'screen,
            CleanupPoll::Completed => {
                for view in &mut views {
                    view.candidates.retain(|candidate| candidate.path.exists());
                }
                if let Some(selection) = overview_state.selection_mut() {
                    selection.clear();
                }
                if let Some(selection) = detail_state.selection_mut() {
                    selection.clear();
                }
                preselected_overview.clear();
            }
            CleanupPoll::None => {}
        }

        let overview_rows = overview_rows(&views, &theme, !active.is_empty());
        let detail_rows = detail
            .and_then(|id| views.iter().find(|view| view.spec.id == id))
            .map_or_else(Vec::new, |view| candidate_rows(view, &theme));
        if detail.is_none() && overview_state.selected().is_none() {
            overview_state.select(overview_rows.first().map(|row| row.id));
        }
        if detail.is_some() && detail_state.selected().is_none() {
            detail_state.select(
                detail_rows
                    .iter()
                    .find(|row| row.enabled)
                    .map(|row| row.id.clone()),
            );
            preselect_detail(&views, detail, &mut detail_state);
        }
        let selected_spec = detail.or(overview_state.selected().copied());
        let explanation = selected_spec
            .and_then(insights::spec)
            .map_or("No cleanup category selected", |spec| spec.explain);
        let total_candidates: usize = views.iter().map(|view| view.candidates.len()).sum();
        let state_copy = if active.is_empty() {
            format!("{total_candidates} candidates · complete")
        } else {
            format!("{total_candidates} candidates · {} sizing", active.len())
        };
        let title = detail
            .and_then(insights::spec)
            .map_or("Cleanup insights", |spec| spec.title);

        terminal.draw(|frame| {
            let [
                header_area,
                progress_area,
                list_area,
                explanation_area,
                hints_area,
            ] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(2),
                Constraint::Length(2),
                Constraint::Length(1),
            ])
            .areas(frame.area());
            let left = [StatusSlot {
                id: HeaderSlot::Title,
                content: title,
                priority: 2,
                min_width: 8,
                enabled: true,
                style: theme.style(Role::Accent),
                hover_style: None,
            }];
            let right = [StatusSlot {
                id: HeaderSlot::State,
                content: &state_copy,
                priority: 1,
                min_width: 12,
                enabled: true,
                style: theme.style(Role::TextMuted),
                hover_style: None,
            }];
            frame.render_stateful_widget(
                &StatusBar::new(&left, &right, &theme),
                header_area,
                &mut status_state,
            );
            if active.is_empty() {
                frame.render_widget(
                    Paragraph::new("Sizing complete").style(theme.style(Role::Success)),
                    progress_area,
                );
            } else {
                frame.render_widget(
                    Progress::new(ProgressKind::Indeterminate { tick }, &theme)
                        .label("Sizing cleanup candidates"),
                    progress_area,
                );
            }
            let panel = Panel::new(&theme)
                .title(if detail.is_some() {
                    " Candidates "
                } else {
                    " Cleanup categories "
                })
                .emphasis(PanelEmphasis::Focused);
            let inner = panel.inner(list_area);
            frame.render_widget(&panel, list_area);
            if detail.is_some() {
                frame.render_stateful_widget(
                    &List::new(&detail_rows, &theme),
                    inner,
                    &mut detail_state,
                );
            } else {
                frame.render_stateful_widget(
                    &List::new(&overview_rows, &theme),
                    inner,
                    &mut overview_state,
                );
            }
            frame.render_widget(
                Paragraph::new(explanation).style(theme.style(Role::TextMuted)),
                explanation_area,
            );
            frame.render_widget(HintBar::new(HINTS, &theme).separator(" · "), hints_area);
            cleanup.render(frame, &theme, tick);
        })?;
        tick = tick.wrapping_add(1);

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let key = termrock::input::KeyEvent::from(key);
            if cleanup.is_open() {
                cleanup.handle_key(key);
                continue;
            }
            match key.code {
                KeyCode::Char('q') => break 'screen,
                KeyCode::Esc if detail.is_some() && filter.is_none() => {
                    detail = None;
                    detail_state = ListState::new(None);
                    detail_state.enable_multi_select();
                }
                KeyCode::Esc => break 'screen,
                KeyCode::Char('d') | KeyCode::Backspace => {
                    let items =
                        selected_cleanup_items(&views, detail, &overview_state, &detail_state);
                    if !items.is_empty() {
                        cleanup.open_confirmation(items);
                    }
                }
                _ if detail.is_some() => match detail_state.handle_key(&detail_rows, key) {
                    Outcome::Cancelled => {
                        if filter.is_none() {
                            detail = None;
                        } else {
                            break 'screen;
                        }
                    }
                    Outcome::Activated(path) => {
                        if let Some(selection) = detail_state.selection_mut() {
                            selection.toggle(&path);
                        }
                    }
                    Outcome::Ignored | Outcome::Changed => {}
                    _ => {}
                },
                _ => match overview_state.handle_key(&overview_rows, key) {
                    Outcome::Activated(id) => {
                        detail = Some(id);
                        detail_state = ListState::new(None);
                        detail_state.enable_multi_select();
                    }
                    Outcome::Cancelled => break 'screen,
                    Outcome::Ignored | Outcome::Changed => {}
                    _ => {}
                },
            }
        }
    }

    for sizer in active {
        sizer
            .handle
            .cancel
            .store(true, std::sync::atomic::Ordering::Release);
    }
    drop(terminal);
    session.restore()?;
    Ok(())
}

fn fill_sizers(views: &[InsightView], active: &mut Vec<ActiveSizer>, next: &mut usize) {
    while active.len() < MAX_SIZERS && *next < views.len() {
        active.push(ActiveSizer {
            view: *next,
            handle: insights::size(views[*next].spec),
        });
        *next += 1;
    }
}

fn pump_sizers(views: &mut [InsightView], active: &mut Vec<ActiveSizer>, next: &mut usize) {
    let mut index = 0;
    while index < active.len() {
        let mut finished = false;
        loop {
            match active[index].handle.events.try_recv() {
                Ok(SizeEvent::Candidate(candidate)) => {
                    views[active[index].view].candidates.push(candidate)
                }
                Ok(SizeEvent::Finished) | Err(mpsc::TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if finished {
            let view = active[index].view;
            views[view].finished = true;
            views[view].candidates.sort_by(|left, right| {
                right
                    .size
                    .cmp(&left.size)
                    .then_with(|| left.path.cmp(&right.path))
            });
            active.remove(index);
        } else {
            index += 1;
        }
    }
    fill_sizers(views, active, next);
}

fn safety_badge(safety: Safety) -> (&'static str, Role) {
    match safety {
        Safety::Rebuildable => ("safe — rebuilt on demand", Role::Success),
        Safety::CacheOldOnly => ("safe if old", Role::Warning),
        Safety::ReviewFirst => ("review first", Role::Warning),
    }
}

fn overview_rows(
    views: &[InsightView],
    theme: &Theme,
    sizing: bool,
) -> Vec<ListRow<'static, &'static str>> {
    views
        .iter()
        .map(|view| {
            let (badge, role) = safety_badge(view.spec.safety);
            let total = view
                .candidates
                .iter()
                .fold(0_u64, |sum, candidate| sum.saturating_add(candidate.size));
            ListRow {
                id: view.spec.id,
                label: Line::from(vec![
                    Span::raw(format!("{}  ", view.spec.title)),
                    Span::styled(badge, theme.style(role)),
                ]),
                trailing: Some(Line::styled(
                    if !view.finished && sizing {
                        format!("{} · sizing…", format_size(total, DECIMAL))
                    } else {
                        format_size(total, DECIMAL)
                    },
                    theme.style(Role::TextMuted),
                )),
                role: RowRole::Item,
                enabled: true,
            }
        })
        .collect()
}

fn candidate_rows(view: &InsightView, theme: &Theme) -> Vec<ListRow<'static, PathBuf>> {
    view.candidates
        .iter()
        .map(|candidate| {
            let name = candidate
                .path
                .file_name()
                .unwrap_or(candidate.path.as_os_str())
                .to_string_lossy();
            ListRow {
                id: candidate.path.clone(),
                label: Line::styled(
                    if candidate.eligible {
                        name.into_owned()
                    } else {
                        format!("{name}  · too recent")
                    },
                    theme.style(if candidate.eligible {
                        Role::Text
                    } else {
                        Role::TextMuted
                    }),
                ),
                trailing: Some(Line::styled(
                    format_size(candidate.size, DECIMAL),
                    theme.style(Role::TextMuted),
                )),
                role: RowRole::Item,
                enabled: candidate.eligible,
            }
        })
        .collect()
}

fn preselect_safe_overview(
    views: &[InsightView],
    state: &mut ListState<&'static str>,
    preselected: &mut HashSet<&'static str>,
) {
    let Some(selection) = state.selection_mut() else {
        return;
    };
    for view in views {
        if view.spec.safety != Safety::ReviewFirst
            && view.candidates.iter().any(|candidate| candidate.eligible)
            && preselected.insert(view.spec.id)
            && !selection.is_checked(&view.spec.id)
        {
            selection.toggle(&view.spec.id);
        }
    }
}

fn preselect_detail(
    views: &[InsightView],
    detail: Option<&'static str>,
    state: &mut ListState<PathBuf>,
) {
    let Some(view) = detail.and_then(|id| views.iter().find(|view| view.spec.id == id)) else {
        return;
    };
    if view.spec.safety == Safety::ReviewFirst {
        return;
    }
    if let Some(selection) = state.selection_mut() {
        for candidate in &view.candidates {
            if candidate.eligible && !selection.is_checked(&candidate.path) {
                selection.toggle(&candidate.path);
            }
        }
    }
}

fn selected_cleanup_items(
    views: &[InsightView],
    detail: Option<&'static str>,
    overview_state: &ListState<&'static str>,
    detail_state: &ListState<PathBuf>,
) -> Vec<CleanupItem> {
    let checked_paths: HashSet<_> = detail_state
        .selection()
        .map(|selection| selection.checked().iter().cloned().collect())
        .unwrap_or_default();
    let checked_insights: HashSet<_> = overview_state
        .selection()
        .map(|selection| selection.checked().iter().copied().collect())
        .unwrap_or_default();
    views
        .iter()
        .filter(|view| {
            detail.map_or_else(
                || checked_insights.contains(view.spec.id),
                |id| view.spec.id == id,
            )
        })
        .flat_map(|view| {
            view.candidates
                .iter()
                .filter(|candidate| {
                    candidate.eligible
                        && (detail.is_none() || checked_paths.contains(&candidate.path))
                })
                .map(|candidate| {
                    CleanupItem::new(candidate.path.clone(), candidate.size).with_policy(
                        CleanupPolicy {
                            skip_if_running: view.spec.skip_if_running,
                            stop_gradle: view.spec.id == "gradle.caches",
                        },
                    )
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn candidate(id: &'static str, path: &str, eligible: bool, safety: Safety) -> Candidate {
        Candidate {
            insight_id: id,
            path: path.into(),
            size: 10,
            modified: SystemTime::now(),
            eligible,
            safety,
        }
    }

    #[test]
    fn review_first_is_never_preselected() {
        let views = vec![InsightView {
            spec: insights::spec("maven.repository").unwrap(),
            candidates: vec![candidate(
                "maven.repository",
                "/tmp/repo",
                true,
                Safety::ReviewFirst,
            )],
            finished: true,
        }];
        let mut state = ListState::new(Some("maven.repository"));
        state.enable_multi_select();
        preselect_safe_overview(&views, &mut state, &mut HashSet::new());
        assert!(state.selection().unwrap().checked().is_empty());
    }

    #[test]
    fn recent_candidates_remain_rows_but_are_disabled() {
        let view = InsightView {
            spec: insights::spec("user.logs").unwrap(),
            candidates: vec![candidate(
                "user.logs",
                "/tmp/recent",
                false,
                Safety::Rebuildable,
            )],
            finished: true,
        };
        let rows = candidate_rows(&view, &Theme::tailrocks_phosphor());
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].enabled);
        assert!(rows[0].label.to_string().contains("too recent"));
    }

    #[test]
    fn selected_items_carry_per_insight_guard_policy() {
        let views = vec![InsightView {
            spec: insights::spec("xcode.derived-data").unwrap(),
            candidates: vec![candidate(
                "xcode.derived-data",
                "/tmp/derived",
                true,
                Safety::Rebuildable,
            )],
            finished: true,
        }];
        let mut overview = ListState::new(Some("xcode.derived-data"));
        overview.enable_multi_select();
        overview
            .selection_mut()
            .unwrap()
            .toggle(&"xcode.derived-data");
        let mut detail = ListState::<PathBuf>::new(None);
        detail.enable_multi_select();
        let items = selected_cleanup_items(&views, None, &overview, &detail);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].policy.skip_if_running, Some("Xcode"));
    }
}
