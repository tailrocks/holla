use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fmt, fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, bail};
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::Modifier,
    text::Line,
    widgets::Paragraph,
};
use termrock::{
    input::KeyCode,
    layout::centered_rect,
    scroll::{DialogScroll, max_line_width},
    style::{Density, DesignSystem as Theme, PanelChrome, Role},
    widgets::{
        Backdrop, FileEntry, FileEntryKind, FilePicker, FilePickerMode, FilePickerOutcome,
        FilePickerPane, FilePickerState, FilePreview, List, ListRow, ListState, Panel, StatusBar,
        StatusBarState, StatusSlot, TextInput, TextInputOutcome, TextInputState, Validation,
        Viewport,
    },
};
use tokio::{sync::mpsc, task};

use crate::{find::FileIndex, tui::file_preview};

const JUMP_RESULT_LIMIT: usize = 50;
const JUMP_DIALOG_WIDTH: u16 = 92;
const JUMP_DIALOG_HEIGHT: u16 = 20;
const MIN_BROWSER_WIDTH: u16 = 64;
const MIN_BROWSER_HEIGHT: u16 = 12;

/// Opens the folder browser at the process working directory.
///
/// # Errors
///
/// Returns an error when the working directory is unavailable or terminal setup,
/// drawing, input, or restoration fails.
pub async fn run() -> anyhow::Result<()> {
    let start = std::env::current_dir().context("current directory is unavailable")?;
    run_at(start).await
}

/// Opens the folder browser at `start`.
///
/// Files are accepted as starting points by opening their parent directory.
///
/// # Errors
///
/// Returns an error when `start` does not exist, has no parent when it is a file,
/// or terminal setup, drawing, input, or restoration fails.
pub async fn run_at(start: PathBuf) -> anyhow::Result<()> {
    let base = std::env::current_dir().context("current directory is unavailable")?;
    let target = task::spawn_blocking(move || jump_target(&start, &base))
        .await
        .context("initial path inspection task failed")??;
    let start = target.directory;
    let theme = Theme::phosphor();
    let mut browser = Browser::new(start, target.highlight);
    browser.shared_mode = false;

    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;
    let result = browser.run_loop(&mut terminal, &theme).await;

    drop(terminal);
    let restore_result = session.restore();
    result?;
    restore_result?;
    Ok(())
}

