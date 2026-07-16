use crate::{frecency::FrecencyStore, model::GroupSpec};
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

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

pub fn search_with_history(
    groups: &[GroupSpec],
    query: &str,
    history: &FrecencyStore,
    now: u64,
) -> Vec<SearchHit> {
    let mut hits = search(groups, query);
    let remembered = history.remembered_action(query);
    hits.sort_by(|left, right| {
        let left_action = &groups[left.group].actions[left.action];
        let right_action = &groups[right.group].actions[right.action];
        let left_rank = history_rank(left.score, history.score(&left_action.id, now));
        let right_rank = history_rank(right.score, history.score(&right_action.id, now));
        let left_remembered = remembered == Some(left_action.id.as_str());
        let right_remembered = remembered == Some(right_action.id.as_str());
        right_remembered
            .cmp(&left_remembered)
            .then_with(|| right_rank.total_cmp(&left_rank))
            .then_with(|| right.score.cmp(&left.score))
            .then_with(|| left.group.cmp(&right.group))
            .then_with(|| left.action.cmp(&right.action))
    });
    hits
}

fn history_rank(base: u32, frecency: f64) -> f64 {
    let base = f64::from(base);
    let frecency_boost = base * frecency.clamp(0.0, 100.0) / 100.0 * 0.25;
    base + frecency_boost
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
    use unicode_segmentation::UnicodeSegmentation;

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
    fn match_indices_count_unicode_graphemes() {
        let groups = fixtures();
        let hit = search(&groups, "logs").remove(0);

        let label_start = groups[hit.group].title.graphemes(true).count() + 1;
        assert_eq!(
            hit.indices,
            [
                label_start as u32 + 7,
                label_start as u32 + 8,
                label_start as u32 + 9,
                label_start as u32 + 10
            ]
        );
    }

    #[test]
    fn empty_history_keeps_original_ranking() {
        let groups = fixtures();
        assert_eq!(
            search_with_history(&groups, "cleanup", &FrecencyStore::default(), 100),
            search(&groups, "cleanup")
        );
    }

    #[test]
    fn frecency_boost_is_capped_at_twenty_five_percent() {
        assert_eq!(history_rank(100, 10_000.0), 125.0);
    }

    #[test]
    fn negative_frecency_never_reduces_text_score() {
        assert_eq!(history_rank(100, -1.0), 100.0);
    }

    #[test]
    fn remembered_query_wins_equal_text_and_max_frecency() {
        let groups = fixtures();
        let mut history = FrecencyStore::default();
        for timestamp in 80..100 {
            history.record("gradle.clean", "", timestamp);
        }
        history.record("docker.stop", "cleanup", 1);

        let hits = search_with_history(&groups, "cleanup", &history, 100);

        assert_eq!(
            groups[hits[0].group].actions[hits[0].action].id,
            "docker.stop"
        );
    }

    #[test]
    fn remembered_query_is_fixed_top_priority_despite_lower_text_score() {
        let groups = vec![GroupSpec {
            id: "tools",
            title: "Tools".into(),
            actions: vec![
                action("strong", "cleanup", &[]),
                action("remembered", "c-l-e-a-n-u-p", &[]),
            ],
        }];
        let baseline = search(&groups, "cleanup");
        assert!(baseline[0].score > baseline[1].score);
        assert_eq!(
            groups[baseline[1].group].actions[baseline[1].action].id,
            "remembered"
        );
        let mut history = FrecencyStore::default();
        history.record("remembered", "cleanup", 100);

        let hits = search_with_history(&groups, "cleanup", &history, 100);

        assert_eq!(
            groups[hits[0].group].actions[hits[0].action].id,
            "remembered"
        );
    }
}
