use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    text::{Line, Span, Text},
};
use std::{collections::HashSet, sync::mpsc};
use termrock::{
    input::KeyCode,
    interaction::Outcome,
    keymap::{KeyBinding, KeyChord, Keymap, Visibility},
    layout::centered_rect,
    scroll::{DialogScroll, max_line_width},
    style::{Density, DesignSystem as Theme, PanelChrome, Role},
    widgets::{
        Action as DialogAction, Backdrop, ChoiceDialog, ChoiceDialogState, Dialog, List, ListRow,
        ListState, Panel, RowRole, Severity, StatusBar, StatusBarState, StatusSlot, TextInput,
        TextInputOutcome, TextInputState, Toast, Validation, Viewport, render_hint_bar,
    },
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    frecency::{FrecencyStore, now_epoch_secs},
    model::{ActionSpec, Danger, GroupSpec},
    providers::{self, ScanEvent},
    search::{SearchHit, search_with_history},
};

const CURRENT_FOLDER_GROUP_ID: &str = "current-folder";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ActionId {
    Separator(String),
    Action { id: String, recent: bool },
}

#[derive(Default)]
pub struct Menu {
    pub groups: Vec<GroupSpec>,
    provider_orders: Vec<usize>,
    provider_ids: Vec<&'static str>,
    warnings: Vec<String>,
}

