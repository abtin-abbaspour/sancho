//! Task recipes: curated multi-step tasks built from primitives (`data/recipes/*.toml`).

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::primitive::{check_values, Primitive};
use crate::template::{is_ident, Part, Ref, Template};
use crate::validate::{check_id, Problems};
use crate::value::{check_path, check_path_fragment, Risk, ValueType};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub id: String,
    pub version: u32,
    pub summary: String,
    #[serde(default)]
    pub domains: Vec<String>,
    pub patterns: Vec<Template>,
    #[serde(default)]
    pub slots: BTreeMap<String, SlotSpec>,
    /// Profile facts that must hold; each lists the accepted values.
    #[serde(default)]
    pub requires: BTreeMap<ProfileKey, Vec<String>>,
    pub risk: Risk,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub notes: Vec<String>,
    pub tests: TestDecl,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotSpec {
    #[serde(rename = "type")]
    pub ty: ValueType,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub filter: BTreeMap<String, FilterValue>,
    /// The user may name several (e.g. packages).
    #[serde(default)]
    pub multiple: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum FilterValue {
    Bool(bool),
    String(String),
}

/// Facts the system profile (crate `sancho-profile`) reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKey {
    Distro,
    Init,
    Network,
    Audio,
    Display,
    AurHelper,
    RootFs,
    SnapshotTool,
    Bootloader,
}

impl ProfileKey {
    pub const ALL: [ProfileKey; 9] = {
        use ProfileKey::*;
        [
            Distro,
            Init,
            Network,
            Audio,
            Display,
            AurHelper,
            RootFs,
            SnapshotTool,
            Bootloader,
        ]
    };

