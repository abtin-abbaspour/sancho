//! `systemd_unit`: installed unit files (`systemctl list-unit-files`).

use sancho_core::value::valid_unit;
use sancho_core::{Query, System};

use crate::{ResolveError, Value};

/// All unit files, or with `pattern` (a systemctl glob) only matching ones.
pub fn enumerate(sys: &dyn System, pattern: Option<String>) -> Result<Vec<Value>, ResolveError> {
    let q = Query::UnitFiles(pattern);
    let out = sys.query(q.clone()).ok_or(ResolveError::Unavailable(q))?;
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
