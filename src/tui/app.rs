    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs},
};
use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::process::Command;

#[derive(Clone, PartialEq)]
pub enum TaskState {
    Pending,
    Running,
    Done(bool),
}

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
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }
}

struct RunningTask {
    label: String,
    output: Arc<Mutex<Vec<String>>>,
    state: TaskState,
}

pub async fn run_tasks(tasks: Vec<TaskDef>) -> Result<()> {
    run_tui(tasks, false).await
}

pub async fn run_parallel_tasks(tasks: Vec<TaskDef>) -> Result<()> {
    run_tui(tasks, true).await
}

async fn run_tui(task_defs: Vec<TaskDef>, parallel: bool) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let running: Vec<RunningTask> = task_defs
        .iter()
        .map(|td| RunningTask {
            label: td.label.clone(),
            output: Arc::new(Mutex::new(Vec::new())),
            state: TaskState::Pending,
        })
        .collect();

    let running = Arc::new(Mutex::new(running));
    let mut selected = 0usize;
    let mut scroll: u16 = 0;

    for (i, td) in task_defs.into_iter().enumerate() {
        let output_buf = {
            let r = running.lock().unwrap();
            Arc::clone(&r[i].output)
        };
        {
            running.lock().unwrap()[i].state = TaskState::Running;
        }

        let running_clone = Arc::clone(&running);
        tokio::spawn(async move {
            let result = Command::new(&td.program)
                .args(&td.args)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await;

            match result {
                Ok(out) => {
                    let text = format!(
                        "{}{}",
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    );
                    output_buf
                        .lock()
                        .unwrap()
                        .extend(text.lines().map(str::to_owned));
                    let success = out.status.success();
                    running_clone.lock().unwrap()[i].state = TaskState::Done(success);
                }
                Err(e) => {
                    output_buf.lock().unwrap().push(format!("Error: {e}"));
                    running_clone.lock().unwrap()[i].state = TaskState::Done(false);
                }
            }
        });

        if !parallel {
            loop {
                if matches!(running.lock().unwrap()[i].state, TaskState::Done(_)) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    // render loop
    loop {
        terminal.draw(|f| {
            let r = running.lock().unwrap();
            let tab_titles: Vec<Line> = r
                .iter()
                .map(|t| {
                    let (icon, color) = match t.state {
                        TaskState::Pending => ("○", Color::DarkGray),
                        TaskState::Running => ("◉", Color::Yellow),
                        TaskState::Done(true) => ("✓", Color::Green),
                        TaskState::Done(false) => ("✗", Color::Red),
                    };
                    Line::from(vec![
                        Span::styled(icon, Style::default().fg(color)),
                        Span::raw(format!(" {}", t.label)),
                    ])
                })
                .collect();

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(0)])
                .split(f.area());

            let tabs = Tabs::new(tab_titles)
                .select(selected)
                .block(Block::default().borders(Borders::ALL).title(" holla "))
                .highlight_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                );
            f.render_widget(tabs, chunks[0]);

            let output = r[selected].output.lock().unwrap();
            let lines: Vec<Line> = output.iter().map(|l| Line::raw(l.as_str())).collect();
            let total = lines.len() as u16;
            let area_h = chunks[1].height.saturating_sub(2);
            let scroll_offset = scroll.min(total.saturating_sub(area_h));
            let paragraph = Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {} ", r[selected].label)),
                )
                .scroll((scroll_offset, 0));
            f.render_widget(paragraph, chunks[1]);
        })?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let count = running.lock().unwrap().len();
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Left | KeyCode::Char('h') => {
                        selected = selected.saturating_sub(1);
                        scroll = 0;
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        selected = (selected + 1).min(count.saturating_sub(1));
                        scroll = 0;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        scroll = scroll.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        scroll += 1;
                    }
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    // print summary
    println!("\n{}", "─".repeat(50));
    for t in running.lock().unwrap().iter() {
        let (icon, status) = match t.state {
            TaskState::Done(true) => ("✓", "ok"),
            TaskState::Done(false) => ("✗", "failed"),
            _ => ("?", "unknown"),
        };
        println!("  {icon}  {}  [{status}]", t.label);
    }
    println!("{}\n", "─".repeat(50));

    Ok(())
}
