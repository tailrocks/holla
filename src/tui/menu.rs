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
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{io, time::Duration};

use crate::probe::Probe;

/// Boxed future returned by an [`Action`] handler.
pub type ActionFuture = std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>>>>;

pub struct Action {
    pub label: String,
    pub description: String,
    pub preview: String,
    pub handler: Box<dyn Fn() -> ActionFuture>,
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

        // ── Current folder ──────────────────────────────────────────────
        let mut current_actions: Vec<Action> = Vec::new();

        if !probe.mise_tasks.is_empty() {
            for task in &probe.mise_tasks {
                let name = task.name.clone();
                let desc = if task.description.is_empty() {
                    format!("Run mise task `{name}`")
                } else {
                    task.description.clone()
                };
                let preview = format!("$ mise run {name}");
                let name_clone = name.clone();
                current_actions.push(Action {
                    label: format!("mise: {name}"),
                    description: desc,
                    preview,
                    handler: Box::new(move || {
                        let n = name_clone.clone();
                        Box::pin(async move { crate::commands::mise::run(&n).await })
                    }),
                });
            }
        }

        if probe.in_git_repo && probe.git {
            current_actions.push(Action {
                label: "git: pull".into(),
                description: "Pull latest changes for this repository".into(),
                preview: "$ git pull".into(),
                handler: Box::new(|| Box::pin(run_shell("git pull"))),
            });
            current_actions.push(Action {
                label: "git: push".into(),
                description: "Push commits to remote".into(),
                preview: "$ git push".into(),
                handler: Box::new(|| Box::pin(run_shell("git push"))),
            });
            current_actions.push(Action {
                label: "git: status".into(),
                description: "Show working tree status".into(),
                preview: "$ git status".into(),
                handler: Box::new(|| Box::pin(run_shell("git status"))),
            });
        }

        if probe.has_gradle_build && probe.gradle {
            current_actions.push(Action {
                label: "gradle: clean".into(),
                description: "Clean build output".into(),
                preview: "$ gradle clean".into(),
                handler: Box::new(|| Box::pin(run_shell("gradle clean"))),
            });
            current_actions.push(Action {
                label: "gradle: build".into(),
                description: "Build the project".into(),
                preview: "$ gradle build".into(),
                handler: Box::new(|| Box::pin(run_shell("gradle build"))),
            });
            current_actions.push(Action {
                label: "gradle: test".into(),
                description: "Run tests".into(),
                preview: "$ gradle test".into(),
                handler: Box::new(|| Box::pin(run_shell("gradle test"))),
            });
        }

        if probe.has_docker_compose && probe.docker {
            current_actions.push(Action {
                label: "compose: up".into(),
                description: "Start services in background".into(),
                preview: "$ docker compose up -d".into(),
                handler: Box::new(|| Box::pin(run_shell("docker compose up -d"))),
            });
            current_actions.push(Action {
                label: "compose: down".into(),
                description: "Stop and remove containers".into(),
                preview: "$ docker compose down".into(),
                handler: Box::new(|| Box::pin(run_shell("docker compose down"))),
            });
            current_actions.push(Action {
                label: "compose: logs".into(),
                description: "Follow service logs".into(),
                preview: "$ docker compose logs -f".into(),
                handler: Box::new(|| Box::pin(run_shell("docker compose logs -f"))),
            });
        }

        if probe.has_idea_dir || probe.idea {
            current_actions.push(Action {
                label: "idea: clean".into(),
                description: "Remove .idea dirs and *.iml files".into(),
                preview:
                    "find . -name .idea -type d ... | rm -rf\nfind . -name '*.iml' ... | rm -f"
                        .into(),
                handler: Box::new(|| Box::pin(crate::commands::idea::clean())),
            });
        }

        if !current_actions.is_empty() {
            groups.push(Group {
                title: "Current folder".into(),
                icon: "",
                actions: current_actions,
            });
        }

