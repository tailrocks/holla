use crossterm::event::{self, Event, KeyEventKind};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    text::{Line, Span, Text},
};
use std::{collections::HashSet, time::Duration};
use termrock::{
    interaction::Outcome,
    keymap::{KeyBinding, KeyChord, Keymap, LogicalKey, Visibility},
    runtime::{StdSubscription, Subscription, SubscriptionPoll},
    scroll::DialogScroll,
    style::{Role, Theme},
    widgets::{
        Action as DialogAction, Backdrop, ChoiceDialog, ChoiceDialogState, Dialog, List,
        ListOutcome, ListRow, ListState, Panel, PanelEmphasis, RowRole, StatusBar, StatusBarState,
        StatusSlot, TextInput, TextInputOutcome, TextInputState, Validation, Viewport,
        render_hint_bar,
    },
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    providers::{self, ScanEvent},
    search::{SearchHit, search},
};

type ActionId = String;

#[derive(Default)]
pub struct Menu {
    pub groups: Vec<GroupSpec>,
    provider_orders: Vec<usize>,
    provider_ids: Vec<&'static str>,
}

impl Menu {
    #[cfg(test)]
    fn from_groups(groups: Vec<GroupSpec>) -> Self {
        let provider_orders = (0..groups.len()).collect();
        Self {
            groups,
            provider_orders,
            provider_ids: Vec::new(),
        }
    }

    fn insert_scanned_group(
        &mut self,
        provider_index: usize,
        provider_id: &'static str,
        group: GroupSpec,
    ) {
        let position = self
            .provider_orders
            .partition_point(|index| *index < provider_index);
        self.provider_orders.insert(position, provider_index);
        self.provider_ids.insert(position, provider_id);
        self.groups.insert(position, group);
    }

    fn action(&self, id: &str) -> Option<&ActionSpec> {
        self.groups
            .iter()
            .flat_map(|group| &group.actions)
            .find(|action| action.id == id)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum MenuKey {
    Navigate,
    Run,
    Preview,
    Quit,
}

#[derive(Clone, Copy, PartialEq)]
enum HeaderSlot {
    Product,
    Context,
}

#[derive(Clone, Copy, PartialEq)]
enum ConfirmChoice {
    Cancel,
    Run,
}

struct PendingConfirm {
    action_id: String,
    state: ChoiceDialogState<ConfirmChoice>,
}

static MENU_KEYMAP: Keymap<MenuKey> = Keymap::new(&[
    KeyBinding {
        chords: &[
            KeyChord::plain(LogicalKey::Up),
            KeyChord::plain(LogicalKey::Down),
        ],
        action: MenuKey::Navigate,
        hint: Some("navigate"),
        visibility: Visibility::Shown,
        glyph: Some("↑↓"),
    },
    KeyBinding {
        chords: &[KeyChord::plain(LogicalKey::Enter)],
        action: MenuKey::Run,
        hint: Some("run"),
        visibility: Visibility::Shown,
        glyph: Some("⏎"),
    },
    KeyBinding {
        chords: &[
            KeyChord::plain(LogicalKey::Tab),
            KeyChord::plain(LogicalKey::Right),
        ],
        action: MenuKey::Preview,
        hint: Some("preview"),
        visibility: Visibility::Shown,
        glyph: Some("tab"),
    },
    KeyBinding {
        chords: &[KeyChord::plain(LogicalKey::Esc)],
        action: MenuKey::Quit,
        hint: Some("clear/quit"),
        visibility: Visibility::Shown,
        glyph: Some("esc"),
    },
]);

fn menu_rows(menu: &Menu, query: &str, theme: &Theme) -> Vec<ListRow<'static, ActionId>> {
    let query = query.trim();
    if query.is_empty() {
        let mut rows = Vec::new();
        let mut previous_group = None;
        for group in &menu.groups {
            if previous_group != Some(group.id) {
                rows.push(ListRow {
                    id: format!("separator:{}", group.id),
                    label: Line::styled(group.title.clone(), theme.style(Role::TextMuted)),
                    role: RowRole::Separator,
                    enabled: false,
                });
                previous_group = Some(group.id);
            }
            rows.extend(group.actions.iter().map(|action| ListRow {
                id: action.id.clone(),
                label: Line::raw(action.label.clone()),
                role: RowRole::Item,
                enabled: true,
            }));
        }
        return rows;
    }

    search(&menu.groups, query)
        .into_iter()
        .map(|hit| {
            let group = &menu.groups[hit.group];
            let action = &group.actions[hit.action];
            ListRow {
                id: action.id.clone(),
                label: highlighted_hit(group, &hit, theme),
                role: RowRole::Item,
                enabled: true,
            }
        })
        .collect()
}

fn highlighted_hit(group: &GroupSpec, hit: &SearchHit, theme: &Theme) -> Line<'static> {
    let action = &group.actions[hit.action];
    let matched: HashSet<_> = hit
        .indices
        .iter()
        .filter_map(|index| usize::try_from(*index).ok())
        .collect();
    let keywords = action.keywords.join(" ");
    let group_start = 0;
    let label_start = group.title.graphemes(true).count() + 1;
    let keywords_start = label_start + action.label.graphemes(true).count() + 1;
    let description_start = keywords_start + keywords.graphemes(true).count() + 1;
    let mut spans = Vec::new();

