//! Okapi BM25 over recipe text, used only to suggest recipes when no pattern
//! matches. It never selects a recipe on its own.

use std::collections::BTreeMap;

const K1: f64 = 1.2;
const B: f64 = 0.75;

/// Words too common to tell recipes apart.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "be", "can", "do", "e", "for", "g", "get", "give", "how", "i", "in",
    "is", "it", "let", "like", "make", "me", "my", "of", "on", "or", "please", "set", "the",
    "this", "to", "up", "want", "what", "what's", "with", "you", "your", ",",
];

pub struct Index {
    docs: Vec<(String, BTreeMap<String, usize>, usize)>,
    df: BTreeMap<String, usize>,
    avg_len: f64,
}

impl Index {
    /// `docs`: (recipe id, words describing it).
    pub fn new(docs: Vec<(String, Vec<String>)>) -> Index {
        let mut df: BTreeMap<String, usize> = BTreeMap::new();
        let docs: Vec<_> = docs
            .into_iter()
            .map(|(id, words)| {
                let mut tf: BTreeMap<String, usize> = BTreeMap::new();
                let words: Vec<String> = words.into_iter().filter(|w| !is_stop(w)).collect();
                for w in &words {
                    *tf.entry(w.clone()).or_default() += 1;
                }
                for w in tf.keys() {
                    *df.entry(w.clone()).or_default() += 1;
                }
                (id, tf, words.len())
            })
            .collect();
        let avg_len = docs.iter().map(|d| d.2 as f64).sum::<f64>() / docs.len().max(1) as f64;
        Index { docs, df, avg_len }
    }

    /// Recipes with a positive score, best first.
    pub fn search(&self, query: &[&str]) -> Vec<(String, f64)> {
        let n = self.docs.len() as f64;
        let mut scores: Vec<(String, f64)> = self
            .docs
            .iter()
            .map(|(id, tf, len)| {
                let score = query
                    .iter()
                    .filter(|q| !is_stop(q))
                    .map(|q| {
                        let f = *tf.get(*q).unwrap_or(&0) as f64;
                        if f == 0.0 {
                            return 0.0;
                        }
                        let df = *self.df.get(*q).unwrap_or(&0) as f64;
                        let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                        idf * f * (K1 + 1.0) / (f + K1 * (1.0 - B + B * *len as f64 / self.avg_len))
                    })
                    .sum::<f64>();
                (id.clone(), score)
            })
            .filter(|(_, s)| *s > 0.0)
            .collect();
        scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scores
    }
}

fn is_stop(w: &str) -> bool {
    STOPWORDS.contains(&w)
}
