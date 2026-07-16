use dashmap::DashMap;

#[derive(Default)]
pub(crate) struct Hardlinks {
    seen: DashMap<(u64, u64), ()>,
}

impl Hardlinks {
    pub(crate) fn should_count_size(&self, nlink: u64, dev: u64, ino: u64) -> bool {
        nlink <= 1 || self.seen.insert((dev, ino), ()).is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_multi_link_inodes_are_deduplicated() {
        let links = Hardlinks::default();
        assert!(links.should_count_size(1, 1, 1));
        assert!(links.should_count_size(1, 1, 1));
        assert!(links.should_count_size(2, 1, 2));
        assert!(!links.should_count_size(2, 1, 2));
    }
}
