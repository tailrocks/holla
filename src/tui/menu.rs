use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use std::{io, time::Duration};

use crate::probe::Probe;

pub struct Action {
    pub label: String,
    pub description: String,
    pub handler: Box<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>>>>>,
}

pub struct Group {
    pub title: String,
    pub icon: &'static str,
    pub actions: Vec<Action>,
}

pub struct Menu {
    pub groups: Vec<Group>,
}

impl Menu {
    pub fn build(probe: &Probe) -> Self {
        let mut groups: Vec<Group> = Vec::new();

        // macOS system group
        if probe.brew || probe.mise || probe.amp {
            let mut actions: Vec<Action> = Vec::new();

            if probe.brew || probe.mise || probe.amp {
                actions.push(Action {
                    label: "Upgrade everything".into(),
                    description: "Upgrade all detected tools in parallel".into(),
                    handler: Box::new(|| {
                        Box::pin(crate::commands::upgrade::run_all())
                    }),
                });
            }
            if probe.brew {
                actions.push(Action {
                    label: "Upgrade brew packages".into(),
                    description: "brew update && brew upgrade".into(),
                    handler: Box::new(|| Box::pin(crate::commands::upgrade::run_brew())),
                });
                actions.push(Action {
                    label: "Upgrade brew casks".into(),
                    description: "brew upgrade --cask --greedy".into(),
                    handler: Box::new(|| Box::pin(crate::commands::upgrade::run_brew_casks())),
                });
            }
            if probe.mise {
                actions.push(Action {
                    label: "Upgrade mise tools".into(),
                    description: "mise upgrade".into(),
                    handler: Box::new(|| Box::pin(crate::commands::upgrade::run_mise())),
                });
            }
            if probe.amp {
                actions.push(Action {
                    label: "Upgrade Amp CLI".into(),
                    description: "amp update".into(),
                    handler: Box::new(|| Box::pin(crate::commands::upgrade::run_amp())),
                });
            }

            groups.push(Group {
                title: "macOS".into(),
                icon: "",
                actions,
            });
        }

        // Git group — always if git present
        if probe.git {
            groups.push(Group {
                title: "Git".into(),
                icon: "",
                actions: vec![
                    Action {
                        label: "Pull all repos".into(),
                        description: "git pull on every repo in current directory".into(),
                        handler: Box::new(|| Box::pin(crate::commands::git::pull_all())),
                    },
                    Action {
                        label: "Push all repos".into(),
                        description: "git push on every repo in current directory".into(),
                        handler: Box::new(|| Box::pin(crate::commands::git::push_all())),
                    },
                    Action {
                        label: "Push all repos to all remotes".into(),
                        description: "git push origin + gitlab on every repo".into(),
                        handler: Box::new(|| Box::pin(crate::commands::git::push_all_remotes())),
                    },
                    Action {
                        label: "Status all repos".into(),
                        description: "git status on every repo in current directory".into(),
                        handler: Box::new(|| Box::pin(crate::commands::git::status_all())),
                    },
                ],
            });
        }

        // Docker group
        if probe.docker {
            groups.push(Group {
                title: "Docker".into(),
                icon: "",
                actions: vec![
                    Action {
                        label: "Stop & remove all containers".into(),
                        description: "docker stop + rm all containers".into(),
                        handler: Box::new(|| Box::pin(crate::commands::docker::stop_all())),
                    },
                    Action {
                        label: "Clean everything".into(),
                        description: "Remove containers, images, volumes, networks".into(),
                        handler: Box::new(|| Box::pin(crate::commands::docker::clean())),
                    },
                ],
            });
        }

        // Gradle group
        if probe.gradle {
            groups.push(Group {
                title: "Gradle".into(),
                icon: "",
                actions: vec![Action {
                    label: "Clean build directories".into(),
                    description: "gradle --stop + remove .gradle and build dirs".into(),
                    handler: Box::new(|| Box::pin(crate::commands::gradle::clean())),
                }],
            });
        }

        // IntelliJ IDEA cleanup — available if .idea exists nearby
        if std::path::Path::new(".idea").exists() || probe.idea {
            groups.push(Group {
                title: "IntelliJ IDEA".into(),
                icon: "",
                actions: vec![Action {
                    label: "Clean IDEA project files".into(),
                    description: "Remove .idea dirs and *.iml files".into(),
                    handler: Box::new(|| Box::pin(crate::commands::idea::clean())),
                }],
            });
        }

        Self { groups }
    }
}

