//! `sancho history`, `sancho undo`, `sancho redo`.

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use sancho_core::style::{stdout_painter, Tone};
use sancho_exec::{Executor, SudoExecutor};
use sancho_journal::{
    apply_undo, plan_undo, Journal, JournalError, RunOutcome, Transaction, TxKind, TxStatus,
    UndoPlan,
};
use sancho_plan::render::shell_words;

use crate::ask;

pub fn journal() -> Result<Journal, String> {
    Journal::default_dir()
        .map(Journal::open)
        .ok_or_else(|| "can't find the journal: neither $XDG_STATE_HOME nor $HOME is set".into())
}

pub fn history() -> ExitCode {
    let txs = match journal().and_then(|j| j.list().map_err(|e| e.to_string())) {
        Ok(t) => t,
        Err(e) => return fail(&e),
    };
    if txs.is_empty() {
        crate::ui::info("Nothing has been done yet.");
    }
    for t in &txs {
        let what = match &t.kind {
            TxKind::Do => t.request.clone(),
            TxKind::Undo { of } => format!("undo of {of}"),
        };
        let (tone, status) = match (&t.status, t.fully_undone()) {
            (TxStatus::Failed { step }, _) => (Tone::Bad, format!("failed at step {}", step + 1)),
            (TxStatus::Running, _) => (Tone::Warn, "interrupted".into()),
            (TxStatus::Completed, true) => (Tone::Muted, "undone".into()),
            (TxStatus::Completed, false) => (Tone::Good, "done".into()),
        };
        let paint = stdout_painter();
        println!(
            "{}  {}  {} {what}",
            paint.paint(Tone::Muted, &t.id),
            paint.paint(Tone::Muted, &when(t.started)),
            paint.paint(tone, &format!("{status:<16}"))
        );
    }
    ExitCode::SUCCESS
}

/// Which transactions an undo is about.
pub enum Target {
    Last,
    /// The last `n` undoable transactions.
    Count(usize),
    /// Undoable transactions started in the last `secs` seconds.
    Since(u64),
    /// A transaction ID, or words matched against requests and resources.
    Search(String),
    /// The most recent undo (redo).
    Redo,
}

pub fn undo(target: Target, steps: &[usize], dry_run: bool) -> ExitCode {
    let j = match journal() {
        Ok(j) => j,
        Err(e) => return fail(&e),
    };
    let mut ex = SudoExecutor;
    match run_undo(&j, &mut ex, target, steps, dry_run) {
        Ok(code) => code,
        Err(e) => fail(&e),
    }
}

