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
    actions.extend(detected.iter().map(|spec| {
        let id = spec.id;
        ActionSpec::new(
            format!("cleanup.{id}"),
            format!("cleanup: {}", spec.title),
            spec.explain,
            format!("$ holla cleanup review {id}"),
            &["cleanup", "cache", "storage"],
            Danger::Safe,
            move || Box::pin(tui::insights::run(Some(id))),
        )
    }));
    GroupSpec {
        id: "cleanup",
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
}
