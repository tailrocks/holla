use crate::{
    du::{
        NodeId, NodeState, ScanEvent, ScanHandle, ScanOptions, ScanTree, SortKey,
        cache::{CachedSize, SizeBaseline, SizeCache, snapshot},
        scan,
        spotlight::{self, TopFiles},
    },
    insights,
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
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use termrock::{
    input::KeyCode,
    layout::centered_rect,
    style::{Density, DesignTokens, Role, Theme},
    widgets::{
        Backdrop, Hint, HintBar, List, ListRow, ListState, Panel, PanelEmphasis, Progress,
        ProgressKind, RowRole, StatusBar, StatusBarState, StatusSlot, TextInput, TextInputOutcome,
        TextInputState, Tree, TreeNode, TreeNodeStatus, TreeOutcome, TreeState, Validation,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnalyzerView {
    Tree,
    TopFiles,
}

struct OverviewItem {
    path: PathBuf,
    label: String,
    cached: Option<CachedSize>,
    insight: bool,
}

enum OverviewChoice {
    Path(PathBuf),
    TopFiles,
}

const OVERVIEW_HINTS: &[Hint<'static>] = &[
    Hint {
        chord: "↑↓",
        label: "navigate",
        priority: 5,
        visible: true,
    },
    Hint {
        chord: "enter",
        label: "analyze",
        priority: 5,
        visible: true,
    },
    Hint {
        chord: "T",
        label: "top files",
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

const CACHED_ID_BASE: u32 = 1 << 31;

const ANALYZER_HINTS: &[Hint<'static>] = &[
    Hint {
        chord: "T",
        label: "top files",
        priority: 4,
        visible: true,
    },
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
                    NodeId(
                        CACHED_ID_BASE
                            + u32::try_from(index).expect("cache projection exceeded u32 nodes"),
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut nodes = entries
            .into_iter()
            .enumerate()
            .map(|(index, (path, size))| CachedNode {
                id: NodeId(
                    CACHED_ID_BASE
                        + u32::try_from(index).expect("cache projection exceeded u32 nodes"),
                ),
                parent: path.parent().and_then(|parent| ids.get(parent)).copied(),
                path,
                children: Vec::new(),
                size,
            })
            .collect::<Vec<_>>();
        for index in 0..nodes.len() {
            if let Some(parent) = nodes[index].parent {
                let id = nodes[index].id;
                nodes[(parent.0 - CACHED_ID_BASE) as usize]
                    .children
                    .push(id);
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

    #[cfg(test)]
    fn root_id(&self) -> Option<NodeId> {
        self.nodes
            .iter()
            .find(|node| node.parent.is_none())
            .map(|node| node.id)
    }

    #[cfg(test)]
    fn node(&self, id: NodeId) -> Option<&CachedNode> {
        let index = id.0.checked_sub(CACHED_ID_BASE)?;
        self.nodes.get(index as usize)
    }

    #[cfg(test)]
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
            let node = self.node(id).expect("cached row identity");
            let is_expanded = !node.children.is_empty() && expanded.contains(&id);
            let age = now
                .duration_since(UNIX_EPOCH + Duration::from_nanos(node.size.scanned_at))
                .map(|duration| Duration::from_secs(duration.as_secs()))
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
                leading: None,
                secondary: None,
                badge: None,
                shortcut: None,
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
                    let left = self.node(*left).expect("cached child identity");
                    let right = self.node(*right).expect("cached child identity");
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
}

pub async fn overview() -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home directory is unavailable"))?;
    let theme = Theme::tailrocks_phosphor();
    let tokens = DesignTokens::new(theme.clone(), Density::default());
    let items = overview_items(&home, &SizeCache::load(), SystemTime::now());
    let rows = overview_rows(&items, SystemTime::now(), &theme);
    let mut state = ListState::new(rows.first().map(|row| row.id));
    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;

    let choice = 'screen: loop {
        terminal.draw(|frame| {
            let [header_area, list_area, copy_area, hints_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(frame.area());
            frame.render_widget(
                Paragraph::new("Disk overview").style(theme.style(Role::Accent)),
                header_area,
            );
            let panel = Panel::new(&tokens)
                .title(" Places to inspect ")
                .emphasis(PanelEmphasis::Focused);
            let inner = panel.inner(list_area);
            frame.render_widget(&panel, list_area);
            if rows.is_empty() {
                frame.render_widget(
                    Paragraph::new("No home directories found").style(theme.style(Role::TextMuted)),
                    inner,
                );
            } else {
                frame.render_stateful_widget(&List::new(&rows, &tokens), inner, &mut state);
            }
            frame.render_widget(
                Paragraph::new("Cached sizes are labeled; selecting any row starts a live scan")
                    .style(theme.style(Role::TextMuted)),
                copy_area,
            );
            frame.render_widget(
                HintBar::new(OVERVIEW_HINTS, &theme).separator(" · "),
                hints_area,
            );
        })?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let key = termrock::input::KeyEvent::from(key);
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break 'screen None,
                KeyCode::Char('T' | 't') => break 'screen Some(OverviewChoice::TopFiles),
                _ => {
                    if let termrock::interaction::Outcome::Activated(id) =
                        state.handle_key(&rows, key)
                    {
                        break 'screen Some(OverviewChoice::Path(items[id].path.clone()));
                    }
                }
            }
        }
    };

    drop(terminal);
    session.restore()?;
    match choice {
        Some(OverviewChoice::Path(path)) => run(path).await,
        Some(OverviewChoice::TopFiles) => run_with_view(home, AnalyzerView::TopFiles).await,
        None => Ok(()),
    }
}

fn overview_items(home: &Path, cache: &SizeCache, now: SystemTime) -> Vec<OverviewItem> {
    let mut home_paths = fs::read_dir(home)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    home_paths.sort();
    let mut items = home_paths
        .into_iter()
        .map(|path| OverviewItem {
            label: path
                .file_name()
                .unwrap_or_else(|| path.as_os_str())
                .to_string_lossy()
                .into_owned(),
            cached: cache.valid(&path, now).cloned(),
            path,
            insight: false,
        })
        .collect::<Vec<_>>();

    if let Some(probe) = insights::Probe::current() {
        let mut seen = HashSet::new();
        for spec in insights::REGISTRY
            .iter()
            .filter(|spec| insights::detect(spec, &probe))
        {
            for path in
                insights::expand_roots_with_xdg(spec, &probe.home, probe.xdg_cache_home.as_deref())
                    .into_iter()
                    .filter(|path| path.is_dir())
                    .filter(|path| seen.insert(path.clone()))
            {
                items.push(OverviewItem {
                    label: format!("{} · {}", spec.title, path.display()),
                    cached: cache.valid(&path, now).cloned(),
                    path,
                    insight: true,
                });
            }
        }
    }
    items
}

fn overview_rows(
    items: &[OverviewItem],
    now: SystemTime,
    theme: &Theme,
) -> Vec<ListRow<'static, usize>> {
    items
        .iter()
        .enumerate()
        .map(|(id, item)| {
            let trailing = item.cached.as_ref().map_or_else(
                || "not scanned yet".to_owned(),
                |cached| {
                    let age = now
                        .duration_since(UNIX_EPOCH + Duration::from_nanos(cached.scanned_at))
                        .map(|duration| Duration::from_secs(duration.as_secs()))
                        .unwrap_or_default();
                    format!(
                        "{} · cached {} ago",
                        format_size(cached.on_disk, DECIMAL),
                        humantime::format_duration(age)
                    )
                },
            );
            ListRow {
                id,
                label: Line::styled(
                    item.label.clone(),
                    theme.style(if item.insight {
                        Role::Accent
                    } else {
                        Role::Text
                    }),
                ),
                leading: None,
                secondary: None,
                badge: None,
                shortcut: None,
                trailing: Some(Line::styled(trailing, theme.style(Role::TextMuted))),
                role: RowRole::Item,
                enabled: true,
                loading: false,
            }
        })
        .collect()
}

struct MergedProjection {
    rows: Vec<TreeNode<'static, PathBuf>>,
    sizes: HashMap<PathBuf, u64>,
}

struct DisplayNode {
    path: PathBuf,
    on_disk: u64,
    apparent: u64,
    state: NodeState,
    is_dir: bool,
    cached_at: Option<u64>,
}

struct ProjectionOptions {
    cache_active: bool,
    folding: bool,
    sort: SortKey,
    now: SystemTime,
}

fn merged_projection(
    root: &Path,
    tree: Option<&ScanTree>,
    cached: &CachedProjection,
    expanded: &HashSet<PathBuf>,
    options: ProjectionOptions,
    theme: &Theme,
) -> MergedProjection {
    let mut nodes = HashMap::<PathBuf, DisplayNode>::new();
    if options.cache_active {
        nodes.extend(cached.nodes.iter().map(|node| {
            (
                node.path.clone(),
                DisplayNode {
                    path: node.path.clone(),
                    on_disk: node.size.on_disk,
                    apparent: node.size.apparent,
                    state: NodeState::Done,
                    is_dir: !node.children.is_empty() || node.path == root,
                    cached_at: Some(node.size.scanned_at),
                },
            )
        }));
    }
    if let Some(tree) = tree {
        for (index, node) in tree.nodes().iter().enumerate() {
            let id = NodeId(u32::try_from(index).expect("scan tree exceeded u32 nodes"));
            let path = node_path(tree, root, id);
            let fresh = node.state != NodeState::Scanning || node.on_disk > 0;
            if fresh || !nodes.contains_key(&path) {
                nodes.insert(
                    path.clone(),
                    DisplayNode {
                        path,
                        on_disk: node.on_disk,
                        apparent: node.apparent,
                        state: node.state,
                        is_dir: node.is_dir,
                        cached_at: None,
                    },
                );
            }
        }
    }

    let mut children = HashMap::<PathBuf, Vec<PathBuf>>::new();
    for path in nodes.keys().filter(|path| path.as_path() != root) {
        if let Some(parent) = path.parent().filter(|parent| nodes.contains_key(*parent)) {
            children
                .entry(parent.to_path_buf())
                .or_default()
                .push(path.clone());
        }
    }
    for paths in children.values_mut() {
        paths.sort_by(|left, right| {
            let left_node = &nodes[left];
            let right_node = &nodes[right];
            match options.sort {
                SortKey::OnDisk => right_node
                    .on_disk
                    .cmp(&left_node.on_disk)
                    .then_with(|| left.cmp(right)),
                SortKey::Apparent => right_node
                    .apparent
                    .cmp(&left_node.apparent)
                    .then_with(|| left.cmp(right)),
                SortKey::Name => left.cmp(right),
            }
        });
    }

    let mut rows = Vec::new();
    let mut pending = nodes
        .contains_key(root)
        .then(|| (root.to_path_buf(), 0_u16))
        .into_iter()
        .collect::<Vec<_>>();
    while let Some((path, depth)) = pending.pop() {
        let node = &nodes[&path];
        let name = path
            .file_name()
            .unwrap_or_else(|| path.as_os_str())
            .to_string_lossy();
        let folded = options.folding && node.is_dir && FOLD_SET.contains(&name.as_ref());
        let node_children = children.get(&path).map(Vec::as_slice).unwrap_or_default();
        let branch = node.is_dir && !folded && !node_children.is_empty();
        let is_expanded = branch && expanded.contains(&path);
        let label = if folded {
            format!("{name} (folded)")
        } else {
            name.into_owned()
        };
        let (label, trailing) = if let Some(scanned_at) = node.cached_at {
            let age = options
                .now
                .duration_since(UNIX_EPOCH + Duration::from_nanos(scanned_at))
                .map(|duration| Duration::from_secs(duration.as_secs()))
                .unwrap_or_default();
            (
                Line::styled(label, theme.style(Role::TextMuted)),
                Line::styled(
                    format!(
                        "{} · cached {} ago",
                        format_size(node.on_disk, DECIMAL),
                        humantime::format_duration(age)
                    ),
                    theme.style(Role::TextMuted),
                ),
            )
        } else {
            let percentage = path.parent().and_then(|parent| {
                let total = nodes.get(parent)?.on_disk;
                (total > 0).then(|| (node.on_disk as f64 / total as f64) * 100.0)
            });
            (
                Line::from(label),
                Line::from(percentage.map_or_else(
                    || format_size(node.on_disk, DECIMAL),
                    |percentage| {
                        format!(
                            "{} · {:.0}%",
                            format_size(node.on_disk, DECIMAL),
                            percentage
                        )
                    },
                )),
            )
        };
        rows.push(TreeNode {
            id: path.clone(),
            label,
            leading: None,
            secondary: None,
            badge: None,
            shortcut: None,
            trailing: Some(trailing),
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
            pending.extend(
                node_children
                    .iter()
                    .rev()
                    .cloned()
                    .map(|child| (child, depth.saturating_add(1))),
            );
        }
    }
    MergedProjection {
        rows,
        sizes: nodes
            .into_values()
            .map(|node| (node.path, node.on_disk))
            .collect(),
    }
}

fn selected_projection_items(
    projection: &MergedProjection,
    checked: &[PathBuf],
) -> Vec<CleanupItem> {
    let mut items = checked
        .iter()
        .filter_map(|path| {
            projection
                .sizes
                .get(path)
                .map(|size| CleanupItem::new(path.clone(), *size))
        })
        .filter(|item| {
            !checked
                .iter()
                .any(|other| other != &item.path && item.path.starts_with(other))
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.path.cmp(&right.path));
    items
}

fn reconcile_tree_state(
    state: &mut TreeState<PathBuf>,
    rows: &[TreeNode<'_, PathBuf>],
    identities: &HashMap<PathBuf, u64>,
) {
    let visible = rows
        .iter()
        .map(|row| row.id.clone())
        .collect::<HashSet<_>>();
    if state.selected().is_none_or(|id| !visible.contains(id)) {
        state.select(rows.first().map(|row| row.id.clone()));
    }
    if let Some(selection) = state.selection_mut() {
        let retained = selection
            .checked()
            .iter()
            .filter(|id| identities.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        selection.clear();
        for id in retained {
            selection.toggle(&id);
        }
    }
}

pub async fn run(root: PathBuf) -> anyhow::Result<()> {
    run_with_view(root, AnalyzerView::Tree).await
}

struct BaselineTask {
    cancel: Arc<AtomicBool>,
    join: Option<tokio::task::JoinHandle<SizeBaseline>>,
}

impl BaselineTask {
    fn is_finished(&self) -> bool {
        self.join
            .as_ref()
            .is_none_or(tokio::task::JoinHandle::is_finished)
    }
}

impl Drop for BaselineTask {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

fn capture_baseline(root: &Path) -> BaselineTask {
    let root = root.to_path_buf();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let join = tokio::task::spawn_blocking(move || {
        SizeBaseline::capture_cancellable(&root, &worker_cancel)
    });
    BaselineTask {
        cancel,
        join: Some(join),
    }
}

async fn run_with_view(root: PathBuf, initial_view: AnalyzerView) -> anyhow::Result<()> {
    let theme = Theme::tailrocks_phosphor();
    let tokens = DesignTokens::new(theme.clone(), Density::default());
    let now = SystemTime::now();
    let cached = CachedProjection::new(&root, SizeCache::load().valid_below(&root, now));
    let mut cache_active = !cached.is_empty();
    let mut first_frame = true;
    let mut view = initial_view;
    let mut handle: Option<ScanHandle> = None;
    let mut size_baseline: Option<SizeBaseline> = None;
    let mut baseline_task = None;
    let mut expanded = HashSet::from([root.clone()]);
    let mut folding = true;
    let mut sort = SortKey::OnDisk;
    let mut tree_state = TreeState::new(Some(root.clone()));
    tree_state.enable_multi_select();
    let mut status_state = StatusBarState::default();
    let mut scanning = handle.is_some();
    let mut bytes_seen = 0_u64;
    let mut inaccessible = 0_u64;
    let mut tick = 0_u64;
    let mut cleanup = CleanupFlow::new();
    let mut cache_write_started = false;
    let mut cache_writes = Vec::new();
    let mut top_files = None;
    let mut spotlight_task = None;
    let mut top_state = ListState::new(None);
    top_state.enable_multi_select();
    if view == AnalyzerView::TopFiles {
        spotlight_task = Some(tokio::spawn(spotlight::discover()));
    }
    let root_label = root.display().to_string();

    let mut session = termrock::crossterm::Session::enter(
        std::io::stdout(),
        termrock::crossterm::SessionOptions::default(),
    )?;
    let backend = CrosstermBackend::new(session.writer_mut());
    let mut terminal = ratatui::Terminal::new(backend)?;

    'screen: loop {
        if baseline_task
            .as_ref()
            .is_some_and(BaselineTask::is_finished)
        {
            let mut completed = baseline_task.take().expect("finished baseline task");
            size_baseline = completed
                .join
                .take()
                .expect("baseline task has a worker")
                .await
                .ok();
        }
        if view == AnalyzerView::Tree && handle.is_none() && size_baseline.is_some() {
            handle = Some(scan(ScanOptions::new(&root)));
            scanning = true;
            first_frame = true;
        }
        match cleanup.poll() {
            CleanupPoll::Exit => break 'screen,
            CleanupPoll::Completed => {
                let restart_tree = view == AnalyzerView::Tree
                    || handle.is_some()
                    || baseline_task.is_some()
                    || size_baseline.is_some();
                if let Some(handle) = handle.as_ref() {
                    handle.cancel.store(true, Ordering::Release);
                }
                handle = None;
                size_baseline = None;
                baseline_task = restart_tree.then(|| capture_baseline(&root));
                cache_active = false;
                expanded = HashSet::from([root.clone()]);
                tree_state = TreeState::new(Some(root.clone()));
                tree_state.enable_multi_select();
                scanning = handle.is_some();
                bytes_seen = 0;
                inaccessible = 0;
                cache_write_started = false;
                if view == AnalyzerView::TopFiles {
                    top_files = None;
                    spotlight_task = Some(tokio::spawn(spotlight::discover()));
                    top_state = ListState::new(None);
                    top_state.enable_multi_select();
                }
            }
            CleanupPoll::None => {}
        }
        let drain = if first_frame {
            DrainResult::default()
        } else {
            handle.as_ref().map_or_else(DrainResult::default, |handle| {
                drain_scan_events(handle, &mut scanning, &mut bytes_seen, &mut inaccessible)
            })
        };
        if drain.finished {
            cache_active = false;
        }
        if drain.finished && !cache_write_started {
            let snapshot = {
                let handle = handle.as_ref().expect("finished scan has a handle");
                let tree = handle.tree.read().expect("disk scan tree lock");
                snapshot(
                    &root,
                    &tree,
                    SystemTime::now(),
                    size_baseline.as_ref().expect("scan has a size baseline"),
                )
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
        if spotlight_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
        {
            top_files = Some(
                spotlight_task
                    .take()
                    .expect("finished Spotlight task")
                    .await
                    .unwrap_or(TopFiles::Unavailable),
            );
        }
        let projection = if let Some(handle) = handle.as_ref() {
            let tree = handle.tree.read().expect("disk scan tree lock");
            merged_projection(
                &root,
                Some(&tree),
                &cached,
                &expanded,
                ProjectionOptions {
                    cache_active,
                    folding,
                    sort,
                    now: SystemTime::now(),
                },
                &theme,
            )
        } else {
            merged_projection(
                &root,
                None,
                &cached,
                &expanded,
                ProjectionOptions {
                    cache_active,
                    folding,
                    sort,
                    now: SystemTime::now(),
                },
                &theme,
            )
        };
        reconcile_tree_state(&mut tree_state, &projection.rows, &projection.sizes);
        let checked = tree_state
            .selection()
            .map(|selection| selection.checked())
            .unwrap_or_default();
        let tree_selected = selected_projection_items(&projection, checked);
        let tree_selected_bytes = tree_selected
            .iter()
            .fold(0_u64, |total, item| total.saturating_add(item.size));
        let root_bytes = projection.sizes.get(&root).copied().unwrap_or_default();
        let rows = projection.rows;
        let top_rows = top_file_rows(top_files.as_ref());
        if top_state.selected().is_none() {
            top_state.select(top_rows.first().map(|row| row.id));
        }
        let (top_selected, top_selected_bytes) = selected_top_files(top_files.as_ref(), &top_state);
        let (selected_count, selected_bytes) = match view {
            AnalyzerView::Tree => (tree_selected.len(), tree_selected_bytes),
            AnalyzerView::TopFiles => (top_selected.len(), top_selected_bytes),
        };
        let scan_bytes = bytes_seen.max(root_bytes);
        let scan_copy = if view == AnalyzerView::TopFiles {
            match top_files.as_ref() {
                None => "searching Spotlight".to_owned(),
                Some(TopFiles::Available(files)) => format!("{} large files", files.len()),
                Some(TopFiles::Unavailable) => "Spotlight unavailable".to_owned(),
            }
        } else if cache_active {
            format!(
                "refreshing · cached {}",
                format_size(cached.root_bytes(), DECIMAL)
            )
        } else if scanning {
            format!(
                "scanning · {} · {inaccessible} unreadable",
                format_size(scan_bytes, DECIMAL)
            )
        } else if handle.is_none() {
            "preparing scan".to_owned()
        } else {
            format!(
                "complete · {} · {inaccessible} unreadable",
                format_size(root_bytes, DECIMAL)
            )
        };
        let selection_copy = if view == AnalyzerView::TopFiles {
            format!(
                "{selected_count} files selected — {} reclaimable · 100 MB minimum · top 50",
                format_size(selected_bytes, DECIMAL)
            )
        } else {
            format!(
                "{selected_count} items selected — {} reclaimable · {} · folding {}",
                format_size(selected_bytes, DECIMAL),
                match sort {
                    SortKey::OnDisk => "on-disk",
                    SortKey::Apparent => "apparent",
                    SortKey::Name => "name",
                },
                if folding { "on" } else { "off" }
            )
        };
        let honesty_copy = if view == AnalyzerView::TopFiles {
            "Spotlight index results; sizes rechecked from the filesystem".to_owned()
        } else if inaccessible == 0 {
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
            if view == AnalyzerView::TopFiles && top_files.is_none() {
                frame.render_widget(
                    Progress::new(ProgressKind::Indeterminate { tick }, &theme)
                        .label("Searching Spotlight"),
                    progress_area,
                );
            } else if view == AnalyzerView::Tree && scanning {
                let label = format!("{} scanned", format_size(scan_bytes, DECIMAL));
                frame.render_widget(
                    Progress::new(ProgressKind::Indeterminate { tick }, &theme).label(&label),
                    progress_area,
                );
            } else if view == AnalyzerView::Tree && handle.is_none() {
                frame.render_widget(
                    Paragraph::new("Preparing scan").style(theme.style(Role::TextMuted)),
                    progress_area,
                );
            } else {
                frame.render_widget(
                    Paragraph::new(if view == AnalyzerView::TopFiles {
                        "Spotlight query complete"
                    } else {
                        "Scan complete"
                    })
                    .style(theme.style(Role::Success)),
                    progress_area,
                );
            }
            let panel = Panel::new(&tokens)
                .title(if view == AnalyzerView::TopFiles {
                    " Top files "
                } else {
                    " Disk usage "
                })
                .emphasis(PanelEmphasis::Focused);
            let inner = panel.inner(tree_area);
            frame.render_widget(&panel, tree_area);
            if view == AnalyzerView::Tree {
                frame.render_stateful_widget(&Tree::new(&rows, &tokens), inner, &mut tree_state);
            } else {
                match top_files.as_ref() {
                    Some(TopFiles::Available(files)) if !files.is_empty() => {
                        frame.render_stateful_widget(
                            &List::new(&top_rows, &tokens),
                            inner,
                            &mut top_state,
                        );
                    }
                    Some(TopFiles::Available(_)) => frame
                        .render_widget(Paragraph::new("No indexed files at least 100 MB"), inner),
                    Some(TopFiles::Unavailable) => frame.render_widget(
                        Paragraph::new("Spotlight unavailable — use the tree scan")
                            .style(theme.style(Role::TextMuted)),
                        inner,
                    ),
                    None => frame.render_widget(
                        Paragraph::new("Searching the Spotlight index…")
                            .style(theme.style(Role::TextMuted)),
                        inner,
                    ),
                }
            }
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
        if view == AnalyzerView::Tree
            && handle.is_none()
            && size_baseline.is_none()
            && baseline_task.is_none()
        {
            baseline_task = Some(capture_baseline(&root));
        }
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
                KeyCode::Char('T' | 't') => {
                    view = match view {
                        AnalyzerView::Tree => AnalyzerView::TopFiles,
                        AnalyzerView::TopFiles => AnalyzerView::Tree,
                    };
                    if view == AnalyzerView::TopFiles
                        && top_files.is_none()
                        && spotlight_task.is_none()
                    {
                        spotlight_task = Some(tokio::spawn(spotlight::discover()));
                    }
                    if view == AnalyzerView::Tree
                        && handle.is_none()
                        && size_baseline.is_none()
                        && baseline_task.is_none()
                    {
                        baseline_task = Some(capture_baseline(&root));
                    }
                }
                KeyCode::Char('d') | KeyCode::Backspace => {
                    let items = if view == AnalyzerView::TopFiles {
                        selected_top_files(top_files.as_ref(), &top_state).0
                    } else {
                        tree_selected.clone()
                    };
                    if !items.is_empty() {
                        cleanup.open_confirmation(items);
                    }
                }
                KeyCode::Char('f') if view == AnalyzerView::Tree => folding = !folding,
                KeyCode::Char('s') if view == AnalyzerView::Tree => {
                    sort = match sort {
                        SortKey::OnDisk => SortKey::Apparent,
                        SortKey::Apparent | SortKey::Name => SortKey::OnDisk,
                    };
                }
                KeyCode::Char('r') if view == AnalyzerView::Tree => {
                    if let Some(handle) = handle.as_ref() {
                        handle.cancel.store(true, Ordering::Release);
                    }
                    handle = None;
                    size_baseline = None;
                    baseline_task = Some(capture_baseline(&root));
                    expanded = HashSet::from([root.clone()]);
                    tree_state = TreeState::new(Some(root.clone()));
                    tree_state.enable_multi_select();
                    scanning = false;
                    bytes_seen = 0;
                    inaccessible = 0;
                    cache_write_started = false;
                    cache_active = false;
                }
                _ if view == AnalyzerView::TopFiles => {
                    top_state.handle_key(&top_rows, key);
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

    if let Some(handle) = handle.as_ref() {
        handle.cancel.store(true, Ordering::Release);
    }
    drop(terminal);
    session.restore()?;
    if let Some(task) = spotlight_task {
        task.abort();
        let _ = task.await;
    }
    for writer in cache_writes {
        if writer.join().is_err() {
            eprintln!("holla: size cache writer panicked");
        }
    }
    Ok(())
}

pub async fn prompt_path() -> anyhow::Result<Option<PathBuf>> {
    let theme = Theme::tailrocks_phosphor();
    let tokens = DesignTokens::new(theme.clone(), Density::default());
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
            let panel = Panel::new(&tokens)
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

#[cfg(test)]
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

fn top_file_rows(files: Option<&TopFiles>) -> Vec<ListRow<'static, usize>> {
    let Some(TopFiles::Available(files)) = files else {
        return Vec::new();
    };
    files
        .iter()
        .enumerate()
        .map(|(id, file)| ListRow {
            id,
            label: Line::from(file.path.display().to_string()),
            leading: None,
            secondary: None,
            badge: None,
            shortcut: None,
            trailing: Some(Line::from(format_size(file.on_disk, DECIMAL))),
            role: RowRole::Item,
            enabled: true,
            loading: false,
        })
        .collect()
}

fn selected_top_files(
    files: Option<&TopFiles>,
    state: &ListState<usize>,
) -> (Vec<CleanupItem>, u64) {
    let Some(TopFiles::Available(files)) = files else {
        return (Vec::new(), 0);
    };
    let checked = state
        .selection()
        .map(|selection| selection.checked())
        .unwrap_or_default();
    let items = checked
        .iter()
        .filter_map(|id| files.get(*id))
        .map(|file| CleanupItem::new(file.path.clone(), file.on_disk))
        .collect::<Vec<_>>();
    let bytes = items
        .iter()
        .fold(0_u64, |total, item| total.saturating_add(item.size));
    (items, bytes)
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

#[cfg(test)]
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
            leading: None,
            secondary: None,
            badge: None,
            shortcut: None,
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
    use crate::du::{NodeState, ScanTree, spotlight::TopFile};
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
            scanned_at: 100_000,
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
            &HashSet::from([cached.root_id().unwrap()]),
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
            &HashSet::from([cached.root_id().unwrap()]),
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
        assert_eq!(
            rows[0].trailing.as_ref().unwrap().style,
            theme.style(Role::TextMuted)
        );
    }

    #[test]
    fn tree_state_drops_focus_and_checks_for_rows_that_disappear() {
        let root = PathBuf::from("/tmp/root");
        let hidden = root.join("hidden");
        let cached = CachedProjection::new(
            &root,
            vec![
                (root.clone(), cached_size(30)),
                (hidden.clone(), cached_size(20)),
            ],
        );
        let projection = merged_projection(
            &root,
            None,
            &cached,
            &HashSet::from([root.clone()]),
            ProjectionOptions {
                cache_active: true,
                folding: false,
                sort: SortKey::OnDisk,
                now: UNIX_EPOCH + Duration::from_secs(200),
            },
            &Theme::default(),
        );
        let mut state = TreeState::new(Some(hidden.clone()));
        state.enable_multi_select();
        state.selection_mut().unwrap().toggle(&hidden);

        reconcile_tree_state(&mut state, &projection.rows[..1], &projection.sizes);

        assert_eq!(state.selected(), Some(&root));
        assert_eq!(
            state.selection().unwrap().checked(),
            std::slice::from_ref(&hidden)
        );

        let only_root = HashMap::from([(root.clone(), 30)]);
        reconcile_tree_state(&mut state, &projection.rows[..1], &only_root);
        assert!(state.selection().unwrap().checked().is_empty());
    }

    #[test]
    fn cached_paths_remain_until_the_matching_live_node_has_size() {
        let root = PathBuf::from("/tmp/root");
        let cached = CachedProjection::new(
            &root,
            vec![
                (root.clone(), cached_size(30)),
                (root.join("large"), cached_size(20)),
                (root.join("large/nested"), cached_size(10)),
            ],
        );
        let mut tree = ScanTree::new("root".into(), true);
        let large = tree.add_dir(tree.root(), "large".into());
        let expanded = HashSet::from([root.clone(), root.join("large")]);
        let theme = Theme::default();
        let projection = merged_projection(
            &root,
            Some(&tree),
            &cached,
            &expanded,
            ProjectionOptions {
                cache_active: true,
                folding: false,
                sort: SortKey::OnDisk,
                now: SystemTime::now(),
            },
            &theme,
        );
        assert!(
            projection
                .rows
                .iter()
                .find(|row| row.label.to_string() == "large")
                .unwrap()
                .trailing
                .as_ref()
                .unwrap()
                .to_string()
                .contains("cached")
        );
        assert_eq!(projection.rows[0].id, root);
        assert_eq!(projection.rows[1].id, root.join("large"));
        assert_eq!(projection.rows[2].id, root.join("large/nested"));
        let checked = vec![root.join("large")];
        assert_eq!(
            selected_projection_items(&projection, &checked)[0].path,
            root.join("large")
        );

        tree.add_sizes(large, 25, 25, 1);
        let projection = merged_projection(
            &root,
            Some(&tree),
            &cached,
            &expanded,
            ProjectionOptions {
                cache_active: true,
                folding: false,
                sort: SortKey::OnDisk,
                now: SystemTime::now(),
            },
            &theme,
        );
        assert!(
            !projection
                .rows
                .iter()
                .find(|row| row.label.to_string() == "large")
                .unwrap()
                .trailing
                .as_ref()
                .unwrap()
                .to_string()
                .contains("cached")
        );
        assert_eq!(projection.rows[1].id, root.join("large"));
        assert_eq!(
            selected_projection_items(&projection, &checked)[0].path,
            root.join("large")
        );
    }

    #[test]
    fn top_file_rows_keep_full_paths_and_exact_sizes() {
        let files = TopFiles::Available(vec![TopFile {
            path: PathBuf::from("/tmp/large image.dmg"),
            on_disk: 250_000_000,
        }]);
        let rows = top_file_rows(Some(&files));

        assert_eq!(rows[0].label.to_string(), "/tmp/large image.dmg");
        assert_eq!(rows[0].trailing.as_ref().unwrap().to_string(), "250 MB");
    }

    #[test]
    fn top_file_selection_builds_shared_cleanup_items() {
        let files = TopFiles::Available(vec![
            TopFile {
                path: PathBuf::from("/tmp/one"),
                on_disk: 100,
            },
            TopFile {
                path: PathBuf::from("/tmp/two"),
                on_disk: 200,
            },
        ]);
        let mut state = ListState::new(Some(1));
        state.enable_multi_select();
        state.selection_mut().unwrap().toggle(&1);

        let (items, bytes) = selected_top_files(Some(&files), &state);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, PathBuf::from("/tmp/two"));
        assert_eq!(bytes, 200);
    }

    #[test]
    fn overview_projection_labels_cache_hits_and_misses() {
        let items = vec![
            OverviewItem {
                path: PathBuf::from("/tmp/cached"),
                label: "cached".into(),
                cached: Some(cached_size(42_000_000)),
                insight: false,
            },
            OverviewItem {
                path: PathBuf::from("/tmp/new"),
                label: "new".into(),
                cached: None,
                insight: false,
            },
        ];
        let rows = overview_rows(
            &items,
            UNIX_EPOCH + Duration::from_secs(200),
            &Theme::default(),
        );

        assert!(
            rows[0]
                .trailing
                .as_ref()
                .unwrap()
                .to_string()
                .contains("cached")
        );
        assert_eq!(
            rows[1].trailing.as_ref().unwrap().to_string(),
            "not scanned yet"
        );
    }

    #[test]
    fn overview_projection_distinguishes_detected_insights() {
        let theme = Theme::tailrocks_phosphor();
        let items = vec![OverviewItem {
            path: PathBuf::from("/tmp/cache"),
            label: "Package cache · /tmp/cache".into(),
            cached: None,
            insight: true,
        }];
        let rows = overview_rows(&items, SystemTime::now(), &theme);

        assert_eq!(rows[0].label.style, theme.style(Role::Accent));
        assert!(rows[0].label.to_string().contains("Package cache"));
    }
}
