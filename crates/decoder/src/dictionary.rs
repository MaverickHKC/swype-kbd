//! The word list and its frequency prior.
//!
//! Each word carries a log-frequency weight used by the decoder's prior. The
//! embedded English list is ordered by descending frequency, so we assign a
//! Zipfian weight from rank (`ln(1/rank)`). A real count-based list can be
//! loaded later (milestone 5) via [`Dictionary::from_counts`].

/// A vocabulary with per-word log-frequency weights, parallel-indexed.
#[derive(Debug, Clone, Default)]
pub struct Dictionary {
    words: Vec<String>,
    ln_freq: Vec<f32>,
}

impl Dictionary {
    /// Build from words already sorted by descending frequency. Duplicates are
    /// dropped, keeping the first (highest-frequency) occurrence. Words are
    /// lowercased; empties are skipped. Weight is Zipfian: `ln(1 / rank)`.
    pub fn from_ranked<I, S>(words: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for w in words {
            let w = w.as_ref().trim().to_ascii_lowercase();
            if w.is_empty() || !seen.insert(w.clone()) {
                continue;
            }
            out.push(w);
        }
        let ln_freq = (0..out.len())
            .map(|rank| -((rank as f32) + 1.0).ln())
            .collect();
        Dictionary { words: out, ln_freq }
    }

    /// Build from `(word, count)` pairs of any order. Weight is `ln(count)`,
    /// normalized so the most frequent word has weight 0 (matching the ranked
    /// constructor's scale). Counts <= 0 are skipped.
    pub fn from_counts<I, S>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (S, f64)>,
        S: AsRef<str>,
    {
        let mut words = Vec::new();
        let mut raw = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut max_ln = f64::NEG_INFINITY;
        for (w, count) in pairs {
            let w = w.as_ref().trim().to_ascii_lowercase();
            if w.is_empty() || count <= 0.0 || !seen.insert(w.clone()) {
                continue;
            }
            let ln = count.ln();
            max_ln = max_ln.max(ln);
            words.push(w);
            raw.push(ln);
        }
        let ln_freq = raw.iter().map(|ln| (ln - max_ln) as f32).collect();
        Dictionary { words, ln_freq }
    }

    /// Parse a whitespace-separated `word count` list (one per line, e.g. the
    /// Norvig unigram list). Lines that don't parse are skipped. Order is
    /// irrelevant; weights come from the counts.
    pub fn parse_counts(text: &str) -> Self {
        let pairs = text.lines().filter_map(|line| {
            let mut it = line.split_whitespace();
            let word = it.next()?;
            let count: f64 = it.next()?.parse().ok()?;
            Some((word.to_string(), count))
        });
        Self::from_counts(pairs)
    }

    /// The embedded English frequency list (~few hundred common words). Used as
    /// a fallback when no external dictionary file is found.
    pub fn english() -> Self {
        Self::from_ranked(include_str!("words_en.txt").lines())
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn word(&self, index: usize) -> &str {
        &self.words[index]
    }

    pub fn ln_freq(&self, index: usize) -> f32 {
        self.ln_freq[index]
    }

    pub fn words(&self) -> &[String] {
        &self.words
    }

    /// Rank (index) of a word, if present. Linear scan — for tests/setup only.
    pub fn rank_of(&self, word: &str) -> Option<usize> {
        let w = word.to_ascii_lowercase();
        self.words.iter().position(|x| *x == w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranked_dedups_and_weights_descend() {
        let d = Dictionary::from_ranked(["the", "and", "the", "for"]);
        assert_eq!(d.len(), 3); // duplicate "the" dropped
        assert_eq!(d.word(0), "the");
        // weights strictly decrease with rank
        assert!(d.ln_freq(0) > d.ln_freq(1));
        assert!(d.ln_freq(1) > d.ln_freq(2));
        assert_eq!(d.ln_freq(0), 0.0); // ln(1/1) = 0
    }

    #[test]
    fn english_list_loads_and_is_deduped() {
        let d = Dictionary::english();
        assert!(d.len() > 200, "expected a few hundred words, got {}", d.len());
        let mut seen = std::collections::HashSet::new();
        for w in d.words() {
            assert!(seen.insert(w), "duplicate survived: {w}");
            assert!(w.chars().all(|c| c.is_ascii_lowercase()));
        }
        assert!(d.rank_of("the").is_some());
    }

    #[test]
    fn parse_counts_reads_word_count_lines() {
        let d = Dictionary::parse_counts("the 100\nword 40\n\nbad-line\nمرحبا 5\nfoo notanumber\n");
        // "the" and "word" parse; blank/garbage/non-ascii-count lines drop.
        assert!(d.rank_of("the").is_some());
        assert!(d.rank_of("word").is_some());
        assert!(d.ln_freq(d.rank_of("the").unwrap()) > d.ln_freq(d.rank_of("word").unwrap()));
    }

    #[test]
    fn counts_normalize_to_zero_max() {
        let d = Dictionary::from_counts([("rare", 2.0), ("common", 100.0)]);
        let common = d.rank_of("common").unwrap();
        let rare = d.rank_of("rare").unwrap();
        assert!((d.ln_freq(common) - 0.0).abs() < 1e-5);
        assert!(d.ln_freq(rare) < 0.0);
    }
}
