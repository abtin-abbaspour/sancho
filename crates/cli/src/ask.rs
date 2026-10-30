//! Questions and confirmations on the terminal. Generic over input/output so
//! they can be tested without one.

use std::io::{self, BufRead, Write};

use sancho_core::plan::Confirmation;

/// Asks the user to pick one of `options` by number. `None` means cancelled
/// (empty answer, `q`, or end of input).
pub fn choose(
    input: &mut impl BufRead,
    out: &mut impl Write,
    question: &str,
    options: &[String],
) -> io::Result<Option<usize>> {
    writeln!(out, "{question}")?;
    for (i, o) in options.iter().enumerate() {
        writeln!(out, "  {}) {o}", i + 1)?;
    }
    loop {
        write!(out, "Choose 1-{} (or q to cancel): ", options.len())?;
        out.flush()?;
        let Some(answer) = read_line(input)? else {
            return Ok(None);
        };
        match answer.as_str() {
            "" | "q" | "quit" => return Ok(None),
            a => match a.parse::<usize>() {
                Ok(n) if (1..=options.len()).contains(&n) => return Ok(Some(n - 1)),
                _ => writeln!(out, "Please answer with a number from the list.")?,
            },
        }
    }
}

/// Asks for confirmation as the plan requires. Anything but an explicit yes
/// is a no.
pub fn confirm(
    input: &mut impl BufRead,
    out: &mut impl Write,
    how: Confirmation,
) -> io::Result<bool> {
    let prompt = match how {
        Confirmation::None => return Ok(true),
        Confirmation::YesNo => "Proceed? [y/N] ",
        Confirmation::TypeYes => "This is high risk. Type yes to proceed: ",
    };
    write!(out, "{prompt}")?;
    out.flush()?;
    let answer = read_line(input)?.unwrap_or_default();
    Ok(match how {
        Confirmation::TypeYes => answer == "yes",
        _ => matches!(answer.as_str(), "y" | "yes"),
    })
}

fn read_line(input: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim().to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Vec<String> {
        vec!["Chromium".into(), "Brave".into()]
    }

    #[test]
    fn choose_accepts_a_listed_number_and_retries_otherwise() {
        let mut out = Vec::new();
        let r = choose(
            &mut "7\nx\n2\n".as_bytes(),
            &mut out,
            "Which browser?",
            &opts(),
        )
        .unwrap();
        assert_eq!(r, Some(1));
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("Which browser?\n  1) Chromium\n  2) Brave\n"));
        assert_eq!(text.matches("Please answer with a number").count(), 2);
    }

    #[test]
    fn choose_can_be_cancelled() {
        for input in ["\n", "q\n", ""] {
            let r = choose(&mut input.as_bytes(), &mut Vec::new(), "?", &opts()).unwrap();
            assert_eq!(r, None, "{input:?}");
        }
    }

    #[test]
    fn high_risk_needs_the_word_yes() {
        let c = |input: &str, how| confirm(&mut input.as_bytes(), &mut Vec::new(), how).unwrap();
        assert!(c("yes\n", Confirmation::TypeYes));
        assert!(c("YES\n", Confirmation::TypeYes));
        assert!(!c("y\n", Confirmation::TypeYes));
        assert!(c("y\n", Confirmation::YesNo));
        assert!(!c("\n", Confirmation::YesNo));
        assert!(!c("", Confirmation::YesNo), "end of input is a no");
        assert!(c("", Confirmation::None), "read-only plans need no answer");
    }
}