pub(crate) struct Browser {
    picker: FilePickerState,
    sender: mpsc::UnboundedSender<WorkerMessage>,
    receiver: mpsc::UnboundedReceiver<WorkerMessage>,
    pending_highlight: Option<PathBuf>,
    jump: Option<JumpDialog>,
    jump_index: Option<Arc<FileIndex>>,
    entry_paths: HashMap<String, HostEntry>,
    cwd: PathBuf,
    jump_index_roots: Vec<PathBuf>,
    pending_jump_index_cwd: Option<PathBuf>,
    jump_index_generation: u64,
    jump_generation: u64,
    pending_listing_path: Option<PathBuf>,
    status_state: StatusBarState<BrowserHeaderSlot>,
    preview_scroll: DialogScroll,
    preview_viewport: (usize, usize),
    preview_size: (usize, usize),
    shared_mode: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum BrowserHeaderSlot {
    Product,
    Mode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowserOutcome {
    Stay,
    ReturnToLauncher,
    Quit,
}

impl Browser {
    pub(crate) fn new(start: PathBuf, pending_highlight: Option<PathBuf>) -> Self {
        let cwd = path_text(&start);
        let mut picker = FilePickerState::new(cwd.clone())
            .with_mode(FilePickerMode::OpenAny)
            .with_preview(true);
        picker.set_focused(true);
        let (sender, receiver) = mpsc::unbounded_channel();
        let jump_index_roots = index_roots(&start);
        let jump_index = Arc::new(FileIndex::build(jump_index_roots.clone()));
        let mut browser = Self {
            picker,
            sender,
            receiver,
            pending_highlight,
            jump: None,
            jump_index: Some(jump_index),
            entry_paths: HashMap::new(),
            cwd: start.clone(),
            jump_index_roots,
            pending_jump_index_cwd: None,
            jump_index_generation: 0,
            jump_generation: 0,
            pending_listing_path: None,
            status_state: StatusBarState::default(),
            preview_scroll: DialogScroll::new(),
            preview_viewport: (0, 0),
            preview_size: (0, 0),
            shared_mode: true,
        };
        let outcome = browser.picker.request_list(cwd);
        browser.handle_picker_outcome(outcome);
        browser
    }

    async fn run_loop(
        &mut self,
        terminal: &mut ratatui::Terminal<CrosstermBackend<&mut std::io::Stdout>>,
        theme: &Theme,
    ) -> anyhow::Result<()> {
        let mut events = EventStream::new();

        loop {
            self.tick();
            terminal.draw(|frame| self.render(frame, theme))?;

            tokio::select! {
                worker = self.receiver.recv() => {
                    if let Some(worker) = worker {
                        self.apply_worker_message(worker);
                    }
                }
                event = events.next() => {
                    let Some(event) = event else {
                        break;
                    };
                    let event = event.context("terminal event stream failed")?;
                    if let Event::Key(key) = event
                        && key.kind == KeyEventKind::Press
                        && self.handle_key(termrock::input::KeyEvent::from(key)) == BrowserOutcome::Quit
                    {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn render(&mut self, frame: &mut ratatui::Frame<'_>, theme: &Theme) {
        let tokens = theme.clone().density(Density::default());
        if frame.area().width < MIN_BROWSER_WIDTH || frame.area().height < MIN_BROWSER_HEIGHT {
            render_small_terminal(frame, theme);
            if let Some(jump) = self.jump.as_mut() {
                render_jump_dialog(frame, jump, theme, &tokens);
            }
            return;
        }

        let [header, context, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        let left_slots = [StatusSlot {
            id: BrowserHeaderSlot::Product,
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
            id: BrowserHeaderSlot::Mode,
            content: "browser",
            priority: 1,
            min_width: 8,
            enabled: true,
            region: termrock::widgets::StatusRegion::Left,
            kind: termrock::widgets::StatusKind::Text,
            glyph: None,
            style_explicit: true,
            style: theme.style(Role::TextMuted),
            hover_style: None,
        }];
        frame.render_stateful_widget(
            &StatusBar::new(&left_slots, &right_slots, theme).alpha(1.0),
            header,
            &mut self.status_state,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("Browse · {}", self.cwd.display()),
                theme.style(Role::TextMuted),
            )),
            context,
        );

        let [list_area, preview_area] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(body);
        self.picker
            .set_presentation(FilePickerState::presentation_for_bounds(list_area));
        frame.render_stateful_widget(
            &FilePicker::new(theme)
                .title("holla")
                .show_count(false)
                .show_preview(false)
                .show_breadcrumbs(false)
                .show_path(false)
                .show_status(true)
                .show_footer(false),
            list_area,
            &mut self.picker,
        );

        let preview_lines = preview_lines(self.picker.preview(), theme);
        self.preview_size = (preview_lines.len(), max_line_width(&preview_lines));
        self.preview_viewport = (
            usize::from(preview_area.height.saturating_sub(2)),
            usize::from(preview_area.width.saturating_sub(2)),
        );
        frame.render_stateful_widget(
            &Viewport::new(&preview_lines, theme)
                .title("Preview")
                .emphasis(if self.picker.pane() == FilePickerPane::Preview {
                    PanelChrome::Focused
                } else {
                    PanelChrome::Normal
                })
                .content_style(theme.style(Role::Text)),
            preview_area,
            &mut self.preview_scroll,
        );
        let mode_hint = if footer.width < 96 {
            if self.shared_mode {
                "ctrl-o/esc"
            } else {
                "esc close"
            }
        } else if self.shared_mode {
            "ctrl-o commands  esc commands"
        } else {
            "esc close"
        };
        let footer_text = if footer.width < 96 {
            format!("↑↓ move  enter open  ← parent  g jump  tab preview  {mode_hint}")
        } else {
            format!("↑↓ select  ⏎/→ open  ←/backspace parent  g jump  tab preview  {mode_hint}")
        };
        frame.render_widget(
            Paragraph::new(Line::styled(footer_text, theme.style(Role::TextMuted))),
            footer,
        );
        if let Some(jump) = self.jump.as_mut() {
            render_jump_dialog(frame, jump, theme, &tokens);
        }
    }

    pub(crate) fn tick(&mut self) {
        while let Ok(worker) = self.receiver.try_recv() {
            self.apply_worker_message(worker);
        }
    }

    pub(crate) fn handle_key(&mut self, key: termrock::input::KeyEvent) -> BrowserOutcome {
        if self.jump.is_some() {
            self.handle_jump_key(key);
            return BrowserOutcome::Stay;
        }
        if self.shared_mode && key.code == KeyCode::Esc && key.modifiers.is_empty() {
            return BrowserOutcome::ReturnToLauncher;
        }
        if opens_jump_dialog(self.picker.pane(), key) {
            self.jump = Some(JumpDialog::new(self.cwd.clone()));
            self.request_jump_suggestions();
            return BrowserOutcome::Stay;
        }
        if key.code == KeyCode::Tab && key.modifiers.is_empty() {
            let outcome = match self.picker.pane() {
                FilePickerPane::List => {
                    let _ = self.picker.handle_key(key);
                    self.picker.handle_key(key)
                }
                FilePickerPane::Path => self.picker.handle_key(termrock::input::KeyEvent::new(
                    KeyCode::Esc,
                    termrock::input::KeyModifiers::NONE,
                )),
                FilePickerPane::Preview => self.picker.handle_key(key),
                _ => self.picker.handle_key(key),
            };
            self.handle_picker_outcome(outcome);
            return BrowserOutcome::Stay;
        }
        if self.picker.pane() == FilePickerPane::Preview {
            if matches!(key.code, KeyCode::Tab | KeyCode::Left | KeyCode::Esc) {
                let outcome = self.picker.handle_key(key);
                self.handle_picker_outcome(outcome);
            } else {
                let _ = self.preview_scroll.handle_key(
                    key,
                    self.preview_size.0,
                    self.preview_viewport.0,
                    self.preview_size.1,
                    self.preview_viewport.1,
                );
            }
            return BrowserOutcome::Stay;
        }
        let previous_cwd = self.cwd.clone();
        let outcome = self.picker.handle_key(key);
        if let FilePickerOutcome::ListRequested { path, .. } = &outcome
            && previous_cwd.parent() == Some(Path::new(path))
        {
            self.pending_highlight = Some(previous_cwd);
        }
        if matches!(outcome, FilePickerOutcome::Cancelled) {
            if self.shared_mode {
                return BrowserOutcome::ReturnToLauncher;
            }
            return BrowserOutcome::Quit;
        }
        self.handle_picker_outcome(outcome);
        BrowserOutcome::Stay
    }

    fn handle_jump_key(&mut self, key: termrock::input::KeyEvent) {
        let Some(jump) = self.jump.as_mut() else {
            return;
        };
        if key.modifiers.is_empty() && key.code == KeyCode::Esc {
            self.jump = None;
            return;
        }

        let rows = jump_rows(&jump.suggestions, &Theme::phosphor());
        if key.modifiers.is_empty() && key.code == KeyCode::Enter {
            let selected = jump.list.selected().and_then(|selected| {
                jump.suggestions
                    .iter()
                    .find(|suggestion| &suggestion.path == selected)
                    .map(|suggestion| suggestion.path.as_path())
            });
            let input = jump.input.value().to_owned();
            let cwd = jump.cwd.clone();
            let selected = selected.map(Path::to_path_buf);
            self.jump_generation = self.jump_generation.saturating_add(1);
            let generation = self.jump_generation;
            spawn_jump_target(self.sender.clone(), input, cwd, selected, generation);
            return;
        }
        if key.modifiers.is_empty() && matches!(key.code, KeyCode::Up | KeyCode::Down) {
            let _ = jump.list.handle_key(&rows, key);
            return;
        }
        if jump.input.handle_key(key) == TextInputOutcome::Changed {
            jump.error = None;
            jump.clear_suggestions();
            self.request_jump_suggestions();
        }
    }

    fn handle_picker_outcome(&mut self, outcome: FilePickerOutcome) {
        match outcome {
            FilePickerOutcome::ListRequested { path, generation } => {
                self.invalidate_preview();
                let path = self
                    .pending_listing_path
                    .take()
                    .filter(|pending| path_text(pending) == path)
                    .or_else(|| self.resolve_picker_path(&path))
                    .unwrap_or_else(|| PathBuf::from(path));
                accept_listing_request(&mut self.cwd, &mut self.entry_paths, &path);
                spawn_listing(self.sender.clone(), path, generation);
            }
            FilePickerOutcome::PreviewRequested { path, generation } => {
                let directory = self.picker_entry_is_directory(&path);
                if let Some(path) = self.resolve_actionable_picker_path(&path) {
                    self.request_preview(path, directory, generation);
                }
            }
            FilePickerOutcome::HighlightChanged { .. } => self.request_highlight_preview(),
            FilePickerOutcome::Confirmed { paths } => {
                if let Some(path) = paths.first()
                    && let Some(path) = self.resolve_actionable_picker_path(path)
                {
                    let generation = self.picker.preview_generation();
                    self.request_preview(path, false, generation);
                }
            }
            FilePickerOutcome::OpenDirectory { path } => {
                if let Some(path) = self.resolve_actionable_picker_path(&path) {
                    self.pending_listing_path = Some(path.clone());
                    let outcome = self.picker.request_list(path_text(&path));
                    self.handle_picker_outcome(outcome);
                }
            }
            FilePickerOutcome::Ignored
            | FilePickerOutcome::Changed
            | FilePickerOutcome::HoverChanged
            | FilePickerOutcome::SelectionChanged
            | FilePickerOutcome::Cancelled
            | FilePickerOutcome::PresentationChanged { .. } => {}
            FilePickerOutcome::FilterChanged => {
                self.invalidate_preview();
                self.request_highlight_preview();
            }
            _ => {}
        }
    }

    fn apply_worker_message(&mut self, message: WorkerMessage) {
        match message {
            WorkerMessage::Listing {
                generation,
                path,
                result,
            } => {
                let succeeded = result.is_ok();
                let applied = match result {
                    Ok(entries) => {
                        let (entries, entry_paths) = project_listing(generation, entries);
                        let applied =
                            self.picker
                                .apply_listing(generation, path_text(&path), entries, None);
                        if applied {
                            self.cwd = path.clone();
                            self.entry_paths = entry_paths;
                        }
                        applied
                    }
                    Err(error) => self.picker.apply_listing_error(generation, error),
                };
                if applied {
                    self.focus_pending_highlight(&path);
                    self.request_highlight_preview();
                    if succeeded {
                        self.refresh_jump_index(path);
                    }
                }
            }
            WorkerMessage::Preview {
                generation,
                path,
                preview,
            } => {
                let highlighted = self
                    .picker
                    .highlight()
                    .and_then(|entry| self.resolve_picker_path(&entry.path))
                    .is_some_and(|highlighted| highlighted == path);
                if highlighted {
                    let _ = self.picker.apply_preview(generation, preview);
                }
            }
            WorkerMessage::JumpSuggestions {
                generation,
                suggestions,
            } => {
                if generation == self.jump_generation
                    && let Some(jump) = self.jump.as_mut()
                {
                    jump.apply_suggestions(suggestions);
                }
            }
            WorkerMessage::JumpTarget { generation, result } => {
                if generation != self.jump_generation || self.jump.is_none() {
                    return;
                }
                match result {
                    Ok(target) => {
                        self.pending_highlight = target.highlight;
                        let outcome = self.picker.request_list(path_text(&target.directory));
                        self.jump = None;
                        self.handle_picker_outcome(outcome);
                    }
                    Err(error) => {
                        if let Some(jump) = self.jump.as_mut() {
                            jump.error = Some(error);
                        }
                    }
                }
            }
            WorkerMessage::JumpIndex {
                generation,
                cwd,
                roots,
                index,
            } => {
                if generation == self.jump_index_generation && self.cwd == cwd {
                    self.pending_jump_index_cwd = None;
                    self.jump_index_roots = roots;
                    if let Some(old) = self.jump_index.replace(index) {
                        retire_jump_index(old);
                    }
                    if self.jump.is_some() {
                        self.request_jump_suggestions();
                    }
                } else {
                    if generation == self.jump_index_generation {
                        self.pending_jump_index_cwd = None;
                    }
                    retire_jump_index(index);
                }
            }
        }
    }

    fn focus_pending_highlight(&mut self, cwd: &Path) {
        let Some(path) = self.pending_highlight.take() else {
            return;
        };
        if path.parent() != Some(cwd) {
            return;
        }
        focus_exact_path(&mut self.picker, &self.entry_paths, &path);
    }

    fn request_highlight_preview(&mut self) {
        let Some(entry) = self.picker.highlight() else {
            self.invalidate_preview();
            let generation = self.picker.preview_generation();
            let _ = self.picker.apply_preview(generation, FilePreview::new());
            return;
        };
        let Some(path) = self.resolve_actionable_picker_path(&entry.path) else {
            self.invalidate_preview();
            return;
        };
        let generation = self.picker.preview_generation();
        self.request_preview(path, entry.kind.is_dir(), generation);
    }

    fn request_preview(&mut self, path: PathBuf, directory: bool, generation: u64) {
        self.preview_scroll = DialogScroll::new();
        let _ = self.picker.apply_preview(
            generation,
            FilePreview::text(
                path.file_name().map_or_else(
                    || path_text(&path),
                    |name| name.to_string_lossy().into_owned(),
                ),
                ["Loading preview…".to_owned()],
            ),
        );
        spawn_preview(self.sender.clone(), path, generation, directory);
    }

    fn invalidate_preview(&mut self) {
        self.preview_scroll = DialogScroll::new();
        let generation = self.picker.preview_generation();
        let _ = self.picker.apply_preview(generation, FilePreview::new());
    }

    fn request_jump_suggestions(&mut self) {
        let Some(jump) = self.jump.as_ref() else {
            return;
        };
        self.jump_generation = self.jump_generation.saturating_add(1);
        spawn_jump_suggestions(
            self.sender.clone(),
            jump.input.value().to_owned(),
            jump.cwd.clone(),
            Arc::clone(self.jump_index.as_ref().expect("jump index is present")),
            self.jump_generation,
        );
    }

    fn refresh_jump_index(&mut self, cwd: PathBuf) {
        if self
            .jump_index_roots
            .iter()
            .any(|root| cwd.starts_with(root))
            || self.pending_jump_index_cwd.as_ref() == Some(&cwd)
        {
            return;
        }
        self.jump_index_generation = self.jump_index_generation.saturating_add(1);
        self.pending_jump_index_cwd = Some(cwd.clone());
        spawn_jump_index(
            self.sender.clone(),
            cwd.clone(),
            self.jump_index_generation,
            index_roots(&cwd),
        );
    }

    fn resolve_picker_path(&self, value: &str) -> Option<PathBuf> {
        self.entry_paths
            .get(value)
            .map(|entry| entry.path.clone())
            .or_else(|| {
                self.cwd
                    .ancestors()
                    .find(|path| path_text(path) == value)
                    .map(Path::to_path_buf)
            })
    }

    fn resolve_actionable_picker_path(&self, value: &str) -> Option<PathBuf> {
        self.entry_paths
            .get(value)
            .filter(|entry| entry.actionable)
            .map(|entry| entry.path.clone())
            .or_else(|| (value == self.picker.cwd()).then(|| self.cwd.clone()))
    }

    fn picker_entry_is_directory(&self, value: &str) -> bool {
        picker_entry_is_directory(&self.picker, value)
    }
}

fn render_small_terminal(frame: &mut ratatui::Frame<'_>, theme: &Theme) {
    let area = centered_rect(60, 3, frame.area());
    let lines = vec![
        Line::styled(
            format!("Holla browser needs at least {MIN_BROWSER_WIDTH}×{MIN_BROWSER_HEIGHT}"),
            theme.style(Role::TextStrong).add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            format!(
                "Current terminal: {}×{}",
                frame.area().width,
                frame.area().height
            ),
            theme.style(Role::TextMuted),
        ),
        Line::styled(
            "Resize the terminal, then retry",
            theme.style(Role::TextMuted),
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center),
        area,
    );
}

fn preview_lines<'a>(preview: Option<&'a FilePreview>, theme: &Theme) -> Vec<Line<'a>> {
    let Some(preview) = preview else {
        return vec![Line::styled("No preview", theme.style(Role::TextMuted))];
    };
    if let Some(error) = preview.error.as_deref() {
        return vec![Line::styled(error, theme.style(Role::Danger))];
    }

    let mut lines = Vec::with_capacity(preview.lines.len() + 1);
    if !preview.title.is_empty() {
        lines.push(Line::styled(
            preview.title.as_str(),
            theme.style(Role::TextStrong).add_modifier(Modifier::BOLD),
        ));
    }
    lines.extend(
        preview
            .lines
            .iter()
            .map(|line| Line::styled(line.as_str(), theme.style(Role::Text))),
    );
    if lines.is_empty() {
        lines.push(Line::styled("No preview", theme.style(Role::TextMuted)));
    }
    lines
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Some(index) = self.jump_index.take() {
            retire_jump_index(index);
        }
    }
}

#[derive(Clone)]
struct HostEntry {
    path: PathBuf,
    actionable: bool,
}

enum WorkerMessage {
    Listing {
        generation: u64,
        path: PathBuf,
        result: Result<Vec<HostEntryProjection>, String>,
    },
    Preview {
        generation: u64,
        path: PathBuf,
        preview: FilePreview,
    },
    JumpSuggestions {
        generation: u64,
        suggestions: Vec<JumpSuggestion>,
    },
    JumpTarget {
        generation: u64,
        result: Result<JumpTarget, String>,
    },
    JumpIndex {
        generation: u64,
        cwd: PathBuf,
        roots: Vec<PathBuf>,
        index: Arc<FileIndex>,
    },
}

fn opens_jump_dialog(pane: FilePickerPane, key: termrock::input::KeyEvent) -> bool {
    pane == FilePickerPane::List
        && key.modifiers.is_empty()
        && matches!(key.code, KeyCode::Char('g'))
}

fn spawn_jump_index(
    sender: mpsc::UnboundedSender<WorkerMessage>,
    cwd: PathBuf,
    generation: u64,
    roots: Vec<PathBuf>,
) {
    tokio::spawn(async move {
        let worker_roots = roots.clone();
        let result = task::spawn_blocking(move || Arc::new(FileIndex::build(worker_roots))).await;
        if let Ok(index) = result {
            let _ = sender.send(WorkerMessage::JumpIndex {
                generation,
                cwd,
                roots,
                index,
            });
        }
    });
}

fn retire_jump_index(index: Arc<FileIndex>) {
    task::spawn_blocking(move || drop(index));
}

fn spawn_jump_suggestions(
    sender: mpsc::UnboundedSender<WorkerMessage>,
    query: String,
    cwd: PathBuf,
    index: Arc<FileIndex>,
    generation: u64,
) {
    tokio::spawn(async move {
        let suggestions = task::spawn_blocking(move || jump_suggestions(&query, &cwd, &index))
            .await
            .unwrap_or_default();
        let _ = sender.send(WorkerMessage::JumpSuggestions {
            generation,
            suggestions,
        });
    });
}

fn spawn_jump_target(
    sender: mpsc::UnboundedSender<WorkerMessage>,
    input: String,
    cwd: PathBuf,
    selected: Option<PathBuf>,
    generation: u64,
) {
    tokio::spawn(async move {
        let result =
            task::spawn_blocking(move || accepted_jump_target(&input, &cwd, selected.as_deref()))
                .await
                .map_err(|error| format!("path inspection task failed: {error}"))
                .and_then(|result| result);
        let _ = sender.send(WorkerMessage::JumpTarget { generation, result });
    });
}

fn spawn_listing(sender: mpsc::UnboundedSender<WorkerMessage>, path: PathBuf, generation: u64) {
    tokio::spawn(async move {
        let worker_path = path.clone();
        let result = task::spawn_blocking(move || list_directory(&worker_path))
            .await
            .map_err(|error| format!("directory listing task failed: {error}"))
            .and_then(|result| result.map_err(|error| error.to_string()));
        let _ = sender.send(WorkerMessage::Listing {
            generation,
            path,
            result,
        });
    });
}

fn spawn_preview(
    sender: mpsc::UnboundedSender<WorkerMessage>,
    path: PathBuf,
    generation: u64,
    directory: bool,
) {
    tokio::spawn(async move {
        let worker_path = path.clone();
        let preview = task::spawn_blocking(move || {
            if directory {
                directory_preview(&worker_path)
            } else {
                file_preview::load(&worker_path).into_picker_preview()
            }
        })
        .await
        .unwrap_or_else(|error| FilePreview::error(format!("preview task failed: {error}")));
        let _ = sender.send(WorkerMessage::Preview {
            generation,
            path,
            preview,
        });
    });
}

trait IntoPickerPreview {
    fn into_picker_preview(self) -> FilePreview;
}

impl IntoPickerPreview for FilePreview {
    fn into_picker_preview(self) -> FilePreview {
        self
    }
}

impl<E> IntoPickerPreview for Result<FilePreview, E>
where
    E: fmt::Display,
{
    fn into_picker_preview(self) -> FilePreview {
        self.unwrap_or_else(|error| FilePreview::error(error.to_string()))
    }
}

fn list_directory(path: &Path) -> std::io::Result<Vec<HostEntryProjection>> {
    let mut entries = Vec::new();
    for result in fs::read_dir(path)? {
        let entry = match result {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        entries.push(project_entry(entry.path(), &entry.file_name()));
    }
    Ok(entries)
}

struct HostEntryProjection {
    entry: FileEntry,
    path: PathBuf,
    actionable: bool,
}

fn project_entry(path: PathBuf, name: &OsStr) -> HostEntryProjection {
    let display_name = name.to_string_lossy().into_owned();
    let display_path = path_text(&path);
    let hidden = display_name.starts_with('.');
    let symlink_metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            let entry = FileEntry::file(display_path.clone(), display_name, display_path)
                .kind(FileEntryKind::Other)
                .hidden(hidden)
                .error(error.to_string())
                .selectable(false);
            return HostEntryProjection {
                entry,
                path,
                actionable: false,
            };
        }
    };
    let target_result = if symlink_metadata.file_type().is_symlink() {
        Some(fs::metadata(&path))
    } else {
        None
    };
    let target_metadata = target_result
        .as_ref()
        .and_then(|result| result.as_ref().ok());
    let metadata = target_metadata.unwrap_or(&symlink_metadata);
    let kind = classify_entry(&symlink_metadata, target_metadata);
    let mut projected = if kind.is_dir() {
        FileEntry::directory(display_path.clone(), display_name, display_path)
    } else {
        FileEntry::file(display_path.clone(), display_name, display_path)
    }
    .kind(kind)
    .hidden(hidden);
    if !kind.is_dir() {
        projected = projected.size(metadata.len());
    }
    if let Ok(modified) = metadata.modified() {
        projected = projected.modified(format_modified(modified));
    }
    if let Some(Err(error)) = target_result {
        projected = projected
            .error(format!("broken symlink: {error}"))
            .selectable(false);
    }
    let actionable = projected.error.is_none() && (projected.selectable || projected.kind.is_dir());
    HostEntryProjection {
        entry: projected,
        path,
        actionable,
    }
}

fn project_listing(
    generation: u64,
    entries: Vec<HostEntryProjection>,
) -> (Vec<FileEntry>, HashMap<String, HostEntry>) {
    let mut paths = HashMap::with_capacity(entries.len());
    let entries = entries
        .into_iter()
        .enumerate()
        .map(|(index, projected)| {
            let identity = format!("\0holla:{generation}:{index}");
            paths.insert(
                identity.clone(),
                HostEntry {
                    path: projected.path,
                    actionable: projected.actionable,
                },
            );
            FileEntry {
                id: identity.clone(),
                path: identity,
                ..projected.entry
            }
        })
        .collect();
    (entries, paths)
}

fn focus_exact_path(
    picker: &mut FilePickerState,
    entry_paths: &HashMap<String, HostEntry>,
    target: &Path,
) -> bool {
    for _ in 0..picker.entries().len() {
        if picker
            .highlight()
            .and_then(|entry| entry_paths.get(&entry.path))
            .is_some_and(|entry| entry.path == target)
        {
            return true;
        }
        let _ = picker.handle_key(termrock::input::KeyEvent::new(
            KeyCode::Down,
            termrock::input::KeyModifiers::NONE,
        ));
    }
    false
}

fn classify_entry(
    symlink_metadata: &fs::Metadata,
    target_metadata: Option<&fs::Metadata>,
) -> FileEntryKind {
    if symlink_metadata.file_type().is_symlink() {
        return match target_metadata {
            Some(metadata) if metadata.is_dir() => FileEntryKind::SymlinkDir,
            Some(metadata) if metadata.is_file() => FileEntryKind::SymlinkFile,
            _ => FileEntryKind::Other,
        };
    }
    if symlink_metadata.is_dir() {
        FileEntryKind::Directory
    } else if symlink_metadata.is_file() {
        FileEntryKind::File
    } else {
        FileEntryKind::Other
    }
}

fn format_modified(modified: SystemTime) -> String {
    humantime::format_rfc3339_seconds(modified).to_string()
}

const DIRECTORY_PREVIEW_ENTRY_LIMIT: usize = 2_048;
const DIRECTORY_PREVIEW_TIME_LIMIT: Duration = Duration::from_millis(40);

#[derive(Clone, Copy, Default)]
struct DirectoryMarkers {
    git: bool,
    cargo: bool,
    package: bool,
    just: bool,
    make: bool,
    compose: bool,
    gradle: bool,
    mise: bool,
    idea: bool,
}

#[derive(Clone, Copy, Default)]
struct AvailableCommands {
    git: bool,
    cargo: bool,
    npm: bool,
    pnpm: bool,
    yarn: bool,
    bun: bool,
    just: bool,
    make: bool,
    docker: bool,
    gradle: bool,
    mise: bool,
    idea: bool,
}

impl AvailableCommands {
    fn detect() -> Self {
        let available = |program| which::which(program).is_ok();
        Self {
            git: available("git"),
            cargo: available("cargo"),
            npm: available("npm"),
            pnpm: available("pnpm"),
            yarn: available("yarn"),
            bun: available("bun"),
            just: available("just"),
            make: available("make"),
            docker: available("docker"),
            gradle: available("gradle"),
            mise: available("mise"),
            idea: available("idea"),
        }
    }
}

fn directory_preview(path: &Path) -> FilePreview {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return FilePreview::error(error.to_string()),
    };
    let markers = directory_markers(path);
    let mut entry_count = 0usize;
    let mut hidden_count = 0usize;
    let mut truncated = false;
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            return FilePreview::error(format!("cannot read {}: {error}", path.display()));
        }
    };
    let started = Instant::now();
    let mut entries = entries;
    loop {
        if entry_count >= DIRECTORY_PREVIEW_ENTRY_LIMIT
            || started.elapsed() >= DIRECTORY_PREVIEW_TIME_LIMIT
        {
            truncated = true;
            break;
        }
        let Some(result) = entries.next() else {
            break;
        };
        let Ok(entry) = result else {
            continue;
        };
        let name = entry.file_name();
        hidden_count += usize::from(name.to_string_lossy().starts_with('.'));
        entry_count = entry_count.saturating_add(1);
    }

    let mut lines = vec![
        format!("Path  {}", path.display()),
        if truncated {
            format!("Entries  ≥{entry_count} ({hidden_count} hidden; preview capped)")
        } else {
            format!("Entries  {entry_count} ({hidden_count} hidden)")
        },
    ];
    if let Ok(modified) = metadata.modified() {
        lines.push(format!("Modified  {}", format_modified(modified)));
    }
    lines.extend([
        String::new(),
        "Current folder · detected commands (preview only)".to_owned(),
    ]);
    lines.extend(current_folder_recommendations(
        path,
        markers,
        AvailableCommands::detect(),
    ));
    lines.extend([
        String::new(),
        "Global Holla recommendations · launcher".to_owned(),
        "find.files  Find a file or folder…  $ holla find".to_owned(),
        "browse.files  Browse files and folders…  $ holla browse".to_owned(),
        String::new(),
        "Global navigation · available everywhere".to_owned(),
        "g  Jump to any path".to_owned(),
        "←  Open parent folder".to_owned(),
        "Ctrl+h  Toggle hidden entries".to_owned(),
    ]);
    FilePreview::text(
        path.file_name().map_or_else(
            || path_text(path),
            |name| name.to_string_lossy().into_owned(),
        ),
        lines,
    )
}

