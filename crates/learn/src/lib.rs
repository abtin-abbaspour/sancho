//! Local learned store (aliases, slot preferences) and scrubbed export.
//!
//! Learned items only ever pre-answer a question sancho would otherwise ask:
//! an alias picks a recipe only from the options the matcher offers, and a
//! preference picks a value only from the live candidates. They are always
//! labeled in plans, never skip confirmation, and can be listed and removed.

pub mod scrub;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Learned {
    /// When the request reads `phrase` (normalized), it means `recipe`.
    Alias { phrase: String, recipe: String },
    /// For `recipe`'s `slot`, the words `mention` mean the value `key`.
    Preference {
        recipe: String,
        slot: String,
        mention: String,
        key: String,
        /// What the user saw when choosing.
        display: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: u32,
    #[serde(flatten)]
    pub learned: Learned,
    /// Times it answered a question (local only; never sent anywhere).
    pub uses: u32,
}

impl Item {
    pub fn describe(&self) -> String {
        match &self.learned {
            Learned::Alias { phrase, recipe } => format!("\"{phrase}\" means {recipe}"),
            Learned::Preference {
                recipe,
                slot,
                mention,
                display,
                ..
            } => {
                let words = if mention.is_empty() {
                    "no mention".to_string()
                } else {
                    format!("\"{mention}\"")
                };
                format!("for {recipe}, {slot} with {words} means {display}")
            }
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    next_id: u32,
    items: Vec<Item>,
}

pub struct Store {
    path: PathBuf,
    file: File,
}

impl Store {
    /// `$XDG_DATA_HOME/sancho/learned.json`, defaulting to `~/.local/share`.
    pub fn default_path() -> Option<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share")))?;
        Some(data.join("sancho/learned.json"))
    }

    pub fn open(path: impl Into<PathBuf>) -> io::Result<Store> {
        let path = path.into();
        let file = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => File::default(),
            Err(e) => return Err(e),
        };
        Ok(Store { path, file })
    }

    pub fn items(&self) -> &[Item] {
        &self.file.items
    }

    pub fn get(&self, id: u32) -> Option<&Item> {
        self.file.items.iter().find(|i| i.id == id)
    }

    /// The alias for this normalized request, if any.
    pub fn alias(&self, phrase: &str) -> Option<&Item> {
        self.file
            .items
            .iter()
            .find(|i| matches!(&i.learned, Learned::Alias { phrase: p, .. } if p == phrase))
    }

    /// The preferred value for a slot given these words, if any.
    pub fn preference(&self, recipe: &str, slot: &str, mention: &str) -> Option<&Item> {
        self.file.items.iter().find(|i| {
            matches!(&i.learned, Learned::Preference { recipe: r, slot: s, mention: m, .. }
                if r == recipe && s == slot && m == mention)
        })
    }

    /// Adds (or replaces the matching) item; returns its ID.
    pub fn learn(&mut self, learned: Learned) -> io::Result<u32> {
        let same = |i: &Item| match (&i.learned, &learned) {
            (Learned::Alias { phrase: a, .. }, Learned::Alias { phrase: b, .. }) => a == b,
            (
                Learned::Preference {
                    recipe: r1,
                    slot: s1,
                    mention: m1,
                    ..
                },
                Learned::Preference {
                    recipe: r2,
                    slot: s2,
                    mention: m2,
                    ..
                },
            ) => r1 == r2 && s1 == s2 && m1 == m2,
            _ => false,
        };
        self.file.items.retain(|i| !same(i));
        self.file.next_id += 1;
        let id = self.file.next_id;
        self.file.items.push(Item {
            id,
            learned,
            uses: 0,
        });
        self.save()?;
        Ok(id)
    }

    pub fn used(&mut self, id: u32) -> io::Result<()> {
        if let Some(i) = self.file.items.iter_mut().find(|i| i.id == id) {
            i.uses += 1;
        }
        self.save()
    }

    /// Removes an item; returns whether it existed.
    pub fn remove(&mut self, id: u32) -> io::Result<bool> {
        let before = self.file.items.len();
        self.file.items.retain(|i| i.id != id);
        let removed = self.file.items.len() != before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(&self.file).map_err(io::Error::other)?;
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &self.path)
    }
}

/// The text written by `sancho learned export`, before scrubbing.
pub fn export_text(item: &Item, sancho_version: &str) -> String {
    let body = match &item.learned {
        Learned::Alias { phrase, recipe } => format!(
            "kind = \"alias\"\nphrase = {}\nrecipe = {}\n",
            quote(phrase),
            quote(recipe)
        ),
        Learned::Preference {
            recipe,
            slot,
            mention,
            key,
            display,
        } => format!(
            "kind = \"preference\"\nrecipe = {}\nslot = {}\nmention = {}\nvalue = {}\ndisplay = {}\n",
            quote(recipe),
            quote(slot),
            quote(mention),
            quote(key),
            quote(display)
        ),
    };
    format!(
        "# Learned by sancho {sancho_version}, exported for sharing.\n\
         # Contributed under MIT OR Apache-2.0, the license of sancho's recipe data.\n\
         # Personal details were replaced with placeholders such as <user>.\n\
         {body}"
    )
}

/// A TOML basic string.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