        // ── Parent folder ────────────────────────────────────────────────
        if probe.git && probe.parent_git_repos.len() > 1 {
            let repo_list = probe.parent_git_repos.join(", ");
            groups.push(Group {
                title: "Parent folder".into(),
                icon: "",
                actions: vec![
                    Action {
                        label: "git: pull all repos".into(),
                        description: format!(
                            "Pull {} repos in parallel",
                            probe.parent_git_repos.len()
                        ),
                        preview: format!("Repos: {repo_list}\n\n$ git pull (parallel)"),
                        handler: Box::new(|| Box::pin(crate::commands::git::pull_all())),
                    },
                    Action {
                        label: "git: push all repos".into(),
                        description: format!(
                            "Push {} repos in parallel",
                            probe.parent_git_repos.len()
                        ),
                        preview: format!("Repos: {repo_list}\n\n$ git push (parallel)"),
                        handler: Box::new(|| Box::pin(crate::commands::git::push_all())),
                    },
                    Action {
                        label: "git: status all repos".into(),
                        description: "Show status of all repos".into(),
                        preview: format!("Repos: {repo_list}\n\n$ git status --short"),
                        handler: Box::new(|| Box::pin(crate::commands::git::status_all())),
                    },
                    Action {
                        label: "git: push all remotes".into(),
                        description: "Push every repo to origin + gitlab".into(),
                        preview: format!(
                            "Repos: {repo_list}\n\n$ git push origin\n$ git push gitlab"
                        ),
                        handler: Box::new(|| Box::pin(crate::commands::git::push_all_remotes())),
                    },
                ],
            });
        }

        // ── System ───────────────────────────────────────────────────────
        let mut system_actions: Vec<Action> = Vec::new();

        if probe.brew || probe.mise || probe.amp || probe.omz {
            system_actions.push(Action {
                label: "upgrade: everything".into(),
                description: "Upgrade all detected tools in parallel".into(),
                preview: build_upgrade_preview(probe),
                handler: Box::new(|| Box::pin(crate::commands::upgrade::run_all())),
            });
        }
        if probe.brew {
            system_actions.push(Action {
                label: "upgrade: brew packages".into(),
                description: "brew update && brew upgrade".into(),
                preview: "$ brew update\n$ brew upgrade --greedy\n$ brew cleanup\n$ brew autoremove\n$ brew doctor".into(),
                handler: Box::new(|| Box::pin(crate::commands::upgrade::run_brew())),
            });
            system_actions.push(Action {
                label: "upgrade: brew casks".into(),
                description: "Upgrade GUI apps via Homebrew".into(),
                preview: "$ brew update\n$ brew upgrade --cask --greedy\n$ brew cleanup\n$ brew autoremove\n$ brew doctor".into(),
                handler: Box::new(|| Box::pin(crate::commands::upgrade::run_brew_casks())),
            });
        }
        if probe.mise {
            system_actions.push(Action {
                label: "upgrade: mise tools".into(),
                description: "Upgrade all mise-managed tools".into(),
                preview: "$ mise upgrade".into(),
                handler: Box::new(|| Box::pin(crate::commands::upgrade::run_mise())),
            });
        }
        if probe.amp {
            system_actions.push(Action {
                label: "upgrade: amp".into(),
                description: "Upgrade Amp CLI".into(),
                preview: "$ amp update".into(),
                handler: Box::new(|| Box::pin(crate::commands::upgrade::run_amp())),
            });
        }
        if probe.omz {
            system_actions.push(Action {
                label: "upgrade: oh-my-zsh".into(),
                description: "Update oh-my-zsh to latest version".into(),
                preview: "$ omz update".into(),
                handler: Box::new(|| Box::pin(crate::commands::upgrade::run_omz())),
            });
        }
        if probe.docker {
            system_actions.push(Action {
                label: "docker: stop all containers".into(),
                description: "Stop and remove all running containers".into(),
                preview: "$ docker ps -qa | xargs docker stop\n$ docker ps -qa | xargs docker rm"
                    .into(),
                handler: Box::new(|| Box::pin(crate::commands::docker::stop_all())),
            });
            system_actions.push(Action {
                label: "docker: clean everything".into(),
                description: "Stop/remove containers, force-remove all images, prune networks/system/volumes (matches legacy docker_clean_all)".into(),
                preview: "$ docker ps -qa | xargs docker stop\n$ docker ps -qa | xargs docker rm\n$ docker rmi --force $(docker images -qa)\n$ docker network rm ...\n$ docker system prune --force\n$ docker volume prune --force".into(),
                handler: Box::new(|| Box::pin(crate::commands::docker::clean())),
            });
        }
        if probe.gradle {
            system_actions.push(Action {
                label: "gradle: clean all".into(),
                description: "Stop daemon and clean all build dirs recursively".into(),
                preview: "$ gradle --stop\n$ find . -name .gradle -exec rm -rf\n$ find . -name build -exec rm -rf".into(),
                handler: Box::new(|| Box::pin(crate::commands::gradle::clean())),
            });
        }

        if !system_actions.is_empty() {
            groups.push(Group {
                title: "System".into(),
                icon: "",
                actions: system_actions,
            });
        }

        Self { groups }
    }
}

