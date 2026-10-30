//! Request patterns in recipes: `"let {browser} (configure|access) (my )?{device}"`.
//!
//! Grammar, over whitespace-separated tokens:
//! - a word matches that exact (normalized) token;
//! - `{slot}` captures one or more tokens;
//! - `(a b|c)` matches one alternative; `(...)?` may also match nothing;
//! - a final `...` matches any remaining words (answer recipes only, where
//!   questions often trail off: "is my wifi card intel or realtek").

use std::fmt;

use serde::{Deserialize, Deserializer};

use crate::template::is_ident;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Elem {
    Word(String),
    Slot(String),
    Group {
        alts: Vec<Vec<Elem>>,
        optional: bool,
    },
    /// `...`: any remaining words, including none.
    Rest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub source: String,
    pub elems: Vec<Elem>,
}

/// One way a pattern matched a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternMatch {
    /// Slot name → captured token range `[start, end)` in the request.
    pub captures: Vec<(String, usize, usize)>,
    /// Literal words matched; more means a more specific match.
    pub literals: usize,
}

impl Pattern {
    pub fn parse(source: &str) -> Result<Pattern, String> {
        let chars: Vec<char> = source.chars().collect();
        let mut pos = 0;
        let elems = parse_seq(&chars, &mut pos, 0)?;
        if pos < chars.len() {
            return Err(format!("unexpected '{}' at offset {pos}", chars[pos]));
        }
        if elems.is_empty() {
            return Err("pattern is empty".into());
        }
        Ok(Pattern {
            source: source.to_string(),
            elems,
        })
    }

