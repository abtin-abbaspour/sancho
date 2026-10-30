//! The `sancho` binary.

use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use sancho_core::LiveSystem;
use sancho_profile::detect;

/// Turn plain-language requests into verified, reversible Arch Linux tasks.
#[derive(Parser, Debug)]
#[command(name = "sancho", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show what sancho detected about this system.
    Profile,
}

const ROOT_REFUSAL: &str = "sancho: refusing to run as root.\n\
Run sancho as your normal user; it asks for sudo for each command that needs it \
and shows you the exact command first.";

/// sancho never runs as root; it escalates per command instead.
fn check_not_root(euid: u32) -> Result<(), &'static str> {
    if euid == 0 {
        Err(ROOT_REFUSAL)
    } else {
        Ok(())
    }
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid() has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

fn main() -> ExitCode {
    // clap handles --help/--version (and usage errors) inside parse() and exits,
    // so those are allowed as root. Everything else is checked before it runs.
    let cli = Cli::parse();

    if let Err(msg) = check_not_root(effective_uid()) {
        eprintln!("{msg}");
        return ExitCode::FAILURE;
    }

    match cli.command {
        None => {
            let _ = Cli::command().print_help();
        }
        Some(Command::Profile) => print_profile(),
    }
    ExitCode::SUCCESS
}

fn print_profile() {
    let profile = detect(&LiveSystem);
    for (key, value) in profile.iter() {
        println!("{:<14}{value}", key.name());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_refused() {
        assert_eq!(check_not_root(0), Err(ROOT_REFUSAL));
    }

    #[test]
    fn normal_users_are_allowed() {
        assert_eq!(check_not_root(1000), Ok(()));
        assert_eq!(check_not_root(1), Ok(()));
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