impl Menu {
    #[cfg(test)]
    fn from_groups(groups: Vec<GroupSpec>) -> Self {
        let provider_orders = (0..groups.len()).collect();
        Self {
            groups,
            provider_orders,
            provider_ids: Vec::new(),
            warnings: Vec::new(),
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
        self.deduplicate_actions();
    }

    fn deduplicate_actions(&mut self) {
        let mut seen = HashSet::new();
        for group in &mut self.groups {
            group.actions.retain(|action| {
                if seen.insert(action.id.clone()) {
                    true
                } else {
                    self.warnings.push(format!(
                        "action id `{}` collides with an earlier provider; earlier action wins",
                        action.id
                    ));
                    false
                }
            });
        }
    }

    fn action(&self, id: &str) -> Option<&ActionSpec> {
        self.groups
            .iter()
            .flat_map(|group| &group.actions)
            .find(|action| action.id == id)
    }

    fn row_action(&self, row: &ActionId) -> Option<&ActionSpec> {
        match row {
            ActionId::Action { id, .. } => self.action(id),
            ActionId::Separator(_) => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum MenuKey {
    Navigate,
    Run,
    Preview,
    Quit,
    ModeToggle,
}

#[derive(Clone, Copy, PartialEq)]
enum HeaderSlot {
    Product,
    Context,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ConfirmChoice {
    Cancel,
    Run,
}

struct PendingConfirm {
    action_id: String,
    state: ChoiceDialogState<ConfirmChoice>,
}

static MENU_BINDINGS: &[KeyBinding<MenuKey>] = &[
    KeyBinding::borrowed(
        &[KeyChord::plain(KeyCode::Up), KeyChord::plain(KeyCode::Down)],
        MenuKey::Navigate,
        Some("navigate"),
        Visibility::Shown,
        Some("↑↓"),
    ),
    KeyBinding::borrowed(
        &[KeyChord::plain(KeyCode::Enter)],
        MenuKey::Run,
        Some("run"),
        Visibility::Shown,
        Some("⏎"),
    ),
    KeyBinding::borrowed(
        &[
            KeyChord::plain(KeyCode::Tab),
            KeyChord::plain(KeyCode::Right),
        ],
        MenuKey::Preview,
        Some("preview"),
        Visibility::Shown,
        Some("tab"),
    ),
    KeyBinding::borrowed(
        &[KeyChord::plain(KeyCode::Esc)],
        MenuKey::Quit,
        Some("clear/quit"),
        Visibility::Shown,
        Some("esc"),
    ),
    KeyBinding::borrowed(
        &[KeyChord::ctrl(KeyCode::Char('o'))],
        MenuKey::ModeToggle,
        Some("browser"),
        Visibility::Shown,
        Some("ctrl-o"),
    ),
];
static MENU_KEYMAP: Keymap<MenuKey> = Keymap::from_static(MENU_BINDINGS);

#[cfg(test)]
fn menu_rows(menu: &Menu, query: &str, theme: &Theme) -> Vec<ListRow<'static, ActionId>> {
    menu_rows_with_history(menu, query, theme, &FrecencyStore::default(), 0)
}

fn menu_rows_with_history(
    menu: &Menu,
    query: &str,
    theme: &Theme,
    history: &FrecencyStore,
    now: u64,
) -> Vec<ListRow<'static, ActionId>> {
    let query = query.trim();
    if query.is_empty() {
        let mut rows = Vec::new();
        let mut recent = menu
            .groups
            .iter()
            .enumerate()
            .flat_map(|(group_index, group)| {
                group
                    .actions
                    .iter()
                    .enumerate()
                    .map(move |(action_index, action)| {
                        (
                            action,
                            history.score(&action.id, now),
                            group_index,
                            action_index,
                        )
                    })
            })
            .filter(|(_, score, _, _)| *score > 0.05)
            .collect::<Vec<_>>();
        recent.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.2.cmp(&right.2))
                .then_with(|| left.3.cmp(&right.3))
        });
        recent.truncate(5);
        if !recent.is_empty() {
            rows.push(ListRow {
                id: ActionId::Separator("recent".to_owned()),
                label: Line::styled("Recent", theme.style(Role::TextMuted)),
                leading: None,
                secondary: None,
                badge: None,
                shortcut: None,
                status: None,
                actions: None,
                custom: None,
                trailing: None,
                role: RowRole::Separator,
                enabled: false,
                loading: false,
            });
            rows.extend(recent.into_iter().map(|(action, _, _, _)| ListRow {
                id: ActionId::Action {
                    id: action.id.clone(),
                    recent: true,
                },
                label: Line::raw(action.label.clone()),
                leading: None,
                secondary: None,
                badge: None,
                shortcut: None,
                status: None,
                actions: None,
                custom: None,
                trailing: None,
                role: RowRole::Item,
                enabled: true,
                loading: false,
            }));
        }
        let mut previous_group = None;
        for group in empty_query_groups(menu) {
            if previous_group != Some(group.id.as_str()) {
                rows.push(ListRow {
                    id: ActionId::Separator(group.id.to_owned()),
                    label: Line::styled(group.title.clone(), theme.style(Role::TextMuted)),
                    leading: None,
                    secondary: None,
                    badge: None,
                    shortcut: None,
                    status: None,
                    actions: None,
                    custom: None,
                    trailing: None,
                    role: RowRole::Separator,
                    enabled: false,
                    loading: false,
                });
                previous_group = Some(group.id.as_str());
            }
            rows.extend(group.actions.iter().map(|action| ListRow {
                id: ActionId::Action {
                    id: action.id.clone(),
                    recent: false,
                },
                label: Line::raw(action.label.clone()),
                leading: None,
                secondary: None,
                badge: None,
                shortcut: None,
                status: None,
                actions: None,
                custom: None,
                trailing: None,
                role: RowRole::Item,
                enabled: true,
                loading: false,
            }));
        }
        return rows;
    }

    search_with_history(&menu.groups, query, history, now)
        .into_iter()
        .map(|hit| {
            let group = &menu.groups[hit.group];
            let action = &group.actions[hit.action];
            ListRow {
                id: ActionId::Action {
                    id: action.id.clone(),
                    recent: false,
                },
                label: highlighted_hit(group, &hit, theme),
                leading: None,
                secondary: None,
                badge: None,
                shortcut: None,
                status: None,
                actions: None,
                custom: None,
                trailing: None,
                role: RowRole::Item,
                enabled: true,
                loading: false,
            }
        })
        .collect()
}

