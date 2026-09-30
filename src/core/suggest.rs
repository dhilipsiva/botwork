//! Near-name suggestions for names a diagnostic reports as undefined.

/// Names longer than this are neither suggested nor given suggestions.
const MAX_LENGTH: usize = 64;
/// Most candidates compared for one suggestion: those that sort first.
const MAX_CANDIDATES: usize = 4096;

/// The candidate nearest to `name`, when one is within a third of its length in
/// edits (at least one edit). Edits are insertions, deletions, substitutions,
/// and swaps of adjacent characters; a case change is one substitution. Ties go
/// to the candidate that sorts first, so the choice never depends on the order
/// candidates arrive in.
pub(crate) fn nearest<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    // Checked before copying: names in diagnostics can be arbitrarily long.
    if name.is_empty() || name.chars().nth(MAX_LENGTH).is_some() {
        return None;
    }
    let target: Vec<char> = name.chars().collect();
    let limit = (target.len() / 3).max(1);
    let mut close: Vec<&str> = candidates
        .into_iter()
        .filter(|candidate| {
            *candidate != name
                && candidate.len() <= 4 * (target.len() + limit)
                && candidate.chars().count().abs_diff(target.len()) <= limit
        })
        .collect();
    if close.len() > MAX_CANDIDATES {
        close.select_nth_unstable(MAX_CANDIDATES);
        close.truncate(MAX_CANDIDATES);
    }
    // The least (edits, name) pair: ties go to the name that sorts first.
    close
        .into_iter()
        .filter_map(|candidate| {
            let chars: Vec<char> = candidate.chars().collect();
            let edits = distance(&target, &chars);
            (edits <= limit).then_some((edits, candidate))
        })
        .min()
        .map(|(_, candidate)| candidate)
}

/// Optimal string alignment distance between two character sequences.
fn distance(left: &[char], right: &[char]) -> usize {
    let width = right.len() + 1;
    let mut rows = vec![0; (left.len() + 1) * width];
    for (column, cell) in rows.iter_mut().take(width).enumerate() {
        *cell = column;
    }
    for row in 1..=left.len() {
        rows[row * width] = row;
        for column in 1..width {
            let cost = usize::from(left[row - 1] != right[column - 1]);
            let mut best = (rows[(row - 1) * width + column] + 1)
                .min(rows[row * width + column - 1] + 1)
                .min(rows[(row - 1) * width + column - 1] + cost);
            if row > 1
                && column > 1
                && left[row - 1] == right[column - 2]
                && left[row - 2] == right[column - 1]
            {
                best = best.min(rows[(row - 2) * width + column - 2] + 1);
            }
            rows[row * width + column] = best;
        }
    }
    rows[left.len() * width + right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_the_nearest_name_within_a_third_of_its_length() {
        let names = ["quantity", "unit_price", "discount", "gross"];
        assert_eq!(nearest("discont", names), Some("discount"));
        assert_eq!(nearest("Discount", names), Some("discount"));
        assert_eq!(nearest("gorss", names), Some("gross"));
        assert_eq!(nearest("unit_prize", names), Some("unit_price"));
        // Too far: "total" is five edits from everything here, and three edits
        // exceed a third of six characters.
        assert_eq!(nearest("total", names), None);
        assert_eq!(nearest("abcdef", ["abcxyz"]), None);
        assert_eq!(nearest("abcdef", ["abcdxy"]), Some("abcdxy"));
        // An exact match is not a suggestion.
        assert_eq!(nearest("gross", names), None);
    }

    #[test]
    fn short_names_allow_one_edit() {
        assert_eq!(nearest("x", ["y", "xs"]), Some("xs"));
        assert_eq!(nearest("ab", ["ba"]), Some("ba"));
        assert_eq!(nearest("abc", ["xyz", "abd"]), Some("abd"));
        assert_eq!(nearest("abc", ["axy"]), None);
    }

    #[test]
    fn ties_go_to_the_name_that_sorts_first_whatever_the_order() {
        assert_eq!(nearest("cat", ["mat", "bat", "hat"]), Some("bat"));
        assert_eq!(nearest("cat", ["hat", "mat", "bat"]), Some("bat"));
        // Fewer edits win over sort order.
        assert_eq!(nearest("items", ["iterms", "atoms"]), Some("iterms"));
    }

    #[test]
    fn counts_characters_not_bytes() {
        assert_eq!(nearest("café", ["cafe", "caff"]), Some("cafe"));
        assert_eq!(nearest("naïve", ["naive"]), Some("naive"));
    }

    #[test]
    fn long_or_empty_names_and_huge_scopes_stay_bounded() {
        let long = "x".repeat(MAX_LENGTH + 1);
        assert_eq!(nearest(&long, [long[1..].as_ref()]), None);
        assert_eq!(nearest("", ["a"]), None);
        // Only the candidates that sort first are compared: a near name that
        // sorts after the cap's worth of far ones is not found.
        let far = |count: usize| {
            (0..count)
                .map(|index| format!("a{index:05}"))
                .collect::<Vec<_>>()
        };
        // The near name arrives first, so only sorting can leave it out.
        let few = far(10);
        let found = nearest(
            "mmmmmm",
            ["mmmmmn"].into_iter().chain(few.iter().map(String::as_str)),
        );
        assert_eq!(found, Some("mmmmmn"));
        let many = far(MAX_CANDIDATES);
        let beyond = nearest(
            "mmmmmm",
            ["mmmmmn"]
                .into_iter()
                .chain(many.iter().map(String::as_str)),
        );
        assert_eq!(beyond, None);
    }

    #[test]
    fn distance_counts_adjacent_swaps_once() {
        let chars = |text: &str| text.chars().collect::<Vec<_>>();
        assert_eq!(distance(&chars("ab"), &chars("ba")), 1);
        assert_eq!(distance(&chars("kitten"), &chars("sitting")), 3);
        assert_eq!(distance(&chars(""), &chars("abc")), 3);
        assert_eq!(distance(&chars("same"), &chars("same")), 0);
    }
}
