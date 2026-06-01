//! PII detection — phone numbers, IBANs, and payment-card numbers.
//!
//! Complements the credential [`crate::detector`]. Where secret detection looks
//! for API keys/tokens, this looks for *personal* identifiers that must be
//! redacted before any turn/document is persisted (R11 finding: the P0 guard
//! detected emails only, so phone/IBAN/card slipped through into stored content
//! and were then exposed via search/replay).
//!
//! Precision over recall: IBAN matches are mod-97 validated and card matches are
//! Luhn validated, so a random 16-digit order number is NOT flagged as a card.
//! Phone matching requires an international prefix (`+` / `00`) for the same
//! reason. The sensitivity classifier treats any hit as fail-closed (raises the
//! level), so a miss is the only real risk — but a false positive only
//! over-protects, which the doctrine accepts.

use regex::Regex;
use std::sync::OnceLock;

/// The kind of personal identifier matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiiKind {
    /// Phone number in international form (`+`/`00` prefix).
    Phone,
    /// IBAN bank account number (mod-97 validated).
    Iban,
    /// Payment-card number (Luhn validated).
    CreditCard,
}

/// A single PII occurrence (byte offsets into the scanned text).
#[derive(Debug, Clone)]
pub struct PiiMatch {
    pub kind: PiiKind,
    pub start: usize,
    pub end: usize,
}

struct PiiPatterns {
    phone: Regex,
    iban: Regex,
    card: Regex,
}

fn patterns() -> &'static PiiPatterns {
    static CELL: OnceLock<PiiPatterns> = OnceLock::new();
    CELL.get_or_init(|| PiiPatterns {
        // International phone: + or 00, then a digit, then 6..=17 more digit/sep
        // chars ending on a digit. Digit-count is validated post-match (8..=15).
        phone: Regex::new(r"(?:\+|00)[0-9][0-9 ().\-]{6,17}[0-9]").unwrap(),
        // IBAN: 2 letters + 2 check digits + 11..=30 alnum, case-insensitive and
        // tolerant of bank-printed single-space grouping. mod-97 validated after
        // whitespace strip, so false positives are filtered.
        iban: Regex::new(
            r"(?i)\b[A-Z]{2}[0-9]{2}(?:[A-Z0-9]{11,30}|(?: [A-Z0-9]{4})+(?: [A-Z0-9]{1,3})?)\b",
        )
        .unwrap(),
        // Card candidate: 13..=19 digits with optional single space/dash between.
        card: Regex::new(r"\b[0-9](?:[ \-]?[0-9]){12,18}\b").unwrap(),
    })
}

/// Scan `text` and return validated PII occurrences, sorted by start offset,
/// non-overlapping (earliest-start wins).
pub fn detect_pii(text: &str) -> Vec<PiiMatch> {
    let p = patterns();
    let mut out: Vec<PiiMatch> = Vec::new();

    for m in p.phone.find_iter(text) {
        let digits = m.as_str().chars().filter(|c| c.is_ascii_digit()).count();
        if (8..=15).contains(&digits) {
            out.push(PiiMatch {
                kind: PiiKind::Phone,
                start: m.start(),
                end: m.end(),
            });
        }
    }
    for m in p.iban.find_iter(text) {
        if iban_valid(m.as_str()) {
            out.push(PiiMatch {
                kind: PiiKind::Iban,
                start: m.start(),
                end: m.end(),
            });
        }
    }
    for m in p.card.find_iter(text) {
        let digits: Vec<u8> = m
            .as_str()
            .bytes()
            .filter(u8::is_ascii_digit)
            .map(|b| b - b'0')
            .collect();
        if luhn_valid(&digits) {
            out.push(PiiMatch {
                kind: PiiKind::CreditCard,
                start: m.start(),
                end: m.end(),
            });
        }
    }

    out.sort_by_key(|m| m.start);
    // Drop overlaps (e.g. an IBAN whose tail digits also look card-shaped).
    let mut dedup: Vec<PiiMatch> = Vec::with_capacity(out.len());
    for m in out {
        if let Some(prev) = dedup.last() {
            if m.start < prev.end {
                continue;
            }
        }
        dedup.push(m);
    }
    dedup
}

