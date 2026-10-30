//! Slot resolvers: enumerate candidates from the real system and validate them.
//!
//! Every value comes from the system (via [`System`]) and is validated for its
//! type before it can become a candidate; entries that fail are dropped. The
//! caller (matcher, later a model) can only pick a candidate by its opaque ID
//! or ask the user — it never supplies a value of its own.

mod accounts;
mod browser;
mod hid;
mod net;
mod packages;
mod units;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use sancho_core::recipe::{FilterValue, SlotSpec};
use sancho_core::value::FilterKind;
use sancho_core::{Query, System, ValueType};

/// A system object a slot can refer to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    /// What fills a scalar argument (unit name, package, ...). For compound
    /// types it is a stable identity (`vid:pid`, desktop file ID).
    pub key: String,
    /// Shown to the user in plans and questions.
    pub display: String,
    /// Template attributes (`{device.vid}`), for compound types.
    pub attrs: BTreeMap<&'static str, String>,
    /// Facts filters and mentions select on (`keyboard`, `installed`, ...).
    pub tags: BTreeSet<&'static str>,
}

impl Value {
    fn new(key: impl Into<String>, display: impl Into<String>) -> Value {
        Value {
            key: key.into(),
            display: display.into(),
            attrs: BTreeMap::new(),
            tags: BTreeSet::new(),
        }
    }

    fn tag(mut self, tag: &'static str, on: bool) -> Value {
        if on {
            self.tags.insert(tag);
        }
        self
    }
}