fn directory_markers(path: &Path) -> DirectoryMarkers {
    let marker = |name: &str| fs::symlink_metadata(path.join(name)).is_ok();
    DirectoryMarkers {
        git: marker(".git"),
        cargo: marker("Cargo.toml"),
        package: marker("package.json"),
        just: marker("justfile") || marker("Justfile"),
        make: marker("Makefile"),
        compose: [
            "docker-compose.yml",
            "docker-compose.yaml",
            "compose.yml",
            "compose.yaml",
        ]
        .iter()
        .any(|name| fs::symlink_metadata(path.join(name)).is_ok()),
        gradle: marker("build.gradle") || marker("build.gradle.kts"),
        mise: marker("mise.toml") || marker(".mise.toml"),
        idea: marker(".idea"),
    }
}

fn current_folder_recommendations(
    path: &Path,
    markers: DirectoryMarkers,
    commands: AvailableCommands,
) -> Vec<String> {
    let mut recommendations = vec!["→ / Enter  Browse this folder".to_owned()];
    let mut command_count = 0;

    if markers.git && commands.git {
        add_command_option(
            &mut recommendations,
            &mut command_count,
            "git.status",
            "git: status",
            "$ git status",
        );
        add_command_option(
            &mut recommendations,
            &mut command_count,
            "git.pull",
            "git: pull",
            "$ git pull",
        );
        add_command_option(
            &mut recommendations,
            &mut command_count,
            "git.push",
            "git: push",
            "$ git push",
        );
    }

    if markers.cargo && commands.cargo {
        for (id, label, command) in [
            ("cargo.test", "cargo: test", "$ cargo test"),
            ("cargo.build", "cargo: build", "$ cargo build"),
            (
                "cargo.clippy",
                "cargo: clippy",
                "$ cargo clippy --all-targets --all-features",
            ),
            ("cargo.clean", "cargo: clean", "$ cargo clean"),
        ] {
            add_command_option(&mut recommendations, &mut command_count, id, label, command);
        }
    }

    if markers.package
        && let Some(program) = package_manager(path, commands)
    {
        for name in package_scripts(path).into_iter().take(8) {
            let id = format!("node.script.{name}");
            let label = format!("{program}: {name}");
            let command = format!("$ {program} run {name}");
            add_command_option(
                &mut recommendations,
                &mut command_count,
                &id,
                &label,
                &command,
            );
        }
    }

    if markers.make && commands.make {
        for name in make_targets(path).into_iter().take(8) {
            let id = format!("make.target.{name}");
            let label = format!("make: {name}");
            let command = format!("$ make {name}");
            add_command_option(
                &mut recommendations,
                &mut command_count,
                &id,
                &label,
                &command,
            );
        }
    }

    if markers.just && commands.just {
        recommendations.push(
            "just.recipe.*  $ just --list  (inspect available recipes; preview only)".to_owned(),
        );
        command_count += 1;
    }
    if markers.mise && commands.mise {
        recommendations
            .push("mise.task.*  $ mise tasks  (inspect available tasks; preview only)".to_owned());
        command_count += 1;
    }

    if markers.compose && commands.docker {
        for (id, label, command) in [
            (
                "compose.logs",
                "compose: logs",
                "$ docker compose logs --tail 200",
            ),
            ("compose.up", "compose: up", "$ docker compose up -d"),
            ("compose.down", "compose: down", "$ docker compose down"),
        ] {
            add_command_option(&mut recommendations, &mut command_count, id, label, command);
        }
    }

    if markers.gradle && commands.gradle {
        for (id, label, command) in [
            ("gradle.test", "gradle: test", "$ gradle test"),
            ("gradle.build", "gradle: build", "$ gradle build"),
            ("gradle.clean", "gradle: clean", "$ gradle clean"),
        ] {
            add_command_option(&mut recommendations, &mut command_count, id, label, command);
        }
    }

    if markers.idea && commands.idea {
        add_command_option(
            &mut recommendations,
            &mut command_count,
            "idea.clean",
            "idea: clean",
            "clean .idea and *.iml (destructive)",
        );
    }

    if command_count == 0 {
        recommendations.push("No runnable project commands detected".to_owned());
    }
    recommendations
}