    push_highlighted(
        &mut spans,
        &group.title,
        group_start,
        &matched,
        Role::TextMuted,
        theme,
    );
    spans.push(Span::styled(" › ", theme.style(Role::TextMuted)));
    push_highlighted(
        &mut spans,
        &action.label,
        label_start,
        &matched,
        Role::Text,
        theme,
    );
    if !keywords.is_empty() {
        spans.push(Span::styled(" · ", theme.style(Role::TextMuted)));
        push_highlighted(
            &mut spans,
            &keywords,
            keywords_start,
            &matched,
            Role::TextMuted,
            theme,
        );
    }
    if !action.description.is_empty() {
        spans.push(Span::styled(" — ", theme.style(Role::TextMuted)));
        push_highlighted(
            &mut spans,
            &action.description,
            description_start,
            &matched,
            Role::TextMuted,
            theme,
        );
    }
    Line::from(spans)
}

fn push_highlighted(
    spans: &mut Vec<Span<'static>>,
    value: &str,
    start: usize,
    matched: &HashSet<usize>,
    base_role: Role,
    theme: &Theme,
) {
    spans.extend(value.graphemes(true).enumerate().map(|(index, grapheme)| {
        Span::styled(
            grapheme.to_owned(),
            theme.style(if matched.contains(&(start + index)) {
                Role::Accent
            } else {
                base_role
            }),
        )
    }));
}

fn preview_lines(menu: &Menu, selected: Option<&str>, theme: &Theme) -> Vec<Line<'static>> {
    let Some(action) = selected.and_then(|id| menu.action(id)) else {
        return vec![Line::styled(
            if menu.groups.is_empty() {
                "Scanning providers…"
            } else {
                "No action selected"
            },
            theme.style(Role::TextMuted),
        )];
    };
    let mut lines = vec![
        Line::styled(action.label.clone(), theme.style(Role::Accent)),
        Line::raw(""),
        Line::styled(action.description.clone(), theme.style(Role::Text)),
        Line::raw(""),
        Line::styled("Command", theme.style(Role::TextMuted)),
    ];
    lines.extend(
        action
            .preview
            .lines()
            .map(|line| Line::styled(line.to_owned(), theme.style(Role::TextMuted))),
    );
    lines
}

fn needs_confirmation(action: &ActionSpec) -> bool {
    action.danger == Danger::Destructive
}

