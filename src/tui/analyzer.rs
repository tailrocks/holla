use crate::{
    du::{
        NodeId, NodeState, ScanEvent, ScanHandle, ScanOptions, ScanTree, SortKey,
        cache::{CachedSize, SizeCache, snapshot},
        scan,
    },
    tui::cleanup_flow::{CleanupFlow, CleanupItem, CleanupPoll},
};
use crossterm::event::{self, Event, KeyEventKind};
use humansize::{DECIMAL, format_size};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    text::Line,
    widgets::{Paragraph, Wrap},
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use termrock::{
    input::KeyCode,
    layout::centered_rect,
    style::{Role, Theme},
    widgets::{
        Backdrop, Hint, HintBar, Panel, PanelEmphasis, Progress, ProgressKind, StatusBar,
        StatusBarState, StatusSlot, TextInput, TextInputOutcome, TextInputState, Tree, TreeNode,
        TreeNodeStatus, TreeOutcome, TreeState, Validation,
    },
};

pub const FOLD_SET: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "build",
    "dist",
    ".venv",
    "venv",
    "__pycache__",
    "DerivedData",
    "Pods",
    ".gradle",
    ".next",
    ".turbo",
    ".cache",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderSlot {
    Root,
    Scan,
}

const ANALYZER_HINTS: &[Hint<'static>] = &[
    Hint {
        chord: "↑↓←→",
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
        chord: "f",
        label: "fold",
        priority: 4,
        visible: true,
    },
    Hint {
        chord: "s",
        label: "size",
        priority: 3,
        visible: true,
    },
    Hint {
        chord: "r",
        label: "rescan",
        priority: 2,
        visible: true,
    },
    Hint {
        chord: "d",
        label: "delete",
        priority: 5,
        visible: true,
    },
    Hint {
        chord: "q",
        label: "back",
        priority: 5,
        visible: true,
    },
];

struct CachedNode {
    id: NodeId,
    path: PathBuf,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    size: CachedSize,
}

struct CachedProjection {
    nodes: Vec<CachedNode>,
}

