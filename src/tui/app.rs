use crossterm::event::{self, Event, KeyEventKind};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    text::{Line, Span, Text},
};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::io::FromRawFd,
    path::PathBuf,
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
    scroll::{DialogScroll, TailScroll},
    style::{DesignSystem as Theme, PanelChrome, Role},
    widgets::{
        Action as DialogAction, Backdrop, ChoiceDialog, ChoiceDialogState, Dialog, StatusBar,
        StatusBarState, StatusSlot, Tab, Tabs, TabsState, Viewport, render_hint_bar,
    },
};
use tokio::process::Command;

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
    pub working_directory: Option<PathBuf>,
}

impl TaskDef {
    pub fn new(label: impl Into<String>, program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            label: label.into(),
            program: program.into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            working_directory: None,
        }
    }
}

static HEADLESS: AtomicBool = AtomicBool::new(false);

pub fn set_headless(enabled: bool) {
    HEADLESS.store(enabled, Ordering::Release);
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
    /// Write end of the task's pseudo-terminal. `None` once the task has
    /// finished and can no longer receive typed input.
    input: Arc<Mutex<Option<File>>>,
}

impl TaskHandle {
    fn accepts_input(&self) -> bool {
        self.input.lock().expect("task input lock").is_some()
    }

    fn send_input(&self, bytes: &[u8]) -> bool {
        let mut input = self.input.lock().expect("task input lock");
        match input.as_mut() {
            Some(pty) => pty.write_all(bytes).is_ok(),
            None => false,
        }
    }

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
    Interact,
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

static RUNNER_BINDINGS: &[KeyBinding<RunnerKey>] = &[
    KeyBinding::borrowed(
        &[
            KeyChord::plain(KeyCode::Left),
            KeyChord::plain(KeyCode::Char('h')),
        ],
        RunnerKey::PreviousTask,
        Some("previous task"),
        Visibility::Shown,
        Some("←"),
    ),
    KeyBinding::borrowed(
        &[
            KeyChord::plain(KeyCode::Right),
            KeyChord::plain(KeyCode::Char('l')),
        ],
        RunnerKey::NextTask,
        Some("next task"),
        Visibility::Shown,
        Some("→"),
    ),
    KeyBinding::borrowed(
        &[
            KeyChord::plain(KeyCode::Up),
            KeyChord::plain(KeyCode::Char('k')),
        ],
        RunnerKey::ScrollUp,
        Some("scroll"),
        Visibility::Shown,
        Some("↑↓"),
    ),
    KeyBinding::borrowed(
        &[
            KeyChord::plain(KeyCode::Down),
            KeyChord::plain(KeyCode::Char('j')),
        ],
        RunnerKey::ScrollDown,
        None,
        Visibility::HiddenAlias,
        None,
    ),
    KeyBinding::borrowed(
        &[KeyChord::plain(KeyCode::PageUp)],
        RunnerKey::PageUp,
        None,
        Visibility::HiddenAlias,
        None,
    ),
    KeyBinding::borrowed(
        &[KeyChord::plain(KeyCode::PageDown)],
        RunnerKey::PageDown,
        None,
        Visibility::HiddenAlias,
        None,
    ),
    KeyBinding::borrowed(
        &[KeyChord::plain(KeyCode::End)],
        RunnerKey::FollowTail,
        Some("follow tail"),
        Visibility::Shown,
        Some("end"),
    ),
    KeyBinding::borrowed(
        &[KeyChord::plain(KeyCode::Char('i'))],
        RunnerKey::Interact,
        Some("type into task"),
        Visibility::Shown,
        Some("i"),
    ),
    KeyBinding::borrowed(
        &[
            KeyChord::plain(KeyCode::Char('q')),
            KeyChord::plain(KeyCode::Esc),
        ],
        RunnerKey::Quit,
        Some("stop/close"),
        Visibility::Shown,
        Some("q/esc"),
    ),
];
static RUNNER_KEYMAP: Keymap<RunnerKey> = Keymap::from_static(RUNNER_BINDINGS);

static INPUT_BINDINGS: &[KeyBinding<RunnerKey>] = &[KeyBinding::borrowed(
    &[KeyChord::plain(KeyCode::Esc)],
    RunnerKey::Interact,
    Some("back to controls"),
    Visibility::Shown,
    Some("esc"),
)];
static INPUT_KEYMAP: Keymap<RunnerKey> = Keymap::from_static(INPUT_BINDINGS);

static DONE_BINDINGS: &[KeyBinding<RunnerKey>] = &[KeyBinding::borrowed(
    &[
        KeyChord::plain(KeyCode::Char('q')),
        KeyChord::plain(KeyCode::Esc),
    ],
    RunnerKey::Quit,
    Some("close"),
    Visibility::Shown,
    Some("q/esc"),
)];
static DONE_KEYMAP: Keymap<RunnerKey> = Keymap::from_static(DONE_BINDINGS);

pub async fn run_tasks(tasks: Vec<TaskDef>) -> anyhow::Result<()> {
    if HEADLESS.load(Ordering::Acquire) {
        anyhow::ensure!(run_tasks_headless_print(tasks, false).await, "task failed");
        return Ok(());
    }
    run_tui(tasks, false).await
}

pub async fn run_parallel_tasks(tasks: Vec<TaskDef>) -> anyhow::Result<()> {
    if HEADLESS.load(Ordering::Acquire) {
        anyhow::ensure!(run_tasks_headless_print(tasks, true).await, "task failed");
        return Ok(());
    }
    run_tui(tasks, true).await
}

pub(crate) async fn run_tasks_headless(tasks: Vec<TaskDef>) -> bool {
    run_tasks_headless_inner(tasks, false, false).await
}

async fn run_tasks_headless_print(tasks: Vec<TaskDef>, parallel: bool) -> bool {
    run_tasks_headless_inner(tasks, parallel, true).await
}

async fn run_tasks_headless_inner(tasks: Vec<TaskDef>, parallel: bool, print: bool) -> bool {
    let task_count = tasks.len();
    let (tx, rx) = mpsc::channel();
    let handles = spawn_tasks(tasks, parallel, tx);
    let mut supervisor = TaskSupervisor::new(handles);
    let success = tokio::task::spawn_blocking(move || {
        let mut completed = vec![false; task_count];
        while let Ok(event) = rx.recv() {
            match event {
                TaskEvent::Line { line, .. } if print => println!("{line}"),
                TaskEvent::Done { task, success } => completed[task] = success,
                TaskEvent::Started { .. } | TaskEvent::Line { .. } => {}
            }
        }
        completed.into_iter().all(|done| done)
    })
    .await
    .unwrap_or(false);
    supervisor.disarm();
    success
}

fn spawn_tasks(defs: Vec<TaskDef>, parallel: bool, tx: mpsc::Sender<TaskEvent>) -> Vec<TaskHandle> {
    enable_child_subreaper();
    let process_groups = Arc::new(Mutex::new(vec![None; defs.len()]));
    let cancelled = Arc::new(AtomicBool::new(false));
    let handles: Vec<TaskHandle> = (0..defs.len())
        .map(|task| TaskHandle {
            task,
            process_groups: Arc::clone(&process_groups),
            cancelled: Arc::clone(&cancelled),
            input: Arc::new(Mutex::new(None)),
        })
        .collect();

    let executor_handles = handles.clone();
    tokio::spawn(async move {
        let handles = executor_handles;
        if parallel {
            let mut jobs = Vec::with_capacity(defs.len());
            for (task, def) in defs.into_iter().enumerate() {
                jobs.push(tokio::spawn(run_task(
                    task,
                    def,
                    tx.clone(),
                    Arc::clone(&process_groups),
                    Arc::clone(&cancelled),
                    Arc::clone(&handles[task].input),
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
                    Arc::clone(&handles[task].input),
                )
                .await;
            }
        }
    });

    handles
}

const PTY_ROWS: u16 = 24;
const PTY_COLS: u16 = 80;

struct TaskPty {
    /// Master end, moved into the output reader thread.
    master: File,
    /// Second handle on the master end for forwarding typed keys.
    writer: File,
    /// Child-side stdio handles on the slave end.
    stdin: File,
    stdout: File,
    stderr: File,
}

fn open_task_pty() -> std::io::Result<TaskPty> {
    let mut winsize = libc::winsize {
        ws_row: PTY_ROWS,
        ws_col: PTY_COLS,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    // SAFETY: openpty allocates a fresh pseudo-terminal pair into the two
    // caller-provided out-fds. The null termios keeps the platform default
    // line discipline; only the initial window size is overridden.
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut winsize,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // From here every fd is owned by a `File`, so drops clean up on error.
    // SAFETY: `master` and `slave` are freshly created fds from openpty that
    // nothing else owns yet.
    let master = unsafe { File::from_raw_fd(master) };
    let stdin = unsafe { File::from_raw_fd(slave) };
    let stdout = stdin.try_clone()?;
    let stderr = stdin.try_clone()?;
    let writer = master.try_clone()?;
    Ok(TaskPty {
        master,
        writer,
        stdin,
        stdout,
        stderr,
    })
}

/// Maps a key pressed while input forwarding is active to the bytes the task
/// should see on its tty. `None` means the key is ignored (navigation keys).
fn key_to_input_bytes(key: termrock::input::KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Enter => Some(b"\r".to_vec()),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(b"\t".to_vec()),
        KeyCode::Char(mut ch) => {
            if key
                .modifiers
                .contains(termrock::input::KeyModifiers::CONTROL)
            {
                let upper = ch.to_ascii_uppercase();
                if upper.is_ascii_uppercase() || upper == '@' {
                    return Some(vec![upper as u8 & 0x1f]);
                }
                return None;
            }
            if key.modifiers.contains(termrock::input::KeyModifiers::SHIFT)
                && ch.is_ascii_lowercase()
            {
                ch = ch.to_ascii_uppercase();
            }
            let mut buffer = [0u8; 4];
            Some(ch.encode_utf8(&mut buffer).as_bytes().to_vec())
        }
        _ => None,
    }
}

/// Recognizes interactive password prompts (sudo and friends) in task output
/// so the runner can point the user at them.
fn is_password_prompt(line: &str) -> bool {
    let trimmed = line.trim_end();
    trimmed.to_ascii_lowercase().contains("password") && trimmed.ends_with(':')
}

async fn run_task(
    task: usize,
    def: TaskDef,
    tx: mpsc::Sender<TaskEvent>,
    process_groups: Arc<Mutex<Vec<Option<u32>>>>,
    cancelled: Arc<AtomicBool>,
    input_writer: Arc<Mutex<Option<File>>>,
) {
    if cancelled.load(Ordering::Acquire) {
        let _ = tx.send(TaskEvent::Done {
            task,
            success: false,
        });
        return;
    }
    let _ = tx.send(TaskEvent::Started { task });

    let pty = match open_task_pty() {
        Ok(pty) => pty,
        Err(error) => {
            let _ = tx.send(TaskEvent::Line {
                task,
                line: format!("Error: could not create terminal: {error}"),
            });
            let _ = tx.send(TaskEvent::Done {
                task,
                success: false,
            });
            return;
        }
    };

    let mut command = Command::new(&def.program);
    command
        .args(&def.args)
        .stdin(Stdio::from(pty.stdin))
        .stdout(Stdio::from(pty.stdout))
        .stderr(Stdio::from(pty.stderr));
    // SAFETY: runs once in the forked child before exec. setsid detaches the
    // child into its own session and process group (replacing the previous
    // process_group(0), which must not run first because a child that is
    // already a group leader cannot setsid) and TIOCSCTTY promotes the
    // already-dup2'd pty slave on fd 0 to its controlling terminal, which
    // sudo and other prompt-based tools require to read credentials.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    if let Some(directory) = &def.working_directory {
        command.current_dir(directory);
    }
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
            *input_writer.lock().expect("task input lock") = None;
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
                input: Arc::clone(&input_writer),
            }
            .cancel();
        }
    }

    *input_writer.lock().expect("task input lock") = Some(pty.writer);
    // The reader thread owns the master end and stops once the executor sets
    // the flag below — BSD masters do not reliably deliver EOF/EIO when the
    // last slave closes, so termination is signaled explicitly.
    let reader_stopped = Arc::new(AtomicBool::new(false));
    let reader_tx = tx.clone();
    let thread_stopped = Arc::clone(&reader_stopped);
    std::thread::spawn(move || stream_pty_output(task, pty.master, reader_tx, thread_stopped));
    let status = child.wait().await;
    if cancelled.load(Ordering::Acquire)
        && let Some(pgid) = registered_pgid
    {
        reap_process_group(pgid).await;
    }
    *input_writer.lock().expect("task input lock") = None;
    reader_stopped.store(true, Ordering::Release);
    process_groups.lock().expect("process group lock")[task] = None;
    let was_cancelled = cancelled.load(Ordering::Acquire);
    let success = !was_cancelled && status.as_ref().is_ok_and(|status| status.success());
    if !was_cancelled && !success {
        let detail = match &status {
            Ok(status) => format!("process exited with {status}"),
            Err(error) => format!("could not wait for process: {error}"),
        };
        let _ = tx.send(TaskEvent::Line {
            task,
            line: format!("Error: {detail}"),
        });
    }
    let _ = tx.send(TaskEvent::Done { task, success });
}

