use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};

pub(crate) fn fuzzy_indices(value: &str, query: &str) -> Option<Vec<usize>> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }

    let (_, indices) = SkimMatcherV2::default().fuzzy_indices(value, query)?;
    let first = *indices.first()?;
    let last = *indices.last()?;
    let unmatched_span = last
        .saturating_sub(first)
        .saturating_add(1)
        .saturating_sub(indices.len());
    let allowed_gap = indices.len().div_ceil(2).clamp(1, 4);

    (unmatched_span <= allowed_gap).then_some(indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_compact_fuzzy_matches() {
        assert!(fuzzy_indices("prod-api", "prdapi").is_some());
        assert!(fuzzy_indices("Billing frontend", "bill").is_some());
    }

    #[test]
    fn rejects_characters_scattered_across_a_sentence() {
        assert!(fuzzy_indices("all hosts from testlab", "home").is_none());
    }
}
