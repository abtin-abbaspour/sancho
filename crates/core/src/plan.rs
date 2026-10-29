//! A concrete plan: what will run, shown to the user before confirmation.
//! Built by crate `sancho-plan` (M6); defined here so every crate shares it.

use serde::Serialize;

use crate::effect::{EffectKind, Reversibility};
use crate::value::Risk;

/// Where the plan came from. Always shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    VerifiedRecipe,
    CheckedCommand,
    Unverified,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub tier: Tier,
    pub recipe_id: Option<String>,
    pub recipe_version: Option<u32>,
    pub risk: Risk,
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanStep {
    pub primitive: String,
    /// Exact argument vector; never passed through a shell.
    pub argv: Vec<String>,
    pub stdin: Option<String>,
    pub needs_root: bool,
    pub interactive: bool,
    pub read_only: bool,
    pub risk: Risk,
    pub explanation: String,
    pub effects: Vec<PlannedEffect>,
    pub reversibility: Reversibility,
    /// Came from the learned store rather than a shipped recipe.
    pub learned: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedEffect {
    pub kind: EffectKind,
    pub target: String,
}

impl Plan {
    /// The least reversible step decides the plan's overall class.
    pub fn reversibility(&self) -> Reversibility {
        self.steps
            .iter()
            .map(|s| s.reversibility)
            .max()
            .unwrap_or(Reversibility::NoUndoNeeded)
    }
}