/// Reads raw pty master output and forwards it as line events. Uses a poll
/// loop with a stop flag because master reads do not reliably observe slave
/// closure on all platforms; the executor raises the flag after reaping.
/// Carriage returns from tty-style progress updates are stripped; ANSI
/// escapes are preserved for the renderer.
fn stream_pty_output(
    task: usize,
    mut master: File,
    tx: mpsc::Sender<TaskEvent>,
    stop: Arc<AtomicBool>,
) {
    use std::os::unix::io::AsRawFd;

    let mut buffer = [0u8; 8192];
    let mut pending: Vec<u8> = Vec::new();
    let send_line = |pending: &mut Vec<u8>, tx: &mpsc::Sender<TaskEvent>| -> bool {
        while let Some(position) = pending.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = pending.drain(..=position).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if tx
                .send(TaskEvent::Line {
                    task,
                    line: String::from_utf8_lossy(&line).into_owned(),
                })
                .is_err()
            {
                return false;
            }
        }
        true
    };
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let mut fds = [libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        // SAFETY: poll only waits on the pty master fd this thread owns.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, 50) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                break;
            }
            continue;
        }
        if ready == 0 {
            continue;
        }
        if fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) == 0 {
            continue;
        }
        match master.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                pending.extend_from_slice(&buffer[..read]);
                if !send_line(&mut pending, &tx) {
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = send_line(&mut pending, &tx);
    if !pending.is_empty() {
        let _ = tx.send(TaskEvent::Line {
            task,
            line: String::from_utf8_lossy(&pending).into_owned(),
        });
    }
}

