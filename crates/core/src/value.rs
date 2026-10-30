//! Value types shared by primitive arguments and recipe slots, and risk levels.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The type of a primitive argument or recipe slot.
///
/// Resolved types get their values from a resolver that reads the real system
/// (crate `sancho-resolve`); they can never be written as literals in a recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    HidDevice,
    SystemdUnit,
    Package,
    NetIface,
    Browser,
    UserGroup,
    User,
    /// Absolute filesystem path.
    Path,
    /// Octal permission mode such as `0644`.
    FileMode,
    /// Non-negative integer.
    Count,
    /// Free text; only ever passed on stdin, never as an argv element.
    Text,
    /// One of a fixed list of `values`.
    Choice,
}

impl ValueType {
    pub fn is_resolved(self) -> bool {
        use ValueType::*;
        matches!(
            self,
            HidDevice | SystemdUnit | Package | NetIface | Browser | UserGroup | User
        )
    }

    /// Attributes usable as `{slot.attr}` in recipe templates.
    pub fn attributes(self) -> &'static [&'static str] {
        match self {
            ValueType::HidDevice => &["name", "vid", "pid"],
            ValueType::Browser => &["name", "desktop_id"],
            _ => &[],
        }
    }

    /// Compound types have no single value; templates must name an attribute.
    pub fn is_compound(self) -> bool {
        !self.attributes().is_empty()
    }

    /// Filters a recipe slot of this type may declare. Resolvers tag each
    /// candidate; a `Bool` filter `k = b` keeps candidates where having tag `k`
    /// equals `b`, a `OneOf` filter `k = "v"` keeps candidates tagged `v`.
    pub fn filters(self) -> &'static [(&'static str, FilterKind)] {
        match self {
            ValueType::HidDevice => &[("input_class", FilterKind::OneOf(INPUT_CLASSES))],
            ValueType::Browser => &[("webhid", FilterKind::Bool)],
            ValueType::Package => &[("installed", FilterKind::Bool)],
            ValueType::NetIface => &[("wireless", FilterKind::Bool)],
            ValueType::SystemdUnit => &[("enabled", FilterKind::Bool)],
            _ => &[],
        }
    }

    /// Checks a literal written in a recipe or primitive default.
    ///
    /// Names of system objects (units, packages, ...) are checked for syntax
    /// here; whether they exist is checked against the system at plan time.
    pub fn check_literal(self, s: &str, values: &[String]) -> Result<(), String> {
        let named = |ok: bool, what: &str| {
            if ok {
                Ok(())
            } else {
                Err(format!("'{s}' is not a valid {what} name"))
            }
        };
        match self {
            t if t.is_compound() => Err(format!(
                "a {t} must come from a slot resolved from the system, not a literal"
            )),
            ValueType::SystemdUnit => named(valid_unit(s), "systemd unit"),
            ValueType::Package => named(valid_package(s), "package"),
            ValueType::NetIface => named(valid_iface(s), "network interface"),
            ValueType::UserGroup => named(valid_account(s), "group"),
            ValueType::User => named(valid_account(s), "user"),
            ValueType::Path => check_path(s),
            ValueType::FileMode => {
                let digits = s.strip_prefix('0').unwrap_or(s);
                if (3..=4).contains(&s.len())
                    && !digits.is_empty()
                    && s.chars().all(|c| matches!(c, '0'..='7'))
                {
                    Ok(())
                } else {
                    Err(format!("'{s}' is not an octal file mode like 0644"))
                }
            }
            ValueType::Count => s
                .parse::<u32>()
                .map(|_| ())
                .map_err(|_| format!("'{s}' is not a non-negative integer")),
            ValueType::Choice => {
                if values.iter().any(|v| v == s) {
                    Ok(())
                } else {
                    Err(format!("'{s}' is not one of: {}", values.join(", ")))
                }
            }
            ValueType::Text => Ok(()),
            _ => unreachable!("resolved types handled above"),
        }
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ValueType::HidDevice => "hid_device",
            ValueType::SystemdUnit => "systemd_unit",
            ValueType::Package => "package",
            ValueType::NetIface => "net_iface",
            ValueType::Browser => "browser",
            ValueType::UserGroup => "user_group",
            ValueType::User => "user",
            ValueType::Path => "path",
            ValueType::FileMode => "file_mode",
            ValueType::Count => "count",
            ValueType::Text => "text",
            ValueType::Choice => "choice",
        };
        f.write_str(s)
    }
}

