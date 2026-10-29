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
            ValueType::HidDevice => &["name", "vid", "pid", "hidraw"],
            ValueType::Browser => &["name", "package", "desktop_id"],
            _ => &[],
        }
    }

    /// Compound types have no single value; templates must name an attribute.
    pub fn is_compound(self) -> bool {
        !self.attributes().is_empty()
    }

    /// Filter keys a recipe slot of this type may declare, and whether each is
    /// a boolean (`true`) or a string (`false`). Semantics live in the resolvers.
    pub fn filter_keys(self) -> &'static [(&'static str, bool)] {
        match self {
            ValueType::HidDevice => &[("input_class", false)],
            ValueType::Browser => &[("webhid", true)],
            ValueType::Package => &[("installed", true)],
            ValueType::NetIface => &[("wireless", true)],
            _ => &[],
        }
    }

    /// Checks a literal written in a recipe or primitive default.
    pub fn check_literal(self, s: &str, values: &[String]) -> Result<(), String> {
        match self {
            t if t.is_resolved() => Err(format!(
                "a {t} value must come from a slot resolved from the system, not a literal"
            )),
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
            .is_err());
    }
}
