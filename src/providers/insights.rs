use crate::{
    insights::{self, InsightSpec},
    model::{ActionSpec, Danger, GroupSpec},
    providers::Provider,
    tui,
};

pub struct InsightsProvider;

impl Provider for InsightsProvider {
    fn id(&self) -> &'static str {
        "insights"
    }

    fn scan(&self) -> Option<GroupSpec> {
        let detected: Vec<_> = insights::Probe::current()
            .map(|probe| {
                insights::REGISTRY
                    .iter()
                    .filter(|spec| insights::detect(spec, &probe))
                    .collect()
            })
            .unwrap_or_default();
        Some(group(&detected))
    }
}

pub(super) fn group(detected: &[&'static InsightSpec]) -> GroupSpec {
    let mut actions = vec![ActionSpec::new(
        "cleanup.review-all",
        "Review all cleanup candidates",
        "Size and review detected developer storage before choosing anything to remove",
        "$ holla cleanup review",
        &["cleanup", "cache", "storage", "disk"],
        Danger::Safe,
        || Box::pin(tui::insights::run(None)),
    )];
    actions.extend(
        detected
            .iter()
            .filter(|spec| spec.id != "docker.data")
            .map(|spec| {
                let id = spec.id;
                let mut action = ActionSpec::new(
                    format!("cleanup.{id}"),
                    format!("cleanup: {}", spec.title),
                    spec.explain,
                    format!("$ holla cleanup review {id}"),
                    &["cleanup", "cache", "storage"],
                    Danger::Safe,
                    move || Box::pin(tui::insights::run(Some(id))),
                );
                action
                    .keywords
                    .push(id.split('.').next().unwrap_or(id).to_owned());
                action
            }),
    );
    GroupSpec {
        id: "cleanup".into(),
        title: "Cleanup".into(),
        actions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_always_has_review_all_and_detected_actions() {
        let group = group(&[
            insights::spec("brew.cache").unwrap(),
            insights::spec("user.logs").unwrap(),
        ]);
        assert_eq!(group.id, "cleanup");
        assert_eq!(group.actions.len(), 3);
        assert_eq!(group.actions[0].id, "cleanup.review-all");
        assert!(
            group
                .actions
                .iter()
                .all(|action| action.danger == Danger::Safe)
        );
    }

    #[test]
    fn detected_action_keywords_include_tool_name() {
        let group = group(&[insights::spec("brew.cache").unwrap()]);
        assert!(group.actions[1].keywords.iter().any(|word| word == "brew"));
    }

    #[test]
    fn docker_pointer_is_left_to_docker_provider_actions() {
        let group = group(&[insights::spec("docker.data").unwrap()]);
        assert_eq!(group.actions.len(), 1);
        assert_eq!(group.actions[0].id, "cleanup.review-all");
    }
}
