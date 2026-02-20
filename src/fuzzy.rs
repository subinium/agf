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

    pub fn filter(
        &mut self,
        sessions: &[Session],
        query: &str,
        max_summaries: usize,
        include_summaries: bool,
    ) -> Vec<MatchResult> {
        if query.is_empty() {
            return sessions
                .iter()
                .enumerate()
                .map(|(i, _)| MatchResult {
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

        let mut results: Vec<MatchResult> = sessions
            .iter()
            .enumerate()
            .filter_map(|(i, session)| {
                let text = session.search_text(max_summaries, include_summaries);
                let haystack = Utf32Str::new(&text, &mut self.buf);
                let mut indices = Vec::new();
                let score = pattern.indices(haystack, &mut self.matcher, &mut indices)?;
                indices.sort_unstable();
                indices.dedup();
                Some(MatchResult {
                    index: i,
                    score,
                    positions: indices,
                })
            })
            .collect();

        results.sort_by(|a, b| b.score.cmp(&a.score));
        results
    }
}
