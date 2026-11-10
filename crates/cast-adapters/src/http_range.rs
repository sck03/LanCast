//! Single byte-range policy, independent of HTTP transport and file I/O.

#[derive(Debug, PartialEq)]
pub(crate) enum Range {
    Full,
    Partial { start: u64, end: u64 },
    Unsatisfiable,
}

impl Range {
    pub(crate) fn parse(header: Option<&str>, length: u64) -> Self {
        let Some((unit, raw)) = header.and_then(|raw| raw.split_once('=')) else {
            return Self::Full;
        };
        // Multipart ranges are intentionally served as a complete representation.
        if !unit.eq_ignore_ascii_case("bytes") || raw.contains(',') {
            return Self::Full;
        }
        let Some((left, right)) = raw.split_once('-') else {
            return Self::Unsatisfiable;
        };
        if length == 0 {
            return Self::Unsatisfiable;
        }
        if left.is_empty() {
            return match decimal(right) {
                Some(n) if n > 0 => Self::Partial {
                    start: length.saturating_sub(n),
                    end: length - 1,
                },
                _ => Self::Unsatisfiable,
            };
        }
        let Some(start) = decimal(left) else {
            return Self::Unsatisfiable;
        };
        let end = if right.is_empty() {
            length - 1
        } else {
            match decimal(right) {
                Some(n) => n.min(length - 1),
                None => return Self::Unsatisfiable,
            }
        };
        if start >= length || start > end {
            Self::Unsatisfiable
        } else {
            Self::Partial { start, end }
        }
    }
}

fn decimal(text: &str) -> Option<u64> {
    if text.is_empty() {
        return None;
    }
    // RFC 9110 allows arbitrarily large decimal numerals. Saturating retains
    // their ordering against every representable file offset, without allocating.
    // Continue validating digits after overflow; signs and trailing junk are invalid.
    text.bytes().try_fold(0u64, |value, digit| {
        digit.is_ascii_digit().then(|| {
            value
                .saturating_mul(10)
                .saturating_add(u64::from(digit - b'0'))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_edges() {
        for (header, start, end) in [
            ("bytes=-4", 6, 9),
            ("bytes=8-99", 8, 9),
            ("bytes=2-", 2, 9),
            ("bytes=0002-0004", 2, 4),
        ] {
            assert_eq!(
                Range::parse(Some(header), 10),
                Range::Partial { start, end }
            );
        }
        for header in ["bytes=10-", "bytes=-0", "bytes=5-4", "bytes=-", "bytes=1"] {
            assert_eq!(Range::parse(Some(header), 10), Range::Unsatisfiable);
        }
        assert_eq!(Range::parse(Some("bytes=0-"), 0), Range::Unsatisfiable);
        assert_eq!(Range::parse(None, 0), Range::Full);
        assert_eq!(Range::parse(Some("bytes=0-1,4-5"), 10), Range::Full);
        assert_eq!(Range::parse(Some("unknown=0-1"), 10), Range::Full);
    }

    #[test]
    fn decimal_overflow_does_not_reject_satisfiable_ranges() {
        for length in [1, 10, u64::MAX] {
            for header in [
                "bytes=0-18446744073709551615",
                "bytes=0-18446744073709551616",
                "bytes=0-9999999999999999999999999999999999999999",
                "bytes=-18446744073709551615",
                "bytes=-18446744073709551616",
            ] {
                assert_eq!(
                    Range::parse(Some(header), length),
                    Range::Partial {
                        start: 0,
                        end: length - 1
                    },
                    "{header}, length {length}"
                );
            }
            for header in ["bytes=18446744073709551615-", "bytes=18446744073709551616-"] {
                assert_eq!(Range::parse(Some(header), length), Range::Unsatisfiable);
            }
        }
        assert_eq!(
            Range::parse(Some("bytes=2-18446744073709551616"), 10),
            Range::Partial { start: 2, end: 9 }
        );
    }

    #[test]
    fn numbers_require_ascii_digits_and_units_ignore_case() {
        for header in [
            "bytes=+2-4",
            "bytes=2-+4",
            "bytes=-+4",
            "bytes=2-999999999999999999999x",
            "bytes=2- 4",
            "bytes=２-4",
            "bytes=2-4-5",
        ] {
            assert_eq!(
                Range::parse(Some(header), 10),
                Range::Unsatisfiable,
                "{header}"
            );
        }
        assert_eq!(
            Range::parse(Some("BYTES=2-4"), 10),
            Range::Partial { start: 2, end: 4 }
        );
    }
}
