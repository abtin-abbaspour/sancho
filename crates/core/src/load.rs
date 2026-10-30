//! Loading and validating the primitive and recipe library from TOML.
//!
//! Every file is parsed strictly (unknown keys are errors) and checked against the schema;
//! all problems across all files are reported, each with file, field and reason.
//! Signature verification of recipe packs (post-MVP) belongs in `read_file`.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::command_doc::CommandDoc;
use crate::primitive::Primitive;
use crate::recipe::Recipe;
use crate::validate::Problems;

/// Largest data file accepted, as a guard against accidents and hostile packs.
const MAX_FILE_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError {
    pub file: PathBuf,
    /// 1-based line and column, when the problem is a YAML/schema error.
    pub position: Option<(usize, usize)>,
    /// Dotted field path, when the problem is a semantic check.
    pub field: Option<String>,
    pub reason: String,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file.display())?;
        if let Some((line, col)) = self.position {
            write!(f, ":{line}:{col}")?;
        }
        write!(f, ": ")?;
        if let Some(field) = &self.field {
            write!(f, "{field}: ")?;
        }
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for LoadError {}

/// Formats a list of errors one per line, for CLI and test output.
pub fn format_errors(errors: &[LoadError]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Default)]
pub struct Library {
    pub primitives: BTreeMap<String, Primitive>,
    pub recipes: BTreeMap<String, Recipe>,
    /// Word → replacement words, applied to requests before matching.
    pub synonyms: BTreeMap<String, String>,
    /// Notes on programs for `sancho explain`, by program name.
    pub commands: BTreeMap<String, CommandDoc>,
}

impl Library {
    /// Loads `<dir>/primitives/*.toml`, `<dir>/recipes/*.toml`,
    /// `<dir>/commands/*.toml` and the optional `<dir>/synonyms.toml`.
    /// Each file holds one item whose id matches the file name.
    pub fn load(dir: &Path) -> Result<Library, Vec<LoadError>> {
        let mut errors = Vec::new();
        let mut lib = Library::default();

        for path in list_data_files(&dir.join("primitives"), true, &mut errors) {
            if let Some(prim) = parse_file(&path, parse_primitive, &mut errors) {
                check_file_name(&path, &prim.id, &mut errors);
                lib.primitives.insert(prim.id.clone(), prim);
            }
        }

        for path in list_data_files(&dir.join("commands"), false, &mut errors) {
            if let Some(doc) = parse_file(&path, parse_command_doc, &mut errors) {
                check_file_name(&path, &doc.command, &mut errors);
                lib.commands.insert(doc.command.clone(), doc);
            }
        }

        let synonyms_file = dir.join("synonyms.toml");
        if synonyms_file.exists() {
            if let Some(syn) = parse_file(&synonyms_file, parse_synonyms, &mut errors) {
                lib.synonyms = syn;
            }
        }

        let mut recipes = Vec::new();
        for path in list_data_files(&dir.join("recipes"), false, &mut errors) {
            if let Some(recipe) = parse_file(&path, parse_recipe, &mut errors) {
                check_file_name(&path, &recipe.id, &mut errors);
                recipes.push((path, recipe));
            }
        }
        for (path, recipe) in recipes {
            let mut p = Problems::default();
            recipe.validate(&mut p, &lib.primitives);
            errors.extend(problems_to_errors(&path, p));
            lib.recipes.insert(recipe.id.clone(), recipe);
        }

        if errors.is_empty() {
            Ok(lib)
        } else {
            Err(errors)
        }
    }
}

/// Parses and self-validates one primitive.
pub fn parse_primitive(file: &Path, src: &str) -> Result<Primitive, Vec<LoadError>> {
    let prim: Primitive = parse_toml(file, src)?;
    let mut p = Problems::default();
    prim.validate(&mut p);
    if p.is_empty() {
        Ok(prim)
    } else {
        Err(problems_to_errors(file, p))
    }
}

/// Parses one recipe. Cross-checks against primitives happen in [`Library::load`]
/// or [`Recipe::validate`].
pub fn parse_recipe(file: &Path, src: &str) -> Result<Recipe, Vec<LoadError>> {
    parse_toml(file, src)
}