impl CachedProjection {
    fn new(root: &Path, entries: Vec<(PathBuf, CachedSize)>) -> Self {
        let mut entries = entries
            .into_iter()
            .filter(|(path, _)| path == root || path.starts_with(root))
            .collect::<Vec<_>>();
        let present = entries
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<HashSet<_>>();
        entries.retain(|(path, _)| {
            path == root || path.parent().is_some_and(|parent| present.contains(parent))
        });
        let ids = entries
            .iter()
            .enumerate()
            .map(|(index, (path, _))| {
                (
                    path.clone(),
                    NodeId(u32::try_from(index).expect("cache projection exceeded u32 nodes")),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut nodes = entries
            .into_iter()
            .enumerate()
            .map(|(index, (path, size))| CachedNode {
                id: NodeId(u32::try_from(index).expect("cache projection exceeded u32 nodes")),
                parent: path.parent().and_then(|parent| ids.get(parent)).copied(),
                path,
                children: Vec::new(),
                size,
            })
            .collect::<Vec<_>>();
        for index in 0..nodes.len() {
            if let Some(parent) = nodes[index].parent {
                let id = nodes[index].id;
                nodes[parent.0 as usize].children.push(id);
            }
        }
        Self { nodes }
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn root_bytes(&self) -> u64 {
        self.nodes
            .iter()
            .find(|node| node.parent.is_none())
            .map_or(0, |node| node.size.on_disk)
    }

    fn rows(
        &self,
        expanded: &HashSet<NodeId>,
        sort: SortKey,
        now: SystemTime,
        theme: &Theme,
    ) -> Vec<TreeNode<'static, NodeId>> {
        let Some(root) = self.nodes.iter().find(|node| node.parent.is_none()) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        let mut pending = vec![(root.id, 0_u16)];
        while let Some((id, depth)) = pending.pop() {
            let node = &self.nodes[id.0 as usize];
            let is_expanded = !node.children.is_empty() && expanded.contains(&id);
            let age = now
                .duration_since(UNIX_EPOCH + Duration::from_secs(node.size.scanned_at))
                .unwrap_or_default();
            rows.push(TreeNode {
                id,
                label: Line::styled(
                    node.path
                        .file_name()
                        .unwrap_or_else(|| node.path.as_os_str())
                        .to_string_lossy()
                        .into_owned(),
                    theme.style(Role::TextMuted),
                ),
                trailing: Some(Line::styled(
                    format!(
                        "{} · cached {} ago",
                        format_size(node.size.on_disk, DECIMAL),
                        humantime::format_duration(age)
                    ),
                    theme.style(Role::TextMuted),
                )),
                depth,
                branch: !node.children.is_empty(),
                expanded: is_expanded,
                enabled: true,
                status: TreeNodeStatus::Ready,
            });
            if is_expanded {
                let mut children = node.children.clone();
                children.sort_by(|left, right| {
                    let left = &self.nodes[left.0 as usize];
                    let right = &self.nodes[right.0 as usize];
                    match sort {
                        SortKey::OnDisk => right
                            .size
                            .on_disk
                            .cmp(&left.size.on_disk)
                            .then_with(|| left.path.cmp(&right.path)),
                        SortKey::Apparent => right
                            .size
                            .apparent
                            .cmp(&left.size.apparent)
                            .then_with(|| left.path.cmp(&right.path)),
                        SortKey::Name => left.path.cmp(&right.path),
                    }
                });
                pending.extend(
                    children
                        .into_iter()
                        .rev()
                        .map(|child| (child, depth.saturating_add(1))),
                );
            }
        }
        rows
    }

    fn selected(&self, checked: &[NodeId]) -> (Vec<CleanupItem>, u64) {
        let checked = checked
            .iter()
            .filter_map(|id| self.nodes.get(id.0 as usize))
            .collect::<Vec<_>>();
        let mut effective = checked
            .iter()
            .filter(|node| {
                !checked
                    .iter()
                    .any(|other| other.id != node.id && node.path.starts_with(&other.path))
            })
            .map(|node| CleanupItem::new(node.path.clone(), node.size.on_disk))
            .collect::<Vec<_>>();
        effective.sort_by(|left, right| left.path.cmp(&right.path));
        let bytes = effective
            .iter()
            .fold(0_u64, |total, item| total.saturating_add(item.size));
        (effective, bytes)
    }
}

pub async fn run(root: PathBuf) -> anyhow::Result<()> {
    let theme = Theme::tailrocks_phosphor();
    let now = SystemTime::now();
    let cached = CachedProjection::new(&root, SizeCache::load().valid_below(&root, now));
    let mut showing_cache = !cached.is_empty();
    let mut first_frame = true;
    let mut handle = scan(ScanOptions::new(&root));
    let mut expanded = HashSet::from([NodeId(0)]);
    let mut folding = true;
    let mut sort = SortKey::OnDisk;
    let mut tree_state = TreeState::new(Some(NodeId(0)));
    tree_state.enable_multi_select();
    let mut status_state = StatusBarState::default();
    let mut scanning = true;
    let mut bytes_seen = 0_u64;
    let mut inaccessible = 0_u64;
    let mut tick = 0_u64;
    let mut cleanup = CleanupFlow::new();
    let mut cache_write_started = false;
    let mut cache_writes = Vec::new();
    let root_label = root.display().to_string();

    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;

    'screen: loop {
        match cleanup.poll() {
            CleanupPoll::Exit => break 'screen,
            CleanupPoll::Completed => {
                handle.cancel.store(true, Ordering::Release);
                handle = scan(ScanOptions::new(&root));
                showing_cache = false;
                expanded = HashSet::from([NodeId(0)]);
                tree_state = TreeState::new(Some(NodeId(0)));
                tree_state.enable_multi_select();
                scanning = true;
                bytes_seen = 0;
                inaccessible = 0;
                cache_write_started = false;
            }
            CleanupPoll::None => {}
        }
        let drain = if first_frame {
            DrainResult::default()
        } else {
            drain_scan_events(&handle, &mut scanning, &mut bytes_seen, &mut inaccessible)
        };
        if showing_cache && drain.material_changed {
            showing_cache = false;
            expanded = HashSet::from([NodeId(0)]);
            tree_state = TreeState::new(Some(NodeId(0)));
            tree_state.enable_multi_select();
        }
        if drain.finished && !cache_write_started {
            let snapshot = {
                let tree = handle.tree.read().expect("disk scan tree lock");
                snapshot(&root, &tree, SystemTime::now())
            };
            cache_write_started = true;
            let writer = std::thread::Builder::new()
                .name("holla-size-cache-write".into())
                .spawn(move || {
                    let mut cache = SizeCache::load();
                    cache.capture_snapshot(snapshot);
                    if let Err(error) = cache.save() {
                        eprintln!("holla: could not save size cache: {error}");
                    }
                })?;
            cache_writes.push(writer);
        }
        let (rows, root_bytes, selected_count, selected_bytes) = if showing_cache {
            let checked = tree_state
                .selection()
                .map(|selection| selection.checked())
                .unwrap_or_default();
            let (items, selected_bytes) = cached.selected(checked);
            (
                cached.rows(&expanded, sort, SystemTime::now(), &theme),
                cached.root_bytes(),
                items.len(),
                selected_bytes,
            )
        } else {
            let tree = handle.tree.read().expect("disk scan tree lock");
            let checked = tree_state
                .selection()
                .map(|selection| selection.checked())
                .unwrap_or_default();
            let (effective, selected_bytes) = effective_selection(&tree, checked);
            (
                project_tree(&tree, &expanded, folding, sort),
                tree.node(tree.root()).on_disk,
                effective.len(),
                selected_bytes,
            )
        };
        if tree_state.selected().is_none() {
            tree_state.select(rows.first().map(|row| row.id));
        }
        let scan_bytes = bytes_seen.max(root_bytes);
        let scan_copy = if showing_cache {
            format!(
                "refreshing · cached {}",
                format_size(cached.root_bytes(), DECIMAL)
            )
        } else if scanning {
            format!(
                "scanning · {} · {inaccessible} unreadable",
                format_size(scan_bytes, DECIMAL)
            )
        } else {
            format!(
                "complete · {} · {inaccessible} unreadable",
                format_size(root_bytes, DECIMAL)
            )
        };
        let selection_copy = format!(
            "{selected_count} items selected — {} reclaimable · {} · folding {}",
            format_size(selected_bytes, DECIMAL),
            match sort {
                SortKey::OnDisk => "on-disk",
                SortKey::Apparent => "apparent",
                SortKey::Name => "name",
            },
            if folding { "on" } else { "off" }
        );
        let honesty_copy = if inaccessible == 0 {
            "sizes are on-disk; APFS clones may overcount; purgeable space not included".to_owned()
        } else {
            format!(
                "{inaccessible} items unreadable — grant Full Disk Access for a complete picture; APFS clones may overcount; purgeable space not included"
            )
        };

        terminal.draw(|frame| {
            let [
                header_area,
                progress_area,
                tree_area,
                selection_area,
                honesty_area,
                hints_area,
            ] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(frame.area());
            let left = [StatusSlot {
                id: HeaderSlot::Root,
                content: &root_label,
                priority: 2,
                min_width: 8,
                enabled: true,
                style: theme.style(Role::Accent),
                hover_style: None,
            }];
            let right = [StatusSlot {
                id: HeaderSlot::Scan,
                content: &scan_copy,
                priority: 1,
                min_width: 12,
                enabled: true,
                style: theme.style(Role::TextMuted),
                hover_style: None,
            }];
            frame.render_stateful_widget(
                &StatusBar::new(&left, &right, &theme).alpha(1.0),
                header_area,
                &mut status_state,
            );
            if scanning {
                let label = format!("{} scanned", format_size(scan_bytes, DECIMAL));
                frame.render_widget(
                    Progress::new(ProgressKind::Indeterminate { tick }, &theme).label(&label),
                    progress_area,
                );
            } else {
                frame.render_widget(
                    Paragraph::new("Scan complete").style(theme.style(Role::Success)),
                    progress_area,
                );
            }
            let panel = Panel::new(&theme)
                .title(" Disk usage ")
                .emphasis(PanelEmphasis::Focused);
            let inner = panel.inner(tree_area);
            frame.render_widget(&panel, tree_area);
            frame.render_stateful_widget(&Tree::new(&rows, &theme), inner, &mut tree_state);
            frame.render_widget(
                Paragraph::new(selection_copy.as_str()).style(theme.style(Role::Text)),
                selection_area,
            );
            frame.render_widget(
                Paragraph::new(honesty_copy.as_str())
                    .style(theme.style(Role::TextMuted))
                    .wrap(Wrap { trim: false }),
                honesty_area,
            );
            frame.render_widget(
                HintBar::new(ANALYZER_HINTS, &theme).separator(" · "),
                hints_area,
            );
            cleanup.render(frame, &theme, tick);
        })?;

        first_frame = false;
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
                KeyCode::Char('q') | KeyCode::Esc => break 'screen,
                KeyCode::Char('d') | KeyCode::Backspace => {
                    let items = if showing_cache {
                        let checked = tree_state
                            .selection()
                            .map(|selection| selection.checked())
                            .unwrap_or_default();
                        cached.selected(checked).0
                    } else {
                        selected_items(&handle, &tree_state, &root)
                    };
                    if !items.is_empty() {
                        cleanup.open_confirmation(items);
                    }
                }
                KeyCode::Char('f') => folding = !folding,
                KeyCode::Char('s') => {
                    sort = match sort {
                        SortKey::OnDisk => SortKey::Apparent,
                        SortKey::Apparent | SortKey::Name => SortKey::OnDisk,
                    };
                }
                KeyCode::Char('r') => {
                    handle.cancel.store(true, Ordering::Release);
                    handle = scan(ScanOptions::new(&root));
                    expanded = HashSet::from([NodeId(0)]);
                    tree_state = TreeState::new(Some(NodeId(0)));
                    tree_state.enable_multi_select();
                    scanning = true;
                    bytes_seen = 0;
                    inaccessible = 0;
                    cache_write_started = false;
                }
                _ => match tree_state.handle_key(&rows, key) {
                    TreeOutcome::Toggle(id) | TreeOutcome::Activated(id) => {
                        if !expanded.remove(&id) {
                            expanded.insert(id);
                        }
                    }
                    TreeOutcome::Ignored
                    | TreeOutcome::SelectionChanged(_)
                    | TreeOutcome::CheckToggled(_) => {}
                    _ => {}
                },
            }
        }
    }

