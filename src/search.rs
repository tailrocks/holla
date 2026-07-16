use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

use crate::model::GroupSpec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub group: usize,
    pub action: usize,
    pub score: u32,
    pub indices: Vec<u32>,
}

pub fn search(groups: &[GroupSpec], query: &str) -> Vec<SearchHit> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut utf32_buf = Vec::new();
    let mut hits = Vec::new();
    for (group_index, group) in groups.iter().enumerate() {
        for action_index in 0..group.actions.len() {
            let haystack = haystack(group, action_index);
            let mut indices = Vec::new();
            if let Some(score) = pattern.indices(
                Utf32Str::new(&haystack, &mut utf32_buf),
                &mut matcher,
                &mut indices,
            ) {
                indices.sort_unstable();
                indices.dedup();
                hits.push(SearchHit {
                    group: group_index,
                    action: action_index,
                    score,
                    indices,
                });
            }
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.group.cmp(&right.group))
            .then_with(|| left.action.cmp(&right.action))
    });
    hits
}

pub fn label_indices(group: &GroupSpec, hit: &SearchHit) -> Vec<usize> {
    let start = group.title.chars().count().saturating_add(1);
    let end = start.saturating_add(group.actions[hit.action].label.chars().count());
    hit.indices
        .iter()
        .filter_map(|index| usize::try_from(*index).ok())
        .filter(|index| (*index >= start) && (*index < end))
        .map(|index| index - start)
        .collect()
}

fn haystack(group: &GroupSpec, action_index: usize) -> String {
    let action = &group.actions[action_index];
    format!(
        "{} {} {} {}",
        group.title,
        action.label,
        action.keywords.join(" "),
        action.description
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ActionSpec, Danger};

    fn action(id: &str, label: &str, keywords: &'static [&'static str]) -> ActionSpec {
        ActionSpec::new(
            id,
            label,
            "description",
            "$ true",
            keywords,
            Danger::Safe,
            || Box::pin(async { Ok(()) }),
        )
    }

    fn fixtures() -> Vec<GroupSpec> {
        vec![
            GroupSpec {
                id: "docker",
                title: "Docker".into(),
                actions: vec![
                    action("docker.stop", "stop containers", &["cleanup"]),
                    action("docker.logs", "follow logs", &["tail"]),
                ],
            },
            GroupSpec {
                id: "build",
                title: "Build".into(),
                actions: vec![action("gradle.clean", "gradle clean", &["cleanup"])],
            },
        ]
    }

    #[test]
    fn empty_query_has_no_ranked_projection() {
        assert!(search(&fixtures(), "").is_empty());
    }

    #[test]
    fn group_title_match_surfaces_every_group_action() {
        let hits = search(&fixtures(), "dock");

        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|hit| hit.group == 0));
    }

    #[test]
    fn keywords_match_across_groups() {
        let hits = search(&fixtures(), "cleanup");

        assert_eq!(hits.len(), 2);
        assert_ne!(hits[0].group, hits[1].group);
    }

    #[test]
    fn ranking_is_deterministic_for_equal_items() {
        let first = search(&fixtures(), "cleanup");
        let second = search(&fixtures(), "cleanup");

        assert_eq!(first, second);
    }

    #[test]
    fn label_indices_are_relative_to_the_visible_label() {
        let groups = fixtures();
        let hit = search(&groups, "logs").remove(0);

        assert_eq!(label_indices(&groups[hit.group], &hit), [7, 8, 9, 10]);
    }
}