fn add_command_option(
    recommendations: &mut Vec<String>,
    command_count: &mut usize,
    id: &str,
    label: &str,
    command: &str,
) {
    *command_count += 1;
    recommendations.push(format!("{id}  {label}  {command}"));
}

fn picker_entry_is_directory(picker: &FilePickerState, value: &str) -> bool {
    picker
        .entries()
        .iter()
        .find(|entry| entry.path == value)
        .is_some_and(|entry| entry.kind.is_dir())
}

fn package_manager(path: &Path, commands: AvailableCommands) -> Option<&'static str> {
    [
        ("pnpm-lock.yaml", commands.pnpm, "pnpm"),
        ("yarn.lock", commands.yarn, "yarn"),
        ("bun.lock", commands.bun, "bun"),
        ("bun.lockb", commands.bun, "bun"),
        ("package.json", commands.npm, "npm"),
    ]
    .into_iter()
    .find(|(marker, available, _)| *available && path.join(marker).is_file())
    .map(|(_, _, program)| program)
}

fn package_scripts(path: &Path) -> Vec<String> {
    let Ok(contents) = fs::read_to_string(path.join("package.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return Vec::new();
    };
    let Some(scripts) = value.get("scripts").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    let mut names = scripts
        .iter()
        .filter(|(_, value)| value.is_string())
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn make_targets(path: &Path) -> Vec<String> {
    let Ok(contents) = fs::read_to_string(path.join("Makefile")) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for line in contents.lines() {
        if line.starts_with(char::is_whitespace) || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((left, right)) = line.split_once(':') else {
            continue;
        };
        if right.trim_start().starts_with('=') {
            continue;
        }
        for target in left.split_whitespace() {
            if !target.starts_with('.')
                && !target.contains(['%', '$'])
                && !target.is_empty()
                && target.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                })
                && seen.insert(target.to_owned())
            {
                targets.push(target.to_owned());
            }
        }
    }
    targets
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JumpSource {
    CurrentFolder,
    PathCompletion,
    Global,
}

impl JumpSource {
    const fn label(self) -> &'static str {
        match self {
            Self::CurrentFolder => "current folder",
            Self::PathCompletion => "path completion",
            Self::Global => "global",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JumpSuggestion {
    path: PathBuf,
    source: JumpSource,
}

struct JumpDialog {
    cwd: PathBuf,
    input: TextInputState,
    list: ListState<PathBuf>,
    suggestions: Vec<JumpSuggestion>,
    error: Option<String>,
}

impl JumpDialog {
    fn new(cwd: PathBuf) -> Self {
        let mut input = TextInputState::new("").with_allow_empty(true);
        input.set_focused(true);
        Self {
            cwd,
            input,
            list: ListState::new(None),
            suggestions: Vec::new(),
            error: None,
        }
    }

    fn apply_suggestions(&mut self, suggestions: Vec<JumpSuggestion>) {
        self.suggestions = suggestions;
        let selected_is_visible = self
            .list
            .selected()
            .is_some_and(|selected| self.suggestions.iter().any(|item| &item.path == selected));
        if !selected_is_visible {
            self.list
                .select(self.suggestions.first().map(|item| item.path.clone()));
        }
    }

    fn clear_suggestions(&mut self) {
        self.suggestions.clear();
        self.list.select(None);
    }
}

fn render_jump_dialog(
    frame: &mut ratatui::Frame<'_>,
    jump: &mut JumpDialog,
    theme: &Theme,
    tokens: &Theme,
) {
    frame.render_widget(Backdrop::default(), frame.area());
    let area = centered_rect(JUMP_DIALOG_WIDTH, JUMP_DIALOG_HEIGHT, frame.area());
    let panel = Panel::new(tokens)
        .title(" Go to path ")
        .emphasis(PanelChrome::Focused);
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    let [input_area, list_area, help_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(inner);
    let validation = jump
        .error
        .as_deref()
        .map_or(Validation::Valid, Validation::Invalid);
    frame.render_stateful_widget(
        &TextInput::new("Path", theme)
            .placeholder("/tm…  ~/Projects…  or fuzzy name…")
            .validation(validation),
        input_area,
        &mut jump.input,
    );
    let rows = jump_rows(&jump.suggestions, theme);
    frame.render_stateful_widget(&List::new(&rows, tokens), list_area, &mut jump.list);
    let help = jump
        .error
        .as_deref()
        .unwrap_or("Exact paths win; ↑↓ selects a suggestion; Enter jumps; Esc closes");
    frame.render_widget(
        Paragraph::new(Line::styled(help.to_owned(), theme.style(Role::TextMuted))),
        help_area,
    );
}

fn jump_rows(suggestions: &[JumpSuggestion], theme: &Theme) -> Vec<ListRow<'static, PathBuf>> {
    suggestions
        .iter()
        .map(|suggestion| {
            ListRow::item(
                suggestion.path.clone(),
                Line::styled(path_text(&suggestion.path), theme.style(Role::Text)),
            )
            .secondary(Line::styled(
                suggestion.source.label().to_owned(),
                theme.style(Role::TextMuted),
            ))
        })
        .collect()
}

fn jump_suggestions(query: &str, cwd: &Path, index: &FileIndex) -> Vec<JumpSuggestion> {
    let mut suggestions = direct_path_suggestions(query, cwd, JUMP_RESULT_LIMIT);
    let mut seen: HashSet<PathBuf> = suggestions.iter().map(|item| item.path.clone()).collect();
    if !query.trim().is_empty() {
        let mut hits = index.query(query, JUMP_RESULT_LIMIT);
        hits.sort_by_key(|hit| !hit.path.starts_with(cwd));
        for hit in hits {
            if seen.insert(hit.path.clone()) {
                let source = if hit.path.starts_with(cwd) {
                    JumpSource::CurrentFolder
                } else {
                    JumpSource::Global
                };
                suggestions.push(JumpSuggestion {
                    path: hit.path,
                    source,
                });
                if suggestions.len() == JUMP_RESULT_LIMIT {
                    break;
                }
            }
        }
    }
    suggestions
}

fn direct_path_suggestions(query: &str, cwd: &Path, limit: usize) -> Vec<JumpSuggestion> {
    if limit == 0 {
        return Vec::new();
    }
    let expanded = expand_home(query.trim());
    let typed = PathBuf::from(&expanded);
    let (directory, partial, source) = if expanded.is_empty() {
        (cwd.to_path_buf(), String::new(), JumpSource::CurrentFolder)
    } else {
        let absolute = if typed.is_absolute() {
            typed
        } else {
            cwd.join(typed)
        };
        let ends_with_separator = expanded.ends_with(std::path::MAIN_SEPARATOR);
        if ends_with_separator {
            (absolute, String::new(), JumpSource::PathCompletion)
        } else {
            (
                absolute.parent().unwrap_or(cwd).to_path_buf(),
                absolute
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                if Path::new(&expanded).parent().is_none() {
                    JumpSource::CurrentFolder
                } else {
                    JumpSource::PathCompletion
                },
            )
        }
    };
    let partial = partial.to_lowercase();
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.to_lowercase().starts_with(&partial) {
                return None;
            }
            Some(entry.path())
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        let left_dir = fs::metadata(left).is_ok_and(|metadata| metadata.is_dir());
        let right_dir = fs::metadata(right).is_ok_and(|metadata| metadata.is_dir());
        right_dir.cmp(&left_dir).then_with(|| left.cmp(right))
    });
    paths.truncate(limit);
    paths
        .into_iter()
        .map(|path| JumpSuggestion { path, source })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JumpTarget {
    directory: PathBuf,
    highlight: Option<PathBuf>,
}

fn accepted_jump_target(
    input: &str,
    cwd: &Path,
    selected: Option<&Path>,
) -> Result<JumpTarget, String> {
    let exact = expand_home(input.trim());
    if !exact.is_empty() {
        let exact = PathBuf::from(exact);
        let exact = if exact.is_absolute() {
            exact
        } else {
            cwd.join(exact)
        };
        match fs::symlink_metadata(&exact) {
            Ok(_) => return jump_target(&exact, cwd).map_err(|error| error.to_string()),
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("cannot inspect {}: {error}", exact.display()));
            }
            Err(_) => {}
        }
    }
    if let Some(selected) = selected {
        return jump_target(selected, cwd).map_err(|error| error.to_string());
    }
    Err("Path does not exist and no suggestion is selected".to_owned())
}