/// Non-empty, at most `max_len` bytes, only ASCII alphanumerics and `extra`,
/// and not starting with `-` (so it can never be read as an option).
pub fn safe_token(s: &str, extra: &str, max_len: usize) -> bool {
    !s.is_empty()
        && s.len() <= max_len
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

/// systemd unit names: `[A-Za-z0-9:_.\-@]` plus `\` escapes, with a type suffix.
pub fn valid_unit(name: &str) -> bool {
    safe_token(name, ":_.-@\\", 255)
        && name
            .rsplit_once('.')
            .is_some_and(|(stem, ty)| !stem.is_empty() && !ty.is_empty())
}

/// pacman package names: lowercase alphanumerics and `@._+-`, not starting
/// with `-` or `.`.
pub fn valid_package(name: &str) -> bool {
    safe_token(name, "@._+-", 255)
        && !name.starts_with('.')
        && !name.chars().any(|c| c.is_ascii_uppercase())
}

/// Kernel interface names: at most 15 bytes, no '/', whitespace or ':'.
pub fn valid_iface(name: &str) -> bool {
    safe_token(name, "_.-", 15)
}

/// shadow-utils' default rule: `[a-z_][a-z0-9_-]*[$]?`, at most 32 bytes.
pub fn valid_account(name: &str) -> bool {
    let body = name.strip_suffix('$').unwrap_or(name);
    safe_token(body, "_-", 32)
        && body.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && !body.chars().any(|c| c.is_ascii_uppercase())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    Bool,
    OneOf(&'static [&'static str]),
}

/// udev `ID_INPUT_*` classes a HID device can be filtered by.
pub const INPUT_CLASSES: &[&str] = &["keyboard", "mouse", "touchpad", "joystick", "tablet"];

/// Literal path (or the literal start of a templated path) must be absolute
/// and must not contain `..`, NUL or empty components.
pub fn check_path(s: &str) -> Result<(), String> {
    if !s.starts_with('/') {
        return Err(format!("path '{s}' must be absolute"));
    }
    check_path_fragment(s)
}

pub fn check_path_fragment(s: &str) -> Result<(), String> {
    if s.contains('\0') {
        return Err("path contains a NUL byte".into());
    }
    if s.split('/').any(|c| c == "..") {
        return Err(format!("path '{s}' must not contain '..'"));
    }
    if s.contains("//") {
        return Err(format!("path '{s}' contains an empty component"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl fmt::Display for Risk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_are_checked_by_type() {
        let none: &[String] = &[];
        assert!(ValueType::Path.check_literal("/etc/x.rules", none).is_ok());
        assert!(ValueType::Path.check_literal("etc/x", none).is_err());
        assert!(ValueType::Path.check_literal("/etc/../x", none).is_err());
        assert!(ValueType::FileMode.check_literal("0644", none).is_ok());
        assert!(ValueType::FileMode.check_literal("644", none).is_ok());
        assert!(ValueType::FileMode.check_literal("0948", none).is_err());
        assert!(ValueType::FileMode.check_literal("00", none).is_err());
        assert!(ValueType::Count.check_literal("2", none).is_ok());
        assert!(ValueType::Count.check_literal("-1", none).is_err());
        let vals = vec!["on".to_string(), "off".to_string()];
        assert!(ValueType::Choice.check_literal("on", &vals).is_ok());
        assert!(ValueType::Choice.check_literal("maybe", &vals).is_err());
        assert!(ValueType::SystemdUnit
            .check_literal("sshd.service", none)
            .is_ok());
        assert!(ValueType::SystemdUnit
            .check_literal("-x.service", none)
            .is_err());
        assert!(ValueType::SystemdUnit.check_literal("sshd", none).is_err());
        assert!(ValueType::Package.check_literal("Firefox", none).is_err());
        assert!(ValueType::UserGroup.check_literal("wheel", none).is_ok());
        assert!(ValueType::HidDevice
            .check_literal("046d:c52b", none)
            .is_err());
    }
}
