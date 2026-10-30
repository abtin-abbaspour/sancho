//! Programs related to a request, from the system's man pages (`man -k`):
//! when no recipe fits, these are offered so the user still gets somewhere.

use sancho_core::{Query, System};

/// Words that don't say what a request is about.
const COMMON: &[&str] = &[
    "about", "all", "and", "any", "are", "can", "check", "computer", "does", "find", "for", "from",
    "get", "have", "how", "into", "know", "make", "need", "out", "please", "set", "show", "some",
    "tell", "that", "the", "there", "this", "use", "using", "want", "what", "when", "where",
    "which", "who", "why", "will", "with", "you", "your", "mine", "linux", "system", "terminal",
    "command",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    pub program: String,
    pub description: String,
}

/// Up to `max` programs (man sections 1 and 8) whose name or description
/// mention the request's words, best match first.
pub fn related_programs(sys: &dyn System, request: &str, max: usize) -> Vec<Related> {
    let words: Vec<String> = request
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter(|w| w.len() >= 3 && !COMMON.contains(w))
        .map(String::from)
        .take(5)
        .collect();
    let mut found: Vec<(usize, Related)> = Vec::new();
    for word in &words {
        let Some(out) = sys.query(Query::ManSearch(word.clone())) else {
            continue;
        };
        // "iw (8)               - show / manipulate wireless devices ..."
        for line in out.lines() {
            let Some((head, description)) = line.split_once(" - ") else {
                continue;
            };
            let mut head = head.split_whitespace();
            let (Some(program), Some(section)) = (head.next(), head.next()) else {
                continue;
            };
            let section = section.trim_matches(|c| c == '(' || c == ')');
            if !(section.starts_with('1') || section.starts_with('8')) {
                continue;
            }
            if found.iter().any(|(_, r)| r.program == program) {
                continue;
            }
            let hay = format!("{program} {description}").to_lowercase();
            let score = words.iter().filter(|w| hay.contains(w.as_str())).count();
            found.push((
                score,
                Related {
                    program: program.to_string(),
                    description: description.trim().to_string(),
                },
            ));
        }
    }
    found.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.program.len().cmp(&b.1.program.len()))
    });
    found.into_iter().take(max).map(|(_, r)| r).collect()
}
