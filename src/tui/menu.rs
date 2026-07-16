use crossterm::event::{self, Event, KeyEventKind};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    text::Line,
};
use std::time::Duration;
use termrock::{
    keymap::{KeyBinding, KeyChord, Keymap, LogicalKey, Visibility},
    scroll::DialogScroll,
    style::{Role, Theme},
    widgets::{
        List, ListOutcome, ListRow, ListState, Panel, PanelEmphasis, RowRole, StatusBar,
        StatusBarState, StatusSlot, Viewport, render_hint_bar,
    },
};

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
                description: "Show recent service logs".into(),
                preview: "$ docker compose logs --tail 200".into(),
                handler: Box::new(|| Box::pin(run_shell("docker compose logs --tail 200"))),
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

        // ── Repositories in this folder ─────────────────────────────────
        if probe.git && probe.child_git_repos.len() > 1 {
            let repo_list = probe.child_git_repos.join(", ");
            let pull_repos = probe.child_git_repos.clone();
            let push_repos = probe.child_git_repos.clone();
            let status_repos = probe.child_git_repos.clone();
            let remote_repos = probe.child_git_repos.clone();
            groups.push(Group {
                title: "Repos in this folder".into(),
                icon: "",
                actions: vec![
                    Action {
                        label: "git: pull all repos".into(),
                        description: format!(
                            "Pull {} repos in parallel",
                            probe.child_git_repos.len()
                        ),
                        preview: format!("Repos: {repo_list}\n\n$ git pull (parallel)"),
                        handler: Box::new(move || {
                            let repos = pull_repos.clone();
                            Box::pin(async move { crate::commands::git::pull_all(&repos).await })
                        }),
                    },
                    Action {
                        label: "git: push all repos".into(),
                        description: format!(
                            "Push {} repos in parallel",
                            probe.child_git_repos.len()
                        ),
                        preview: format!("Repos: {repo_list}\n\n$ git push (parallel)"),
                        handler: Box::new(move || {
                            let repos = push_repos.clone();
                            Box::pin(async move { crate::commands::git::push_all(&repos).await })
                        }),
                    },
                    Action {
                        label: "git: status all repos".into(),
                        description: "Show status of all repos".into(),
                        preview: format!("Repos: {repo_list}\n\n$ git status --short"),
                        handler: Box::new(move || {
                            let repos = status_repos.clone();
                            Box::pin(async move { crate::commands::git::status_all(&repos).await })
                        }),
                    },
                    Action {
                        label: "git: push all remotes".into(),
                        description: "Push every repo to origin + gitlab".into(),
                        preview: format!(
                            "Repos: {repo_list}\n\n$ git push origin\n$ git push gitlab"
                        ),
                        handler: Box::new(move || {
                            let repos = remote_repos.clone();
                            Box::pin(
                                async move { crate::commands::git::push_all_remotes(&repos).await },
                            )
                        }),
                    },
                ],
            });
        }

        // ── System ───────────────────────────────────────────────────────
        let mut system_actions: Vec<Action> = Vec::new();

        if probe.brew || probe.mise || probe.amp || probe.omz_dir.is_some() {
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
        if let Some(omz_dir) = &probe.omz_dir {
            let omz_dir = omz_dir.clone();
            system_actions.push(Action {
                label: "upgrade: oh-my-zsh".into(),
                description: "Update oh-my-zsh to latest version".into(),
                preview: "$ sh ~/.oh-my-zsh/tools/upgrade.sh".into(),
                handler: Box::new(move || {
                    Box::pin(crate::commands::upgrade::run_omz(omz_dir.clone()))
                }),
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
    if probe.omz_dir.is_some() {
        lines.push("  $ sh ~/.oh-my-zsh/tools/upgrade.sh".into());
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

type ActionId = (usize, usize);

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
    Directory,
}

static MENU_KEYMAP: Keymap<MenuKey> = Keymap::new(&[
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
            KeyChord::plain(LogicalKey::Esc),
            KeyChord::plain(LogicalKey::Char('q')),
        ],
        action: MenuKey::Quit,
        hint: Some("quit"),
        visibility: Visibility::Shown,
        glyph: Some("esc/q"),
    },
]);

fn menu_rows<'a>(menu: &'a Menu, theme: &Theme) -> Vec<ListRow<'a, ActionId>> {
    let mut rows = Vec::new();
    for (group_index, group) in menu.groups.iter().enumerate() {
        rows.push(ListRow {
            id: (group_index, usize::MAX),
            label: Line::styled(
                if group.icon.is_empty() {
                    group.title.clone()
                } else {
                    format!("{} {}", group.icon, group.title)
                },
                theme.style(Role::TextMuted),
            ),
            role: RowRole::Separator,
            enabled: false,
        });
        rows.extend(
            group
                .actions
                .iter()
                .enumerate()
                .map(|(action_index, action)| ListRow {
                    id: (group_index, action_index),
                    label: Line::raw(action.label.as_str()),
                    role: RowRole::Item,
                    enabled: true,
                }),
        );
    }
    rows
}