fn build_upgrade_preview(probe: &Probe) -> String {
    let mut lines = vec!["Runs in parallel:".to_string()];
    if probe.omz {
        lines.push("  $ omz update".into());
    }
    if probe.mise {
        lines.push("  $ mise upgrade".into());
    }
    if probe.amp {
        lines.push("  $ amp update".into());
    }
    if probe.brew {
        lines.push("  $ brew update && brew upgrade --greedy && brew cleanup && brew autoremove && brew doctor".into());
    }
    lines.join("\n")
}

async fn run_shell(cmd: &str) -> anyhow::Result<()> {
    use crate::tui::{TaskDef, run_tasks};
    run_tasks(vec![TaskDef {
        label: cmd.to_owned(),
        program: "sh".into(),
        args: vec!["-c".into(), cmd.to_owned()],
    }])
    .await
}

pub async fn run(menu: Menu) -> anyhow::Result<()> {
    if menu.groups.is_empty() {
        println!("No supported tools or context detected.");
        return Ok(());
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut group_idx: usize = 0;
    let mut action_idx: usize = 0;
    let mut focus_left = true;
    let mut group_state = ListState::default();
    let mut action_state = ListState::default();
    group_state.select(Some(0));
    action_state.select(Some(0));

    let result = loop {
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
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let header = Paragraph::new(Line::from(vec![
                Span::styled(
                    " holla ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(&cwd, Style::default().fg(Color::DarkGray)),
            ]))
            .block(Block::default().borders(Borders::ALL));
            f.render_widget(header, chunks[0]);

            // body: scope | actions | preview
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(20),
                    Constraint::Percentage(40),
                    Constraint::Percentage(40),
                ])
                .split(chunks[1]);

            render_groups(
                f,
                body[0],
                groups,
                group_idx,
                focus_left,
                &mut group_state.clone(),
            );
            render_actions(
                f,
                body[1],
                current_group,
                action_idx,
                !focus_left,
                &mut action_state.clone(),
            );
            render_preview(f, body[2], current_group, action_idx);

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

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
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
                KeyCode::Right | KeyCode::Tab => focus_left = false,
                KeyCode::Left => focus_left = true,
                KeyCode::Enter => {
                    if focus_left {
                        focus_left = false;
                    } else {
                        break Some((group_idx, action_idx));
                    }
                }
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    if let Some((gi, ai)) = result {
        (menu.groups[gi].actions[ai].handler)().await?;
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
                .title(" Scope "),
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
        .map(|a| ListItem::new(Line::raw(&a.label)))
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
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, state);
}

fn render_preview(f: &mut ratatui::Frame, area: Rect, group: &Group, selected: usize) {
    let action = &group.actions[selected];

    let text = vec![
        Line::from(Span::styled(
            &action.label,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            &action.description,
            Style::default().fg(Color::White),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "Command",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::UNDERLINED),
        )),
    ]
    .into_iter()
    .chain(action.preview.lines().map(|l| {
        Line::from(Span::styled(
            l.to_owned(),
            Style::default().fg(Color::Yellow),
        ))
    }))
    .collect::<Vec<_>>();

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Preview "),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::MiseTask;

    fn group<'a>(menu: &'a Menu, title: &str) -> &'a Group {
        menu.groups
            .iter()
            .find(|group| group.title == title)
            .unwrap_or_else(|| panic!("missing group {title}"))
    }

    fn action_labels(group: &Group) -> Vec<&str> {
        group
            .actions
            .iter()
            .map(|action| action.label.as_str())
            .collect()
    }

    #[test]
    fn empty_probe_builds_empty_menu() {
        assert!(Menu::build(&Probe::empty()).groups.is_empty());
    }

    #[test]
    fn docker_probe_builds_system_cleanup_actions() {
        let mut probe = Probe::empty();
        probe.docker = true;

        let menu = Menu::build(&probe);
        let labels = action_labels(group(&menu, "System"));

        assert!(labels.contains(&"docker: stop all containers"));
        assert!(labels.contains(&"docker: clean everything"));
    }

    #[test]
    fn git_repo_builds_current_folder_actions() {
        let mut probe = Probe::empty();
        probe.git = true;
        probe.in_git_repo = true;

        let menu = Menu::build(&probe);
        let labels = action_labels(group(&menu, "Current folder"));

        assert!(labels.contains(&"git: pull"));
        assert!(labels.contains(&"git: push"));
        assert!(labels.contains(&"git: status"));
    }

    #[test]
    fn mise_task_builds_action_with_command_preview() {
        let mut probe = Probe::empty();
        probe.mise_tasks.push(MiseTask {
            name: "build".into(),
            description: "Build app".into(),
        });

        let menu = Menu::build(&probe);
        let action = group(&menu, "Current folder")
            .actions
            .iter()
            .find(|action| action.label == "mise: build")
            .expect("mise action");

        assert_eq!(action.preview, "$ mise run build");
    }
}