pub async fn run(menu: Menu) -> Result<()> {
    if menu.groups.is_empty() {
        println!("No supported tools detected in this environment.");
        return Ok(());
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut group_idx: usize = 0;
    let mut action_idx: usize = 0;
    let mut group_state = ListState::default();
    let mut action_state = ListState::default();
    group_state.select(Some(0));
    action_state.select(Some(0));

    let mut focus_left = true; // true = group list focused

    let result: Option<usize> = loop {
        let groups = &menu.groups;
        let current_group = &groups[group_idx];
        let action_count = current_group.actions.len();

        terminal.draw(|f| {
            let area = f.area();

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(0),
                    Constraint::Length(3),
                ])
                .split(area);

            // header
            let header = Paragraph::new(Line::from(vec![
                Span::styled(" holla ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw("— adaptive dev environment"),
            ]))
            .block(Block::default().borders(Borders::ALL));
            f.render_widget(header, chunks[0]);

            // body: left groups | right actions
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                .split(chunks[1]);

            render_groups(f, body[0], groups, group_idx, focus_left, &mut group_state.clone());
            render_actions(f, body[1], current_group, action_idx, !focus_left, &mut action_state.clone());

            // footer
            let footer = Paragraph::new(Line::from(vec![
                Span::styled(" ↑↓ ", Style::default().fg(Color::DarkGray)),
                Span::raw("navigate  "),
                Span::styled("→/Enter ", Style::default().fg(Color::DarkGray)),
                Span::raw("select  "),
                Span::styled("← ", Style::default().fg(Color::DarkGray)),
                Span::raw("back  "),
                Span::styled("q ", Style::default().fg(Color::DarkGray)),
                Span::raw("quit"),
            ]))
            .block(Block::default().borders(Borders::ALL));
            f.render_widget(footer, chunks[2]);
        })?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') => break None,
                    KeyCode::Up => {
                        if focus_left {
                            group_idx = group_idx.saturating_sub(1);
                            action_idx = 0;
                        } else {
                            action_idx = action_idx.saturating_sub(1);
                        }
                    }
                    KeyCode::Down => {
                        if focus_left {
                            group_idx = (group_idx + 1).min(menu.groups.len().saturating_sub(1));
                            action_idx = 0;
                        } else {
                            action_idx = (action_idx + 1).min(action_count.saturating_sub(1));
                        }
                    }
                    KeyCode::Right | KeyCode::Tab => {
                        focus_left = false;
                    }
                    KeyCode::Left => {
                        focus_left = true;
                    }
                    KeyCode::Enter => {
                        if focus_left {
                            focus_left = false;
                        } else {
                            break Some(action_idx);
                        }
                    }
                    _ => {}
                }
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    if let Some(idx) = result {
        let action = &menu.groups[group_idx].actions[idx];
        (action.handler)().await?;
    }

    Ok(())
}

fn render_groups(
    f: &mut ratatui::Frame,
    area: Rect,
    groups: &[Group],
    selected: usize,
    focused: bool,
    state: &mut ListState,
) {
    state.select(Some(selected));
    let items: Vec<ListItem> = groups
        .iter()
        .map(|g| {
            ListItem::new(Line::from(vec![
                Span::raw(format!("{} ", g.icon)),
                Span::raw(&g.title),
            ]))
        })
        .collect();

    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(" Groups "),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, state);
}

fn render_actions(
    f: &mut ratatui::Frame,
    area: Rect,
    group: &Group,
    selected: usize,
    focused: bool,
    state: &mut ListState,
) {
    state.select(Some(selected));
    let items: Vec<ListItem> = group
        .actions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let style = if i == selected && focused {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(vec![
                Line::from(Span::styled(&a.label, style)),
                Line::from(Span::styled(
                    format!("  {}", a.description),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();

    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(format!(" {} {} ", group.icon, group.title)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(list, area, state);
}