    pub fn name(self) -> &'static str {
        use ProfileKey::*;
        match self {
            Distro => "distro",
            Init => "init",
            Network => "network",
            Audio => "audio",
            Display => "display",
            AurHelper => "aur_helper",
            RootFs => "root_fs",
            SnapshotTool => "snapshot_tool",
            Bootloader => "bootloader",
        }
    }

    pub fn from_name(s: &str) -> Option<ProfileKey> {
        Self::ALL.into_iter().find(|k| k.name() == s)
    }

    /// Every value detection can report for this fact.
    pub fn values(self) -> &'static [&'static str] {
        use ProfileKey::*;
        match self {
            Distro => &[
                "arch",
                "artix",
                "cachyos",
                "endeavouros",
                "garuda",
                "manjaro",
                "arch_derivative",
                "unsupported",
            ],
            Init => &["systemd", "openrc", "runit", "dinit", "s6", "unknown"],
            Network => &[
                "networkmanager",
                "connman",
                "iwd",
                "systemd_networkd",
                "none",
            ],
            Audio => &["pipewire", "pulseaudio", "none"],
            Display => &["wayland", "x11", "none"],
            AurHelper => &["paru", "yay", "none"],
            RootFs => &["btrfs", "ext4", "xfs", "f2fs", "zfs", "bcachefs", "other"],
            SnapshotTool => &["snapper", "timeshift", "none"],
            Bootloader => &["systemd_boot", "limine", "refind", "grub", "unknown"],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub primitive: String,
    /// One template per primitive arg; all are required.
    #[serde(default)]
    pub args: BTreeMap<String, Template>,
    pub explain: Template,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestDecl {
    pub verified_by: Verification,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    Container,
    Mock,
    Vm,
    Manual,
}

/// Placeholder prefix for profile facts, e.g. `{profile.aur_helper}`.
pub const PROFILE: &str = "profile";

impl Recipe {
    /// Checks the recipe on its own and against the primitives it uses.
    pub fn validate(&self, p: &mut Problems, primitives: &BTreeMap<String, Primitive>) {
        check_id(p, "id", &self.id);
        if self.version == 0 {
            p.push("version", "must be at least 1");
        }
        if self.summary.trim().is_empty() {
            p.push("summary", "must not be empty");
        }
        for (i, d) in self.domains.iter().enumerate() {
            if !is_ident(d) {
                p.push(format!("domains[{i}]"), "must be a lowercase identifier");
            }
        }
        if self.patterns.is_empty() {
            p.push("patterns", "at least one pattern is required");
        }
        for (i, pat) in self.patterns.iter().enumerate() {
            for r in pat.refs() {
                if r.attr.is_some() || !self.slots.contains_key(&r.name) {
                    p.push(format!("patterns[{i}]"), format!("{r} is not a slot"));
                }
            }
        }

        for (name, slot) in &self.slots {
            self.validate_slot(p, name, slot);
        }

        for (key, values) in &self.requires {
            let field = format!("requires.{}", key.name());
            if values.is_empty() {
                p.push(&field, "must list at least one value");
            }
            for v in values
                .iter()
                .filter(|v| !key.values().contains(&v.as_str()))
            {
                p.push(
                    &field,
                    format!("'{v}' is not one of: {}", key.values().join(", ")),
                );
            }
        }

        if self.steps.is_empty() {
            p.push("steps", "at least one step is required");
        }
        let mut used = BTreeSet::new();
        for (i, step) in self.steps.iter().enumerate() {
            self.validate_step(p, &format!("steps[{i}]"), step, primitives, &mut used);
        }
        for name in self.slots.keys() {
            if !used.contains(name.as_str()) {
                p.push(format!("slots.{name}"), "slot is never used by any step");
            }
        }

        if self.tests.verified_by != Verification::Container
            && self
                .tests
                .reason
                .as_deref()
                .is_none_or(|r| r.trim().is_empty())
        {
            p.push(
                "tests.reason",
                "a reason is required when not verified by container",
            );
        }
    }

    fn validate_slot(&self, p: &mut Problems, name: &str, slot: &SlotSpec) {
        let field = format!("slots.{name}");
        if !is_ident(name) || name == PROFILE {
            p.push(
                &field,
                "slot names must be lowercase identifiers other than 'profile'",
            );
        }
        check_values(p, &field, slot.ty, &slot.values);
        for (key, value) in &slot.filter {
            let ffield = format!("{field}.filter.{key}");
            match slot.ty.filter_keys().iter().find(|(k, _)| k == key) {
                None => p.push(ffield, format!("{} slots have no '{key}' filter", slot.ty)),
                Some((_, is_bool)) if *is_bool != matches!(value, FilterValue::Bool(_)) => {
                    let want = if *is_bool { "true/false" } else { "a string" };
                    p.push(ffield, format!("must be {want}"));
                }
                Some(_) => {}
            }
        }
        if slot.multiple && !slot.ty.is_resolved() {
            p.push(&field, "only system-resolved slot types can be multiple");
        }
    }

    fn validate_step<'a>(
        &'a self,
        p: &mut Problems,
        field: &str,
        step: &'a Step,
        primitives: &BTreeMap<String, Primitive>,
        used: &mut BTreeSet<&'a str>,
    ) {
        for r in step.explain.refs() {
            self.check_ref(p, &format!("{field}.explain"), r, used);
        }
        let Some(prim) = primitives.get(&step.primitive) else {
            p.push(
                format!("{field}.primitive"),
                format!("unknown primitive '{}'", step.primitive),
            );
            return;
        };
        if prim.risk > self.risk {
            p.push(
                "risk",
                format!(
                    "recipe risk {} is lower than step primitive {} ({})",
                    self.risk, prim.id, prim.risk
                ),
            );
        }
        for name in prim.args.keys() {
            if !step.args.contains_key(name) {
                p.push(
                    format!("{field}.args"),
                    format!("missing arg '{name}' of {}", prim.id),
                );
            }
        }
        for (name, tmpl) in &step.args {
            let afield = format!("{field}.args.{name}");
            let Some(arg) = prim.args.get(name) else {
                p.push(&afield, format!("{} has no arg '{name}'", prim.id));
                continue;
            };
            for r in tmpl.refs() {
                self.check_ref(p, &afield, r, used);
            }
            self.check_arg_value(p, &afield, arg.ty, &arg.values, arg.variadic, tmpl);
        }
    }

    fn check_ref<'a>(
        &'a self,
        p: &mut Problems,
        field: &str,
        r: &Ref,
        used: &mut BTreeSet<&'a str>,
    ) {
        if r.name == PROFILE {
            match r.attr.as_deref().and_then(ProfileKey::from_name) {
                None => p.push(field, format!("{r} is not a profile fact")),
                Some(key) if !self.requires.contains_key(&key) => p.push(
                    field,
                    format!("{r} is used but requires does not constrain it"),
                ),
                Some(_) => {}
            }
            return;
        }
        let Some((name, slot)) = self.slots.get_key_value(&r.name) else {
            p.push(field, format!("{r} is not a slot"));
            return;
        };
        used.insert(name);
        match &r.attr {
            Some(attr) if !slot.ty.attributes().contains(&attr.as_str()) => p.push(
                field,
                format!("{} slots have no attribute '{attr}'", slot.ty),
            ),
            None if slot.ty.is_compound() => p.push(
                field,
                format!(
                    "{r} needs an attribute: one of {}",
                    slot.ty.attributes().join(", ")
                ),
            ),
            _ => {}
        }
    }

    /// The core safety check: values reaching a typed argument must be typed.
    fn check_arg_value(
        &self,
        p: &mut Problems,
        field: &str,
        ty: ValueType,
        values: &[String],
        variadic: bool,
        tmpl: &Template,
    ) {
        if let Some(r) = tmpl.as_single_ref() {
            if r.name == PROFILE {
                if ty != ValueType::Choice {
                    p.push(field, "profile facts can only fill choice args");
                } else if let Some(key) = r.attr.as_deref().and_then(ProfileKey::from_name) {
                    let allowed = self.requires.get(&key).into_iter().flatten();
                    for v in allowed.filter(|v| !values.contains(v)) {
                        p.push(
                            field,
                            format!("required value '{v}' is not accepted by the arg"),
                        );
                    }
                }
                return;
            }
            let Some(slot) = self.slots.get(&r.name) else {
                return; // reported by check_ref
            };
            if r.attr.is_none() {
                if slot.ty != ty {
                    p.push(
                        field,
                        format!("slot '{}' is {} but the arg expects {ty}", r.name, slot.ty),
                    );
                } else if let Some(v) = slot.values.iter().find(|v| !values.contains(v)) {
                    p.push(
                        field,
                        format!("slot value '{v}' is not accepted by the arg"),
                    );
                }
                if slot.multiple && !variadic {
                    p.push(field, "a multiple slot can only fill a variadic arg");
                }
                return;
            }
        }
        for r in tmpl.refs() {
            if self.slots.get(&r.name).is_some_and(|s| s.multiple) {
                p.push(
                    field,
                    format!("multiple slot {r} must be the whole value of a variadic arg"),
                );
            }
        }
        if let Some(lit) = tmpl.as_literal() {
            if let Err(e) = ty.check_literal(&lit, values) {
                p.push(field, e);
            }
            return;
        }
        // Mixed text and placeholders.
        match ty {
            ValueType::Text => {}
            ValueType::Path => {
                match tmpl.parts.first() {
                    Some(Part::Literal(first)) => {
                        if let Err(e) = check_path(first) {
                            p.push(field, e);
                        }
                    }
                    _ => p.push(
                        field,
                        "a templated path must start with a literal absolute path",
                    ),
                }
                for lit in tmpl.literals() {
                    if let Err(e) = check_path_fragment(lit) {
                        p.push(field, e);
                    }
                }
            }
            t => p.push(
                field,
                format!("a {t} arg must be filled by exactly one slot or literal, not a template"),
            ),
        }
    }
}
