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
        actions: vec![
            ActionSpec::new(
                "find.files",
                "Find a file or folder…",
                "Fuzzy-search files and folders below your home directory",
                "$ holla find",
                &["find", "file", "folder", "path", "search", "fff"],
                Danger::Safe,
                || Box::pin(finder::run()),
            ),
            ActionSpec::new(
                "browse.files",
                "Browse files and folders…",
                "Navigate folders and preview files from the current directory",
                "$ holla browse",
                &["browse", "file", "folder", "navigate", "preview"],
                Danger::Safe,
                || Box::pin(crate::tui::browser::run()),
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_find_files_action_is_preserved() {
        let group = group();
        assert_eq!(group.id, "find");
        assert_eq!(group.title, "Find");
        let action = &group.actions[0];
        assert_eq!(action.id, "find.files");
        assert_eq!(action.label, "Find a file or folder…");
        assert_eq!(
            action.description,
            "Fuzzy-search files and folders below your home directory"
        );
        assert_eq!(action.preview, "$ holla find");
        assert_eq!(
            action.keywords,
            ["find", "file", "folder", "path", "search", "fff"]
        );
        assert_eq!(action.danger, Danger::Safe);
        assert!(!action.confirm);
        assert!(!action.trust_required);
    }

    #[test]
    fn browse_files_action_is_always_present_and_safe() {
        let group = FindProvider
            .scan()
            .expect("find provider should always return its group");
        assert_eq!(group.actions.len(), 2);
        let action = group
            .actions
            .iter()
            .find(|action| action.id == "browse.files")
            .expect("browse.files action should always be available");
        assert_eq!(action.label, "Browse files and folders…");
        assert_eq!(
            action.description,
            "Navigate folders and preview files from the current directory"
        );
        assert_eq!(action.preview, "$ holla browse");
        assert_eq!(
            action.keywords,
            ["browse", "file", "folder", "navigate", "preview"]
        );
        assert_eq!(action.danger, Danger::Safe);
        assert!(!action.confirm);
        assert!(!action.trust_required);
    }
}