fn empty_query_groups(menu: &Menu) -> impl Iterator<Item = &GroupSpec> {
    menu.groups
        .iter()
        .filter(|group| group.id == CURRENT_FOLDER_GROUP_ID)
        .chain(
            menu.groups
                .iter()
                .filter(|group| group.id != CURRENT_FOLDER_GROUP_ID),
        )
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

fn preview_lines(menu: &Menu, selected: Option<&ActionId>, theme: &Theme) -> Vec<Line<'static>> {
    let Some(action) = selected.and_then(|id| menu.row_action(id)) else {
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
    action.danger == Danger::Destructive || action.confirm
}

pub async fn run() -> anyhow::Result<()> {
    crate::tui::session::run().await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LauncherOutcome {
    Stay,
    Quit,
    Run,
}

pub(crate) struct LauncherState {
    search_state: TextInputState,
    list_state: ListState<ActionId>,
    preview_scroll: DialogScroll,
    preview_focused: bool,
    status_state: StatusBarState<HeaderSlot>,
    pending_confirm: Option<PendingConfirm>,
    rows: Vec<ListRow<'static, ActionId>>,
    preview: Vec<Line<'static>>,
    preview_width: usize,
    preview_viewport: (usize, usize),
}

pub(crate) struct LauncherRenderContext<'a> {
    pub(crate) menu: &'a Menu,
    pub(crate) scanning: bool,
    pub(crate) cwd: &'a str,
    pub(crate) history: &'a FrecencyStore,
    pub(crate) theme: &'a Theme,
    pub(crate) tokens: &'a Theme,
    pub(crate) confirm_actions: &'a [DialogAction<'static, ConfirmChoice>; 2],
}

impl LauncherState {
    pub(crate) fn new() -> Self {
        Self {
            search_state: TextInputState::new("").with_allow_empty(true),
            list_state: ListState::new(None),
            preview_scroll: DialogScroll::new(),
            preview_focused: false,
            status_state: StatusBarState::default(),
            pending_confirm: None,
            rows: Vec::new(),
            preview: Vec::new(),
            preview_width: 0,
            preview_viewport: (0, 0),
        }
    }

    pub(crate) fn render(
        &mut self,
        terminal: &mut ratatui::Terminal<CrosstermBackend<&mut std::io::Stdout>>,
        context: LauncherRenderContext<'_>,
    ) -> anyhow::Result<()> {
        let LauncherRenderContext {
            menu,
            scanning,
            cwd,
            history,
            theme,
            tokens,
            confirm_actions,
        } = context;
        self.rows = menu_rows_with_history(
            menu,
            self.search_state.value(),
            theme,
            history,
            now_epoch_secs(),
        );
        if !self
            .rows
            .iter()
            .any(|row| row.enabled && self.list_state.selected().is_some_and(|id| id == &row.id))
        {
            self.list_state.select(
                self.rows
                    .iter()
                    .find(|row| row.enabled)
                    .map(|row| row.id.clone()),
            );
        }
        self.preview = preview_lines(menu, self.list_state.selected(), theme);
        self.preview_width = max_line_width(&self.preview);

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
                cwd.to_owned()
            };
            let left_slots = [StatusSlot {
                id: HeaderSlot::Product,
                content: " holla ",
                priority: 2,
                min_width: 0,
                enabled: true,
                region: termrock::widgets::StatusRegion::Left,
                kind: termrock::widgets::StatusKind::Text,
                glyph: None,
                style_explicit: true,
                style: theme.style(Role::Accent),
                hover_style: None,
            }];
            let right_slots = [StatusSlot {
                id: HeaderSlot::Context,
                content: &context,
                priority: 1,
                min_width: 8,
                enabled: !context.is_empty(),
                region: termrock::widgets::StatusRegion::Left,
                kind: termrock::widgets::StatusKind::Text,
                glyph: None,
                style_explicit: true,
                style: theme.style(Role::TextMuted),
                hover_style: None,
            }];
            frame.render_stateful_widget(
                &StatusBar::new(&left_slots, &right_slots, theme).alpha(1.0),
                header_area,
                &mut self.status_state,
            );
            frame.render_stateful_widget(
                &TextInput::new("Search", theme)
                    .placeholder("Search actions…")
                    .validation(Validation::Valid),
                search_area,
                &mut self.search_state,
            );

