//! Reciprocal Rank Fusion (RRF) — merges two ranked result lists (e.g. FTS5 lexical
//! BM25 + dense KNN) into one, using ONLY positions, never raw scores. This sidesteps
//! the incompatible-scale problem (BM25 ~14.x vs cosine ~0.8): no normalization needed.
//!
//!   score(doc) = Σ over lists  1 / (k + rank_in_list)        (k ≈ 60)
//!
//! A document ranked high in BOTH lists wins. This is the application-side fusion for
//! Altevra's opt-in hybrid layer (R15); it lives in plain Rust with zero new deps and
//! is independent of the embedding feature so it is always compiled and tested.

/// Default RRF constant from the original Cormack et al. paper. Dampens the influence
/// of exact position while rewarding "near the top of both lists".
pub const DEFAULT_RRF_K: f32 = 60.0;

/// Fuse any number of ranked id-lists (each already sorted best-first) into one ranked
/// list, highest fused score first. Ids may be any hashable+orderable key.
pub fn rrf_fuse<I>(lists: &[Vec<I>], k: f32) -> Vec<(I, f32)>
where
    I: Clone + Eq + std::hash::Hash + Ord,
{
    use std::collections::HashMap;
    let mut scores: HashMap<I, f32> = HashMap::new();
    for list in lists {
        for (rank, id) in list.iter().enumerate() {
            *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k + rank as f32);
        }
    }
    let mut out: Vec<(I, f32)> = scores.into_iter().collect();
    // Sort by fused score desc; tie-break by id asc for deterministic output.
    out.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    out
}

/// Convenience for the canonical two-list case (lexical + dense), default k.
pub fn rrf_fuse_two<I>(lexical: Vec<I>, dense: Vec<I>) -> Vec<(I, f32)>
where
    I: Clone + Eq + std::hash::Hash + Ord,
{
    rrf_fuse(&[lexical, dense], DEFAULT_RRF_K)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_high_in_both_lists_wins() {
        // "b" is rank 1 in lexical and rank 0 in dense → should top the fusion.
        let lexical = vec!["a", "b", "c"];
        let dense = vec!["b", "d", "a"];
        let fused = rrf_fuse_two(lexical, dense);
        assert_eq!(fused[0].0, "b", "present near top of both lists wins");
    }

    #[test]
    fn no_normalization_needed_pure_rank() {
        // Identical lists → first element keeps the highest score.
        let l = vec![1u32, 2, 3];
        let fused = rrf_fuse(&[l.clone(), l], DEFAULT_RRF_K);
        assert_eq!(fused[0].0, 1);
        assert!(fused[0].1 > fused[1].1);
    }

    #[test]
    fn deterministic_tie_break_by_id() {
        // Two docs each appearing once at rank 0 in separate lists → equal score,
        // tie-broken by id ascending.
        let fused = rrf_fuse(&[vec!["z"], vec!["a"]], DEFAULT_RRF_K);
        assert_eq!(fused[0].0, "a");
        assert_eq!(fused[1].0, "z");
        assert!((fused[0].1 - fused[1].1).abs() < f32::EPSILON);
    }

    #[test]
    fn empty_lists_yield_empty() {
        let fused: Vec<(u32, f32)> = rrf_fuse(&[], DEFAULT_RRF_K);
        assert!(fused.is_empty());
    }

    #[test]
    fn union_of_ids_is_preserved() {
        let fused = rrf_fuse_two(vec!["a", "b"], vec!["c"]);
        assert_eq!(fused.len(), 3);
    }
}