fn preview_lines(menu: &Menu, selected: Option<ActionId>, theme: &Theme) -> Vec<Line<'static>> {
    let Some((group_index, action_index)) = selected else {
        return vec![Line::styled(
            "No action selected",
            theme.style(Role::TextMuted),
        )];
    };
    let action = &menu.groups[group_index].actions[action_index];
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

pub async fn run(menu: Menu) -> anyhow::Result<()> {
    if menu.groups.is_empty() {
        println!("No supported tools or context detected.");
        return Ok(());
    }

    let theme = Theme::tailrocks_phosphor();
    let rows = menu_rows(&menu, &theme);
    let first_action = rows.iter().find(|row| row.enabled).map(|row| row.id);
    let mut list_state = ListState::new(first_action);
    let mut preview_scroll = DialogScroll::new();
    let mut preview_focused = false;
    let mut status_state = StatusBarState::default();
    let cwd = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default();

    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;

    let result = loop {
        list_state.focused = !preview_focused;
        let preview = preview_lines(&menu, list_state.selected, &theme);
        let preview_width = termrock::max_line_width(&preview);
        let mut preview_viewport = (0usize, 0usize);
        terminal.draw(|f| {
            let [header_area, body_area, footer_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .areas(f.area());
            let [list_area, preview_area] =
                Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .areas(body_area);

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
                id: HeaderSlot::Directory,
                content: &cwd,
                priority: 1,
                min_width: 8,
                enabled: !cwd.is_empty(),
                style: theme.style(Role::TextMuted),
                hover_style: None,
            }];
            let status = StatusBar {
                left: &left_slots,
                right: &right_slots,
                style: theme.style(Role::Surface),
                alpha: 1.0,
            };
            f.render_stateful_widget(&status, header_area, &mut status_state);

            let list_panel = Panel::new(&theme)
                .title(" holla ")
                .emphasis(if preview_focused {
                    PanelEmphasis::Normal
                } else {
                    PanelEmphasis::Focused
                });
            let list_inner = list_panel.inner(list_area);
            f.render_widget(&list_panel, list_area);
            f.render_stateful_widget(
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
            f.render_stateful_widget(
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

            render_hint_bar(f, footer_area, &MENU_KEYMAP.hint_spans());
        })?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let key = termrock::input::KeyEvent::from(key);
            let chord = KeyChord::from(key);
            if MENU_KEYMAP.dispatch(chord) == Some(MenuKey::Quit) {
                break None;
            }
            if preview_focused {
                if matches!(
                    key.code,
                    termrock::input::KeyCode::Tab | termrock::input::KeyCode::Left
                ) {
                    preview_focused = false;
                    continue;
                }
                preview_scroll.handle_key(
                    key,
                    preview.len(),
                    preview_viewport.0,
                    preview_width,
                    preview_viewport.1,
                );
                continue;
            }
            match list_state.handle_key(&rows, key) {
                ListOutcome::Activated(id) => break Some(id),
                ListOutcome::Cancelled => break None,
                ListOutcome::Changed => preview_scroll = DialogScroll::new(),
                ListOutcome::Ignored => {
                    if MENU_KEYMAP.dispatch(chord) == Some(MenuKey::Preview) {
                        preview_focused = true;
                    }
                }
            }
        }
    };

    drop(terminal);
    session.restore()?;

    if let Some((gi, ai)) = result {
        (menu.groups[gi].actions[ai].handler)().await?;
    }

    Ok(())
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

    #[test]
    fn multiple_child_repositories_build_repo_group() {
        let mut probe = Probe::empty();
        probe.git = true;
        probe.child_git_repos = vec!["beta".into(), "alpha".into()];

        let menu = Menu::build(&probe);

        assert_eq!(
            action_labels(group(&menu, "Repos in this folder")),
            vec![
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

        let menu = Menu::build(&probe);

        assert!(
            menu.groups
                .iter()
                .all(|group| group.title != "Repos in this folder")
        );
    }

    #[test]
    fn omz_directory_builds_upgrade_action() {
        let mut probe = Probe::empty();
        probe.omz_dir = Some("/tmp/.oh-my-zsh".into());

        let menu = Menu::build(&probe);
        let action = group(&menu, "System")
            .actions
            .iter()
            .find(|action| action.label == "upgrade: oh-my-zsh")
            .expect("oh-my-zsh action");

        assert_eq!(action.preview, "$ sh ~/.oh-my-zsh/tools/upgrade.sh");
    }

    #[test]
    fn missing_omz_directory_omits_upgrade_action() {
        let menu = Menu::build(&Probe::empty());

        assert!(
            menu.groups
                .iter()
                .flat_map(|group| &group.actions)
                .all(|action| action.label != "upgrade: oh-my-zsh")
        );
    }

    #[test]
    fn compose_logs_action_is_bounded() {
        let mut probe = Probe::empty();
        probe.docker = true;
        probe.has_docker_compose = true;

        let menu = Menu::build(&probe);
        let action = group(&menu, "Current folder")
            .actions
            .iter()
            .find(|action| action.label == "compose: logs")
            .expect("compose logs action");

        assert_eq!(action.description, "Show recent service logs");
        assert_eq!(action.preview, "$ docker compose logs --tail 200");
    }

    #[test]
    fn menu_rows_flatten_groups_and_actions_with_stable_ids() {
        fn action(label: &str) -> Action {
            Action {
                label: label.into(),
                description: String::new(),
                preview: String::new(),
                handler: Box::new(|| Box::pin(async { Ok(()) })),
            }
        }

        let menu = Menu {
            groups: vec![
                Group {
                    title: "First".into(),
                    icon: "",
                    actions: vec![action("one"), action("two")],
                },
                Group {
                    title: "Second".into(),
                    icon: "",
                    actions: vec![action("three"), action("four")],
                },
            ],
        };

        let rows = menu_rows(&menu, &termrock::Theme::default());

        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0].role, termrock::widgets::RowRole::Separator);
        assert_eq!(rows[0].id, (0, usize::MAX));
        assert!(!rows[0].enabled);
        assert_eq!(rows[1].id, (0, 0));
        assert_eq!(rows[1].role, termrock::widgets::RowRole::Item);
        assert!(rows[1].enabled);
        assert_eq!(rows[2].id, (0, 1));
        assert_eq!(rows[2].role, termrock::widgets::RowRole::Item);
        assert!(rows[2].enabled);
        assert_eq!(rows[3].role, termrock::widgets::RowRole::Separator);
        assert_eq!(rows[3].id, (1, usize::MAX));
        assert!(!rows[3].enabled);
        assert_eq!(rows[4].id, (1, 0));
        assert_eq!(rows[4].role, termrock::widgets::RowRole::Item);
        assert!(rows[4].enabled);
        assert_eq!(rows[5].id, (1, 1));
        assert_eq!(rows[5].role, termrock::widgets::RowRole::Item);
        assert!(rows[5].enabled);
    }
}