            let list_panel =
                Panel::new(tokens)
                    .title(" holla ")
                    .emphasis(if self.preview_focused {
                        PanelChrome::Normal
                    } else {
                        PanelChrome::Focused
                    });
            let list_inner = list_panel.inner(list_area);
            frame.render_widget(&list_panel, list_area);
            frame.render_stateful_widget(
                &List::new(&self.rows, tokens).focused(!self.preview_focused),
                list_inner,
                &mut self.list_state,
            );

            self.preview_viewport = (
                usize::from(preview_area.height.saturating_sub(2)),
                usize::from(preview_area.width.saturating_sub(2)),
            );
            frame.render_stateful_widget(
                &Viewport::new(&self.preview, theme)
                    .title("Preview")
                    .emphasis(if self.preview_focused {
                        PanelChrome::Focused
                    } else {
                        PanelChrome::Normal
                    })
                    .content_style(theme.style(Role::Text)),
                preview_area,
                &mut self.preview_scroll,
            );
            render_hint_bar(frame, footer_area, &MENU_KEYMAP.hint_spans(), theme);
            if let Some(warning) = menu.warnings.last() {
                frame.render_widget(Toast::new(theme, warning, Severity::Warning), frame.area());
            }

            if let Some(pending) = self.pending_confirm.as_mut()
                && let Some(action) = menu.action(&pending.action_id)
            {
                let mut body = vec![
                    Line::styled(action.description.clone(), theme.style(Role::Text)),
                    Line::raw(""),
                ];
                body.extend(
                    action
                        .preview
                        .lines()
                        .map(|line| Line::styled(line.to_owned(), theme.style(Role::TextMuted))),
                );
                body.push(Line::raw(""));
                let warning = Line::styled(
                    if action.danger == Danger::Destructive {
                        "Warning: this deletes local data."
                    } else {
                        "This action explicitly requires confirmation."
                    },
                    theme.style(Role::Warning),
                );
                body.push(warning.clone());
                frame.render_widget(Backdrop::default(), frame.area());
                let area = centered_rect(68, 18, frame.area());
                let max_body_lines = usize::from(area.height.saturating_sub(4));
                if body.len() > max_body_lines {
                    body.truncate(max_body_lines.saturating_sub(1));
                    if max_body_lines > 0 {
                        body.push(warning);
                    }
                }
                frame.render_stateful_widget(
                    &ChoiceDialog::new(
                        Dialog::new("Confirm action", Text::from(body), theme)
                            .style(theme.style(Role::Text))
                            .emphasis(PanelChrome::Focused),
                        confirm_actions,
                    )
                    .gap("  "),
                    area,
                    &mut pending.state,
                );
            }
        })?;
        Ok(())
    }

    pub(crate) fn handle_key(
        &mut self,
        key: termrock::input::KeyEvent,
        menu: &Menu,
        confirm_actions: &[DialogAction<'static, ConfirmChoice>; 2],
    ) -> LauncherOutcome {
        if let Some(pending) = self.pending_confirm.as_mut() {
            match pending.state.handle_key(confirm_actions, key) {
                Outcome::Activated(ConfirmChoice::Run) => return LauncherOutcome::Run,
                Outcome::Activated(ConfirmChoice::Cancel) | Outcome::Cancelled => {
                    self.pending_confirm = None;
                }
                Outcome::Ignored | Outcome::Changed => {}
                _ => {}
            }
            return LauncherOutcome::Stay;
        }

        if key.code == termrock::input::KeyCode::Esc {
            if self.search_state.value().is_empty() {
                return LauncherOutcome::Quit;
            }
            self.search_state = TextInputState::new("").with_allow_empty(true);
            self.list_state.select(None);
            return LauncherOutcome::Stay;
        }

        if matches!(
            key.code,
            termrock::input::KeyCode::Char(_)
                | termrock::input::KeyCode::Backspace
                | termrock::input::KeyCode::Delete
        ) {
            if self.search_state.handle_key(key) == TextInputOutcome::Changed {
                self.list_state.select(None);
                self.preview_scroll = DialogScroll::new();
            }
            return LauncherOutcome::Stay;
        }

        if self.preview_focused {
            if matches!(
                key.code,
                termrock::input::KeyCode::Tab | termrock::input::KeyCode::Left
            ) {
                self.preview_focused = false;
            } else {
                self.preview_scroll.handle_key(
                    key,
                    self.preview.len(),
                    self.preview_viewport.0,
                    self.preview_width,
                    self.preview_viewport.1,
                );
            }
            return LauncherOutcome::Stay;
        }

        match self.list_state.handle_key(&self.rows, key) {
            Outcome::Activated(id) => {
                if let Some(action) = menu.row_action(&id) {
                    if needs_confirmation(action) {
                        self.pending_confirm = Some(PendingConfirm {
                            action_id: action.id.clone(),
                            state: ChoiceDialogState::new(Some(ConfirmChoice::Cancel)),
                        });
                    } else {
                        return LauncherOutcome::Run;
                    }
                }
            }
            Outcome::Changed => self.preview_scroll = DialogScroll::new(),
            Outcome::Cancelled => return LauncherOutcome::Quit,
            Outcome::Ignored => {
                if matches!(
                    key.code,
                    termrock::input::KeyCode::Tab | termrock::input::KeyCode::Right
                ) {
                    self.preview_focused = true;
                }
            }
            _ => {}
        }
        LauncherOutcome::Stay
    }

    pub(crate) fn selected_action(&self) -> Option<String> {
        self.list_state.selected().and_then(menu_action_id)
    }

    pub(crate) fn search(&self) -> &str {
        self.search_state.value()
    }
}

