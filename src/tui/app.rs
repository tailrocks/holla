use crossterm::event::{self, Event, KeyEventKind};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    text::{Line, Span, Text},
};
use std::{
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use termrock::{
    input::KeyCode,
    interaction::Outcome,
    keymap::{KeyBinding, KeyChord, Keymap, Visibility},
    layout::centered_rect,
    runtime::{StdSubscription, Subscription, SubscriptionPoll},
    scroll::{DialogScroll, TailScroll},
    style::{Role, Theme},
    widgets::{
        Action as DialogAction, Backdrop, ChoiceDialog, ChoiceDialogState, Dialog, PanelEmphasis,
        StatusBar, StatusBarState, StatusSlot, Tab, Tabs, TabsState, Viewport, render_hint_bar,
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
};

#[derive(Debug, Clone, PartialEq)]
pub enum TaskState {
    Pending,
    Running,
    Cancelling,
    Done(bool),
}

#[derive(Debug, Clone)]
pub struct TaskDef {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
}

impl TaskDef {
    pub fn new(label: impl Into<String>, program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            label: label.into(),
            program: program.into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskEvent {
    Line { task: usize, line: String },
    Started { task: usize },
    Done { task: usize, success: bool },
}

#[derive(Clone)]
struct TaskHandle {
    task: usize,
    process_groups: Arc<Mutex<Vec<Option<u32>>>>,
    cancelled: Arc<AtomicBool>,
}

impl TaskHandle {
    fn cancel(&self) -> bool {
        self.cancelled.store(true, Ordering::Release);
        let pid = self.process_groups.lock().expect("process group lock")[self.task];
        let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) else {
            return false;
        };
        // SAFETY: `pid` is the positive id returned by the child we placed in
        // its own process group. Negating it targets only that group. SIGTERM
        // does not access Rust memory and the return value is checked.
        let sent = unsafe { libc::kill(-pid, libc::SIGTERM) == 0 };
        if sent {
            let process_groups = Arc::clone(&self.process_groups);
            let task = self.task;
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(750)).await;
                let still_running = process_groups.lock().expect("process group lock")[task]
                    == u32::try_from(pid).ok();
                if still_running {
                    // SAFETY: the executor still records this exact child-owned
                    // process group. Escalation prevents TERM-resistant children
                    // from surviving cancellation.
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
            });
        }
        sent
    }
}

struct TaskSupervisor {
    handles: Vec<TaskHandle>,
    armed: bool,
}

impl TaskSupervisor {
    fn new(handles: Vec<TaskHandle>) -> Self {
        Self {
            handles,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TaskSupervisor {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        for handle in &self.handles {
            let _ = handle.cancel();
        }
        std::thread::sleep(Duration::from_millis(750));
        let groups = self.handles.first().map(|handle| {
            handle
                .process_groups
                .lock()
                .expect("process group lock")
                .clone()
        });
        for pid in groups.into_iter().flatten().flatten() {
            if let Ok(pid) = i32::try_from(pid) {
                // SAFETY: shutdown escalation targets only process groups still
                // owned by this supervisor.
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
    }
}

struct RunningTask {
    label: String,
    lines: Vec<String>,
    state: TaskState,
    tail: TailScroll,
}

#[derive(Clone, Copy, PartialEq)]
enum RunnerKey {
    PreviousTask,
    NextTask,
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    FollowTail,
    Quit,
}

#[derive(Clone, Copy, PartialEq)]
enum CancelChoice {
    Stop,
    KeepRunning,
}

#[derive(Clone, Copy, PartialEq)]
enum StatusSlotId {
    Product,
    Counts,
}

static RUNNER_KEYMAP: Keymap<RunnerKey> = Keymap::new(&[
    KeyBinding {
        chords: &[
            KeyChord::plain(KeyCode::Left),
            KeyChord::plain(KeyCode::Char('h')),
        ],
        action: RunnerKey::PreviousTask,
        hint: Some("previous task"),
        visibility: Visibility::Shown,
        glyph: Some("←"),
    },
    KeyBinding {
        chords: &[
            KeyChord::plain(KeyCode::Right),
            KeyChord::plain(KeyCode::Char('l')),
        ],
        action: RunnerKey::NextTask,
        hint: Some("next task"),
        visibility: Visibility::Shown,
        glyph: Some("→"),
    },
    KeyBinding {
        chords: &[
            KeyChord::plain(KeyCode::Up),
            KeyChord::plain(KeyCode::Char('k')),
        ],
        action: RunnerKey::ScrollUp,
        hint: Some("scroll"),
        visibility: Visibility::Shown,
        glyph: Some("↑↓"),
    },
    KeyBinding {
        chords: &[
            KeyChord::plain(KeyCode::Down),
            KeyChord::plain(KeyCode::Char('j')),
        ],
        action: RunnerKey::ScrollDown,
        hint: None,
        visibility: Visibility::HiddenAlias,
        glyph: None,
    },
    KeyBinding {
        chords: &[KeyChord::plain(KeyCode::PageUp)],
        action: RunnerKey::PageUp,
        hint: None,
        visibility: Visibility::HiddenAlias,
        glyph: None,
    },
    KeyBinding {
        chords: &[KeyChord::plain(KeyCode::PageDown)],
        action: RunnerKey::PageDown,
        hint: None,
        visibility: Visibility::HiddenAlias,
        glyph: None,
    },
    KeyBinding {
        chords: &[KeyChord::plain(KeyCode::End)],
        action: RunnerKey::FollowTail,
        hint: Some("follow tail"),
        visibility: Visibility::Shown,
        glyph: Some("end"),
    },
    KeyBinding {
        chords: &[
            KeyChord::plain(KeyCode::Char('q')),
            KeyChord::plain(KeyCode::Esc),
        ],
        action: RunnerKey::Quit,
        hint: Some("stop/close"),
        visibility: Visibility::Shown,
        glyph: Some("q/esc"),
    },
]);

static DONE_KEYMAP: Keymap<RunnerKey> = Keymap::new(&[KeyBinding {
    chords: &[
        KeyChord::plain(KeyCode::Char('q')),
        KeyChord::plain(KeyCode::Esc),
    ],
    action: RunnerKey::Quit,
    hint: Some("close"),
    visibility: Visibility::Shown,
    glyph: Some("q/esc"),
}]);

pub async fn run_tasks(tasks: Vec<TaskDef>) -> anyhow::Result<()> {
    run_tui(tasks, false).await
}

pub async fn run_parallel_tasks(tasks: Vec<TaskDef>) -> anyhow::Result<()> {
    run_tui(tasks, true).await
}

fn spawn_tasks(defs: Vec<TaskDef>, parallel: bool, tx: mpsc::Sender<TaskEvent>) -> Vec<TaskHandle> {
    let process_groups = Arc::new(Mutex::new(vec![None; defs.len()]));
    let cancelled = Arc::new(AtomicBool::new(false));
    let handles = (0..defs.len())
        .map(|task| TaskHandle {
            task,
            process_groups: Arc::clone(&process_groups),
            cancelled: Arc::clone(&cancelled),
        })
        .collect();

    tokio::spawn(async move {
        if parallel {
            let mut jobs = Vec::with_capacity(defs.len());
            for (task, def) in defs.into_iter().enumerate() {
                jobs.push(tokio::spawn(run_task(
                    task,
                    def,
                    tx.clone(),
                    Arc::clone(&process_groups),
                    Arc::clone(&cancelled),
                )));
            }
            drop(tx);
            for job in jobs {
                let _ = job.await;
            }
        } else {
            for (task, def) in defs.into_iter().enumerate() {
                if cancelled.load(Ordering::Acquire) {
                    let _ = tx.send(TaskEvent::Done {
                        task,
                        success: false,
                    });
                    continue;
                }
                run_task(
                    task,
                    def,
                    tx.clone(),
                    Arc::clone(&process_groups),
                    Arc::clone(&cancelled),
                )
                .await;
            }
        }
    });

    handles
}

async fn run_task(
    task: usize,
    def: TaskDef,
    tx: mpsc::Sender<TaskEvent>,
    process_groups: Arc<Mutex<Vec<Option<u32>>>>,
    cancelled: Arc<AtomicBool>,
) {
    if cancelled.load(Ordering::Acquire) {
        let _ = tx.send(TaskEvent::Done {
            task,
            success: false,
        });
        return;
    }
    let _ = tx.send(TaskEvent::Started { task });

    let mut command = Command::new(&def.program);
    command
        .args(&def.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = tx.send(TaskEvent::Line {
                task,
                line: format!("Error: {error}"),
            });
            let _ = tx.send(TaskEvent::Done {
                task,
                success: false,
            });
            return;
        }
    };

    let mut registered_pgid = None;
    if let Some(pid) = child.id() {
        registered_pgid = i32::try_from(pid).ok();
        process_groups.lock().expect("process group lock")[task] = Some(pid);
        if cancelled.load(Ordering::Acquire) {
            let _ = TaskHandle {
                task,
                process_groups: Arc::clone(&process_groups),
                cancelled: Arc::clone(&cancelled),
            }
            .cancel();
        }
    }

    let stdout_reader = child
        .stdout
        .take()
        .map(|stdout| tokio::spawn(stream_lines(task, stdout, tx.clone())));
    let stderr_reader = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(stream_lines(task, stderr, tx.clone())));
    let status = child.wait().await;
    if cancelled.load(Ordering::Acquire)
        && let Some(pgid) = registered_pgid
    {
        reap_process_group(pgid).await;
    }
    if let Some(reader) = stdout_reader {
        let _ = reader.await;
    }
    if let Some(reader) = stderr_reader {
        let _ = reader.await;
    }
    process_groups.lock().expect("process group lock")[task] = None;
    let _ = tx.send(TaskEvent::Done {
        task,
        success: !cancelled.load(Ordering::Acquire) && status.is_ok_and(|status| status.success()),
    });
}

async fn reap_process_group(pgid: i32) {
    // SAFETY: this is the positive id of the process group created for the
    // cancelled task. Killing the whole group removes descendants even when
    // the direct child already exited or closed its output pipes.
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
    while process_group_exists(pgid) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn process_group_exists(pgid: i32) -> bool {
    // SAFETY: signal 0 performs existence/permission checking only.
    let result = unsafe { libc::kill(-pgid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

async fn stream_lines(task: usize, stream: impl AsyncRead + Unpin, tx: mpsc::Sender<TaskEvent>) {
    let mut lines = BufReader::new(stream).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if tx.send(TaskEvent::Line { task, line }).is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                let _ = tx.send(TaskEvent::Line {
                    task,
                    line: format!("Error reading output: {error}"),
                });
                break;
            }
        }
    }
}

fn apply_event(tasks: &mut [RunningTask], event: TaskEvent) {
    match event {
        TaskEvent::Started { task } => tasks[task].state = TaskState::Running,
        TaskEvent::Line { task, line } => {
            if tasks[task].tail.offset() > 0 {
                tasks[task].tail = TailScroll::new(tasks[task].tail.offset().saturating_add(1));
            }
            tasks[task].lines.push(line);
        }
        TaskEvent::Done { task, success } => tasks[task].state = TaskState::Done(success),
    }
}

fn all_done(tasks: &[RunningTask]) -> bool {
    tasks
        .iter()
        .all(|task| matches!(task.state, TaskState::Done(_)))
}

fn cancel_tasks(tasks: &mut [RunningTask], handles: &[TaskHandle]) {
    for handle in handles {
        handle.cancelled.store(true, Ordering::Release);
    }
    for handle in handles {
        let _ = handle.cancel();
    }
    for task in tasks {
        if !matches!(task.state, TaskState::Done(_)) {
            task.lines.push("cancelled".into());
            task.state = TaskState::Cancelling;
        }
    }
}

fn scroll_selected(task: &mut RunningTask, viewport_height: usize, delta: isize) {
    let filled = task.lines.len().saturating_sub(viewport_height);
    task.tail.scroll_by(filled, delta);
}

async fn run_tui(task_defs: Vec<TaskDef>, parallel: bool) -> anyhow::Result<()> {
    if task_defs.is_empty() {
        println!("No tasks to run.");
        return Ok(());
    }

    let mut tasks: Vec<_> = task_defs
        .iter()
        .map(|task| RunningTask {
            label: task.label.clone(),
            lines: Vec::new(),
            state: TaskState::Pending,
            tail: TailScroll::default(),
        })
        .collect();
    let (tx, rx) = mpsc::channel();
    let mut supervisor = TaskSupervisor::new(spawn_tasks(task_defs, parallel, tx));
    let mut events = StdSubscription(rx);
    let theme = Theme::tailrocks_phosphor();
    let mut selected = 0usize;
    let mut tabs_state = TabsState {
        selected: Some(selected),
        focused: true,
        ..TabsState::default()
    };
    let mut status_state = StatusBarState::default();
    let mut cancel_dialog: Option<ChoiceDialogState<CancelChoice>> = None;
    let cancel_actions = [
        DialogAction {
            id: CancelChoice::Stop,
            label: "Stop",
            enabled: true,
            style: Some(theme.style(Role::Danger)),
        },
        DialogAction {
            id: CancelChoice::KeepRunning,
            label: "Keep running",
            enabled: true,
            style: None,
        },
    ];
    let mut output_viewport_height = 1usize;

    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;

    loop {
        while let SubscriptionPoll::Ready(event) = events.poll_next() {
            apply_event(&mut tasks, event);
        }
        let done = all_done(&tasks);
        tabs_state.selected = Some(selected);

        terminal.draw(|frame| {
            let [status_area, tabs_area, output_area, footer_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .areas(frame.area());
            output_viewport_height = usize::from(output_area.height.saturating_sub(2)).max(1);

            let running = tasks
                .iter()
                .filter(|task| matches!(task.state, TaskState::Running | TaskState::Cancelling))
                .count();
            let ok = tasks
                .iter()
                .filter(|task| task.state == TaskState::Done(true))
                .count();
            let failed = tasks
                .iter()
                .filter(|task| task.state == TaskState::Done(false))
                .count();
            let counts = if done {
                format!("done — {ok} ok, {failed} failed — press q to close ")
            } else {
                format!("{running} running, {ok} ok, {failed} failed ")
            };
            let left_slots = [StatusSlot {
                id: StatusSlotId::Product,
                content: " holla tasks ",
                priority: 2,
                min_width: 0,
                enabled: true,
                style: theme.style(Role::Accent),
                hover_style: None,
            }];
            let right_slots = [StatusSlot {
                id: StatusSlotId::Counts,
                content: &counts,
                priority: 1,
                min_width: 10,
                enabled: true,
                style: theme.style(if failed > 0 {
                    Role::Danger
                } else {
                    Role::TextMuted
                }),
                hover_style: None,
            }];
            frame.render_stateful_widget(
                &StatusBar::new(&left_slots, &right_slots, &theme).alpha(1.0),
                status_area,
                &mut status_state,
            );

            let tabs: Vec<_> = tasks
                .iter()
                .enumerate()
                .map(|(index, task)| Tab {
                    id: index,
                    label: task.label.as_str(),
                    glyph: Some(match task.state {
                        TaskState::Pending => Span::styled("○", theme.style(Role::TextMuted)),
                        TaskState::Running => Span::styled("◉", theme.style(Role::Accent)),
                        TaskState::Cancelling => Span::styled("✗", theme.style(Role::Danger)),
                        TaskState::Done(true) => Span::styled("✓", theme.style(Role::Success)),
                        TaskState::Done(false) => Span::styled("✗", theme.style(Role::Danger)),
                    }),
                    active: index == selected,
                    enabled: true,
                })
                .collect();
            frame.render_stateful_widget(
                &Tabs::new(&tabs, &theme).gap(1),
                tabs_area,
                &mut tabs_state,
            );

            let output_lines: Vec<Line<'static>> = tasks[selected]
                .lines
                .iter()
                .map(|line| {
                    Line::from(termrock::ansi_text::styled_spans(
                        line,
                        theme.style(Role::Text),
                    ))
                })
                .collect();
            let top = tasks[selected]
                .tail
                .to_top_offset(output_lines.len(), output_viewport_height);
            let mut viewport_state = DialogScroll {
                scroll_x: 0,
                scroll_y: u16::try_from(top).unwrap_or(u16::MAX),
            };
            frame.render_stateful_widget(
                &Viewport::new(&output_lines, &theme)
                    .title(tasks[selected].label.as_str())
                    .content_style(theme.style(Role::Text)),
                output_area,
                &mut viewport_state,
            );

            let hints = if done {
                DONE_KEYMAP.hint_spans()
            } else {
                RUNNER_KEYMAP.hint_spans()
            };
            render_hint_bar(frame, footer_area, &hints, &theme);

            if let Some(dialog_state) = cancel_dialog.as_mut() {
                frame.render_widget(Backdrop::default(), frame.area());
                let area = centered_rect(54, 7, frame.area());
                frame.render_stateful_widget(
                    &ChoiceDialog::new(
                        Dialog::new(
                            "Stop running tasks?",
                            Text::from("Tasks are still running — stop them?"),
                            &theme,
                        )
                        .style(theme.style(Role::Text))
                        .emphasis(PanelEmphasis::Focused),
                        &cancel_actions,
                    )
                    .gap("  "),
                    area,
                    dialog_state,
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
            if let Some(dialog_state) = cancel_dialog.as_mut() {
                match dialog_state.handle_key(&cancel_actions, key) {
                    Outcome::Activated(CancelChoice::Stop) => {
                        cancel_tasks(&mut tasks, &supervisor.handles);
                        cancel_dialog = None;
                    }
                    Outcome::Activated(CancelChoice::KeepRunning) | Outcome::Cancelled => {
                        cancel_dialog = None;
                    }
                    Outcome::Ignored | Outcome::Changed => {}
                    _ => {}
                }
                continue;
            }

            let Some(action) = RUNNER_KEYMAP.dispatch(KeyChord::from(key)) else {
                continue;
            };
            match action {
                RunnerKey::PreviousTask => selected = selected.saturating_sub(1),
                RunnerKey::NextTask => {
                    selected = selected
                        .saturating_add(1)
                        .min(tasks.len().saturating_sub(1));
                }
                RunnerKey::ScrollUp => {
                    scroll_selected(&mut tasks[selected], output_viewport_height, 1);
                }
                RunnerKey::ScrollDown => {
                    scroll_selected(&mut tasks[selected], output_viewport_height, -1);
                }
                RunnerKey::PageUp => {
                    scroll_selected(
                        &mut tasks[selected],
                        output_viewport_height,
                        output_viewport_height as isize,
                    );
                }
                RunnerKey::PageDown => {
                    scroll_selected(
                        &mut tasks[selected],
                        output_viewport_height,
                        -(output_viewport_height as isize),
                    );
                }
                RunnerKey::FollowTail => tasks[selected].tail = TailScroll::default(),
                RunnerKey::Quit if done => break,
                RunnerKey::Quit => {
                    cancel_dialog = Some(ChoiceDialogState::new(Some(CancelChoice::Stop)));
                }
            }
        }
    }

    drop(terminal);
    session.restore()?;
    supervisor.disarm();

    println!("\n{}", "─".repeat(50));
    for task in &tasks {
        let (icon, status) = match task.state {
            TaskState::Done(true) => ("✓", "ok"),
            TaskState::Done(false) => ("✗", "failed"),
            TaskState::Pending | TaskState::Running | TaskState::Cancelling => ("?", "unknown"),
        };
        println!("  {icon}  {}  [{status}]", task.label);
    }
    println!("{}\n", "─".repeat(50));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    fn collect_events(rx: &mpsc::Receiver<TaskEvent>, done_count: usize) -> Vec<TaskEvent> {
        let mut events = Vec::new();
        while events
            .iter()
            .filter(|event| matches!(event, TaskEvent::Done { .. }))
            .count()
            < done_count
        {
            events.push(rx.recv_timeout(Duration::from_secs(2)).expect("task event"));
        }
        events
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn executor_streams_stdout_and_stderr_before_failed_completion() {
        let (tx, rx) = mpsc::channel();
        let _handles = spawn_tasks(
            vec![TaskDef::new(
                "mixed output",
                "sh",
                &["-c", "echo a; sleep 0.03; echo b 1>&2; exit 3"],
            )],
            false,
            tx,
        );

        let events = collect_events(&rx, 1);

        assert!(
            events
                .iter()
                .any(|event| matches!(event, TaskEvent::Line { line, .. } if line == "a"))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TaskEvent::Line { line, .. } if line == "b"))
        );
        assert!(matches!(
            events.last(),
            Some(TaskEvent::Done {
                task: 0,
                success: false
            })
        ));
        let stdout = events
            .iter()
            .position(|event| matches!(event, TaskEvent::Line { line, .. } if line == "a"))
            .expect("stdout line");
        let stderr = events
            .iter()
            .position(|event| matches!(event, TaskEvent::Line { line, .. } if line == "b"))
            .expect("stderr line");
        assert!(stdout < stderr);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sequential_executor_finishes_one_task_before_starting_next() {
        let (tx, rx) = mpsc::channel();
        let _handles = spawn_tasks(
            vec![
                TaskDef::new("first", "sh", &["-c", "sleep 0.05; echo first"]),
                TaskDef::new("second", "sh", &["-c", "echo second"]),
            ],
            false,
            tx,
        );

        let events = collect_events(&rx, 2);
        let first_done = events
            .iter()
            .position(|event| matches!(event, TaskEvent::Done { task: 0, .. }))
            .expect("first done");
        let second_started = events
            .iter()
            .position(|event| matches!(event, TaskEvent::Started { task: 1 }))
            .expect("second started");

        assert!(first_done < second_started);
    }

    #[test]
    fn tail_scroll_follows_bottom_and_can_unpin() {
        let mut tail = TailScroll::default();
        assert_eq!(tail.to_top_offset(20, 5), 15);

        tail.scroll_by(15, 3);

        assert_eq!(tail.to_top_offset(20, 5), 12);
    }

    #[test]
    fn appended_lines_hold_the_unpinned_view_in_place() {
        let mut tasks = vec![RunningTask {
            label: "test".into(),
            lines: (0..20).map(|line| line.to_string()).collect(),
            state: TaskState::Running,
            tail: TailScroll::new(3),
        }];
        let before = tasks[0].tail.to_top_offset(tasks[0].lines.len(), 5);

        apply_event(
            &mut tasks,
            TaskEvent::Line {
                task: 0,
                line: "new".into(),
            },
        );

        assert_eq!(tasks[0].tail.to_top_offset(tasks[0].lines.len(), 5), before);
    }

    #[test]
    fn cancellation_does_not_report_done_before_executor_reaps() {
        let process_groups = Arc::new(Mutex::new(vec![None]));
        let cancelled = Arc::new(AtomicBool::new(false));
        let handles = vec![TaskHandle {
            task: 0,
            process_groups,
            cancelled,
        }];
        let mut tasks = vec![RunningTask {
            label: "test".into(),
            lines: vec![],
            state: TaskState::Running,
            tail: TailScroll::default(),
        }];

        cancel_tasks(&mut tasks, &handles);

        assert_eq!(tasks[0].state, TaskState::Cancelling);
        assert!(!all_done(&tasks));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_reaps_a_detached_grandchild_before_done() {
        let (tx, rx) = mpsc::channel();
        let handles = spawn_tasks(
            vec![TaskDef::new(
                "long task",
                "sh",
                &[
                    "-c",
                    "trap 'exit 0' TERM; (trap ':' TERM; echo ready >&3; exec 3>&-; while :; do sleep 1; done) 3>&1 </dev/null >/dev/null 2>&1 & wait",
                ],
            )],
            false,
            tx,
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(TaskEvent::Started { task: 0 })
        ));
        loop {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(TaskEvent::Line { line, .. }) if line == "ready" => break,
                Ok(_) => {}
                Err(error) => panic!("readiness event: {error}"),
            }
        }
        let pgid = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(pgid) = handles[0]
                    .process_groups
                    .lock()
                    .expect("process group lock")[0]
                {
                    break i32::try_from(pgid).expect("pgid fits i32");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("process group registered");

        assert!(handles[0].cancel());
        let events = collect_events(&rx, 1);

        assert!(matches!(
            events.last(),
            Some(TaskEvent::Done { success: false, .. })
        ));
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if !process_group_exists(pgid) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("process group reaped");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_escalates_when_the_group_leader_resists_term() {
        let (tx, rx) = mpsc::channel();
        let handles = spawn_tasks(
            vec![TaskDef::new(
                "resistant task",
                "sh",
                &[
                    "-c",
                    "trap ':' TERM; echo ready; while :; do sleep 100 & wait; done",
                ],
            )],
            false,
            tx,
        );
        loop {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(TaskEvent::Line { line, .. }) if line == "ready" => break,
                Ok(_) => {}
                Err(error) => panic!("readiness event: {error}"),
            }
        }

        let cancel_started = std::time::Instant::now();
        assert!(handles[0].cancel());
        let events = collect_events(&rx, 1);

        assert!(cancel_started.elapsed() >= Duration::from_millis(650));
        assert!(matches!(
            events.last(),
            Some(TaskEvent::Done { success: false, .. })
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn parallel_executor_starts_all_tasks_before_completion() {
        let (tx, rx) = mpsc::channel();
        let _handles = spawn_tasks(
            vec![
                TaskDef::new("first", "sh", &["-c", "sleep 0.05"]),
                TaskDef::new("second", "sh", &["-c", "sleep 0.05"]),
            ],
            true,
            tx,
        );

        let events = collect_events(&rx, 2);
        let second_started = events
            .iter()
            .position(|event| matches!(event, TaskEvent::Started { task: 1 }))
            .expect("second started");
        let first_done = events
            .iter()
            .position(|event| matches!(event, TaskEvent::Done { .. }))
            .expect("first completion");

        assert!(second_started < first_done);
    }

    #[test]
    fn task_events_update_runner_model() {
        let mut tasks = vec![RunningTask {
            label: "test".into(),
            lines: vec![],
            state: TaskState::Pending,
            tail: TailScroll::default(),
        }];

        apply_event(&mut tasks, TaskEvent::Started { task: 0 });
        apply_event(
            &mut tasks,
            TaskEvent::Line {
                task: 0,
                line: "live".into(),
            },
        );
        apply_event(
            &mut tasks,
            TaskEvent::Done {
                task: 0,
                success: true,
            },
        );

        assert_eq!(tasks[0].lines, ["live"]);
        assert_eq!(tasks[0].state, TaskState::Done(true));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_failure_reports_error_and_failed_completion() {
        let (tx, rx) = mpsc::channel();
        let _handles = spawn_tasks(
            vec![TaskDef::new(
                "missing",
                "/definitely/missing/holla-command",
                &[],
            )],
            false,
            tx,
        );

        let events = collect_events(&rx, 1);

        assert!(events.iter().any(
            |event| matches!(event, TaskEvent::Line { line, .. } if line.starts_with("Error: "))
        ));
        assert!(matches!(
            events.last(),
            Some(TaskEvent::Done { success: false, .. })
        ));
    }
}
