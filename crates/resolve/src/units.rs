//! `systemd_unit`: installed unit files (`systemctl list-unit-files`).

use sancho_core::{Query, System};

use crate::{safe_token, ResolveError, Value};

pub fn enumerate(sys: &dyn System) -> Result<Vec<Value>, ResolveError> {
    let out = sys
        .query(Query::UnitFiles)
        .ok_or(ResolveError::Unavailable(Query::UnitFiles))?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let (name, state) = (cols.next()?, cols.next()?);
            // Templates (`getty@.service`) need an instance name to be used.
            if name.contains("@.") || !valid_unit(name) {
                return None;
            }
            Some(Value::new(name, format!("{name} ({state})")).tag("enabled", state == "enabled"))
        })
        .collect())
}

/// systemd unit names: `[A-Za-z0-9:_.\-@]` plus `\` escapes, with a type suffix.
fn valid_unit(name: &str) -> bool {
    safe_token(name, ":_.-@\\", 255)
        && name
            .rsplit_once('.')
            .is_some_and(|(stem, _)| !stem.is_empty())
}
