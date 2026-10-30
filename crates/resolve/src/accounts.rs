//! `user_group` (`/etc/group`) and `user` (regular accounts in `/etc/passwd`).

use sancho_core::System;

use crate::{safe_token, Value};

pub fn groups(sys: &dyn System) -> Vec<Value> {
    sys.read("/etc/group")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split(':').next())
        .filter(|name| valid_account(name))
        .map(|name| Value::new(name, name))
        .collect()
}

/// Regular login accounts (UID 1000–59999 with a login shell). The current
/// user (`$USER`, falling back to `$LOGNAME`) is tagged `current`.
pub fn users(sys: &dyn System) -> Vec<Value> {
    let current = sys.env("USER").or_else(|| sys.env("LOGNAME"));
    sys.read("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            let (name, uid, shell) = (*f.first()?, f.get(2)?.parse::<u32>().ok()?, *f.get(6)?);
            let login = !shell.ends_with("/nologin") && !shell.ends_with("/false");
            ((1000..60000).contains(&uid) && login && valid_account(name))
                .then(|| Value::new(name, name).tag("current", current.as_deref() == Some(name)))
        })
        .collect()
}

/// shadow-utils' default rule: `[a-z_][a-z0-9_-]*[$]?`, at most 32 bytes.
fn valid_account(name: &str) -> bool {
    let body = name.strip_suffix('$').unwrap_or(name);
    safe_token(body, "_-", 32)
        && body.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && !body.chars().any(|c| c.is_ascii_uppercase())
}