    /// Slot names in order of appearance (with repeats).
    pub fn slots(&self) -> Vec<&str> {
        fn walk<'a>(elems: &'a [Elem], out: &mut Vec<&'a str>) {
            for e in elems {
                match e {
                    Elem::Slot(s) => out.push(s),
                    Elem::Group { alts, .. } => alts.iter().for_each(|a| walk(a, out)),
                    Elem::Word(_) | Elem::Rest => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.elems, &mut out);
        out
    }

    /// Every literal word the pattern can match.
    pub fn words(&self) -> Vec<&str> {
        fn walk<'a>(elems: &'a [Elem], out: &mut Vec<&'a str>) {
            for e in elems {
                match e {
                    Elem::Word(w) => out.push(w),
                    Elem::Group { alts, .. } => alts.iter().for_each(|a| walk(a, out)),
                    Elem::Slot(_) | Elem::Rest => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.elems, &mut out);
        out
    }

    /// Ends with `...` (matches trailing words).
    pub fn has_rest(&self) -> bool {
        self.elems.last() == Some(&Elem::Rest)
    }

    /// All ways the whole token list matches, most specific first.
    pub fn match_tokens(&self, tokens: &[&str]) -> Vec<PatternMatch> {
        let mut out: Vec<PatternMatch> = match_seq(&self.elems, tokens, 0)
            .into_iter()
            .filter(|(end, _)| *end == tokens.len())
            .map(|(_, m)| m)
            .collect();
        // Most literals first; then shorter earlier captures, for determinism.
        out.sort_by(|a, b| {
            b.literals.cmp(&a.literals).then_with(|| {
                let spans =
                    |m: &PatternMatch| m.captures.iter().map(|c| c.2 - c.1).collect::<Vec<_>>();
                spans(a).cmp(&spans(b))
            })
        });
        out.dedup();
        out
    }
}

/// Every (end position, match) reachable by matching `elems` from `pos`.
fn match_seq(elems: &[Elem], tokens: &[&str], pos: usize) -> Vec<(usize, PatternMatch)> {
    let Some((first, rest)) = elems.split_first() else {
        return vec![(
            pos,
            PatternMatch {
                captures: vec![],
                literals: 0,
            },
        )];
    };
    let mut heads: Vec<(usize, PatternMatch)> = Vec::new();
    match first {
        Elem::Word(w) => {
            if tokens.get(pos) == Some(&w.as_str()) {
                heads.push((
                    pos + 1,
                    PatternMatch {
                        captures: vec![],
                        literals: 1,
                    },
                ));
            }
        }
        Elem::Slot(name) => {
            for end in pos + 1..=tokens.len() {
                heads.push((
                    end,
                    PatternMatch {
                        captures: vec![(name.clone(), pos, end)],
                        literals: 0,
                    },
                ));
            }
        }
        Elem::Rest => heads.push((
            tokens.len().max(pos),
            PatternMatch {
                captures: vec![],
                literals: 0,
            },
        )),
        Elem::Group { alts, optional } => {
            for alt in alts {
                heads.extend(match_seq(alt, tokens, pos));
            }
            if *optional {
                heads.push((
                    pos,
                    PatternMatch {
                        captures: vec![],
                        literals: 0,
                    },
                ));
            }
        }
    }
    let mut out = Vec::new();
    for (mid, head) in heads {
        for (end, tail) in match_seq(rest, tokens, mid) {
            let mut captures = head.captures.clone();
            captures.extend(tail.captures);
            out.push((
                end,
                PatternMatch {
                    captures,
                    literals: head.literals + tail.literals,
                },
            ));
        }
    }
    out
}

fn parse_seq(chars: &[char], pos: &mut usize, depth: usize) -> Result<Vec<Elem>, String> {
    let mut elems = Vec::new();
    while *pos < chars.len() {
        match chars[*pos] {
            c if c.is_whitespace() => *pos += 1,
            ')' | '|' if depth > 0 => break,
            '.' if chars[*pos..].starts_with(&['.', '.', '.']) => {
                *pos += 3;
                if depth > 0 || chars[*pos..].iter().any(|c| !c.is_whitespace()) {
                    return Err("`...` may only come last, outside groups".into());
                }
                elems.push(Elem::Rest);
            }
            '(' => {
                *pos += 1;
                let mut alts = vec![parse_seq(chars, pos, depth + 1)?];
                while chars.get(*pos) == Some(&'|') {
                    *pos += 1;
                    alts.push(parse_seq(chars, pos, depth + 1)?);
                }
                if chars.get(*pos) != Some(&')') {
                    return Err("unclosed '('".into());
                }
                *pos += 1;
                if alts.iter().any(Vec::is_empty) {
                    return Err("empty alternative in group (use '(...)?' for optional)".into());
                }
                let optional = chars.get(*pos) == Some(&'?');
                if optional {
                    *pos += 1;
                }
                elems.push(Elem::Group { alts, optional });
            }
            '{' => {
                let start = *pos + 1;
                let end = chars[start..]
                    .iter()
                    .position(|&c| c == '}')
                    .map(|i| start + i)
                    .ok_or("unclosed '{'")?;
                let name: String = chars[start..end].iter().collect();
                if !is_ident(&name) {
                    return Err(format!("invalid slot name '{{{name}}}'"));
                }
                *pos = end + 1;
                elems.push(Elem::Slot(name));
            }
            _ => {
                let start = *pos;
                while *pos < chars.len()
                    && !chars[*pos].is_whitespace()
                    && !"(){}|?".contains(chars[*pos])
                {
                    *pos += 1;
                }
                let word: String = chars[start..*pos].iter().collect();
                if word.is_empty() {
                    return Err(format!("unexpected '{}' at offset {start}", chars[start]));
                }
                if !word
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "'-".contains(c))
                {
                    return Err(format!(
                        "word '{word}' must be lowercase letters, digits, ' or -"
                    ));
                }
                elems.push(Elem::Word(word));
            }
        }
    }
    Ok(elems)
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl<'de> Deserialize<'de> for Pattern {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Pattern::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, req: &str) -> Vec<PatternMatch> {
        let tokens: Vec<&str> = req.split_whitespace().collect();
        Pattern::parse(p).unwrap().match_tokens(&tokens)
    }

    fn caps(p: &str, req: &str) -> Option<Vec<(String, String)>> {
        let tokens: Vec<&str> = req.split_whitespace().collect();
        let best = Pattern::parse(p)
            .unwrap()
            .match_tokens(&tokens)
            .into_iter()
            .next()?;
        Some(
            best.captures
                .into_iter()
                .map(|(n, s, e)| (n, tokens[s..e].join(" ")))
                .collect(),
        )
    }

    #[test]
    fn literals_groups_and_slots() {
        let p = "let {browser} (configure|access) (my )?{device}";
        assert_eq!(
            caps(p, "let brave access my keyboard").unwrap(),
            [
                ("browser".into(), "brave".into()),
                ("device".into(), "keyboard".into())
            ]
        );
        assert_eq!(
            caps(p, "let google chrome configure keychron q2").unwrap(),
            [
                ("browser".into(), "google chrome".into()),
                ("device".into(), "keychron q2".into())
            ]
        );
        assert!(caps(p, "let brave use my keyboard").is_none());
        assert!(caps(p, "let brave access").is_none(), "slots need a token");
    }

    #[test]
    fn optional_words_are_preferred_over_capturing_them() {
        let p = "add {user} to (the )?{group}( group)?";
        assert_eq!(
            caps(p, "add me to the video group").unwrap(),
            [
                ("user".into(), "me".into()),
                ("group".into(), "video".into())
            ]
        );
        assert_eq!(m(p, "add me to the video group")[0].literals, 4);
    }

    #[test]
    fn whole_request_must_match() {
        assert!(caps("update (my )?system", "update my system please").is_none());
        assert!(caps("update (my )?system", "update system").is_some());
    }

    #[test]
    fn parse_errors() {
        for (bad, why) in [
            ("(my", "unclosed '('"),
            ("{dev", "unclosed '{'"),
            ("{Dev}", "invalid slot name"),
            ("a () b", "empty alternative"),
            ("(a|)", "empty alternative"),
            ("Update", "must be lowercase"),
            ("", "empty"),
            ("a ) b", "unexpected ')'"),
        ] {
            let e = Pattern::parse(bad).unwrap_err();
            assert!(e.contains(why), "{bad:?}: {e}");
        }
    }

    #[test]
    fn trailing_rest_matches_any_words() {
        let p = "(is|are) my (wifi|wireless) (card|chip) ...";
        assert!(caps(p, "is my wifi card realtek or intel").is_some());
        assert!(caps(p, "is my wifi card").is_some());
        assert!(caps(p, "is my bluetooth card intel").is_none());
        assert!(Pattern::parse(p).unwrap().has_rest());
        for bad in ["... my wifi", "(wifi ...)", "a ... b"] {
            assert!(
                Pattern::parse(bad).unwrap_err().contains("only come last"),
                "{bad}"
            );
        }
    }

    #[test]
    fn words_and_slots_are_listed() {
        let p = Pattern::parse("restart (my )?(wifi|wireless) {x}").unwrap();
        assert_eq!(p.words(), ["restart", "my", "wifi", "wireless"]);
        assert_eq!(p.slots(), ["x"]);
    }
}
