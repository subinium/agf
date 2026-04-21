use nucleo::{
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};

use crate::model::Session;

pub struct FuzzyMatcher {
    matcher: Matcher,
    buf: Vec<char>,
}

pub struct MatchResult {
    /// Index into the `indices` slice passed to `filter` (NOT into `sessions`).
    /// Callers that passed `indices = &[usize]` should resolve to the session
    /// via `sessions[indices[result.index]]`.
    pub index: usize,
    pub score: u32,
    pub positions: Vec<u32>,
}

impl FuzzyMatcher {
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
            buf: Vec::new(),
        }
    }

    /// Filter a subset of `sessions` (addressed by `indices`) against `query`.
    ///
    /// `indices` is a list of indices into `sessions` identifying which sessions
    /// to consider — this lets callers pre-filter (e.g. by agent) without
    /// cloning `Session`s into an intermediate `Vec`.
    ///
    /// Each `MatchResult.index` is a position into `indices`, so the matching
    /// session is `&sessions[indices[result.index]]`.
    pub fn filter(
        &mut self,
        sessions: &[Session],
        indices: &[usize],
        query: &str,
        max_summaries: usize,
        include_summaries: bool,
    ) -> Vec<MatchResult> {
        if query.is_empty() {
            return (0..indices.len())
                .map(|i| MatchResult {
                    index: i,
                    score: 0,
                    positions: Vec::new(),
                })
                .collect();
        }

        let pattern = Pattern::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        let mut results: Vec<MatchResult> = indices
            .iter()
            .enumerate()
            .filter_map(|(i, &session_idx)| {
                let session = sessions.get(session_idx)?;
                let text = session.search_text(max_summaries, include_summaries);
                let haystack = Utf32Str::new(&text, &mut self.buf);
                let mut positions = Vec::new();
                let score = pattern.indices(haystack, &mut self.matcher, &mut positions)?;
                positions.sort_unstable();
                positions.dedup();
                Some(MatchResult {
                    index: i,
                    score,
                    positions,
                })
            })
            .collect();

        results.sort_by_key(|r| std::cmp::Reverse(r.score));
        results
    }
}
