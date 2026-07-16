use std::{ffi::OsString, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrKind {
    PermissionDenied,
    Dataless,
    NotFound,
    Io,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    Scanning,
    Done,
    Errored(ErrKind),
}

#[derive(Debug, Clone)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub name: OsString,
    pub on_disk: u64,
    pub apparent: u64,
    pub entry_count: u64,
    pub is_dir: bool,
    pub state: NodeState,
    pub children: Vec<NodeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    OnDisk,
    Apparent,
    Name,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanError {
    pub path: PathBuf,
    pub kind: ErrKind,
}

#[derive(Debug)]
pub struct ScanTree {
    arena: Vec<Node>,
    root: NodeId,
    errors: Vec<ScanError>,
}

impl ScanTree {
    pub(crate) fn new(name: OsString, is_dir: bool) -> Self {
        Self {
            arena: vec![Node {
                parent: None,
                name,
                on_disk: 0,
                apparent: 0,
                entry_count: 0,
                is_dir,
                state: NodeState::Scanning,
                children: Vec::new(),
            }],
            root: NodeId(0),
            errors: Vec::new(),
        }
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.arena[id.0 as usize]
    }

    pub fn nodes(&self) -> &[Node] {
        &self.arena
    }

    pub fn errors(&self) -> &[ScanError] {
        &self.errors
    }

    pub(crate) fn add_dir(&mut self, parent: NodeId, name: OsString) -> NodeId {
        self.add_node(parent, name, true)
    }

    pub(crate) fn add_file(&mut self, parent: NodeId, name: OsString) -> NodeId {
        self.add_node(parent, name, false)
    }

    fn add_node(&mut self, parent: NodeId, name: OsString, is_dir: bool) -> NodeId {
        let id = NodeId(u32::try_from(self.arena.len()).expect("scan tree exceeded u32 nodes"));
        self.arena.push(Node {
            parent: Some(parent),
            name,
            on_disk: 0,
            apparent: 0,
            entry_count: 0,
            is_dir,
            state: if is_dir {
                NodeState::Scanning
            } else {
                NodeState::Done
            },
            children: Vec::new(),
        });
        self.arena[parent.0 as usize].children.push(id);
        id
    }

    pub(crate) fn set_root_is_dir(&mut self, is_dir: bool) {
        self.arena[self.root.0 as usize].is_dir = is_dir;
        if !is_dir {
            self.arena[self.root.0 as usize].state = NodeState::Done;
        }
    }

    pub(crate) fn mark_done(&mut self, id: NodeId) {
        let node = &mut self.arena[id.0 as usize];
        if node.state == NodeState::Scanning {
            node.state = NodeState::Done;
        }
    }

    pub(crate) fn add_sizes(
        &mut self,
        start: NodeId,
        on_disk: u64,
        apparent: u64,
        entry_count: u64,
    ) {
        let mut current = Some(start);
        while let Some(id) = current {
            let node = &mut self.arena[id.0 as usize];
            node.on_disk = node.on_disk.saturating_add(on_disk);
            node.apparent = node.apparent.saturating_add(apparent);
            node.entry_count = node.entry_count.saturating_add(entry_count);
            current = node.parent;
        }
    }

    pub(crate) fn record_error(&mut self, id: NodeId, path: PathBuf, kind: ErrKind) {
        self.arena[id.0 as usize].state = NodeState::Errored(kind);
        self.errors.push(ScanError { path, kind });
    }

    #[cfg(test)]
    pub(crate) fn finish_scanning_nodes(&mut self) {
        for node in &mut self.arena {
            if node.state == NodeState::Scanning {
                node.state = NodeState::Done;
            }
        }
    }

    pub fn sorted_children(&self, id: NodeId, by: SortKey) -> Vec<NodeId> {
        let mut children = self.node(id).children.clone();
        children.sort_by(|left, right| {
            let left = self.node(*left);
            let right = self.node(*right);
            match by {
                SortKey::OnDisk => right
                    .on_disk
                    .cmp(&left.on_disk)
                    .then_with(|| left.name.cmp(&right.name)),
                SortKey::Apparent => right
                    .apparent
                    .cmp(&left.apparent)
                    .then_with(|| left.name.cmp(&right.name)),
                SortKey::Name => left.name.cmp(&right.name),
            }
        });
        children
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn sizes_propagate_to_every_ancestor() {
        let mut tree = ScanTree::new("root".into(), true);
        let first = tree.add_dir(tree.root(), "first".into());
        let second = tree.add_dir(first, "second".into());

        tree.add_sizes(second, 512, 123, 1);

        for id in [tree.root(), first, second] {
            assert_eq!(tree.node(id).on_disk, 512);
            assert_eq!(tree.node(id).apparent, 123);
            assert_eq!(tree.node(id).entry_count, 1);
        }
    }

    #[test]
    fn children_sort_by_size_descending_then_name() {
        let mut tree = ScanTree::new("root".into(), true);
        let small = tree.add_dir(tree.root(), "small".into());
        let beta = tree.add_dir(tree.root(), "beta".into());
        let alpha = tree.add_dir(tree.root(), "alpha".into());
        tree.add_sizes(small, 1, 30, 0);
        tree.add_sizes(beta, 10, 20, 0);
        tree.add_sizes(alpha, 10, 10, 0);

        assert_eq!(
            tree.sorted_children(tree.root(), SortKey::OnDisk),
            [alpha, beta, small]
        );
        assert_eq!(
            tree.sorted_children(tree.root(), SortKey::Apparent),
            [small, beta, alpha]
        );
        assert_eq!(
            tree.sorted_children(tree.root(), SortKey::Name),
            [alpha, beta, small]
        );
    }

    #[test]
    fn empty_directory_finishes_with_zero_totals() {
        let mut tree = ScanTree::new("root".into(), true);
        tree.finish_scanning_nodes();

        let root = tree.node(tree.root());
        assert_eq!((root.on_disk, root.apparent, root.entry_count), (0, 0, 0));
        assert_eq!(root.state, NodeState::Done);
    }

    #[test]
    fn deep_chain_propagation_is_prompt() {
        let mut tree = ScanTree::new("root".into(), true);
        let mut leaf = tree.root();
        for index in 0..1_000 {
            leaf = tree.add_dir(leaf, OsString::from(format!("d{index}")));
        }

        let started = Instant::now();
        tree.add_sizes(leaf, 512, 1, 1);

        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(tree.node(tree.root()).entry_count, 1);
    }
}
