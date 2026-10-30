//! Request normalization, pattern matching and BM25 scoring over recipes.
//!
//! Matching is deterministic and conservative: a plan comes only from a full
//! pattern match whose slots all resolve to one system value. Everything else
//! — ties, search suggestions, ambiguous or unknown slot values — becomes a
//! question for the user, never a guess.

mod bm25;
pub mod normalize;

use std::collections::{BTreeMap, BTreeSet};

use sancho_core::{Library, Recipe, SlotSpec, System, ValueType};
use sancho_profile::Profile;
use sancho_resolve::{resolve, Candidate, Resolution, Value};

use normalize::{correct, normalize, Request};

/// Which recipe a request is about, before slots are resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// A pattern matched. `mentions` are the user's words for each slot.
    Recipe {
        id: String,
        mentions: BTreeMap<String, String>,
        /// Typos fixed to make the pattern match: (typed, read as).
        corrections: Vec<(String, String)>,
    },
    /// The user must pick a recipe. `suggested` means these came from search,
    /// not from a matching pattern.
    Choose {
        options: Vec<String>,
        suggested: bool,
    },
    /// Nothing fits; the reason is shown to the user.
    Unsupported { reason: String },
}

pub struct Matcher<'a> {
    lib: &'a Library,
    vocab: BTreeSet<String>,
    index: bm25::Index,
}

/// Search suggestions scoring below this fraction of the best are dropped.
const SUGGEST_RATIO: f64 = 0.5;
const MAX_SUGGESTIONS: usize = 3;