/// Luhn checksum over already-extracted decimal digits (13..=19 length).
fn luhn_valid(digits: &[u8]) -> bool {
    if !(13..=19).contains(&digits.len()) {
        return false;
    }
    let mut sum = 0u32;
    let mut alt = false;
    for &d in digits.iter().rev() {
        let mut v = d as u32;
        if alt {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
        alt = !alt;
    }
    sum.is_multiple_of(10)
}

/// ISO 7064 mod-97 IBAN validation. Whitespace is ignored; length 15..=34.
fn iban_valid(raw: &str) -> bool {
    let s: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    if !(15..=34).contains(&s.len()) {
        return false;
    }
    let bytes = s.as_bytes();
    // First two chars must be letters (country), next two digits (check).
    if !(bytes[0].is_ascii_alphabetic()
        && bytes[1].is_ascii_alphabetic()
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit())
    {
        return false;
    }
    // Rearrange: move the first 4 chars to the end, then reduce mod 97.
    let (head, tail) = s.split_at(4);
    let mut rem: u32 = 0;
    for c in tail.chars().chain(head.chars()) {
        let val = if c.is_ascii_digit() {
            c as u32 - '0' as u32
        } else if c.is_ascii_alphabetic() {
            (c.to_ascii_uppercase() as u32 - 'A' as u32) + 10
        } else {
            return false;
        };
        rem = if val >= 10 {
            (rem * 100 + val) % 97
        } else {
            (rem * 10 + val) % 97
        };
    }
    rem == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_international_phone() {
        let hits = detect_pii("call Elena at +381 64 123 4567 tonight");
        assert!(hits.iter().any(|h| h.kind == PiiKind::Phone), "{hits:?}");
        // A bare 4-digit number is not a phone.
        assert!(detect_pii("room 4567")
            .iter()
            .all(|h| h.kind != PiiKind::Phone));
    }

    #[test]
    fn detects_valid_iban_rejects_invalid() {
        // Valid example IBAN (mod-97 == 1).
        let ok = detect_pii("IBAN GB82WEST12345698765432");
        assert!(ok.iter().any(|h| h.kind == PiiKind::Iban), "{ok:?}");
        // Same shape, wrong check digits → rejected by mod-97.
        let bad = detect_pii("GB00WEST12345698765432");
        assert!(bad.iter().all(|h| h.kind != PiiKind::Iban));
    }

    #[test]
    fn detects_spaced_and_lowercase_iban() {
        // Bank-printed grouping (spaces) must be detected after whitespace strip.
        let spaced = detect_pii("acct GB82 WEST 1234 5698 7654 32 ok");
        assert!(spaced.iter().any(|h| h.kind == PiiKind::Iban), "{spaced:?}");
        // Lowercase IBAN must also match (case-insensitive prefix).
        let lower = detect_pii("gb82west12345698765432");
        assert!(lower.iter().any(|h| h.kind == PiiKind::Iban), "{lower:?}");
    }

    #[test]
    fn detects_luhn_valid_card_rejects_random() {
        // Classic Luhn-valid test PAN.
        let ok = detect_pii("card 4111 1111 1111 1111 exp");
        assert!(ok.iter().any(|h| h.kind == PiiKind::CreditCard), "{ok:?}");
        // A 16-digit number that fails Luhn (valid PAN with last digit flipped)
        // must NOT be flagged.
        let bad = detect_pii("order 4111111111111112"); // Luhn sum 31 → invalid
        assert!(bad.iter().all(|h| h.kind != PiiKind::CreditCard));
    }

    #[test]
    fn empty_and_clean_text_no_matches() {
        assert!(detect_pii("").is_empty());
        assert!(detect_pii("just some harmless prose with no identifiers").is_empty());
    }
}
