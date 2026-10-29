//! `{name}` / `{name.attr}` placeholders in recipe and primitive strings.
//!
//! Templates are filled into single argv elements, file contents or explanation
//! text; they are never passed to a shell. `{{` and `}}` are literal braces.

use std::fmt;

use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    pub attr: Option<String>,
}

impl fmt::Display for Ref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.attr {
            Some(attr) => write!(f, "{{{}.{attr}}}", self.name),
            None => write!(f, "{{{}}}", self.name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Literal(String),
    Ref(Ref),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub parts: Vec<Part>,
}

impl Template {
    pub fn parse(s: &str) -> Result<Template, String> {
        let mut parts = Vec::new();
        let mut lit = String::new();
        let mut chars = s.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            match c {
                '{' if chars.peek().map(|&(_, c)| c) == Some('{') => {
                    chars.next();
                    lit.push('{');
                }
                '}' if chars.peek().map(|&(_, c)| c) == Some('}') => {
                    chars.next();
                    lit.push('}');
                }
                '}' => {
                    return Err(format!(
                        "unmatched '}}' at offset {i} (write '}}}}' for a literal brace)"
                    ))
                }
                '{' => {
                    let rest = &s[i + 1..];
                    let Some(end) = rest.find('}') else {
                        return Err(format!(
                            "unclosed '{{' at offset {i} (write '{{{{' for a literal brace)"
                        ));
                    };
                    let r = parse_ref(&rest[..end])?;
                    for _ in 0..rest[..=end].chars().count() {
                        chars.next();
                    }
                    if !lit.is_empty() {
                        parts.push(Part::Literal(std::mem::take(&mut lit)));
                    }
                    parts.push(Part::Ref(r));
                }
                c => lit.push(c),
            }
        }
        if !lit.is_empty() {
            parts.push(Part::Literal(lit));
        }
        Ok(Template { parts })
    }

    pub fn refs(&self) -> impl Iterator<Item = &Ref> {
        self.parts.iter().filter_map(|p| match p {
            Part::Ref(r) => Some(r),
            Part::Literal(_) => None,
        })
    }

    /// The reference, if the template is exactly one placeholder and nothing else.
    pub fn as_single_ref(&self) -> Option<&Ref> {
        match self.parts.as_slice() {
            [Part::Ref(r)] => Some(r),
            _ => None,
        }
    }

    /// The text, if the template contains no placeholders.
    pub fn as_literal(&self) -> Option<String> {
        match self.parts.as_slice() {
            [] => Some(String::new()),
            [Part::Literal(s)] => Some(s.clone()),
            _ => None,
        }
    }

    pub fn literals(&self) -> impl Iterator<Item = &str> {
        self.parts.iter().filter_map(|p| match p {
            Part::Literal(s) => Some(s.as_str()),
            Part::Ref(_) => None,
        })
    }
}

impl fmt::Display for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for part in &self.parts {
            match part {
                Part::Literal(s) => write!(f, "{}", s.replace('{', "{{").replace('}', "}}"))?,
                Part::Ref(r) => write!(f, "{r}")?,
            }
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for Template {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Template::parse(&s).map_err(serde::de::Error::custom)
    }
}

fn parse_ref(inner: &str) -> Result<Ref, String> {
    let (name, attr) = match inner.split_once('.') {
        Some((n, a)) => (n, Some(a)),
        None => (inner, None),
    };
    let bad = || {
        format!(
            "invalid placeholder '{{{inner}}}': expected {{name}} or {{name.attr}} \
             using lowercase letters, digits and '_' (write '{{{{' for a literal brace)"
        )
    };
    if !is_ident(name) || attr.is_some_and(|a| !is_ident(a)) {
        return Err(bad());
    }
    Ok(Ref {
        name: name.to_string(),
        attr: attr.map(str::to_string),
    })
}

/// Lowercase identifier: `[a-z][a-z0-9_]*`.
pub fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(name: &str, attr: Option<&str>) -> Part {
        Part::Ref(Ref {
            name: name.into(),
            attr: attr.map(Into::into),
        })
    }

    #[test]
    fn parses_literals_refs_and_escapes() {
        let t = Template::parse(r#"ATTRS{{idVendor}}=="{device.vid}", x={n}"#).unwrap();
        assert_eq!(
            t.parts,
            vec![
                Part::Literal(r#"ATTRS{idVendor}==""#.into()),
                r("device", Some("vid")),
                Part::Literal(r#"", x="#.into()),
                r("n", None),
            ]
        );
        assert_eq!(t.to_string(), r#"ATTRS{{idVendor}}=="{device.vid}", x={n}"#);
    }

    #[test]
    fn single_ref_and_literal() {
        assert_eq!(
            Template::parse("{unit}").unwrap().as_single_ref(),
            Some(&Ref {
                name: "unit".into(),
                attr: None
            })
        );
        assert!(Template::parse("x{unit}")
            .unwrap()
            .as_single_ref()
            .is_none());
        assert_eq!(
            Template::parse("a{{b").unwrap().as_literal().as_deref(),
            Some("a{b")
        );
    }

    #[test]
    fn rejects_malformed_placeholders() {
        for bad in [
            "{",
            "a}",
            "{}",
            "{Unit}",
            "{a.b.c}",
            "{a b}",
            "{1x}",
            "{idVendor}",
        ] {
            assert!(Template::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn handles_multibyte_text() {
        let t = Template::parse("café {x} ✓").unwrap();
        assert_eq!(t.refs().count(), 1);
        assert_eq!(t.to_string(), "café {x} ✓");
    }
}