impl<'a> Matcher<'a> {
    pub fn new(lib: &'a Library) -> Matcher<'a> {
        let mut vocab = BTreeSet::new();
        let mut docs = Vec::new();
        for (id, r) in &lib.recipes {
            let mut words: Vec<String> = r
                .patterns
                .iter()
                .flat_map(|p| p.words())
                .map(String::from)
                .collect();
            vocab.extend(words.iter().cloned());
            words.extend(text_words(&r.summary));
            words.extend(r.domains.iter().cloned());
            docs.push((id.clone(), words));
        }
        vocab.extend(
            lib.synonyms
                .values()
                .flat_map(|v| v.split_whitespace().map(String::from)),
        );
        Matcher {
            lib,
            vocab,
            index: bm25::Index::new(docs),
        }
    }

    pub fn match_request(&self, text: &str, profile: &Profile) -> Intent {
        let req = normalize(text, &self.lib.synonyms);
        if req.tokens.is_empty() {
            return Intent::Unsupported {
                reason: "the request is empty".into(),
            };
        }
        let mut corrections = Vec::new();
        let mut matched = self.pattern_matches(&req);
        let mut query = req.clone();
        if matched.is_empty() {
            let (fixed, fixes) = correct(&req, &self.vocab);
            matched = self.pattern_matches(&fixed);
            corrections = fixes;
            query = fixed;
        }

        let available = |id: &str| profile.unmet(&self.lib.recipes[id].requires).is_empty();
        // Only the most specific matches count: a weaker match must not stand
        // in for a better one that this system can't run.
        let best_any = matched.iter().map(|(_, m)| m.literals).max();
        let (eligible, excluded): (Vec<_>, Vec<_>) = matched
            .into_iter()
            .filter(|(_, m)| Some(m.literals) == best_any)
            .partition(|(id, _)| available(id));

        if let Some(best) = eligible.iter().map(|(_, m)| m.literals).max() {
            let top: Vec<_> = eligible
                .into_iter()
                .filter(|(_, m)| m.literals == best)
                .collect();
            if let [(id, m)] = top.as_slice() {
                let mentions = m
                    .captures
                    .iter()
                    .map(|(slot, s, e)| (slot.clone(), req.original_text(*s, *e)))
                    .collect();
                return Intent::Recipe {
                    id: id.clone(),
                    mentions,
                    corrections,
                };
            }
            return Intent::Choose {
                options: top.into_iter().map(|(id, _)| id).collect(),
                suggested: false,
            };
        }
        if let Some((id, _)) = excluded.first() {
            let r = &self.lib.recipes[id];
            let why: Vec<String> = profile
                .unmet(&r.requires)
                .iter()
                .map(ToString::to_string)
                .collect();
            return Intent::Unsupported {
                reason: format!("\"{}\" {} on this system", r.summary, why.join("; ")),
            };
        }

        let hits = self.index.search(&query.token_refs());
        let top = hits.first().map_or(0.0, |h| h.1);
        let options: Vec<String> = hits
            .into_iter()
            .filter(|(id, s)| *s >= top * SUGGEST_RATIO && available(id))
            .take(MAX_SUGGESTIONS)
            .map(|(id, _)| id)
            .collect();
        if options.is_empty() {
            Intent::Unsupported {
                reason: "no recipe knows how to do that yet".into(),
            }
        } else {
            Intent::Choose {
                options,
                suggested: true,
            }
        }
    }

    /// The user's words for each slot of `recipe_id`, if one of its patterns
    /// matches the request (e.g. after the user picked it from a tie);
    /// otherwise none, and slots are resolved without a mention.
    pub fn mentions_for(
        &self,
        text: &str,
        recipe_id: &str,
    ) -> (BTreeMap<String, String>, Vec<(String, String)>) {
        let req = normalize(text, &self.lib.synonyms);
        let mut found = self.pattern_matches(&req);
        let mut corrections = Vec::new();
        if !found.iter().any(|(id, _)| id == recipe_id) {
            let (fixed, fixes) = correct(&req, &self.vocab);
            found = self.pattern_matches(&fixed);
            corrections = fixes;
        }
        match found.into_iter().find(|(id, _)| id == recipe_id) {
            Some((_, m)) => (
                m.captures
                    .iter()
                    .map(|(slot, s, e)| (slot.clone(), req.original_text(*s, *e)))
                    .collect(),
                corrections,
            ),
            None => (BTreeMap::new(), Vec::new()),
        }
    }

    /// The request as matching sees it (normalized words), for remembering
    /// answers to questions about it.
    pub fn phrase(&self, text: &str) -> String {
        normalize(text, &self.lib.synonyms).tokens.join(" ")
    }

    pub fn library(&self) -> &Library {
        self.lib
    }

    /// Best full match per recipe: (recipe id, match).
    fn pattern_matches(&self, req: &Request) -> Vec<(String, sancho_core::pattern::PatternMatch)> {
        let tokens = req.token_refs();
        self.lib
            .recipes
            .iter()
            .filter_map(|(id, r)| {
                r.patterns
                    .iter()
                    .filter_map(|p| p.match_tokens(&tokens).into_iter().next())
                    .max_by_key(|m| m.literals)
                    .map(|m| (id.clone(), m))
            })
            .collect()
    }
}

/// Lowercase words of free text (summaries), for search.
fn text_words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// The result of matching and resolving a request.
#[derive(Debug, Clone, PartialEq)]
pub enum Interpretation {
    /// Recipe chosen and every slot resolved to system values.
    Ready {
        recipe: String,
        values: BTreeMap<String, Vec<Value>>,
        corrections: Vec<(String, String)>,
    },
    /// Ask which recipe was meant.
    ChooseRecipe {
        options: Vec<String>,
        suggested: bool,
    },
    /// Ask which value a slot should have.
    ChooseValue {
        recipe: String,
        slot: String,
        candidates: Vec<Candidate>,
    },
    /// The recipe fits but a slot value was not found; tell the user why.
    NotFound {
        recipe: String,
        slot: String,
        reason: String,
    },
    Unsupported {
        reason: String,
    },
}

/// Matches `text` and resolves the chosen recipe's slots against `sys`.
pub fn interpret(
    matcher: &Matcher,
    profile: &Profile,
    sys: &dyn System,
    text: &str,
) -> Interpretation {
    interpret_with(matcher, profile, sys, text, &Answers::default())
}

/// The user's answers to earlier questions about the same request.
#[derive(Debug, Clone, Default)]
pub struct Answers {
    /// The recipe the user picked.
    pub recipe: Option<String>,
    /// Values the user picked, by slot; these slots are not resolved again.
    pub values: BTreeMap<String, Vec<Value>>,
}

/// Like [`interpret`], taking the user's answers to earlier questions into account.
pub fn interpret_with(
    matcher: &Matcher,
    profile: &Profile,
    sys: &dyn System,
    text: &str,
    answers: &Answers,
) -> Interpretation {
    let (id, mentions, corrections) = match &answers.recipe {
        Some(id) => {
            let (mentions, corrections) = matcher.mentions_for(text, id);
            (id.clone(), mentions, corrections)
        }
        None => match matcher.match_request(text, profile) {
            Intent::Recipe {
                id,
                mentions,
                corrections,
            } => (id, mentions, corrections),
            Intent::Choose { options, suggested } => {
                return Interpretation::ChooseRecipe { options, suggested }
            }
            Intent::Unsupported { reason } => return Interpretation::Unsupported { reason },
        },
    };
    let Some(recipe) = matcher.lib.recipes.get(&id) else {
        return Interpretation::Unsupported {
            reason: format!("unknown recipe '{id}'"),
        };
    };
    let recipe: &Recipe = recipe;
    let mut values = BTreeMap::new();
    for (slot_name, slot) in &recipe.slots {
        if let Some(chosen) = answers.values.get(slot_name) {
            values.insert(slot_name.clone(), chosen.clone());
            continue;
        }
        let mention = mentions.get(slot_name).map(String::as_str);
        let not_found = |reason: String| Interpretation::NotFound {
            recipe: id.clone(),
            slot: slot_name.clone(),
            reason,
        };
        if slot.ty == ValueType::Choice {
            match mention {
                Some(m) if slot.values.iter().any(|v| v == m) => {
                    values.insert(slot_name.clone(), vec![choice_value(m)]);
                    continue;
                }
                Some(m) => {
                    return not_found(format!("\"{m}\" is not one of: {}", slot.values.join(", ")))
                }
                None => {
                    return Interpretation::ChooseValue {
                        recipe: id.clone(),
                        slot: slot_name.clone(),
                        candidates: slot
                            .values
                            .iter()
                            .enumerate()
                            .map(|(i, v)| Candidate {
                                id: format!("c{}", i + 1),
                                value: choice_value(v),
                            })
                            .collect(),
                    };
                }
            }
        }
        let parts: Vec<Option<String>> = match (slot.multiple, mention) {
            (true, Some(m)) => split_list(m).into_iter().map(Some).collect(),
            (_, m) => vec![m.map(String::from)],
        };
        let mut resolved = Vec::new();
        for part in parts {
            match resolve_item(sys, slot, part.as_deref()) {
                Ok(Resolution::Selected(v)) => resolved.push(v),
                Ok(Resolution::Ask(candidates)) => {
                    return Interpretation::ChooseValue {
                        recipe: id.clone(),
                        slot: slot_name.clone(),
                        candidates,
                    }
                }
                Ok(Resolution::NoMatch(reason)) => {
                    // "vim git htop": several exact names in one item.
                    match part
                        .as_deref()
                        .filter(|_| slot.multiple)
                        .and_then(|p| exact_names(sys, slot, p))
                    {
                        Some(vs) => resolved.extend(vs),
                        None => return not_found(reason),
                    }
                }
                Err(e) => return not_found(e.to_string()),
            }
        }
        resolved.dedup_by(|a, b| a.key == b.key);
        values.insert(slot_name.clone(), resolved);
    }
    Interpretation::Ready {
        recipe: id,
        values,
        corrections,
    }
}

fn resolve_item(
    sys: &dyn System,
    slot: &SlotSpec,
    item: Option<&str>,
) -> Result<Resolution, sancho_resolve::ResolveError> {
    resolve(sys, slot, item)
}

/// Each word of `item` resolved as an exact name, if every one is.
fn exact_names(sys: &dyn System, slot: &SlotSpec, item: &str) -> Option<Vec<Value>> {
    let words: Vec<&str> = item.split_whitespace().collect();
    if words.len() < 2 {
        return None;
    }
    words
        .iter()
        .map(|w| match resolve(sys, slot, Some(w)) {
            Ok(Resolution::Selected(v)) if v.key == *w => Some(v),
            _ => None,
        })
        .collect()
}

/// "vlc, obs-studio and gimp" → ["vlc", "obs-studio", "gimp"].
fn split_list(mention: &str) -> Vec<String> {
    mention
        .split(',')
        .flat_map(|chunk| chunk.split(" and "))
        .map(|s| s.trim().trim_start_matches("and ").trim().to_string())
        .filter(|s| !s.is_empty() && s != "and")
        .collect()
}

fn choice_value(v: &str) -> Value {
    Value {
        key: v.to_string(),
        display: v.to_string(),
        attrs: BTreeMap::new(),
        tags: BTreeSet::new(),
    }
}
