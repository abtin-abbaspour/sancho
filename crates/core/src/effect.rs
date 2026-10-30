//! Effects a primitive has on the system, and how undoable each kind is.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::template::Template;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    FileWrite,
    FileEdit,
    ServiceEnablement,
    ServiceRuntime,
    PackageInstall,
    PackageRemove,
    /// Full or partial system upgrade; only a filesystem snapshot can undo it.
    PackageUpgrade,
    /// Deletes cached packages, which can invalidate package undo.
    PackageCachePrune,
    GroupMembership,
    /// Radio on/off state (e.g. Wi-Fi).
    RadioState,
    /// Creates a filesystem snapshot; the snapshot tool's own retention
    /// policy cleans it up, so there is nothing to undo.
    Snapshot,
    /// Reloads, restarts, triggers: transient, nothing to restore.
    Runtime,
    /// rm, mkfs, dd and the like.
    Destructive,
}

/// How a change can be undone. Shown to the user before confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    NoUndoNeeded,
    Reversible,
    /// Reversible while the package files are still in the pacman cache.
    ReversibleWhileCached,
    /// Only a filesystem snapshot taken beforehand can undo it.
    SnapshotOnly,
    Irreversible,
}

impl EffectKind {
    /// The class this kind has once its capture/restore handler exists.
    /// The planner downgrades to `Irreversible` when no handler is registered.
    pub fn reversibility(self) -> Reversibility {
        use EffectKind::*;
        match self {
            FileWrite | FileEdit | ServiceEnablement | ServiceRuntime | GroupMembership
            | RadioState => Reversibility::Reversible,
            PackageInstall | PackageRemove => Reversibility::ReversibleWhileCached,
            PackageUpgrade => Reversibility::SnapshotOnly,
            Runtime | Snapshot => Reversibility::NoUndoNeeded,
            PackageCachePrune | Destructive => Reversibility::Irreversible,
        }
    }
}

impl fmt::Display for EffectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use EffectKind::*;
        f.write_str(match self {
            FileWrite => "file_write",
            FileEdit => "file_edit",
            ServiceEnablement => "service_enablement",
            ServiceRuntime => "service_runtime",
            PackageInstall => "package_install",
            PackageRemove => "package_remove",
            PackageUpgrade => "package_upgrade",
            PackageCachePrune => "package_cache_prune",
            GroupMembership => "group_membership",
            RadioState => "radio_state",
            Snapshot => "snapshot",
            Runtime => "runtime",
            Destructive => "destructive",
        })
    }
}

/// An effect declared by a primitive. `target` names the resource touched,
/// filled from the primitive's arguments.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectSpec {
    pub kind: EffectKind,
    pub target: Template,
}