/// Parses and validates notes on one program.
pub fn parse_command_doc(file: &Path, src: &str) -> Result<CommandDoc, Vec<LoadError>> {
    let doc: CommandDoc = parse_toml(file, src)?;
    let mut p = Problems::default();
    doc.validate(&mut p);
    if p.is_empty() {
        Ok(doc)
    } else {
        Err(problems_to_errors(file, p))
    }
}

/// `word = "replacement words"`; both sides lowercase words.
pub fn parse_synonyms(file: &Path, src: &str) -> Result<BTreeMap<String, String>, Vec<LoadError>> {
    let map: BTreeMap<String, String> = parse_toml(file, src)?;
    let word = |w: &str| {
        !w.is_empty()
            && w.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "'-".contains(c))
    };
    let mut p = Problems::default();
    for (k, v) in &map {
        if !word(k) {
            p.push(k.clone(), "must be a single lowercase word");
        }
        if v.split_whitespace().next().is_none() || !v.split_whitespace().all(word) {
            p.push(k.clone(), "replacement must be lowercase words");
        }
    }
    if p.is_empty() {
        Ok(map)
    } else {
        Err(problems_to_errors(file, p))
    }
}

fn parse_toml<T: DeserializeOwned>(file: &Path, src: &str) -> Result<T, Vec<LoadError>> {
    toml::from_str(src).map_err(|e| {
        vec![LoadError {
            file: file.to_path_buf(),
            position: e.span().map(|span| line_col(src, span.start)),
            field: None,
            reason: e.message().trim_end().to_string(),
        }]
    })
}

/// 1-based line and column (in characters) of a byte offset.
fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let before = &src[..offset.min(src.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, col)
}

fn problems_to_errors(file: &Path, p: Problems) -> Vec<LoadError> {
    p.0.into_iter()
        .map(|(field, reason)| LoadError {
            file: file.to_path_buf(),
            position: None,
            field: Some(field),
            reason,
        })
        .collect()
}

fn parse_file<T>(
    path: &Path,
    parse: fn(&Path, &str) -> Result<T, Vec<LoadError>>,
    errors: &mut Vec<LoadError>,
) -> Option<T> {
    match read_file(path).and_then(|src| parse(path, &src)) {
        Ok(v) => Some(v),
        Err(e) => {
            errors.extend(e);
            None
        }
    }
}

fn read_file(path: &Path) -> Result<String, Vec<LoadError>> {
    let err = |reason: String| {
        vec![LoadError {
            file: path.to_path_buf(),
            position: None,
            field: None,
            reason,
        }]
    };
    let meta = fs::metadata(path).map_err(|e| err(format!("cannot read: {e}")))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(err(format!(
            "file is {} bytes; the limit is {MAX_FILE_BYTES}",
            meta.len()
        )));
    }
    fs::read_to_string(path).map_err(|e| err(format!("cannot read: {e}")))
}

fn check_file_name(path: &Path, id: &str, errors: &mut Vec<LoadError>) {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if stem != id {
        errors.push(LoadError {
            file: path.to_path_buf(),
            position: None,
            field: None,
            reason: format!("file name must match the id: expected '{id}.toml'"),
        });
    }
}

/// Sorted `*.toml` files in `dir`. Any other visible file is an error, so
/// nothing is silently skipped.
fn list_data_files(dir: &Path, required: bool, errors: &mut Vec<LoadError>) -> Vec<PathBuf> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            errors.push(LoadError {
                file: dir.to_path_buf(),
                position: None,
                field: None,
                reason: format!("cannot read directory: {e}"),
            });
            return Vec::new();
        }
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if hidden {
            continue;
        }
        if path.extension().is_some_and(|e| e == "toml") && path.is_file() {
            files.push(path);
        } else {
            errors.push(LoadError {
                file: path,
                position: None,
                field: None,
                reason: "unexpected file: only *.toml files belong here".into(),
            });
        }
    }
    files.sort();
    files
}