async fn reap_process_group(pgid: i32) {
    // SAFETY: this is the positive id of the process group created for the
    // cancelled task. Killing the whole group removes descendants even when
    // the direct child already exited or closed its output pipes.
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
    while process_group_exists(pgid) {
        reap_adopted_group_children(pgid);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    reap_adopted_group_children(pgid);
}

#[cfg(target_os = "linux")]
fn enable_child_subreaper() {
    use std::sync::Once;

    static ENABLE: Once = Once::new();
    ENABLE.call_once(|| {
        // SAFETY: PR_SET_CHILD_SUBREAPER changes only this process's child
        // reparenting policy. It lets the executor adopt and reap orphaned
        // descendants instead of depending on the host's PID 1 behavior.
        unsafe {
            libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
        }
    });
}

#[cfg(not(target_os = "linux"))]
fn enable_child_subreaper() {}

#[cfg(target_os = "linux")]
fn reap_adopted_group_children(pgid: i32) {
    loop {
        let mut status = 0;
        // SAFETY: a negative pid restricts waiting to children in this task's
        // process group; WNOHANG makes the call non-blocking. Adopted children
        // are ours because holla enabled PR_SET_CHILD_SUBREAPER before spawn.
        let result = unsafe { libc::waitpid(-pgid, &mut status, libc::WNOHANG) };
        if result <= 0 {
            break;
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn reap_adopted_group_children(_pgid: i32) {}

fn process_group_exists(pgid: i32) -> bool {
    // SAFETY: signal 0 performs existence/permission checking only.
    let result = unsafe { libc::kill(-pgid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
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
    let events = rx;
    let theme = Theme::phosphor();
    let mut selected = 0usize;
    let mut tabs_state = TabsState::default();
    tabs_state.selected = Some(selected);
    tabs_state.focused = true;
    let mut status_state = StatusBarState::default();
    let mut cancel_dialog: Option<ChoiceDialogState<CancelChoice>> = None;
    // Task currently receiving typed keys; `Some` disables runner shortcuts
    // so passwords and prompts reach the child untouched.
    let mut input_target: Option<usize> = None;
    // Task whose output last looked like a password prompt.
    let mut prompt_task: Option<usize> = None;
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
        while let Ok(event) = events.try_recv() {
            if let TaskEvent::Line { task, line } = &event {
                if is_password_prompt(line) {
                    prompt_task = Some(*task);
                    if !matches!(tasks[*task].state, TaskState::Done(_)) {
                        selected = *task;
                    }
                } else if prompt_task == Some(*task) {
                    // The prompt was answered (or abandoned); a follow-up
                    // non-prompt line means the child moved on.
                    prompt_task = None;
                }
            }
            if let TaskEvent::Done { task, .. } = &event {
                if input_target == Some(*task) {
                    input_target = None;
                }
                if prompt_task == Some(*task) {
                    prompt_task = None;
                }
            }
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
            let mut counts = if done {
                format!("done — {ok} ok, {failed} failed — press q to close ")
            } else {
                format!("{running} running, {ok} ok, {failed} failed ")
            };
            if let Some(target) = input_target {
                counts.push_str("⌨ typing → ");
                counts.push_str(&tasks[target].label);
                counts.push_str(" (esc ends) ");
            } else if let Some(task) = prompt_task
                && matches!(
                    tasks[task].state,
                    TaskState::Running | TaskState::Cancelling
                )
            {
                counts.push_str(&format!(
                    "⌨ password prompt — press i: {}",
                    tasks[task].label
                ));
            }
            let left_slots = [StatusSlot {
                id: StatusSlotId::Product,
                content: " holla tasks ",
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
                id: StatusSlotId::Counts,
                content: &counts,
                priority: 1,
                min_width: 10,
                enabled: true,
                region: termrock::widgets::StatusRegion::Left,
                kind: termrock::widgets::StatusKind::Text,
                glyph: None,
                style_explicit: true,
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
                    badge: None,
                    status: termrock::widgets::TabStatus::None,
                    closable: false,
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
            let mut viewport_state = DialogScroll::default();
            viewport_state.scroll_y = u16::try_from(top).unwrap_or(u16::MAX);
            frame.render_stateful_widget(
                &Viewport::new(&output_lines, &theme)
                    .title(tasks[selected].label.as_str())
                    .emphasis(PanelChrome::Focused)
                    .content_style(theme.style(Role::Text)),
                output_area,
                &mut viewport_state,
            );

            let hints = if input_target.is_some() {
                INPUT_KEYMAP.hint_spans()
            } else if done {
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
                        .emphasis(PanelChrome::Focused),
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
            if input_target.is_some() {
                // Input forwarding: every keystroke belongs to the task, so
                // runner shortcuts are suspended. Esc is the only exit.
                if key.code == KeyCode::Esc {
                    input_target = None;
                } else if let Some(target) = input_target
                    && let Some(bytes) = key_to_input_bytes(key)
                {
                    let _ = supervisor.handles[target].send_input(&bytes);
                }
                continue;
            }
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
                RunnerKey::Interact => {
                    let handle = &supervisor.handles[selected];
                    input_target = if !done && handle.accepts_input() {
                        Some(selected)
                    } else {
                        None
                    };
                }
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
            input: Arc::new(Mutex::new(None)),
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

        assert!(
            events.iter().any(
                |event| matches!(event, TaskEvent::Line { line, .. } if line.starts_with("Error: "))
            ),
            "events: {events:?}"
        );
        assert!(matches!(
            events.last(),
            Some(TaskEvent::Done { success: false, .. })
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn typed_input_reaches_the_task_and_output_streams_back() {
        let (tx, rx) = mpsc::channel();
        let handles = spawn_tasks(
            vec![TaskDef::new(
                "echo",
                "sh",
                &["-c", "read -r line; echo got:$line"],
            )],
            false,
            tx,
        );

        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(TaskEvent::Started { task: 0 })
        ));

        // The pty writer is registered shortly after spawn; wait for it.
        let mut sent = false;
        for _ in 0..100 {
            if handles[0].send_input(b"hello\n") {
                sent = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(sent, "task never accepted input");

        let mut saw_echo = false;
        loop {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(TaskEvent::Line { line, .. }) if line == "got:hello" => saw_echo = true,
                Ok(TaskEvent::Done { success, .. }) => {
                    assert!(success);
                    break;
                }
                Ok(_) => {}
                Err(error) => panic!("task event: {error} (saw echo: {saw_echo})"),
            }
        }
        assert!(saw_echo);

        // Finished tasks stop accepting input.
        assert!(!handles[0].accepts_input());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn task_can_read_from_its_controlling_terminal_like_sudo() {
        let (tx, rx) = mpsc::channel();
        let handles = spawn_tasks(
            vec![TaskDef::new(
                "prompt",
                "sh",
                &[
                    "-c",
                    "echo ready; read -r line < /dev/tty && echo got:$line",
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

        let mut sent = false;
        for _ in 0..100 {
            if handles[0].send_input(b"hunter2\n") {
                sent = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(sent, "task never accepted input");

        let mut saw_prompt_answer = false;
        loop {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(TaskEvent::Line { line, .. }) if line == "got:hunter2" => {
                    saw_prompt_answer = true;
                }
                Ok(TaskEvent::Done { success, .. }) => {
                    assert!(success);
                    break;
                }
                Ok(_) => {}
                Err(error) => panic!("task event: {error} (answered: {saw_prompt_answer})"),
            }
        }
        assert!(saw_prompt_answer, "child could not read /dev/tty");
    }

    #[test]
    fn key_mapping_covers_password_typing() {
        use termrock::input::KeyModifiers;

        let plain = |ch: char| termrock::input::KeyEvent {
            kind: termrock::input::KeyEventKind::Press,
            code: KeyCode::Char(ch),
            modifiers: KeyModifiers::NONE,
            state: termrock::input::KeyEventState::NONE,
        };
        let with = |code, modifiers| termrock::input::KeyEvent {
            kind: termrock::input::KeyEventKind::Press,
            code,
            modifiers,
            state: termrock::input::KeyEventState::NONE,
        };

        // Letters that collide with runner shortcuts must forward verbatim.
        assert_eq!(
            key_to_input_bytes(plain('q')).as_deref(),
            Some(b"q".as_slice())
        );
        assert_eq!(
            key_to_input_bytes(with(KeyCode::Char('i'), KeyModifiers::CONTROL)).as_deref(),
            Some([0x09].as_slice())
        );
        assert_eq!(
            key_to_input_bytes(plain('é')).as_deref(),
            Some("é".as_bytes())
        );
        assert_eq!(
            key_to_input_bytes(with(KeyCode::Enter, KeyModifiers::NONE)).as_deref(),
            Some(b"\r".as_slice())
        );
        assert_eq!(
            key_to_input_bytes(with(KeyCode::Backspace, KeyModifiers::NONE)).as_deref(),
            Some([0x7f].as_slice())
        );
        assert_eq!(
            key_to_input_bytes(with(KeyCode::Char('c'), KeyModifiers::CONTROL)).as_deref(),
            Some([0x03].as_slice())
        );
        assert_eq!(
            key_to_input_bytes(with(KeyCode::Char('c'), KeyModifiers::SHIFT)).as_deref(),
            Some(b"C".as_slice())
        );
        assert!(key_to_input_bytes(with(KeyCode::Left, KeyModifiers::NONE)).is_none());
    }

    #[test]
    fn password_prompts_are_recognized() {
        assert!(is_password_prompt("Password:"));
        assert!(is_password_prompt("[sudo] Password for don:"));
        assert!(is_password_prompt("password: "));
        assert!(!is_password_prompt(
            "==> Pouring gogcli--0.38.1.bottle.tar.gz"
        ));
        assert!(!is_password_prompt(""));
    }
}
