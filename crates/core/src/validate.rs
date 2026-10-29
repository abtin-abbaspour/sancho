//! Collects schema problems as (field path, reason) pairs.

use crate::template::is_ident;

#[derive(Debug, Default)]
pub struct Problems(pub Vec<(String, String)>);

impl Problems {
    pub fn push(&mut self, field: impl Into<String>, reason: impl Into<String>) {
        self.0.push((field.into(), reason.into()));
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// IDs look like `group.name`: dot-separated lowercase identifiers, at least two parts.
pub fn check_id(p: &mut Problems, field: &str, id: &str) {
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() < 2 || !parts.iter().all(|s| is_ident(s)) {
        p.push(
            field,
            format!("'{id}' is not a valid id (expected e.g. 'systemctl.enable')"),
        );
    }
}