    handle.cancel.store(true, Ordering::Release);
    drop(terminal);
    session.restore()?;
    for writer in cache_writes {
        if writer.join().is_err() {
            eprintln!("holla: size cache writer panicked");
        }
    }
    Ok(())
}

pub async fn prompt_path() -> anyhow::Result<Option<PathBuf>> {
    let theme = Theme::tailrocks_phosphor();
    let initial = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let mut input = TextInputState::new(initial);
    let mut error_message: Option<String> = None;
    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;

    let selected = loop {
        terminal.draw(|frame| {
            frame.render_widget(Backdrop::default(), frame.area());
            let area = centered_rect(76, 7, frame.area());
            let panel = Panel::new(&theme)
                .title(" Analyze a path ")
                .emphasis(PanelEmphasis::Focused);
            let inner = panel.inner(area);
            frame.render_widget(&panel, area);
            let [copy_area, input_area, error_area, hints_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(inner);
            frame.render_widget(Paragraph::new("Enter an absolute existing path"), copy_area);
            frame.render_stateful_widget(
                &TextInput::new("Path", &theme)
                    .placeholder("/Users/name")
                    .validation(
                        error_message
                            .as_deref()
                            .map_or(Validation::Valid, Validation::Invalid),
                    ),
                input_area,
                &mut input,
            );
            if let Some(error) = error_message.as_deref() {
                frame.render_widget(
                    Paragraph::new(error).style(theme.style(Role::Danger)),
                    error_area,
                );
            }
            frame.render_widget(
                HintBar::new(
                    &[
                        Hint {
                            chord: "enter",
                            label: "analyze",
                            priority: 1,
                            visible: true,
                        },
                        Hint {
                            chord: "esc",
                            label: "cancel",
                            priority: 1,
                            visible: true,
                        },
                    ],
                    &theme,
                )
                .separator(" · "),
                hints_area,
            );
        })?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match input.handle_key(termrock::input::KeyEvent::from(key)) {
                TextInputOutcome::Submitted(value) => {
                    let path = PathBuf::from(value.trim());
                    if !path.is_absolute() {
                        error_message = Some("Path must be absolute".into());
                    } else if !path.exists() {
                        error_message = Some("Path does not exist".into());
                    } else {
                        break Some(path);
                    }
                }
                TextInputOutcome::Cancelled => break None,
                TextInputOutcome::Changed => error_message = None,
                TextInputOutcome::Ignored => {}
                _ => {}
            }
        }
    };

    drop(terminal);
    session.restore()?;
    Ok(selected)
}

