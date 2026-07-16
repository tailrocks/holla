use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    providers::Provider,
    tui::analyzer,
};

pub struct DiskProvider;

impl Provider for DiskProvider {
    fn id(&self) -> &'static str {
        "disk"
    }

    fn scan(&self) -> Option<GroupSpec> {
        Some(group())
    }
}

pub(super) fn group() -> GroupSpec {
    GroupSpec {
        id: "disk",
        title: "Disk usage".into(),
        actions: vec![
            ActionSpec::new(
                "disk.scan-home",
                "Analyze home folder",
                "Find what uses space in your home folder",
                "$ holla disk analyze ~",
                &["disk", "space", "storage", "home"],
                Danger::Safe,
                || {
                    Box::pin(async {
                        let root = dirs::home_dir()
                            .ok_or_else(|| anyhow::anyhow!("home directory is unavailable"))?;
                        analyzer::run(root).await
                    })
                },
            ),
            ActionSpec::new(
                "disk.scan-here",
                "Analyze current folder",
                "Find what uses space below the current folder",
                "$ holla disk analyze .",
                &["disk", "space", "storage", "current", "folder"],
                Danger::Safe,
                || Box::pin(async { analyzer::run(std::env::current_dir()?).await }),
            ),
            ActionSpec::new(
                "disk.scan-custom",
                "Analyze a path…",
                "Choose an absolute folder or file to analyze",
                "$ holla disk analyze <path>",
                &["disk", "space", "storage", "path", "custom"],
                Danger::Safe,
                || {
                    Box::pin(async {
                        if let Some(root) = analyzer::prompt_path().await? {
                            analyzer::run(root).await?;
                        }
                        Ok(())
                    })
                },
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_group_is_always_present_with_safe_actions() {
        let group = group();
        assert_eq!(group.id, "disk");
        assert_eq!(group.actions.len(), 3);
        assert_eq!(
            group
                .actions
                .iter()
                .map(|action| action.id.as_str())
                .collect::<Vec<_>>(),
            ["disk.scan-home", "disk.scan-here", "disk.scan-custom"]
        );
        assert!(
            group
                .actions
                .iter()
                .all(|action| action.danger == Danger::Safe)
        );
    }
}
