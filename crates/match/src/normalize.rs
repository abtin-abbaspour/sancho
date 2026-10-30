//! Request normalization: tokens, synonyms, and typo correction against the
//! library's own vocabulary.

use std::collections::{BTreeMap, BTreeSet};

/// A normalized request. `tokens` drive matching; `words` are what the user
/// typed (lowercased, trimmed) and `source[i]` is the word token `i` came
/// from, so slot values are taken from the user's words, never "corrected".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub tokens: Vec<String>,
    pub words: Vec<String>,
    pub source: Vec<usize>,
}

impl Request {
    pub fn token_refs(&self) -> Vec<&str> {
        self.tokens.iter().map(String::as_str).collect()
    }

    /// The user's words behind tokens `[start, end)`.
    pub fn original_text(&self, start: usize, end: usize) -> String {
        if start >= end {
            return String::new();
        }
        self.words[self.source[start]..=self.source[end - 1]].join(" ")
    }
}

/// Politeness words that never change what a request means.
const IGNORED: &[&str] = &["please", "pls", "plz", "kindly"];

/// Lowercases, splits on whitespace, separates commas into their own token,
/// trims surrounding punctuation, and applies synonyms.
pub fn normalize(text: &str, synonyms: &BTreeMap<String, String>) -> Request {
    let text = text
        .to_lowercase()
        .replace(['\u{2019}', '\u{2018}'], "'")
        .replace(',', " , ");
    let mut tokens = Vec::new();
    let mut words = Vec::new();
    let mut source = Vec::new();
    for raw in text.split_whitespace() {
        let word = raw.trim_matches(|c: char| "\"'.!?;:()[]".contains(c));
        // Keep a trailing apostrophe word like "don't" intact.
        let word = if raw.ends_with("n't") {
            raw.trim_end_matches(['.', '!', '?'])
        } else {
            word
        };
        if word.is_empty() || IGNORED.contains(&word) {
            continue;
        }
        let replacement = synonyms.get(word).map(String::as_str).unwrap_or(word);
        for w in replacement.split_whitespace() {
            tokens.push(w.to_string());
            source.push(words.len());
        }
        words.push(word.to_string());
    }
    Request {
        tokens,
        words,
        source,
    }
}

/// Replaces words that are not in `vocab` with the single closest vocabulary
/// word, if there is exactly one within the allowed distance. Short words and
/// words with digits or symbols (likely names) are left alone.
/// Returns the corrected request and the (from, to) pairs applied.
pub fn correct(req: &Request, vocab: &BTreeSet<String>) -> (Request, Vec<(String, String)>) {
    let mut out = req.clone();
    let mut fixes = Vec::new();
    for tok in out.tokens.iter_mut() {
        if tok.len() < 5
            || vocab.contains(tok.as_str())
            || !tok.chars().all(|c| c.is_ascii_lowercase())
        {
            continue;
        }
        let max = if tok.len() >= 8 { 2 } else { 1 };
        let mut best: Vec<(&str, usize)> = vocab
            .iter()
            .map(|v| (v.as_str(), distance(tok, v)))
            .filter(|(_, d)| *d <= max)
            .collect();
        best.sort_by_key(|(_, d)| *d);
        let unique = match best.as_slice() {
            [(w, _)] => Some(*w),
            [(w, d1), (_, d2), ..] if d1 < d2 => Some(*w),
            _ => None,
        };
        if let Some(w) = unique {
            fixes.push((tok.clone(), w.to_string()));
            *tok = w.to_string();
        }
    }
    (out, fixes)
}

/// Optimal string alignment distance (Damerau-Levenshtein with adjacent swaps).
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Vec<String> {
        normalize(s, &BTreeMap::from([("wi-fi".into(), "wifi".into())])).tokens
    }

    #[test]
    fn tokens_are_lowercased_trimmed_and_mapped() {
        assert_eq!(
            n("Restart my Wi-Fi, please!"),
            ["restart", "my", "wifi", ","]
        );
        assert_eq!(
            n("install lib32-mesa sshd.service gtk+"),
            ["install", "lib32-mesa", "sshd.service", "gtk+"]
        );
        assert_eq!(n("don't start it."), ["don't", "start", "it"]);
        assert_eq!(n("“quoted”  'words'"), ["“quoted”", "words"]);
    }

    #[test]
    fn typos_are_fixed_only_when_unambiguous() {
        let vocab: BTreeSet<String> = ["restart", "install", "update", "upgrade", "remove"]
            .into_iter()
            .map(String::from)
            .collect();
        let req = normalize("instal firefox", &BTreeMap::new());
        let (fixed, fixes) = correct(&normalize("restrat sshd", &BTreeMap::new()), &vocab);
        assert_eq!(fixed.tokens, ["restart", "sshd"]);
        assert_eq!(fixed.words, ["restrat", "sshd"]);
        assert_eq!(fixes, [("restrat".into(), "restart".into())]);
        // "instal" is 6 letters, distance 1 from install: fixed. "firefox" is
        // a name: untouched (no vocabulary word near it).
        assert_eq!(correct(&req, &vocab).0.tokens, ["install", "firefox"]);
        // "updrade" is 1 edit from "upgrade" but 2 from "update": fixed.
        assert_eq!(
            correct(&normalize("updrade", &BTreeMap::new()), &vocab)
                .0
                .tokens,
            ["upgrade"]
        );
        // "remobe" is 1 edit from both "remove" and "remote": left alone.
        let mut both = vocab.clone();
        both.insert("remote".into());
        assert_eq!(
            correct(&normalize("remobe", &BTreeMap::new()), &both)
                .0
                .tokens,
            ["remobe"]
        );
        // Short words and names with digits are never touched.
        assert_eq!(
            correct(&normalize("vim lib32", &BTreeMap::new()), &vocab)
                .0
                .tokens,
            ["vim", "lib32"]
        );
    }

    #[test]
    fn distance_counts_swaps_as_one() {
        assert_eq!(distance("restrat", "restart"), 1);
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance("", "abc"), 3);
    }
}
