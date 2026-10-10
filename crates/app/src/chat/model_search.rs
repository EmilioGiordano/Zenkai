use zenkai_agent::chat::state::Choice;

pub(super) fn matching<'a>(choices: &'a [Choice], query: &str) -> Vec<&'a Choice> {
    let needle = query.trim().to_lowercase();
    choices
        .iter()
        .filter(|choice| choice.label.to_lowercase().contains(&needle))
        .collect()
}

pub(super) fn step(index: usize, count: usize, forward: bool) -> usize {
    if count == 0 {
        return 0;
    }
    let index = index.min(count - 1);
    if forward {
        (index + 1) % count
    } else {
        (index + count - 1) % count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(label: &str) -> Choice {
        Choice {
            value: label.to_string(),
            label: label.to_string(),
            description: None,
        }
    }

    #[test]
    fn filters_by_case_insensitive_substring_of_the_name() {
        let choices = vec![
            choice("GPT-5 Mini"),
            choice("Claude Sonnet"),
            choice("gpt-4o"),
        ];
        let found: Vec<&str> = matching(&choices, "GPT")
            .iter()
            .map(|c| c.label.as_str())
            .collect();
        assert_eq!(found, ["GPT-5 Mini", "gpt-4o"]);
    }

    #[test]
    fn empty_query_keeps_everything_and_no_match_gives_nothing() {
        let choices = vec![choice("a"), choice("b")];
        assert_eq!(matching(&choices, "  ").len(), 2);
        assert!(matching(&choices, "zzz").is_empty());
    }

    #[test]
    fn step_wraps_inside_the_filtered_list() {
        assert_eq!(step(0, 3, true), 1);
        assert_eq!(step(2, 3, true), 0);
        assert_eq!(step(0, 3, false), 2);
    }

    #[test]
    fn step_clamps_a_stale_index_and_survives_an_empty_list() {
        assert_eq!(step(9, 2, true), 0);
        assert_eq!(step(9, 2, false), 0);
        assert_eq!(step(4, 0, true), 0);
    }
}