fn menu_action_id(id: &ActionId) -> Option<String> {
    match id {
        ActionId::Action { id, .. } => Some(id.clone()),
        ActionId::Separator(_) => None,
    }
}

pub(crate) struct LauncherExit {
    menu: Menu,
    selected_action: Option<String>,
    search: String,
    history: FrecencyStore,
    history_receiver: mpsc::Receiver<FrecencyStore>,
    history_loaded: bool,
}

pub(crate) async fn run_with_session(
    terminal: &mut ratatui::Terminal<CrosstermBackend<&mut std::io::Stdout>>,
    events: &mut EventStream,
    theme: &Theme,
) -> anyhow::Result<LauncherExit> {
    let tokens = theme.clone().density(Density::default());
    let mut menu = Menu::default();
    let mut scans: Option<mpsc::Receiver<ScanEvent>> = None;
    let mut scanning = true;
    let mut launcher = LauncherState::new();
    let mut history = FrecencyStore::default();
    let (history_sender, history_receiver) = mpsc::sync_channel(1);
    let mut history_sender = Some(history_sender);
    let mut history_loaded = false;
    let cwd = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let mut browser: Option<crate::tui::browser::Browser> = None;
    let mut mode = crate::tui::session::Mode::Launcher;
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

    let selected_action = loop {
        if let Some(browser) = browser.as_mut()
            && mode == crate::tui::session::Mode::Browser
        {
            browser.tick();
        }
        if !history_loaded {
            match history_receiver.try_recv() {
                Ok(loaded) => {
                    history = loaded;
                    history_loaded = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => history_loaded = true,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(scans) = scans.as_mut() {
            loop {
                match scans.try_recv() {
                    Ok(ScanEvent::Group {
                        provider_index,
                        provider_id,
                        group,
                    }) => menu.insert_scanned_group(provider_index, provider_id, group),
                    Ok(ScanEvent::Warning(warning)) => {
                        menu.warnings.push(warning);
                    }
                    Ok(ScanEvent::Finished) | Err(mpsc::TryRecvError::Disconnected) => {
                        scanning = false;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
        }

        if mode == crate::tui::session::Mode::Browser {
            if let Some(browser) = browser.as_mut() {
                terminal.draw(|frame| browser.render(frame, theme))?;
            }
        } else {
            launcher.render(
                terminal,
                LauncherRenderContext {
                    menu: &menu,
                    scanning,
                    cwd: &cwd,
                    history: &history,
                    theme,
                    tokens: &tokens,
                    confirm_actions: &confirm_actions,
                },
            )?;
        }

        if scans.is_none() {
            // First frame is now visible. Only then may blocking provider work start.
            scans = Some(providers::spawn_scans());
            if let Some(sender) = history_sender.take() {
                std::thread::Builder::new()
                    .name("holla-history-load".into())
                    .spawn(move || {
                        let _ = sender.send(FrecencyStore::load());
                    })?;
            }
            continue;
        }

        let event = tokio::select! {
            event = events.next() => match event {
                Some(event) => Some(event?),
                None => break None,
            },
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => None,
        };
        if let Some(Event::Key(key)) = event {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let key = termrock::input::KeyEvent::from(key);
            if crate::tui::session::is_mode_toggle(key) {
                mode = mode.toggle();
                if mode == crate::tui::session::Mode::Browser && browser.is_none() {
                    browser = Some(crate::tui::browser::Browser::new(
                        std::env::current_dir()?,
                        None,
                    ));
                }
                continue;
            }
            if mode == crate::tui::session::Mode::Browser {
                if let Some(browser) = browser.as_mut() {
                    match browser.handle_key(key) {
                        crate::tui::browser::BrowserOutcome::ReturnToLauncher => {
                            mode = crate::tui::session::Mode::Launcher;
                        }
                        crate::tui::browser::BrowserOutcome::Quit => break None,
                        crate::tui::browser::BrowserOutcome::Stay => {}
                    }
                }
                continue;
            }
            match launcher.handle_key(key, &menu, &confirm_actions) {
                LauncherOutcome::Run => break launcher.selected_action(),
                LauncherOutcome::Quit => break None,
                LauncherOutcome::Stay => {}
            }
        }
    };

    Ok(LauncherExit {
        menu,
        selected_action,
        search: launcher.search().to_owned(),
        history,
        history_receiver,
        history_loaded,
    })
}

pub(crate) async fn execute(exit: LauncherExit) -> anyhow::Result<()> {
    let LauncherExit {
        menu,
        selected_action,
        search,
        mut history,
        history_receiver,
        history_loaded,
    } = exit;
    if let Some(id) = selected_action
        && let Some(action) = menu.action(&id)
    {
        if !history_loaded {
            history =
                tokio::task::spawn_blocking(move || history_receiver.recv().unwrap_or_default())
                    .await
                    .unwrap_or_default();
        }
        let now = now_epoch_secs();
        history.record(&id, &search, now);
        let save = tokio::task::spawn_blocking(move || history.save(now));
        let action_result = (action.run)().await;
        match save.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("holla: could not save action history: {error}"),
            Err(error) => eprintln!("holla: action history task failed: {error}"),
        }
        action_result?;
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

    #[test]
    fn launcher_state_survives_mode_round_trip() {
        let mut launcher = LauncherState::new();
        for character in "cargo".chars() {
            assert_eq!(
                launcher
                    .search_state
                    .handle_key(termrock::input::KeyEvent::new(
                        KeyCode::Char(character),
                        termrock::input::KeyModifiers::NONE,
                    )),
                TextInputOutcome::Changed
            );
        }
        launcher.preview_focused = true;
        let mut mode = crate::tui::session::Mode::Launcher;

        mode = mode.toggle();
        mode = mode.toggle();

        assert_eq!(mode, crate::tui::session::Mode::Launcher);
        assert_eq!(launcher.search(), "cargo");
        assert!(launcher.preview_focused);
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
    fn empty_probe_still_builds_always_available_groups() {
        let menu = menu(&Probe::empty());
        assert_eq!(menu.groups.len(), 3);
        assert_eq!(menu.groups[0].id, "find");
        assert_eq!(menu.groups[1].id, "disk");
        assert_eq!(menu.groups[2].id, "cleanup");
    }

    #[test]
    fn docker_probe_builds_system_cleanup_actions() {
        let mut probe = Probe::empty();
        probe.docker = true;
        let menu = menu(&probe);

        assert!(labels(&menu, "System").contains(&"docker: stop all containers"));
        assert!(labels(&menu, "System").contains(&"docker: clean everything"));
        assert!(labels(&menu, "System").contains(&"docker: prune builder cache"));
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
    fn compose_logs_action_is_bounded() {
        let mut probe = Probe::empty();
        probe.docker = true;
        probe.has_docker_compose = true;
        let menu = menu(&probe);
        let action = actions(&menu, "Current folder")
            .into_iter()
            .find(|action| action.label == "compose: logs")
            .expect("compose logs action");

        assert_eq!(action.description, "Show recent service logs");
        assert_eq!(action.preview, "$ docker compose logs --tail 200");
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

    fn row_id(id: &ActionId) -> String {
        match id {
            ActionId::Separator(id) => format!("separator:{id}"),
            ActionId::Action { id, recent: true } => format!("recent:{id}"),
            ActionId::Action { id, recent: false } => id.clone(),
        }
    }

    #[test]
    fn menu_rows_flatten_groups_and_actions_with_stable_ids() {
        let menu = Menu::from_groups(vec![
            GroupSpec {
                id: "first".into(),
                title: "First".into(),
                actions: vec![
                    test_action("one", "one", Danger::Safe),
                    test_action("two", "two", Danger::Safe),
                ],
            },
            GroupSpec {
                id: "second".into(),
                title: "Second".into(),
                actions: vec![
                    test_action("three", "three", Danger::Safe),
                    test_action("four", "four", Danger::Safe),
                ],
            },
        ]);
        let rows = menu_rows(&menu, "", &Theme::phosphor());

        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0].role, RowRole::Separator);
        assert!(!rows[0].enabled);
        assert_eq!(row_id(&rows[1].id), "one");
        assert_eq!(rows[1].role, RowRole::Item);
        assert!(rows[1].enabled);
        assert_eq!(row_id(&rows[5].id), "four");
    }

    #[test]
    fn whitespace_only_query_keeps_grouped_projection() {
        let menu = Menu::from_groups(vec![GroupSpec {
            id: "first".into(),
            title: "First".into(),
            actions: vec![test_action("one", "one", Danger::Safe)],
        }]);

        let rows = menu_rows(&menu, " \t ", &Theme::phosphor());

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].role, RowRole::Separator);
        assert_eq!(row_id(&rows[1].id), "one");
    }

    #[test]
    fn adjacent_provider_contributions_share_one_group_header() {
        let menu = Menu::from_groups(vec![
            GroupSpec {
                id: "system".into(),
                title: "System".into(),
                actions: vec![test_action("one", "one", Danger::Safe)],
            },
            GroupSpec {
                id: "system".into(),
                title: "System".into(),
                actions: vec![test_action("two", "two", Danger::Safe)],
            },
        ]);

        let rows = menu_rows(&menu, "", &Theme::phosphor());

        assert_eq!(
            rows.iter()
                .filter(|row| row.role == RowRole::Separator)
                .count(),
            1
        );
    }

    #[test]
    fn empty_query_puts_current_folder_rows_before_global_provider_rows() {
        let menu = Menu::from_groups(vec![
            GroupSpec {
                id: "find".into(),
                title: "Find".into(),
                actions: vec![test_action("find.files", "find", Danger::Safe)],
            },
            GroupSpec {
                id: CURRENT_FOLDER_GROUP_ID.into(),
                title: "Current folder".into(),
                actions: vec![test_action("git.status", "git: status", Danger::Safe)],
            },
            GroupSpec {
                id: "cargo-project".into(),
                title: "Cargo".into(),
                actions: vec![test_action("cargo.test", "cargo: test", Danger::Safe)],
            },
        ]);
        let rows = menu_rows(&menu, "", &Theme::phosphor());
        let separators = rows
            .iter()
            .filter_map(|row| match &row.id {
                ActionId::Separator(id) => Some(id.as_str()),
                ActionId::Action { .. } => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            separators,
            [CURRENT_FOLDER_GROUP_ID, "find", "cargo-project"]
        );
        assert_eq!(row_id(&rows[1].id), "git.status");
        assert_eq!(row_id(&rows[3].id), "find.files");
        assert_eq!(row_id(&rows[5].id), "cargo.test");
    }

    #[test]
    fn recent_projection_precedes_normal_groups_and_keeps_normal_action() {
        let menu = Menu::from_groups(vec![GroupSpec {
            id: "first".into(),
            title: "First".into(),
            actions: vec![test_action("one", "one", Danger::Safe)],
        }]);
        let mut history = FrecencyStore::default();
        history.record("one", "", 100);

        let rows = menu_rows_with_history(&menu, "", &Theme::phosphor(), &history, 100);

        assert_eq!(row_id(&rows[0].id), "separator:recent");
        assert_eq!(row_id(&rows[1].id), "recent:one");
        assert_eq!(row_id(&rows[2].id), "separator:first");
        assert_eq!(row_id(&rows[3].id), "one");
        assert_ne!(rows[1].id, rows[3].id);
        assert_eq!(menu.row_action(&rows[1].id).unwrap().id, "one");
    }

    #[test]
    fn recent_projection_is_limited_to_five_actions() {
        let menu = Menu::from_groups(vec![GroupSpec {
            id: "first".into(),
            title: "First".into(),
            actions: (0..6)
                .map(|index| test_action(&format!("action-{index}"), "action", Danger::Safe))
                .collect(),
        }]);
        let mut history = FrecencyStore::default();
        for index in 0..6 {
            history.record(&format!("action-{index}"), "", 100 + index);
        }

        let rows = menu_rows_with_history(&menu, "", &Theme::phosphor(), &history, 106);

        assert_eq!(
            rows.iter()
                .take_while(|row| row_id(&row.id) != "separator:first")
                .count(),
            6
        );
    }

    #[test]
    fn fuzzy_highlight_keeps_combining_graphemes_atomic() {
        let menu = Menu::from_groups(vec![GroupSpec {
            id: "unicode".into(),
            title: "Unicode".into(),
            actions: vec![test_action("accent", "e\u{301}clair", Danger::Safe)],
        }]);

        let rows = menu_rows(&menu, "e", &Theme::phosphor());

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
        let theme = Theme::phosphor();
        let menu = Menu::from_groups(vec![GroupSpec {
            id: "docker".into(),
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
                id: "late".into(),
                title: "Late".into(),
                actions: vec![],
            },
        );
        menu.insert_scanned_group(
            1,
            "early-provider",
            GroupSpec {
                id: "early".into(),
                title: "Early".into(),
                actions: vec![],
            },
        );

        assert_eq!(menu.provider_orders, [1, 4]);
        assert_eq!(menu.provider_ids, ["early-provider", "late-provider"]);
        assert_eq!(menu.groups[0].id, "early");
    }
}
