//! `sancho learned list | rm | export`.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use sancho_core::{LiveSystem, Query, System};
use sancho_learn::scrub::{scrub, Secrets};
use sancho_learn::{export_text, Store};

use crate::ask;

pub fn open() -> Result<Store, String> {
    let path = Store::default_path()
        .ok_or("can't find the learned store: neither $XDG_DATA_HOME nor $HOME is set")?;
    Store::open(path).map_err(|e| format!("learned store: {e}"))
}

pub fn list() -> ExitCode {
    let store = match open() {
        Ok(s) => s,
        Err(e) => return fail(&e),
    };
    if store.items().is_empty() {
        println!("Nothing learned yet. sancho remembers your answers when it has to ask.");
    }
    for i in store.items() {
        println!("{:>4}  {}  (used {} times)", i.id, i.describe(), i.uses);
    }
    ExitCode::SUCCESS
}

pub fn remove(id: u32) -> ExitCode {
    let mut store = match open() {
        Ok(s) => s,
        Err(e) => return fail(&e),
    };
    match store.remove(id) {
        Ok(true) => {
            println!("Forgot {id}.");
            ExitCode::SUCCESS
        }
        Ok(false) => fail(&format!(
            "nothing learned has ID {id}; see `sancho learned list`"
        )),
        Err(e) => fail(&e.to_string()),
    }
}

pub fn export(id: u32, output: Option<PathBuf>) -> ExitCode {
    let store = match open() {
        Ok(s) => s,
        Err(e) => return fail(&e),
    };
    let Some(item) = store.get(id) else {
        return fail(&format!(
            "nothing learned has ID {id}; see `sancho learned list`"
        ));
    };
    let text = export_text(item, env!("CARGO_PKG_VERSION"));
    let scrubbed = scrub(&text, &secrets(&LiveSystem));
    let path = output.unwrap_or_else(|| PathBuf::from(format!("sancho-learned-{id}.toml")));

    println!("This exact text would be written to {}:\n", path.display());
    for line in scrubbed.text.lines() {
        println!("  | {line}");
    }
    if !scrubbed.replaced.is_empty() {
        let what: Vec<String> = scrubbed
            .replaced
            .iter()
            .map(|(p, n)| format!("{p} ×{n}"))
            .collect();
        println!("\nReplaced personal details: {}", what.join(", "));
    }
    println!();
    if path.exists() {
        return fail(&format!(
            "{} already exists; choose another --output",
            path.display()
        ));
    }
    if !io::stdin().is_terminal() {
        return fail("writing the export needs confirmation in a terminal");
    }
    let ok = ask::confirm(
        &mut io::stdin().lock(),
        &mut io::stdout(),
        sancho_core::plan::Confirmation::YesNo,
    )
    .unwrap_or(false);
    if !ok {
        println!("Cancelled; nothing was written.");
        return ExitCode::SUCCESS;
    }
    match std::fs::write(&path, &scrubbed.text) {
        Ok(()) => {
            println!("Wrote {}. Nothing was sent anywhere.", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("{}: {e}", path.display())),
    }
}

/// Personal names on this machine: login users, hostname, saved Wi-Fi names.
fn secrets(sys: &dyn System) -> Secrets {
    let mut users: Vec<String> = sys
        .read("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            let uid: u32 = f.get(2)?.parse().ok()?;
            (1000..60000).contains(&uid).then(|| f[0].to_string())
        })
        .collect();
    users.extend(sys.env("USER"));
    let hostnames: Vec<String> = sys
        .read("/etc/hostname")
        .into_iter()
        .chain(sys.env("HOSTNAME"))
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .collect();
    let ssids = sys
        .query(Query::NmConnections)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.rsplit_once(':'))
        .filter(|(_, ty)| *ty == "802-11-wireless")
        .map(|(name, _)| name.replace("\\:", ":"))
        .collect();
    Secrets {
        users,
        hostnames,
        ssids,
    }
}

fn fail(msg: &str) -> ExitCode {
    let _ = writeln!(io::stderr(), "sancho: {msg}");
    ExitCode::FAILURE
}