pub async fn run() -> anyhow::Result<()> {
    let theme = Theme::tailrocks_phosphor();
    let mut menu = Menu::default();
    let mut scans: Option<StdSubscription<ScanEvent>> = None;
    let mut scanning = true;
    let mut search_state = TextInputState::new("").with_allow_empty(true);
    let mut list_state = ListState::new(None::<String>);
    let mut preview_scroll = DialogScroll::new();
    let mut preview_focused = false;
    let mut status_state = StatusBarState::default();
    let mut pending_confirm: Option<PendingConfirm> = None;
    let cwd = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let confirm_actions = [
        DialogAction {
            id: ConfirmChoice::Cancel,
            label: "Cancel",
            enabled: true,
            style: None,
        },
        DialogAction {
            id: ConfirmChoice::Run,
            label: "Run",
            enabled: true,
            style: Some(theme.style(Role::Danger)),
        },
    ];

    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;

    let selected_action =
        loop {
            if let Some(scans) = scans.as_mut() {
                loop {
                    match scans.poll_next() {
                        SubscriptionPoll::Ready(ScanEvent::Group {
                            provider_index,
                            provider_id,
                            group,
                        }) => menu.insert_scanned_group(provider_index, provider_id, group),
                        SubscriptionPoll::Ready(ScanEvent::Finished) | SubscriptionPoll::Closed => {
                            scanning = false;
                            break;
                        }
                        SubscriptionPoll::Pending => break,
                    }
                }
            }

            let rows = menu_rows(&menu, search_state.value(), &theme);
            if !rows.iter().any(|row| {
                row.enabled && list_state.selected.as_ref().is_some_and(|id| id == &row.id)
            }) {
                list_state.select(
                    rows.iter()
                        .find(|row| row.enabled)
                        .map(|row| row.id.clone()),
                );
            }
            list_state.focused = !preview_focused;
            let preview = preview_lines(&menu, list_state.selected.as_deref(), &theme);
            let preview_width = termrock::max_line_width(&preview);
            let mut preview_viewport = (0usize, 0usize);

            terminal.draw(|frame| {
                let [header_area, search_area, body_area, footer_area] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(0),
                    Constraint::Length(1),
                ])
                .areas(frame.area());
                let [list_area, preview_area] =
                    Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                        .areas(body_area);
                let context = if scanning {
                    if cwd.is_empty() {
                        "scanning…".to_owned()
                    } else {
                        format!("scanning… · {cwd}")
                    }
                } else {
                    cwd.clone()
                };
                let left_slots = [StatusSlot {
                    id: HeaderSlot::Product,
                    content: " holla ",
                    priority: 2,
                    min_width: 0,
                    enabled: true,
                    style: theme.style(Role::Accent),
                    hover_style: None,
                }];
                let right_slots = [StatusSlot {
                    id: HeaderSlot::Context,
                    content: &context,
                    priority: 1,
                    min_width: 8,
                    enabled: !context.is_empty(),
                    style: theme.style(Role::TextMuted),
                    hover_style: None,
                }];
                frame.render_stateful_widget(
                    &StatusBar {
                        left: &left_slots,
                        right: &right_slots,
                        style: theme.style(Role::Surface),
                        alpha: 1.0,
                    },
                    header_area,
                    &mut status_state,
                );
                frame.render_stateful_widget(
                    &TextInput {
                        label: "Search",
                        placeholder: "Search actions…",
                        validation: Validation::Valid,
                        theme: &theme,
                    },
                    search_area,
                    &mut search_state,
                );

                let list_panel = Panel::new(&theme)
                    .title(" holla ")
                    .emphasis(if preview_focused {
                        PanelEmphasis::Normal
                    } else {
                        PanelEmphasis::Focused
                    });
                let list_inner = list_panel.inner(list_area);
                frame.render_widget(&list_panel, list_area);
                frame.render_stateful_widget(
                    &List {
                        rows: &rows,
                        theme: &theme,
                    },
                    list_inner,
                    &mut list_state,
                );

                preview_viewport = (
                    usize::from(preview_area.height.saturating_sub(2)),
                    usize::from(preview_area.width.saturating_sub(2)),
                );
                frame.render_stateful_widget(
                    &Viewport {
                        lines: &preview,
                        title: Some("Preview"),
                        content_style: theme.style(Role::Text),
                        border_style: theme.style(if preview_focused {
                            Role::BorderFocused
                        } else {
                            Role::Border
                        }),
                        title_style: theme.style(Role::Text),
                        scroll_track_style: theme.style(Role::ScrollTrack),
                        scroll_thumb_style: theme.style(Role::ScrollThumb),
                    },
                    preview_area,
                    &mut preview_scroll,
                );
                render_hint_bar(frame, footer_area, &MENU_KEYMAP.hint_spans());

                if let Some(pending) = pending_confirm.as_mut()
                    && let Some(action) = menu.action(&pending.action_id)
                {
                    let mut body = vec![
                        Line::styled(action.description.clone(), theme.style(Role::Text)),
                        Line::raw(""),
                    ];
                    body.extend(
                        action.preview.lines().map(|line| {
                            Line::styled(line.to_owned(), theme.style(Role::TextMuted))
                        }),
                    );
                    body.push(Line::raw(""));
                    let warning = Line::styled(
                        "Warning: this deletes local data.",
                        theme.style(Role::Warning),
                    );
                    body.push(warning.clone());
                    frame.render_widget(&Backdrop::default(), frame.area());
                    let area = termrock::centered_rect(68, 18, frame.area());
                    let max_body_lines = usize::from(area.height.saturating_sub(4));
                    if body.len() > max_body_lines {
                        body.truncate(max_body_lines.saturating_sub(1));
                        if max_body_lines > 0 {
                            body.push(warning);
                        }
                    }
                    frame.render_stateful_widget(
                        &ChoiceDialog {
                            dialog: Dialog {
                                title: "Confirm destructive action",
                                body: Text::from(body),
                                style: theme.style(Role::Text),
                                theme: &theme,
                                emphasis: PanelEmphasis::Focused,
                            },
                            actions: &confirm_actions,
                            gap: "  ",
                        },
                        area,
                        &mut pending.state,
                    );
                }
            })?;

            if scans.is_none() {
                // First frame is now visible. Only then may blocking provider work start.
                scans = Some(StdSubscription(providers::spawn_scans()));
                continue;
            }

            if event::poll(Duration::from_millis(50))?
                && let Event::Key(key) = event::read()?
            {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let key = termrock::input::KeyEvent::from(key);
                if let Some(pending) = pending_confirm.as_mut() {
                    match pending.state.handle_key(key, &confirm_actions) {
                        Outcome::Activated(ConfirmChoice::Run) => {
                            break Some(pending.action_id.clone());
                        }
                        Outcome::Activated(ConfirmChoice::Cancel) | Outcome::Cancelled => {
                            pending_confirm = None;
                        }
                        Outcome::Ignored | Outcome::Changed => {}
                    }
                    continue;
                }

                if key.code == termrock::input::KeyCode::Esc {
                    if search_state.value().is_empty() {
                        break None;
                    }
                    search_state = TextInputState::new("").with_allow_empty(true);
                    list_state.select(None);
                    continue;
                }

                if matches!(
                    key.code,
                    termrock::input::KeyCode::Char(_)
                        | termrock::input::KeyCode::Backspace
                        | termrock::input::KeyCode::Delete
                ) {
                    if search_state.handle_key(key) == TextInputOutcome::Changed {
                        list_state.select(None);
                        preview_scroll = DialogScroll::new();
                    }
                    continue;
                }

                if preview_focused {
                    if matches!(
                        key.code,
                        termrock::input::KeyCode::Tab | termrock::input::KeyCode::Left
                    ) {
                        preview_focused = false;
                    } else {
                        preview_scroll.handle_key(
                            key,
                            preview.len(),
                            preview_viewport.0,
                            preview_width,
                            preview_viewport.1,
                        );
                    }
                    continue;
                }

                match list_state.handle_key(&rows, key) {
                    ListOutcome::Activated(id) => {
                        if let Some(action) = menu.action(&id) {
                            if needs_confirmation(action) {
                                pending_confirm = Some(PendingConfirm {
                                    action_id: id,
                                    state: ChoiceDialogState::new(Some(ConfirmChoice::Cancel)),
                                });
                            } else {
                                break Some(id);
                            }
                        }
                    }
                    ListOutcome::Changed => preview_scroll = DialogScroll::new(),
                    ListOutcome::Cancelled => break None,
                    ListOutcome::Ignored => {
                        if matches!(
                            key.code,
                            termrock::input::KeyCode::Tab | termrock::input::KeyCode::Right
                        ) {
                            preview_focused = true;
                        }
                    }
                }
            }
        };

    drop(terminal);
    session.restore()?;
    if let Some(id) = selected_action
        && let Some(action) = menu.action(&id)
    {
        (action.run)().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::ActionFuture,
        probe::{MiseTask, Probe},
        providers::groups_from_probe,
    };

    fn menu(probe: &Probe) -> Menu {
        Menu::from_groups(groups_from_probe(probe))
    }

    fn actions<'a>(menu: &'a Menu, title: &str) -> Vec<&'a ActionSpec> {
        menu.groups
            .iter()
            .filter(|group| group.title == title)
            .flat_map(|group| &group.actions)
            .collect()
    }

    fn labels<'a>(menu: &'a Menu, title: &str) -> Vec<&'a str> {
        actions(menu, title)
            .into_iter()
            .map(|action| action.label.as_str())
            .collect()
    }

    #[test]
    fn empty_probe_builds_empty_menu() {
        assert!(menu(&Probe::empty()).groups.is_empty());
    }

    #[test]
    fn docker_probe_builds_system_cleanup_actions() {
        let mut probe = Probe::empty();
        probe.docker = true;
        let menu = menu(&probe);

        assert!(labels(&menu, "System").contains(&"docker: stop all containers"));
        assert!(labels(&menu, "System").contains(&"docker: clean everything"));
    }

    #[test]
    fn git_repo_builds_current_folder_actions() {
        let mut probe = Probe::empty();
        probe.git = true;
        probe.in_git_repo = true;
        let menu = menu(&probe);

        assert!(labels(&menu, "Current folder").contains(&"git: pull"));
        assert!(labels(&menu, "Current folder").contains(&"git: push"));
        assert!(labels(&menu, "Current folder").contains(&"git: status"));
    }

    #[test]
    fn mise_task_builds_action_with_command_preview() {
        let mut probe = Probe::empty();
        probe.mise_tasks.push(MiseTask {
            name: "build".into(),
            description: "Build app".into(),
        });
        let menu = menu(&probe);
        let action = actions(&menu, "Current folder")
            .into_iter()
            .find(|action| action.label == "mise: build")
            .expect("mise action");

        assert_eq!(action.id, "mise.task.build");
        assert_eq!(action.preview, "$ mise run build");
    }

    #[test]
    fn multiple_child_repositories_build_repo_group() {
        let mut probe = Probe::empty();
        probe.git = true;
        probe.child_git_repos = vec!["beta".into(), "alpha".into()];

        assert_eq!(
            labels(&menu(&probe), "Repos in this folder"),
            [
                "git: pull all repos",
                "git: push all repos",
                "git: status all repos",
                "git: push all remotes",
            ]
        );
    }

    #[test]
    fn single_child_repository_does_not_build_repo_group() {
        let mut probe = Probe::empty();
        probe.git = true;
        probe.child_git_repos = vec!["alpha".into()];

        assert!(actions(&menu(&probe), "Repos in this folder").is_empty());
    }

    #[test]
    fn omz_directory_builds_upgrade_action() {
        let mut probe = Probe::empty();
        probe.omz_dir = Some("/tmp/.oh-my-zsh".into());
        let menu = menu(&probe);
        let action = actions(&menu, "System")
            .into_iter()
            .find(|action| action.label == "upgrade: oh-my-zsh")
            .expect("oh-my-zsh action");

        assert_eq!(action.preview, "$ sh ~/.oh-my-zsh/tools/upgrade.sh");
    }

    #[test]
    fn missing_omz_directory_omits_upgrade_action() {
        assert!(
            actions(&menu(&Probe::empty()), "System")
                .into_iter()
                .all(|action| action.label != "upgrade: oh-my-zsh")
        );
    }

    #[test]
    fn compose_logs_action_follows_until_cancelled() {
        let mut probe = Probe::empty();
        probe.docker = true;
        probe.has_docker_compose = true;
        let menu = menu(&probe);
        let action = actions(&menu, "Current folder")
            .into_iter()
            .find(|action| action.label == "compose: logs (follow)")
            .expect("compose logs action");

        assert_eq!(action.description, "Follow service logs until cancelled");
        assert_eq!(action.preview, "$ docker compose logs -f");
        assert_eq!(action.danger, Danger::Safe);
    }

    #[test]
    fn idea_cleanup_is_destructive() {
        let mut probe = Probe::empty();
        probe.has_idea_dir = true;
        let menu = menu(&probe);
        let action = menu.action("idea.clean").expect("idea clean action");

        assert_eq!(action.danger, Danger::Destructive);
    }

    #[test]
    fn every_action_id_is_nonempty_and_unique() {
        let mut probe = Probe::empty();
        probe.git = true;
        probe.docker = true;
        probe.gradle = true;
        probe.mise = true;
        probe.brew = true;
        probe.amp = true;
        probe.idea = true;
        probe.in_git_repo = true;
        probe.has_docker_compose = true;
        probe.has_gradle_build = true;
        probe.child_git_repos = vec!["alpha".into(), "beta".into()];
        probe.mise_tasks = vec![
            MiseTask {
                name: "build".into(),
                description: String::new(),
            },
            MiseTask {
                name: "test".into(),
                description: String::new(),
            },
        ];
        let menu = menu(&probe);
        let ids: Vec<_> = menu
            .groups
            .iter()
            .flat_map(|group| &group.actions)
            .map(|action| action.id.as_str())
            .collect();
        let unique: HashSet<_> = ids.iter().copied().collect();

        assert!(ids.iter().all(|id| !id.is_empty()));
        assert_eq!(ids.len(), unique.len());
    }

    fn test_action(id: &str, label: &str, danger: Danger) -> ActionSpec {
        let run = || -> ActionFuture { Box::pin(async { Ok(()) }) };
        ActionSpec::new(id, label, "", "", &[], danger, run)
    }

    fn searchable_action(
        id: &str,
        label: &str,
        description: &str,
        keywords: &'static [&'static str],
    ) -> ActionSpec {
        let run = || -> ActionFuture { Box::pin(async { Ok(()) }) };
        ActionSpec::new(id, label, description, "", keywords, Danger::Safe, run)
    }

    fn has_accent(row: &ListRow<'_, ActionId>, theme: &Theme) -> bool {
        row.label
            .spans
            .iter()
            .any(|span| span.style == theme.style(Role::Accent))
    }

    #[test]
    fn menu_rows_flatten_groups_and_actions_with_stable_ids() {
        let menu = Menu::from_groups(vec![
            GroupSpec {
                id: "first",
                title: "First".into(),
                actions: vec![
                    test_action("one", "one", Danger::Safe),
                    test_action("two", "two", Danger::Safe),
                ],
            },
            GroupSpec {
                id: "second",
                title: "Second".into(),
                actions: vec![
                    test_action("three", "three", Danger::Safe),
                    test_action("four", "four", Danger::Safe),
                ],
            },
        ]);
        let rows = menu_rows(&menu, "", &Theme::default());

        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0].role, RowRole::Separator);
        assert!(!rows[0].enabled);
        assert_eq!(rows[1].id, "one");
        assert_eq!(rows[1].role, RowRole::Item);
        assert!(rows[1].enabled);
        assert_eq!(rows[5].id, "four");
    }

    #[test]
    fn whitespace_only_query_keeps_grouped_projection() {
        let menu = Menu::from_groups(vec![GroupSpec {
            id: "first",
            title: "First".into(),
            actions: vec![test_action("one", "one", Danger::Safe)],
        }]);

        let rows = menu_rows(&menu, " \t ", &Theme::default());

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].role, RowRole::Separator);
        assert_eq!(rows[1].id, "one");
    }

    #[test]
    fn adjacent_provider_contributions_share_one_group_header() {
        let menu = Menu::from_groups(vec![
            GroupSpec {
                id: "system",
                title: "System".into(),
                actions: vec![test_action("one", "one", Danger::Safe)],
            },
            GroupSpec {
                id: "system",
                title: "System".into(),
                actions: vec![test_action("two", "two", Danger::Safe)],
            },
        ]);

        let rows = menu_rows(&menu, "", &Theme::default());

        assert_eq!(
            rows.iter()
                .filter(|row| row.role == RowRole::Separator)
                .count(),
            1
        );
    }

    #[test]
    fn fuzzy_highlight_keeps_combining_graphemes_atomic() {
        let menu = Menu::from_groups(vec![GroupSpec {
            id: "unicode",
            title: "Unicode".into(),
            actions: vec![test_action("accent", "e\u{301}clair", Danger::Safe)],
        }]);

        let rows = menu_rows(&menu, "e", &Theme::default());

        assert!(
            rows[0]
                .label
                .spans
                .iter()
                .any(|span| span.content == "e\u{301}")
        );
    }

    #[test]
    fn group_and_keyword_matches_have_visible_accents() {
        let theme = Theme::default();
        let menu = Menu::from_groups(vec![GroupSpec {
            id: "docker",
            title: "Docker".into(),
            actions: vec![searchable_action(
                "docker.stop",
                "stop containers",
                "Stop running containers",
                &["cleanup"],
            )],
        }]);

        assert!(has_accent(&menu_rows(&menu, "dock", &theme)[0], &theme));
        assert!(has_accent(&menu_rows(&menu, "cleanup", &theme)[0], &theme));
    }

    #[test]
    fn destructive_actions_are_the_only_actions_requiring_confirmation() {
        assert!(!needs_confirmation(&test_action(
            "safe",
            "safe",
            Danger::Safe
        )));
        assert!(!needs_confirmation(&test_action(
            "mutating",
            "mutating",
            Danger::Mutating
        )));
        assert!(needs_confirmation(&test_action(
            "destructive",
            "destructive",
            Danger::Destructive
        )));
    }

    #[test]
    fn docker_clean_all_is_destructive() {
        let mut probe = Probe::empty();
        probe.docker = true;
        let menu = menu(&probe);
        let action = menu
            .action("docker.clean-all")
            .expect("docker clean action");

        assert_eq!(action.danger, Danger::Destructive);
    }

    #[test]
    fn out_of_order_scan_arrivals_are_inserted_in_provider_order() {
        let mut menu = Menu::default();
        menu.insert_scanned_group(
            4,
            "late-provider",
            GroupSpec {
                id: "late",
                title: "Late".into(),
                actions: vec![],
            },
        );
        menu.insert_scanned_group(
            1,
            "early-provider",
            GroupSpec {
                id: "early",
                title: "Early".into(),
                actions: vec![],
            },
        );

        assert_eq!(menu.provider_orders, [1, 4]);
        assert_eq!(menu.provider_ids, ["early-provider", "late-provider"]);
        assert_eq!(menu.groups[0].id, "early");
    }
}