/// A value offered for selection, under an opaque per-resolution ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Selected(Value),
    /// Several fit; the user must choose.
    Ask(Vec<Candidate>),
    /// Nothing fits (or too many to list); the reason is shown to the user.
    NoMatch(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// The system query a resolver depends on failed.
    Unavailable(Query),
    /// The type is not resolved from the system (literals, choices).
    NotResolvable(ValueType),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::Unavailable(q) => write!(f, "could not run `{}`", q.argv().join(" ")),
            ResolveError::NotResolvable(t) => write!(f, "{t} values are not read from the system"),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Questions list at most this many candidates; beyond that, ask for detail.
pub const MAX_CANDIDATES: usize = 10;

/// Every value of `ty` on the system, sorted by display name.
pub fn enumerate(sys: &dyn System, ty: ValueType) -> Result<Vec<Value>, ResolveError> {
    let mut values = match ty {
        ValueType::HidDevice => hid::enumerate(sys)?,
        ValueType::SystemdUnit => units::enumerate(sys)?,
        ValueType::Package => packages::enumerate(sys)?,
        ValueType::NetIface => net::enumerate(sys),
        ValueType::Browser => browser::enumerate(sys),
        ValueType::UserGroup => accounts::groups(sys),
        ValueType::User => accounts::users(sys),
        t => return Err(ResolveError::NotResolvable(t)),
    };
    values.sort_by(|a, b| a.display.cmp(&b.display).then(a.key.cmp(&b.key)));
    Ok(values)
}

/// Re-checks that a previously chosen key still exists (e.g. at plan time).
pub fn validate(sys: &dyn System, ty: ValueType, key: &str) -> Result<Option<Value>, ResolveError> {
    Ok(enumerate(sys, ty)?.into_iter().find(|v| v.key == key))
}

/// Resolves a recipe slot from what the user said (`mention`), if anything.
///
/// 1. Values are filtered by the slot's declared filters.
/// 2. An exact key match on the mention selects that value.
/// 3. Otherwise class words in the mention ("keyboard", "wifi") act as tag
///    filters, and remaining words must all appear in the name.
/// 4. One survivor is selected; several are offered; none is reported.
pub fn resolve(
    sys: &dyn System,
    slot: &SlotSpec,
    mention: Option<&str>,
) -> Result<Resolution, ResolveError> {
    let ty = slot.ty;
    let values: Vec<Value> = enumerate(sys, ty)?
        .into_iter()
        .filter(|v| passes_filters(v, ty, &slot.filter))
        .collect();
    let what = type_noun(ty);

    let Some(mention) = mention.map(str::trim).filter(|m| !m.is_empty()) else {
        let defaults = default_tags(ty);
        let values: Vec<Value> = values
            .into_iter()
            .filter(|v| defaults.iter().all(|t| v.tags.contains(t)))
            .collect();
        return Ok(decide(values, &format!("no {what} found")));
    };

    let lower = mention.to_lowercase();
    for key in exact_keys(ty, &lower) {
        if let Some(v) = values.iter().find(|v| v.key.to_lowercase() == key) {
            return Ok(Resolution::Selected(v.clone()));
        }
    }

    let mut tags = Vec::new();
    let mut words = Vec::new();
    for word in lower
        .split_whitespace()
        .filter(|w| !FILLER.contains(w) && !generic_noun(ty, w))
    {
        match class_word(ty, word) {
            Some(tag) => tags.push(tag),
            None => words.push(word),
        }
    }
    let matching: Vec<Value> = values
        .into_iter()
        .filter(|v| tags.iter().all(|t| v.tags.contains(t)))
        .filter(|v| {
            let hay = format!("{} {}", v.display, v.key).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .collect();
    Ok(decide(
        matching,
        &format!("no {what} matches \"{mention}\""),
    ))
}

fn decide(values: Vec<Value>, none: &str) -> Resolution {
    match values.len() {
        0 => Resolution::NoMatch(none.to_string()),
        1 => Resolution::Selected(values.into_iter().next().unwrap()),
        n if n > MAX_CANDIDATES => {
            Resolution::NoMatch(format!("{n} matches; please be more specific"))
        }
        _ => Resolution::Ask(
            values
                .into_iter()
                .enumerate()
                .map(|(i, value)| Candidate {
                    id: format!("c{}", i + 1),
                    value,
                })
                .collect(),
        ),
    }
}

fn passes_filters(v: &Value, ty: ValueType, filter: &BTreeMap<String, FilterValue>) -> bool {
    filter.iter().all(|(key, want)| {
        let kind = ty.filters().iter().find(|(k, _)| k == key).map(|(_, k)| *k);
        match (kind, want) {
            (Some(FilterKind::Bool), FilterValue::Bool(b)) => v.tags.contains(key.as_str()) == *b,
            (Some(FilterKind::OneOf(_)), FilterValue::String(s)) => v.tags.contains(s.as_str()),
            // Rejected at recipe load time; never match if it slips through.
            _ => false,
        }
    })
}

/// Words that carry no meaning for selection ("my keyboard", "the mouse").
const FILLER: &[&str] = &["my", "the", "a", "an", "this", "that"];

/// Nouns that name the type itself ("my browser", "the sshd service") and so
/// say nothing about which value is meant.
fn generic_noun(ty: ValueType, word: &str) -> bool {
    let nouns: &[&str] = match ty {
        ValueType::HidDevice => &["device", "devices"],
        ValueType::SystemdUnit => &["service", "services", "daemon", "unit"],
        ValueType::Package => &["package", "packages", "program", "app"],
        ValueType::NetIface => &["interface", "adapter", "connection", "network"],
        ValueType::Browser => &["browser", "web"],
        ValueType::UserGroup => &["group"],
        ValueType::User => &["user", "account"],
        _ => &[],
    };
    nouns.contains(&word)
}

/// Keys a mention can name exactly, following each tool's own conventions.
fn exact_keys(ty: ValueType, mention: &str) -> Vec<String> {
    match ty {
        // systemctl assumes `.service` when no unit suffix is given.
        ValueType::SystemdUnit if !mention.contains('.') => {
            vec![mention.to_string(), format!("{mention}.service")]
        }
        _ => vec![mention.to_string()],
    }
}

/// Words that name a class of value rather than a specific one.
fn class_word(ty: ValueType, word: &str) -> Option<&'static str> {
    match ty {
        ValueType::HidDevice => match word {
            "keyboard" | "keyboards" => Some("keyboard"),
            "mouse" | "mice" => Some("mouse"),
            "touchpad" | "trackpad" => Some("touchpad"),
            "joystick" | "gamepad" | "controller" => Some("joystick"),
            "tablet" => Some("tablet"),
            _ => None,
        },
        ValueType::NetIface => match word {
            "wifi" | "wi-fi" | "wireless" | "wlan" => Some("wireless"),
            "ethernet" | "wired" | "lan" => Some("wired"),
            _ => None,
        },
        ValueType::User => match word {
            "me" | "myself" | "i" => Some("current"),
            _ => None,
        },
        _ => None,
    }
}

/// Tags assumed when the user names nothing ("add me to ..." → current user).
fn default_tags(ty: ValueType) -> &'static [&'static str] {
    match ty {
        ValueType::User => &["current"],
        _ => &[],
    }
}

fn type_noun(ty: ValueType) -> &'static str {
    match ty {
        ValueType::HidDevice => "device",
        ValueType::SystemdUnit => "service",
        ValueType::Package => "package",
        ValueType::NetIface => "network interface",
        ValueType::Browser => "browser",
        ValueType::UserGroup => "group",
        ValueType::User => "user",
        _ => "value",
    }
}
