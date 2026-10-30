//! Transactions, capture/restore handlers, undo, drift and dependency checks.
//!
//! Every run of a plan is a transaction. Before each step, the state of each
//! resource it changes is captured; after it, the new state. The manifest is
//! rewritten atomically after every step, so a crash or failure leaves a
//! consistent record. Undo restores captured state and is itself a
//! transaction, so it can be undone too (redo).

pub mod state;

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use sancho_core::{EffectKind, Plan};
use sancho_exec::{Executor, Invocation};

use state::{read, resources, restore, State};

/// Root-owned backups of files sancho changes (never in the user's home).
pub const BACKUP_ROOT: &str = "/var/lib/sancho/backups";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TxKind {
    /// Ran a plan for a request.
    Do,
    /// Restored state captured by transaction `of`.
    Undo { of: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TxStatus {
    Running,
    Completed,
    /// Step `step` (0-based) failed; later steps did not run.
    Failed {
        step: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Done,
    Failed { exit: i32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectRecord {
    pub kind: EffectKind,
    pub target: String,
    pub before: Option<State>,
    pub after: Option<State>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRecord {
    pub explanation: String,
    pub argv: Vec<String>,
    pub root: bool,
    pub status: StepStatus,
    pub effects: Vec<EffectRecord>,
    /// The transaction that undid this step, if any.
    pub undone_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: String,
    /// Seconds since the Unix epoch.
    pub started: u64,
    pub request: String,
    #[serde(flatten)]
    pub kind: TxKind,
    pub recipe: Option<String>,
    pub summary: String,
    /// Resources and names this transaction touched, for search and linking.
    pub tags: BTreeSet<String>,
    pub steps: Vec<StepRecord>,
    #[serde(flatten)]
    pub status: TxStatus,
}

impl Transaction {
    /// Steps that ran and haven't been undone.
    pub fn live_steps(&self) -> impl Iterator<Item = (usize, &StepRecord)> {
        self.steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.status == StepStatus::Done && s.undone_by.is_none())
    }

    pub fn fully_undone(&self) -> bool {
        self.live_steps().next().is_none()
    }
}

#[derive(Debug)]
pub enum JournalError {
    Io(io::Error),
    Corrupt(PathBuf, String),
    UnknownTransaction(String),
    State(String),
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JournalError::Io(e) => write!(f, "journal: {e}"),
            JournalError::Corrupt(p, e) => {
                write!(f, "journal entry {} is unreadable: {e}", p.display())
            }
            JournalError::UnknownTransaction(id) => write!(f, "no transaction '{id}'"),
            JournalError::State(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for JournalError {}

impl From<io::Error> for JournalError {
    fn from(e: io::Error) -> Self {
        JournalError::Io(e)
    }
}

/// The on-disk journal: `<dir>/<id>/manifest.json`.
pub struct Journal {
    pub dir: PathBuf,
    /// Base directory for root-owned file backups.
    pub backups: String,
}

impl Journal {
    pub fn open(dir: impl Into<PathBuf>) -> Journal {
        Journal {
            dir: dir.into(),
            backups: BACKUP_ROOT.into(),
        }
    }

    /// `$XDG_STATE_HOME/sancho/journal`, defaulting to `~/.local/state`.
    pub fn default_dir() -> Option<PathBuf> {
        let state = std::env::var_os("XDG_STATE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/state")))?;
        Some(state.join("sancho/journal"))
    }

    pub fn save(&self, tx: &Transaction) -> Result<(), JournalError> {
        let dir = self.dir.join(&tx.id);
        fs::create_dir_all(&dir)?;
        let tmp = dir.join("manifest.json.tmp");
        let json = serde_json::to_vec_pretty(tx).map_err(|e| JournalError::State(e.to_string()))?;
        fs::write(&tmp, json)?;
        // Atomic replace: readers see the old or the new manifest, never half.
        fs::rename(&tmp, dir.join("manifest.json"))?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<Transaction, JournalError> {
        let path = self.dir.join(id).join("manifest.json");
        let bytes = fs::read(&path).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => JournalError::UnknownTransaction(id.to_string()),
            _ => JournalError::Io(e),
        })?;
        serde_json::from_slice(&bytes).map_err(|e| JournalError::Corrupt(path, e.to_string()))
    }

    /// All transactions, oldest first.
    pub fn list(&self) -> Result<Vec<Transaction>, JournalError> {
        let mut ids: Vec<String> = match fs::read_dir(&self.dir) {
            Ok(entries) => entries
                .flatten()
                .filter(|e| e.path().join("manifest.json").exists())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => vec![],
            Err(e) => return Err(e.into()),
        };
        ids.sort();
        ids.iter().map(|id| self.load(id)).collect()
    }

    /// A new, sortable transaction ID: `<unix seconds>-<n>`.
    fn new_id(&self) -> Result<(String, u64), JournalError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        for n in 0..1000 {
            let id = format!("{now:011}-{n:03}");
            if !self.dir.join(&id).exists() {
                return Ok((id, now));
            }
        }
        Err(JournalError::State(
            "too many transactions in one second".into(),
        ))
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Completed {
        tx: String,
    },
    /// Step `step` failed; steps before it completed and can be rolled back.
    Failed {
        tx: String,
        step: usize,
        exit: i32,
    },
}

/// Runs a plan step by step, journaling state before and after each.
/// `on_step` is called with each step's index before it runs.
pub fn execute(
    journal: &Journal,
    ex: &mut dyn Executor,
    plan: &Plan,
    request: &str,
    on_step: &mut dyn FnMut(usize),
) -> Result<RunOutcome, JournalError> {
    let (id, started) = journal.new_id()?;
    let mut tx = Transaction {
        id: id.clone(),
        started,
        request: request.to_string(),
        kind: TxKind::Do,
        recipe: plan.recipe_id.clone(),
        summary: plan.summary.clone(),
        tags: BTreeSet::new(),
        steps: plan
            .steps
            .iter()
            .map(|s| StepRecord {
                explanation: s.explanation.clone(),
                argv: s.argv.clone(),
                root: s.needs_root,
                status: StepStatus::Pending,
                effects: s
                    .effects
                    .iter()
                    .map(|e| EffectRecord {
                        kind: e.kind,
                        target: e.target.clone(),
                        before: None,
                        after: None,
                    })
                    .collect(),
                undone_by: None,
            })
            .collect(),
        status: TxStatus::Running,
    };
    journal.save(&tx)?;

    for (i, step) in plan.steps.iter().enumerate() {
        on_step(i);
        let inv = Invocation {
            argv: step.argv.clone(),
            stdin: step.stdin.clone(),
            root: step.needs_root,
            // Read-only steps exist to show something: give them the terminal.
            interactive: step.interactive || step.read_only,
            quiet: false,
        };
        let status = run_step(journal, ex, &mut tx, i, &inv)?;
        if status != 0 {
            tx.steps[i].status = StepStatus::Failed { exit: status };
            tx.status = TxStatus::Failed { step: i };
            journal.save(&tx)?;
            return Ok(RunOutcome::Failed {
                tx: id,
                step: i,
                exit: status,
            });
        }
    }
    tx.status = TxStatus::Completed;
    journal.save(&tx)?;
    Ok(RunOutcome::Completed { tx: id })
}

/// Captures before-state, runs one command, captures after-state.
/// Returns the command's exit status.
fn run_step(
    journal: &Journal,
    ex: &mut dyn Executor,
    tx: &mut Transaction,
    i: usize,
    inv: &Invocation,
) -> Result<i32, JournalError> {
    for (j, e) in tx.steps[i].effects.iter_mut().enumerate() {
        let backup = format!("{}/{}/{i}-{j}", journal.backups, tx.id);
        e.before = read(ex, e.kind, &e.target, Some(&backup))
            .map_err(|err| JournalError::State(err.to_string()))?;
    }
    journal.save(tx)?;
    let out = ex
        .run(inv)
        .map_err(|e| JournalError::State(format!("`{}`: {e}", inv.argv.join(" "))))?;
    if out.success() {
        tx.steps[i].status = StepStatus::Done;
    }
    // Record what changed even on failure, so a rollback knows the truth.
    for e in tx.steps[i].effects.iter_mut() {
        e.after = read(ex, e.kind, &e.target, None)
            .map_err(|err| JournalError::State(err.to_string()))?;
        let touched = resources(e.kind, &e.target, e.before.as_ref(), e.after.as_ref());
        tx.tags.extend(touched);
    }
    journal.save(tx)?;
    Ok(out.status)
}

/// One effect to put back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoAction {
    pub tx: String,
    pub step: usize,
    pub kind: EffectKind,
    pub target: String,
    pub restores: Vec<state::Restore>,
    /// The resource no longer looks the way this transaction left it.
    pub drift: Option<Drift>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    pub expected: String,
    pub found: String,
}

/// A preview of an undo, shown before confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UndoPlan {
    pub txs: Vec<String>,
    pub actions: Vec<UndoAction>,
    /// Steps that can't be undone, with the reason.
    pub cannot: Vec<String>,
    /// Later transactions that changed the same resources and aren't included.
    pub dependents: Vec<String>,
}

impl UndoPlan {
    pub fn has_drift(&self) -> bool {
        self.actions.iter().any(|a| a.drift.is_some())
    }
}

/// Plans undoing `txs` (all their live steps, or only `steps` when given),
/// last change first.
pub fn plan_undo(
    journal: &Journal,
    ex: &mut dyn Executor,
    txs: &[String],
    steps: Option<&[usize]>,
) -> Result<UndoPlan, JournalError> {
    let all = journal.list()?;
    let mut chosen: Vec<&Transaction> = Vec::new();
    for id in txs {
        let tx = all
            .iter()
            .find(|t| &t.id == id)
            .ok_or_else(|| JournalError::UnknownTransaction(id.clone()))?;
        chosen.push(tx);
    }
    chosen.sort_by(|a, b| b.id.cmp(&a.id));

    let mut plan = UndoPlan {
        txs: chosen.iter().map(|t| t.id.clone()).collect(),
        ..Default::default()
    };
    let mut touched: BTreeSet<String> = BTreeSet::new();
    for tx in &chosen {
        for (i, step) in tx.live_steps().collect::<Vec<_>>().into_iter().rev() {
            if steps.is_some_and(|s| !s.contains(&i)) {
                continue;
            }
            for e in step.effects.iter().rev() {
                touched.extend(resources(
                    e.kind,
                    &e.target,
                    e.before.as_ref(),
                    e.after.as_ref(),
                ));
                let (Some(before), Some(after)) = (&e.before, &e.after) else {
                    if matches!(
                        e.kind,
                        EffectKind::PackageUpgrade
                            | EffectKind::PackageCachePrune
                            | EffectKind::Destructive
                    ) {
                        plan.cannot.push(format!(
                            "{} ({})",
                            step.explanation,
                            cannot_reason(e.kind)
                        ));
                    }
                    continue;
                };
                let now = read(ex, e.kind, &e.target, None)
                    .map_err(|err| JournalError::State(err.to_string()))?
                    .expect("kinds with a before-state can be read");
                let drift = (!now.same_as(after)).then(|| Drift {
                    expected: after.describe(),
                    found: now.describe(),
                });
                match restore(ex, &e.target, before, &now) {
                    Ok(restores) => plan.actions.push(UndoAction {
                        tx: tx.id.clone(),
                        step: i,
                        kind: e.kind,
                        target: e.target.clone(),
                        restores,
                        drift,
                    }),
                    Err(reason) => plan.cannot.push(format!("{}: {reason}", step.explanation)),
                }
            }
        }
    }

    // Anything after the oldest transaction being undone may depend on it.
    let oldest = chosen.iter().map(|t| t.id.as_str()).min().unwrap_or("");
    for later in all
        .iter()
        .filter(|t| t.id.as_str() > oldest && !txs.contains(&t.id))
    {
        let live: BTreeSet<String> = later
            .live_steps()
            .flat_map(|(_, s)| &s.effects)
            .flat_map(|e| resources(e.kind, &e.target, e.before.as_ref(), e.after.as_ref()))
            .collect();
        if !live.is_disjoint(&touched) {
            plan.dependents.push(later.id.clone());
        }
    }
    Ok(plan)
}

fn cannot_reason(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::PackageUpgrade => "an upgrade can only be undone by rolling back a snapshot",
        EffectKind::PackageCachePrune => "deleted cache files are gone",
        _ => "this change is irreversible",
    }
}

/// Runs an undo plan as a new transaction. With `skip_drifted`, resources
/// changed since the original transaction are left alone.
pub fn apply_undo(
    journal: &Journal,
    ex: &mut dyn Executor,
    plan: &UndoPlan,
    request: &str,
    skip_drifted: bool,
) -> Result<RunOutcome, JournalError> {
    let (id, started) = journal.new_id()?;
    let actions: Vec<&UndoAction> = plan
        .actions
        .iter()
        .filter(|a| !(skip_drifted && a.drift.is_some()))
        .collect();
    let mut tx = Transaction {
        id: id.clone(),
        started,
        request: request.to_string(),
        kind: TxKind::Undo {
            of: plan.txs.join(","),
        },
        recipe: None,
        summary: format!("Undo of {}", plan.txs.join(", ")),
        tags: BTreeSet::new(),
        steps: Vec::new(),
        status: TxStatus::Running,
    };
    let mut invocations = Vec::new();
    for a in &actions {
        for r in &a.restores {
            tx.steps.push(StepRecord {
                explanation: r.explanation.clone(),
                argv: r.invocation.argv.clone(),
                root: r.invocation.root,
                status: StepStatus::Pending,
                effects: vec![EffectRecord {
                    kind: a.kind,
                    target: a.target.clone(),
                    before: None,
                    after: None,
                }],
                undone_by: None,
            });
            invocations.push(r.invocation.clone());
        }
    }
    journal.save(&tx)?;

    for (i, inv) in invocations.iter().enumerate() {
        let status = run_step(journal, ex, &mut tx, i, inv)?;
        if status != 0 {
            tx.steps[i].status = StepStatus::Failed { exit: status };
            tx.status = TxStatus::Failed { step: i };
            journal.save(&tx)?;
            return Ok(RunOutcome::Failed {
                tx: id,
                step: i,
                exit: status,
            });
        }
    }
    tx.status = TxStatus::Completed;
    journal.save(&tx)?;

    // Mark what was undone on the original transactions.
    for a in &actions {
        let mut orig = journal.load(&a.tx)?;
        orig.steps[a.step].undone_by = Some(id.clone());
        journal.save(&orig)?;
    }
    Ok(RunOutcome::Completed { tx: id })
}

/// The most recent transaction that still has live steps; `sancho undo`'s default.
pub fn last_undoable(journal: &Journal) -> Result<Option<Transaction>, JournalError> {
    Ok(journal
        .list()?
        .into_iter()
        .rev()
        .find(|t| t.live_steps().next().is_some()))
}