#[derive(Default)]
struct DrainResult {
    material_changed: bool,
    finished: bool,
}

fn drain_scan_events(
    handle: &ScanHandle,
    scanning: &mut bool,
    bytes_seen: &mut u64,
    inaccessible: &mut u64,
) -> DrainResult {
    let mut result = DrainResult::default();
    while let Ok(event) = handle.events.try_recv() {
        match event {
            ScanEvent::DirErrored { .. } => {
                *inaccessible = inaccessible.saturating_add(1);
                result.material_changed = true;
            }
            ScanEvent::Progress {
                bytes_seen: seen, ..
            } => *bytes_seen = seen,
            ScanEvent::Finished {
                inaccessible: final_inaccessible,
                ..
            } => {
                *inaccessible = final_inaccessible;
                *scanning = false;
                result.material_changed = true;
                result.finished = true;
            }
            ScanEvent::DirAdded { .. } | ScanEvent::SizesUpdated => {
                result.material_changed = true;
            }
        }
    }
    result
}

pub fn effective_selection(tree: &ScanTree, checked: &[NodeId]) -> (Vec<NodeId>, u64) {
    let checked_set: HashSet<_> = checked.iter().copied().collect();
    let effective: Vec<_> = checked
        .iter()
        .copied()
        .filter(|id| tree.nodes().get(id.0 as usize).is_some())
        .filter(|id| {
            let mut parent = tree.node(*id).parent;
            while let Some(ancestor) = parent {
                if checked_set.contains(&ancestor) {
                    return false;
                }
                parent = tree.node(ancestor).parent;
            }
            true
        })
        .collect();
    let bytes = effective.iter().fold(0_u64, |total, id| {
        total.saturating_add(tree.node(*id).on_disk)
    });
    (effective, bytes)
}