fn run_undo(
    j: &Journal,
    ex: &mut dyn Executor,
    target: Target,
    steps: &[usize],
    dry_run: bool,
) -> Result<ExitCode, String> {
    let err = |e: JournalError| e.to_string();
    let undoable: Vec<Transaction> = j
        .list()
        .map_err(err)?
        .into_iter()
        .filter(|t| t.live_steps().next().is_some())
        .collect();
    let chosen: Vec<&Transaction> = match &target {
        Target::Last => undoable.last().into_iter().collect(),
        Target::Count(n) => undoable.iter().rev().take(*n).collect(),
        Target::Since(secs) => {
            let cutoff = now().saturating_sub(*secs);
            undoable.iter().filter(|t| t.started >= cutoff).collect()
        }
        Target::Redo => undoable
            .iter()
            .rev()
            .find(|t| matches!(t.kind, TxKind::Undo { .. }))
            .into_iter()
            .collect(),
        Target::Search(words) => search(&undoable, words)?,
    };
    if chosen.is_empty() {
        crate::ui::info("Nothing to undo.");
        return Ok(ExitCode::SUCCESS);
    }
    let mut ids: Vec<String> = chosen.iter().map(|t| t.id.clone()).collect();
    // 1-based step numbers from the user; partial undo needs one transaction.
    let steps: Option<Vec<usize>> =
        (!steps.is_empty()).then(|| steps.iter().map(|s| s.saturating_sub(1)).collect());
    if steps.is_some() && ids.len() != 1 {
        return Err("--step needs exactly one transaction".into());
    }

    let mut plan = plan_undo(j, ex, &ids, steps.as_deref()).map_err(err)?;
    if !plan.dependents.is_empty() {
        let all = j.list().map_err(err)?;
        let later: Vec<String> = plan
            .dependents
            .iter()
            .filter_map(|d| all.iter().find(|t| &t.id == d))
            .map(|t| format!("{} ({})", t.id, t.request))
            .collect();
        println!(
            "Later changes touched the same things:\n  {}",
            later.join("\n  ")
        );
        let options = vec![
            "Undo those too".to_string(),
            "Undo only what I asked (the later changes may stop making sense)".to_string(),
        ];
        match choose("What should happen?", &options)? {
            Some(0) => {
                ids.extend(plan.dependents.clone());
                plan = plan_undo(j, ex, &ids, None).map_err(err)?;
            }
            Some(_) => {}
            None => return cancelled(),
        }
    }

    print!("{}", preview(&plan, &chosen));
    if plan.actions.iter().all(|a| a.restores.is_empty()) {
        crate::ui::info("There is nothing to put back.");
        return Ok(ExitCode::SUCCESS);
    }
    if dry_run {
        return Ok(ExitCode::SUCCESS);
    }
    let mut skip_drifted = false;
    if plan.has_drift() {
        let options = vec![
            "Restore anyway (overwrite the later changes)".to_string(),
            "Leave the changed items as they are".to_string(),
        ];
        match choose(
            "Some things changed after sancho touched them. What should happen?",
            &options,
        )? {
            Some(0) => {}
            Some(_) => skip_drifted = true,
            None => return cancelled(),
        }
    }
    if !confirm()? {
        return cancelled();
    }
    let request = format!("undo {}", ids.join(" "));
    match apply_undo(j, ex, &plan, &request, skip_drifted).map_err(err)? {
        RunOutcome::Completed { tx } => {
            crate::ui::done(&format!(
                "Undone. This undo is {tx}; `sancho redo` reverses it."
            ));
            Ok(ExitCode::SUCCESS)
        }
        RunOutcome::Failed { tx, step, exit } => {
            println!(
                "Step {} of the undo failed (exit {exit}); see `sancho history` ({tx}).",
                step + 1
            );
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Transactions whose ID equals `words`, or whose request, summary or
/// resources contain every word.
fn search<'a>(txs: &'a [Transaction], words: &str) -> Result<Vec<&'a Transaction>, String> {
    if let Some(t) = txs.iter().find(|t| t.id == words) {
        return Ok(vec![t]);
    }
    let words: Vec<String> = words.split_whitespace().map(str::to_lowercase).collect();
    let hits: Vec<&Transaction> = txs
        .iter()
        .filter(|t| {
            let hay = format!(
                "{} {} {}",
                t.request,
                t.summary,
                t.tags.iter().cloned().collect::<Vec<_>>().join(" ")
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect();
    match hits.len() {
        0 => Err(format!(
            "nothing in the history matches \"{}\"",
            words.join(" ")
        )),
        1 => Ok(hits),
        _ => {
            let labels: Vec<String> = hits
                .iter()
                .rev()
                .map(|t| format!("{} {}", when(t.started), t.request))
                .collect();
            match choose("Which one?", &labels)? {
                Some(i) => Ok(vec![hits[hits.len() - 1 - i]]),
                None => Ok(vec![]),
            }
        }
    }
}

fn preview(plan: &UndoPlan, chosen: &[&Transaction]) -> String {
    let paint = stdout_painter();
    let mut out = String::new();
    let names: Vec<String> = chosen
        .iter()
        .map(|t| format!("\"{}\" ({})", t.request, t.id))
        .collect();
    out.push_str(&format!(
        "{}\n\n",
        paint.paint(Tone::Title, &format!("Undo {}:", names.join(", ")))
    ));
    let mut n = 0;
    for a in &plan.actions {
        if let Some(d) = &a.drift {
            out.push_str(&format!(
                "  {}\n",
                paint.paint(
                    Tone::Warn,
                    &format!(
                        "! {} changed since: expected {}, found {}",
                        a.target, d.expected, d.found
                    )
                )
            ));
        }
        for r in &a.restores {
            n += 1;
            let sudo = if r.invocation.root { "sudo " } else { "" };
            out.push_str(&format!(
                "  {n}. {}\n     {} {}\n",
                r.explanation,
                paint.paint(Tone::Muted, "$"),
                paint.paint(
                    Tone::Command,
                    &format!("{sudo}{}", shell_words(&r.invocation.argv))
                )
            ));
        }
    }
    if !plan.cannot.is_empty() {
        out.push_str(&format!("\n{}\n", paint.paint(Tone::Bad, "Can't undo:")));
        for c in &plan.cannot {
            out.push_str(&format!("  - {c}\n"));
        }
    }
    out.push('\n');
    out
}

fn choose(question: &str, options: &[String]) -> Result<Option<usize>, String> {
    if !io::stdin().is_terminal() {
        return Err(format!(
            "{question} (needs an answer; run it in a terminal)"
        ));
    }
    ask::choose(
        &mut io::stdin().lock(),
        &mut io::stdout(),
        sancho_core::style::stdout_painter().as_ref(),
        question,
        options,
    )
    .map_err(|e| e.to_string())
}

fn confirm() -> Result<bool, String> {
    if !io::stdin().is_terminal() {
        return Err("confirmation needs a terminal; nothing was changed".into());
    }
    ask::confirm(
        &mut io::stdin().lock(),
        &mut io::stdout(),
        sancho_core::style::stdout_painter().as_ref(),
        sancho_core::plan::Confirmation::YesNo,
    )
    .map_err(|e| e.to_string())
}

fn cancelled() -> Result<ExitCode, String> {
    crate::ui::info("Cancelled; nothing was changed.");
    Ok(ExitCode::SUCCESS)
}

fn fail(msg: &str) -> ExitCode {
    let _ = writeln!(io::stderr(), "sancho: {msg}");
    ExitCode::FAILURE
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `30m`, `2h`, `3d`, `today` → seconds back from now.
pub fn parse_since(s: &str) -> Result<u64, String> {
    if s == "today" {
        return Ok(now() % 86_400);
    }
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = num
        .parse()
        .map_err(|_| format!("'{s}' is not a duration like 30m, 2h, 3d or today"))?;
    let mult = match unit {
        "m" | "min" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => return Err(format!("'{s}' is not a duration like 30m, 2h, 3d or today")),
    };
    Ok(n * mult)
}

/// `2026-10-29 14:05 UTC` from seconds since the epoch.
pub fn when(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3_600,
        rem % 3_600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_durations() {
        assert_eq!(when(0), "1970-01-01 00:00 UTC");
        assert_eq!(when(1_793_319_897), "2026-10-30 00:24 UTC");
        assert_eq!(when(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(parse_since("30m"), Ok(1_800));
        assert_eq!(parse_since("2h"), Ok(7_200));
        assert_eq!(parse_since("3d"), Ok(259_200));
        assert!(parse_since("soon").is_err());
        assert!(parse_since("h").is_err());
    }
}
