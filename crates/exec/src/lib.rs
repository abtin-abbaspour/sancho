//! Executor trait and implementations (sudo, mock, container). argv only.
//!
//! Commands are argument vectors. Nothing is ever passed through a shell.
//! Root commands get `sudo --` prepended (or run as root in a container);
//! interactive commands inherit the terminal so prompts reach the user.

mod container;
mod mock;

use std::io::{self, Write};
use std::process::{Command, Stdio};

pub use container::ContainerExecutor;
pub use mock::{MockExecutor, MockState};

/// One command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub argv: Vec<String>,
    pub stdin: Option<String>,
    pub root: bool,
    /// Inherit the terminal (prompts, progress) instead of capturing output.
    pub interactive: bool,
    /// A state query: stderr is discarded (a missing file is an answer, not
    /// an error to show the user).
    pub quiet: bool,
}

impl Invocation {
    /// A non-interactive command whose output is captured.
    pub fn query(argv: &[&str], root: bool) -> Invocation {
        Invocation {
            argv: argv.iter().map(|s| s.to_string()).collect(),
            stdin: None,
            root,
            interactive: false,
            quiet: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    /// Exit status; -1 if killed by a signal.
    pub status: i32,
    /// Captured stdout (empty for interactive commands).
    pub stdout: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status == 0
    }
}

pub trait Executor {
    fn run(&mut self, inv: &Invocation) -> io::Result<Output>;
}

/// Runs on this machine, escalating root commands with `sudo`.
#[derive(Debug, Default)]
pub struct SudoExecutor;

impl Executor for SudoExecutor {
    fn run(&mut self, inv: &Invocation) -> io::Result<Output> {
        let mut argv: Vec<&str> = Vec::new();
        if inv.root {
            argv.extend(["sudo", "--"]);
        }
        argv.extend(inv.argv.iter().map(String::as_str));
        run_argv(&argv, inv)
    }
}

/// Spawns `argv` directly (no shell) with the invocation's stdin and mode.
pub(crate) fn run_argv(argv: &[&str], inv: &Invocation) -> io::Result<Output> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
    let mut cmd = Command::new(program);
    cmd.args(args);
    if !inv.interactive {
        // Stable, untranslated output for parsing.
        cmd.env("LC_ALL", "C");
        cmd.stdout(Stdio::piped());
    }
    if inv.quiet {
        cmd.stderr(Stdio::null());
    }
    cmd.stdin(if inv.stdin.is_some() {
        Stdio::piped()
    } else if inv.interactive {
        Stdio::inherit()
    } else {
        Stdio::null()
    });
    let mut child = cmd.spawn()?;
    if let Some(input) = &inv.stdin {
        let mut stdin = child.stdin.take().expect("stdin is piped");
        stdin.write_all(input.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    Ok(Output {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
    })
}