fn selected_items(handle: &ScanHandle, state: &TreeState<NodeId>, root: &Path) -> Vec<CleanupItem> {
    let tree = handle.tree.read().expect("disk scan tree lock");
    let checked = state
        .selection()
        .map(|selection| selection.checked())
        .unwrap_or_default();
    effective_selection(&tree, checked)
        .0
        .into_iter()
        .map(|id| CleanupItem::new(node_path(&tree, root, id), tree.node(id).on_disk))
        .collect()
}

fn node_path(tree: &ScanTree, root: &Path, id: NodeId) -> PathBuf {
    if id == tree.root() {
        return root.to_path_buf();
    }
    let mut names = Vec::new();
    let mut current = Some(id);
    while let Some(node_id) = current {
        if node_id == tree.root() {
            break;
        }
        let node = tree.node(node_id);
        names.push(node.name.clone());
        current = node.parent;
    }
    let mut path = root.to_path_buf();
    for name in names.into_iter().rev() {
        path.push(name);
    }
    path
}

fn project_tree(
    tree: &ScanTree,
    expanded: &HashSet<NodeId>,
    folding: bool,
    sort: SortKey,
) -> Vec<TreeNode<'static, NodeId>> {
    let mut rows = Vec::new();
    let mut pending = vec![(tree.root(), 0_u16)];
    while let Some((id, depth)) = pending.pop() {
        let node = tree.node(id);
        let name = node.name.to_string_lossy();
        let folded = folding && node.is_dir && FOLD_SET.contains(&name.as_ref());
        let branch = node.is_dir
            && !folded
            && (!node.children.is_empty() || node.state == NodeState::Scanning);
        let is_expanded = branch && expanded.contains(&id);
        let label = if folded {
            format!("{name} (folded)")
        } else {
            name.into_owned()
        };
        let percentage = node.parent.and_then(|parent| {
            let total = tree.node(parent).on_disk;
            (total > 0).then(|| (node.on_disk as f64 / total as f64) * 100.0)
        });
        let trailing = match percentage {
            Some(percentage) => format!(
                "{} · {:.0}%",
                format_size(node.on_disk, DECIMAL),
                percentage
            ),
            None => format_size(node.on_disk, DECIMAL),
        };
        rows.push(TreeNode {
            id,
            label: Line::from(label),
            trailing: Some(Line::from(trailing)),
            depth,
            branch,
            expanded: is_expanded,
            enabled: true,
            status: match node.state {
                NodeState::Scanning => TreeNodeStatus::Loading,
                NodeState::Done => TreeNodeStatus::Ready,
                NodeState::Errored(_) => TreeNodeStatus::Error,
            },
        });
        if is_expanded {
            let children = tree.sorted_children(id, sort);
            pending.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|child| (child, depth.saturating_add(1))),
            );
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::du::{NodeState, ScanTree};
    use termrock::widgets::TreeNodeStatus;

    struct Fixture {
        tree: ScanTree,
        root: NodeId,
        parent: NodeId,
        child: NodeId,
        sibling: NodeId,
    }

    fn fixture() -> Fixture {
        let mut tree = ScanTree::new("root".into(), true);
        let root = tree.root();
        let parent = tree.add_dir(root, "parent".into());
        let child = tree.add_dir(parent, "child".into());
        let sibling = tree.add_dir(root, "sibling".into());
        tree.add_sizes(child, 20, 40, 1);
        tree.add_sizes(sibling, 10, 80, 1);
        Fixture {
            tree,
            root,
            parent,
            child,
            sibling,
        }
    }

    fn cached_size(on_disk: u64) -> CachedSize {
        CachedSize {
            on_disk,
            apparent: on_disk,
            entry_count: 1,
            scanned_at: 100,
            root_mtime: 1,
        }
    }

    #[test]
    fn effective_selection_empty_is_zero() {
        let fixture = fixture();
        assert_eq!(effective_selection(&fixture.tree, &[]), (vec![], 0));
    }

    #[test]
    fn effective_selection_sums_disjoint_nodes() {
        let fixture = fixture();
        assert_eq!(
            effective_selection(&fixture.tree, &[fixture.child, fixture.sibling]),
            (vec![fixture.child, fixture.sibling], 30)
        );
    }

    #[test]
    fn effective_selection_deduplicates_nested_parent_first() {
        let fixture = fixture();
        assert_eq!(
            effective_selection(&fixture.tree, &[fixture.parent, fixture.child]),
            (vec![fixture.parent], 20)
        );
    }

    #[test]
    fn effective_selection_deduplicates_nested_child_first() {
        let fixture = fixture();
        assert_eq!(
            effective_selection(&fixture.tree, &[fixture.child, fixture.parent]),
            (vec![fixture.parent], 20)
        );
    }

    #[test]
    fn node_paths_reconstruct_without_canonicalizing() {
        let fixture = fixture();
        assert_eq!(
            node_path(&fixture.tree, Path::new("/tmp/root"), fixture.child),
            PathBuf::from("/tmp/root/parent/child")
        );
        assert_eq!(
            node_path(&fixture.tree, Path::new("/tmp/root"), fixture.root),
            PathBuf::from("/tmp/root")
        );
    }

    #[test]
    fn projection_starts_with_root_and_first_level() {
        let fixture = fixture();
        let rows = project_tree(
            &fixture.tree,
            &HashSet::from([fixture.root]),
            false,
            SortKey::OnDisk,
        );
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            [fixture.root, fixture.parent, fixture.sibling]
        );
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[1].depth, 1);
    }

    #[test]
    fn projection_sorts_by_requested_size() {
        let fixture = fixture();
        let expanded = HashSet::from([fixture.root]);
        let on_disk = project_tree(&fixture.tree, &expanded, false, SortKey::OnDisk);
        let apparent = project_tree(&fixture.tree, &expanded, false, SortKey::Apparent);
        assert_eq!(on_disk[1].id, fixture.parent);
        assert_eq!(apparent[1].id, fixture.sibling);
    }

    #[test]
    fn folded_directory_is_selectable_but_not_expandable() {
        let mut tree = ScanTree::new("root".into(), true);
        let root = tree.root();
        let folded = tree.add_dir(root, "node_modules".into());
        tree.add_dir(folded, "package".into());
        let rows = project_tree(&tree, &HashSet::from([root, folded]), true, SortKey::OnDisk);
        let row = rows.iter().find(|row| row.id == folded).unwrap();
        assert!(row.enabled);
        assert!(!row.branch);
        assert!(!row.expanded);
        assert!(row.label.to_string().contains("(folded)"));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn folding_can_be_disabled() {
        let mut tree = ScanTree::new("root".into(), true);
        let root = tree.root();
        let folded = tree.add_dir(root, "target".into());
        let child = tree.add_dir(folded, "debug".into());
        let rows = project_tree(
            &tree,
            &HashSet::from([root, folded]),
            false,
            SortKey::OnDisk,
        );
        assert!(rows.iter().any(|row| row.id == child));
    }

    #[test]
    fn scanning_and_error_states_map_to_tree_status() {
        let fixture = fixture();
        assert_eq!(fixture.tree.node(fixture.parent).state, NodeState::Scanning);
        let rows = project_tree(
            &fixture.tree,
            &HashSet::from([fixture.root]),
            false,
            SortKey::OnDisk,
        );
        assert_eq!(
            rows.iter()
                .find(|row| row.id == fixture.parent)
                .unwrap()
                .status,
            TreeNodeStatus::Loading
        );
    }

    #[test]
    fn trailing_metadata_has_size_and_parent_percentage() {
        let fixture = fixture();
        let rows = project_tree(
            &fixture.tree,
            &HashSet::from([fixture.root]),
            false,
            SortKey::OnDisk,
        );
        let trailing = rows
            .iter()
            .find(|row| row.id == fixture.parent)
            .unwrap()
            .trailing
            .as_ref()
            .unwrap()
            .to_string();
        assert!(trailing.contains("20 B"));
        assert!(trailing.contains("67%"));
    }

    #[test]
    fn cached_projection_starts_with_root_and_first_level() {
        let root = PathBuf::from("/tmp/root");
        let cached = CachedProjection::new(
            &root,
            vec![
                (root.clone(), cached_size(30)),
                (root.join("large"), cached_size(20)),
                (root.join("small"), cached_size(10)),
                (root.join("large/nested"), cached_size(5)),
            ],
        );
        let rows = cached.rows(
            &HashSet::from([NodeId(0)]),
            SortKey::OnDisk,
            UNIX_EPOCH + Duration::from_secs(200),
            &Theme::default(),
        );

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].label.to_string(), "root");
        assert_eq!(rows[1].label.to_string(), "large");
        assert_eq!(rows[2].label.to_string(), "small");
    }

    #[test]
    fn cached_projection_is_visually_distinct_until_live_replacement() {
        let root = PathBuf::from("/tmp/root");
        let theme = Theme::tailrocks_phosphor();
        let cached = CachedProjection::new(&root, vec![(root.clone(), cached_size(30))]);
        let rows = cached.rows(
            &HashSet::from([NodeId(0)]),
            SortKey::OnDisk,
            UNIX_EPOCH + Duration::from_secs(200),
            &theme,
        );

        assert!(
            rows[0]
                .trailing
                .as_ref()
                .unwrap()
                .to_string()
                .contains("cached")
        );
        assert_eq!(rows[0].label.style, theme.style(Role::TextMuted));
    }
}
