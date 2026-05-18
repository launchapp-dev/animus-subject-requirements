//! Sequential `REQ-NNNN` id generator.
//!
//! IDs are zero-padded to four digits, e.g. `REQ-0001`, `REQ-0042`,
//! `REQ-1234`. When the existing maximum exceeds four digits the padding
//! grows automatically — `REQ-10000` follows `REQ-9999`.

/// Width of the zero-padded numeric suffix.
const MIN_PAD: usize = 4;

/// Compute the next sequential id given the set of existing native ids
/// (e.g. `["REQ-0001", "REQ-0002"]`).
///
/// - `prefix` is the id prefix (without trailing dash), e.g. `"REQ"`.
/// - `existing` is an iterator of bare native ids (no `requirement:` wrap).
///
/// Returns `"REQ-0001"` when the existing set is empty or has no matches.
pub fn next_id<I, S>(prefix: &str, existing: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let max = existing
        .into_iter()
        .filter_map(|s| extract_number(prefix, s.as_ref()))
        .max()
        .unwrap_or(0);
    let next = max + 1;
    let pad = std::cmp::max(MIN_PAD, decimal_width(next));
    format!(
        "{prefix}-{next:0>pad$}",
        prefix = prefix,
        next = next,
        pad = pad
    )
}

fn extract_number(prefix: &str, id: &str) -> Option<u64> {
    let remainder = id.strip_prefix(prefix)?.strip_prefix('-')?;
    remainder.parse::<u64>().ok()
}

fn decimal_width(n: u64) -> usize {
    if n == 0 {
        1
    } else {
        let mut w = 0;
        let mut x = n;
        while x > 0 {
            w += 1;
            x /= 10;
        }
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_set_yields_one() {
        assert_eq!(next_id("REQ", std::iter::empty::<&str>()), "REQ-0001");
    }

    #[test]
    fn skips_holes_uses_max_plus_one() {
        let existing = ["REQ-0001", "REQ-0003", "REQ-0007"];
        assert_eq!(next_id("REQ", existing), "REQ-0008");
    }

    #[test]
    fn ignores_other_prefixes() {
        let existing = ["TASK-0099", "REQ-0002"];
        assert_eq!(next_id("REQ", existing), "REQ-0003");
    }

    #[test]
    fn ignores_malformed_ids() {
        let existing = ["REQ-abc", "REQ-", "REQ-0005"];
        assert_eq!(next_id("REQ", existing), "REQ-0006");
    }

    #[test]
    fn grows_padding_beyond_four_digits() {
        let existing = ["REQ-9999"];
        assert_eq!(next_id("REQ", existing), "REQ-10000");
    }

    #[test]
    fn honors_alternate_prefix() {
        let existing = ["RQ-0004"];
        assert_eq!(next_id("RQ", existing), "RQ-0005");
    }
}