fn jump_target(path: &Path, cwd: &Path) -> anyhow::Result<JumpTarget> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let link_metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("cannot inspect {}", path.display()))?;
    let metadata = if link_metadata.file_type().is_symlink() {
        fs::metadata(&path).with_context(|| format!("broken symlink {}", path.display()))?
    } else {
        link_metadata
    };
    if metadata.is_dir() {
        return Ok(JumpTarget {
            directory: path,
            highlight: None,
        });
    }
    let Some(parent) = path.parent() else {
        bail!("{} has no browsable parent", path.display());
    };
    Ok(JumpTarget {
        directory: parent.to_path_buf(),
        highlight: Some(path),
    })
}

fn expand_home(value: &str) -> String {
    if value == "~" {
        return dirs::home_dir().map_or_else(|| value.to_owned(), |home| path_text(&home));
    }
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return path_text(&home.join(rest));
    }
    value.to_owned()
}

fn index_roots(start: &Path) -> Vec<PathBuf> {
    let mut roots = vec![start.to_path_buf()];
    if let Some(home) = dirs::home_dir()
        && !roots.contains(&home)
    {
        roots.push(home);
    }
    roots
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn accept_listing_request(
    cwd: &mut PathBuf,
    entry_paths: &mut HashMap<String, HostEntry>,
    path: &Path,
) {
    *cwd = path.to_path_buf();
    entry_paths.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use tempfile::tempdir;

    fn screen_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer.cell((x, y)).expect("buffer cell").symbol());
            }
            text.push('\n');
        }
        text
    }

    #[tokio::test]
    async fn shared_escape_returns_to_launcher_but_standalone_escape_quits() {
        let fixture = tempdir().unwrap();
        let mut shared = Browser::new(fixture.path().to_path_buf(), None);
        assert_eq!(
            shared.handle_key(termrock::input::KeyEvent::new(
                KeyCode::Esc,
                termrock::input::KeyModifiers::NONE,
            )),
            BrowserOutcome::ReturnToLauncher
        );

        let mut standalone = Browser::new(fixture.path().to_path_buf(), None);
        standalone.shared_mode = false;
        assert_eq!(
            standalone.handle_key(termrock::input::KeyEvent::new(
                KeyCode::Esc,
                termrock::input::KeyModifiers::NONE,
            )),
            BrowserOutcome::Quit
        );

        let mut shared_preview = Browser::new(fixture.path().to_path_buf(), None);
        let tab = termrock::input::KeyEvent::new(KeyCode::Tab, termrock::input::KeyModifiers::NONE);
        let _ = shared_preview.handle_key(tab);
        assert_eq!(shared_preview.picker.pane(), FilePickerPane::Preview);
        assert_eq!(
            shared_preview.handle_key(termrock::input::KeyEvent::new(
                KeyCode::Esc,
                termrock::input::KeyModifiers::NONE,
            )),
            BrowserOutcome::ReturnToLauncher
        );
    }

    #[tokio::test]
    async fn render_matches_launcher_shell_without_picker_chrome() {
        let fixture = tempdir().unwrap();
        let mut browser = Browser::new(fixture.path().to_path_buf(), None);
        let listing_generation = browser.picker.listing_generation();
        assert!(browser.picker.apply_listing(
            listing_generation,
            path_text(fixture.path()),
            vec![FileEntry::file(
                "README.md",
                "README.md",
                path_text(&fixture.path().join("README.md")),
            )],
            None,
        ));
        let preview_generation = browser.picker.preview_generation();
        assert!(browser.picker.apply_preview(
            preview_generation,
            FilePreview::text("README.md", ["preview body".to_owned()]),
        ));
        let theme = Theme::phosphor();
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| browser.render(frame, &theme))
            .unwrap();

        let screen = screen_text(&terminal);
        let rows = screen.lines().collect::<Vec<_>>();
        assert!(rows[0].contains("holla"));
        assert!(rows[0].contains("browser"));
        assert!(rows[1].starts_with("Browse · "));
        assert!(
            screen.contains("┌ holla"),
            "left panel title missing: {screen}"
        );
        assert!(
            screen.contains("┌ Preview"),
            "right panel title missing: {screen}"
        );
        assert!(screen.contains("README.md"));
        assert!(screen.contains("preview body"));
        assert!(screen.contains("ctrl-o commands"));
        assert!(!screen.contains("Path…"));
        assert!(!screen.contains(" selected"));
    }

    #[tokio::test]
    async fn small_terminal_shows_message_instead_of_browser_chrome() {
        let fixture = tempdir().unwrap();
        let mut browser = Browser::new(fixture.path().to_path_buf(), None);
        let theme = Theme::phosphor();
        let backend = TestBackend::new(52, 9);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| browser.render(frame, &theme))
            .unwrap();

        let screen = screen_text(&terminal);
        assert!(screen.contains("Holla browser needs at least 64×12"));
        assert!(screen.contains("Current terminal: 52×9"));
        assert!(!screen.contains("┌ holla"));
        assert!(!screen.contains("┌ Preview"));
    }

    #[tokio::test]
    async fn minimum_terminal_uses_compact_footer_controls() {
        let fixture = tempdir().unwrap();
        let mut browser = Browser::new(fixture.path().to_path_buf(), None);
        let theme = Theme::phosphor();
        let backend = TestBackend::new(MIN_BROWSER_WIDTH, MIN_BROWSER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| browser.render(frame, &theme))
            .unwrap();

        let screen = screen_text(&terminal);
        assert!(
            screen.contains("ctrl-o/esc"),
            "compact footer missing: {screen}"
        );
        assert!(!screen.contains("ctrl-o commands"));
    }

    #[test]
    fn bare_g_opens_jump_only_from_list_pane() {
        let bare_g =
            termrock::input::KeyEvent::new(KeyCode::Char('g'), termrock::input::KeyModifiers::NONE);

        assert!(opens_jump_dialog(FilePickerPane::List, bare_g));
        assert!(!opens_jump_dialog(FilePickerPane::Path, bare_g));
        assert!(!opens_jump_dialog(FilePickerPane::Preview, bare_g));
    }

    #[test]
    fn changing_jump_query_clears_old_selection() {
        let mut jump = JumpDialog::new(PathBuf::from("/fixture"));
        jump.apply_suggestions(vec![JumpSuggestion {
            path: PathBuf::from("/fixture/old"),
            source: JumpSource::CurrentFolder,
        }]);
        assert!(jump.list.selected().is_some());

        jump.clear_suggestions();

        assert!(jump.list.selected().is_none());
        assert!(jump.suggestions.is_empty());
    }

    #[test]
    fn listing_projects_metadata_and_entry_kinds() {
        let fixture = tempdir().unwrap();
        fs::create_dir(fixture.path().join("folder")).unwrap();
        fs::write(fixture.path().join("file.txt"), b"hello").unwrap();
        fs::write(fixture.path().join(".hidden"), b"secret").unwrap();

        let entries = list_directory(fixture.path()).unwrap();
        let file = entries
            .iter()
            .find(|entry| entry.entry.name == "file.txt")
            .unwrap();
        let folder = entries
            .iter()
            .find(|entry| entry.entry.name == "folder")
            .unwrap();
        let hidden = entries
            .iter()
            .find(|entry| entry.entry.name == ".hidden")
            .unwrap();

        assert_eq!(file.entry.size, Some(5));
        assert_eq!(folder.entry.kind, FileEntryKind::Directory);
        assert!(hidden.entry.hidden);
    }

    #[test]
    fn absolute_prefix_completion_finds_tmp_shape() {
        let fixture = tempdir().unwrap();
        fs::create_dir(fixture.path().join("tmp")).unwrap();
        fs::create_dir(fixture.path().join("tm-alpha")).unwrap();
        fs::write(fixture.path().join("tm-file"), b"file").unwrap();
        fs::create_dir(fixture.path().join("var")).unwrap();
        let query = format!("{}/tm", fixture.path().display());

        let paths = direct_path_suggestions(&query, fixture.path(), 10)
            .into_iter()
            .map(|suggestion| suggestion.path)
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            [
                fixture.path().join("tm-alpha"),
                fixture.path().join("tmp"),
                fixture.path().join("tm-file"),
            ]
        );
    }

    #[test]
    fn home_path_expansion_preserves_suffix() {
        let Some(home) = dirs::home_dir() else {
            return;
        };

        assert_eq!(expand_home("~"), path_text(&home));
        assert_eq!(expand_home("~/Projects"), path_text(&home.join("Projects")));
        assert_eq!(expand_home("relative/path"), "relative/path");
    }

    #[test]
    fn exact_jump_path_wins_over_selected_suggestion() {
        let fixture = tempdir().unwrap();
        let exact = fixture.path().join("exact");
        let selected = fixture.path().join("selected");
        fs::create_dir(&exact).unwrap();
        fs::create_dir(&selected).unwrap();

        let target =
            accepted_jump_target(exact.to_str().unwrap(), fixture.path(), Some(&selected)).unwrap();

        assert_eq!(target.directory, exact);
    }

    #[test]
    fn file_jump_opens_parent_and_marks_file_for_preview() {
        let fixture = tempdir().unwrap();
        let file = fixture.path().join("README.md");
        fs::write(&file, b"hello").unwrap();

        let target = jump_target(&file, fixture.path()).unwrap();

        assert_eq!(target.directory, fixture.path());
        assert_eq!(target.highlight.as_deref(), Some(file.as_path()));
    }

    #[test]
    fn exact_highlight_does_not_select_suffix_match() {
        let mut picker = FilePickerState::new("/fixture");
        picker.set_focused(true);
        let outcome = picker.request_list("/fixture");
        let FilePickerOutcome::ListRequested { generation, .. } = outcome else {
            panic!("expected listing request");
        };
        assert!(picker.apply_listing(
            generation,
            "/fixture",
            vec![
                FileEntry::file("afoo", "afoo", "/fixture/afoo"),
                FileEntry::file("foo", "foo", "/fixture/foo"),
            ],
            None,
        ));
        picker.set_name_filter("foo");

        let entry_paths = HashMap::from([
            (
                "/fixture/afoo".to_owned(),
                HostEntry {
                    path: PathBuf::from("/fixture/afoo"),
                    actionable: true,
                },
            ),
            (
                "/fixture/foo".to_owned(),
                HostEntry {
                    path: PathBuf::from("/fixture/foo"),
                    actionable: true,
                },
            ),
        ]);
        assert!(focus_exact_path(
            &mut picker,
            &entry_paths,
            Path::new("/fixture/foo")
        ));
        assert_eq!(picker.highlight().unwrap().path, "/fixture/foo");
    }

    #[cfg(unix)]
    #[test]
    fn broken_symlink_is_visible_but_disabled() {
        use std::os::unix::fs::symlink;

        let fixture = tempdir().unwrap();
        symlink(
            fixture.path().join("missing"),
            fixture.path().join("broken"),
        )
        .unwrap();

        let entries = list_directory(fixture.path()).unwrap();
        let broken = entries
            .iter()
            .find(|entry| entry.entry.name == "broken")
            .unwrap();
        assert_eq!(broken.entry.kind, FileEntryKind::Other);
        assert!(broken.entry.error.is_some());
        assert!(!broken.actionable);

        let mut picker =
            FilePickerState::new(path_text(fixture.path())).with_mode(FilePickerMode::OpenAny);
        let FilePickerOutcome::ListRequested { generation, .. } =
            picker.request_list(path_text(fixture.path()))
        else {
            panic!("expected listing request");
        };
        let (entries, paths) = project_listing(generation, entries);
        assert!(picker.apply_listing(generation, path_text(fixture.path()), entries, None));
        picker.set_show_hidden(true);
        let broken = picker
            .entries()
            .iter()
            .find(|entry| entry.name == "broken")
            .unwrap();
        assert!(broken.error.is_some());
        assert!(broken.selectable, "OpenAny reprocessing rewrites this flag");
        assert!(!paths.get(&broken.path).unwrap().actionable);
        assert_eq!(
            picker.handle_key(termrock::input::KeyEvent::new(
                KeyCode::Enter,
                termrock::input::KeyModifiers::NONE,
            )),
            FilePickerOutcome::Ignored
        );
        assert!(jump_target(&fixture.path().join("broken"), fixture.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn invalid_utf8_names_keep_distinct_host_identities() {
        use std::os::unix::ffi::OsStringExt;

        let first = PathBuf::from(std::ffi::OsString::from_vec(vec![b'x', 0x80]));
        let second = PathBuf::from(std::ffi::OsString::from_vec(vec![b'x', 0x81]));
        assert_eq!(path_text(&first), path_text(&second));
        let projected = [&first, &second]
            .into_iter()
            .map(|path| HostEntryProjection {
                entry: FileEntry::file(path_text(path), path_text(path), path_text(path)),
                path: path.clone(),
                actionable: true,
            })
            .collect();
        let (entries, paths) = project_listing(7, projected);

        assert_eq!(entries.len(), 2);
        assert_ne!(entries[0].path, entries[1].path);
        let resolved = entries
            .iter()
            .map(|entry| paths.get(&entry.path).unwrap().path.clone())
            .collect::<HashSet<_>>();
        assert_eq!(resolved, HashSet::from([first, second]));
    }

    #[test]
    fn stale_listing_cannot_replace_current_directory() {
        let mut picker = FilePickerState::new("/start");
        let first = picker.request_list("/old");
        let second = picker.request_list("/new");
        let FilePickerOutcome::ListRequested {
            generation: old_generation,
            ..
        } = first
        else {
            panic!("expected first list request");
        };
        let FilePickerOutcome::ListRequested {
            generation: new_generation,
            ..
        } = second
        else {
            panic!("expected second list request");
        };

        assert!(!picker.apply_listing(old_generation, "/old", Vec::new(), None));
        assert!(picker.apply_listing(new_generation, "/new", Vec::new(), None));
        assert_eq!(picker.cwd(), "/new");
    }

    #[test]
    fn stale_listing_error_cannot_replace_current_request() {
        let mut picker = FilePickerState::new("/start");
        let first = picker.request_list("/old");
        let second = picker.request_list("/new");
        let FilePickerOutcome::ListRequested {
            generation: old_generation,
            ..
        } = first
        else {
            panic!("expected first list request");
        };
        let FilePickerOutcome::ListRequested {
            generation: new_generation,
            ..
        } = second
        else {
            panic!("expected second list request");
        };

        assert!(!picker.apply_listing_error(old_generation, "stale"));
        assert!(picker.apply_listing_error(new_generation, "current"));
        assert_eq!(
            picker.listing_status(),
            termrock::widgets::FileListingStatus::Error
        );
    }

    #[test]
    fn accepted_listing_request_clears_old_identity_before_error() {
        let mut picker = FilePickerState::new("/old");
        let FilePickerOutcome::ListRequested { generation, .. } = picker.request_list("/new")
        else {
            panic!("expected listing request");
        };
        let mut cwd = PathBuf::from("/old");
        let mut entry_paths = HashMap::from([(
            "old-entry".to_owned(),
            HostEntry {
                path: PathBuf::from("/old/entry"),
                actionable: true,
            },
        )]);

        accept_listing_request(&mut cwd, &mut entry_paths, Path::new("/new"));

        assert_eq!(picker.cwd(), "/new");
        assert_eq!(cwd, PathBuf::from("/new"));
        assert!(entry_paths.is_empty());
        assert!(picker.apply_listing_error(generation, "permission denied"));
        assert_eq!(picker.cwd(), path_text(&cwd));
    }

    #[test]
    fn stale_preview_cannot_replace_current_preview() {
        let mut picker = FilePickerState::new("/fixture").with_preview(true);
        picker.set_focused(true);
        let FilePickerOutcome::ListRequested { generation, .. } = picker.request_list("/fixture")
        else {
            panic!("expected listing request");
        };
        assert!(picker.apply_listing(
            generation,
            "/fixture",
            vec![
                FileEntry::file("a", "a", "/fixture/a"),
                FileEntry::file("b", "b", "/fixture/b"),
            ],
            None,
        ));
        let first = picker.handle_key(termrock::input::KeyEvent::new(
            KeyCode::Down,
            termrock::input::KeyModifiers::NONE,
        ));
        let second = picker.handle_key(termrock::input::KeyEvent::new(
            KeyCode::Up,
            termrock::input::KeyModifiers::NONE,
        ));
        let FilePickerOutcome::PreviewRequested {
            generation: old_generation,
            ..
        } = first
        else {
            panic!("expected first preview request");
        };
        let FilePickerOutcome::PreviewRequested {
            generation: new_generation,
            ..
        } = second
        else {
            panic!("expected second preview request");
        };

        assert!(picker.apply_preview(new_generation, FilePreview::text("new", [])));
        assert!(!picker.apply_preview(old_generation, FilePreview::text("old", [])));
    }

    #[test]
    fn preview_requested_for_directory_is_classified_as_directory() {
        let mut picker = FilePickerState::new("/fixture").with_preview(true);
        picker.set_focused(true);
        let FilePickerOutcome::ListRequested { generation, .. } = picker.request_list("/fixture")
        else {
            panic!("expected listing request");
        };
        assert!(picker.apply_listing(
            generation,
            "/fixture",
            vec![
                FileEntry::directory("folder", "folder", "/fixture/folder"),
                FileEntry::file("file", "file", "/fixture/file"),
            ],
            None,
        ));

        let _ = picker.handle_key(termrock::input::KeyEvent::new(
            KeyCode::Down,
            termrock::input::KeyModifiers::NONE,
        ));
        let outcome = picker.handle_key(termrock::input::KeyEvent::new(
            KeyCode::Up,
            termrock::input::KeyModifiers::NONE,
        ));
        let FilePickerOutcome::PreviewRequested { path, .. } = outcome else {
            panic!("expected directory preview request");
        };

        assert!(picker_entry_is_directory(&picker, &path));
    }

    #[test]
    fn directory_recommendations_put_context_before_global() {
        let fixture = tempdir().unwrap();
        fs::write(fixture.path().join("Cargo.toml"), b"[package]").unwrap();

        let preview = directory_preview(fixture.path());
        let current = preview
            .lines
            .iter()
            .position(|line| line == "Current folder · detected commands (preview only)")
            .unwrap();
        let global = preview
            .lines
            .iter()
            .position(|line| line == "Global Holla recommendations · launcher")
            .unwrap();
        let navigation = preview
            .lines
            .iter()
            .position(|line| line == "Global navigation · available everywhere")
            .unwrap();

        assert!(current < global);
        assert!(global < navigation);
        assert!(
            !preview
                .lines
                .iter()
                .any(|line| line.contains("Rust project detected"))
        );

        let markers = DirectoryMarkers {
            cargo: true,
            ..DirectoryMarkers::default()
        };
        let recommendations = current_folder_recommendations(
            fixture.path(),
            markers,
            AvailableCommands {
                cargo: true,
                ..AvailableCommands::default()
            },
        );
        assert!(
            recommendations
                .iter()
                .any(|line| line == "cargo.test  cargo: test  $ cargo test")
        );
    }

    #[test]
    fn directory_preview_caps_scan_without_losing_marker_detection() {
        let fixture = tempdir().unwrap();
        fs::write(fixture.path().join("Cargo.toml"), b"[package]").unwrap();
        for index in 0..=DIRECTORY_PREVIEW_ENTRY_LIMIT {
            fs::write(fixture.path().join(format!("entry-{index}")), b"").unwrap();
        }

        let preview = directory_preview(fixture.path());

        assert!(
            preview
                .lines
                .iter()
                .any(|line| line.starts_with("Entries  ≥"))
        );
        assert!(
            preview
                .lines
                .iter()
                .any(|line| line == "Current folder · detected commands (preview only)")
        );
        assert!(
            !preview
                .lines
                .iter()
                .any(|line| line.contains("Rust project detected"))
        );
    }
}
