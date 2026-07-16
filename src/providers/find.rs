use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    providers::Provider,
    tui::finder,
};

pub struct FindProvider;

impl Provider for FindProvider {
    fn id(&self) -> &'static str {
        "find"
    }

    fn scan(&self) -> Option<GroupSpec> {
        Some(group())
    }
}

pub(super) fn group() -> GroupSpec {
    GroupSpec {
        id: "find".into(),
        title: "Find".into(),
        actions: vec![ActionSpec::new(
            "find.files",
            "Find a file or folder…",
            "Fuzzy-search files and folders below your home directory",
            "$ holla find",
            &["find", "file", "folder", "path", "search", "fff"],
            Danger::Safe,
            || Box::pin(finder::run()),
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_group_is_always_present_and_safe() {
        let group = group();
        assert_eq!(group.id, "find");
        assert_eq!(group.actions.len(), 1);
        assert_eq!(group.actions[0].id, "find.files");
        assert_eq!(group.actions[0].danger, Danger::Safe);
    }
}
