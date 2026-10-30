//! Replaces personal details in text meant for sharing: usernames,
//! hostnames, home paths, IP and MAC addresses, and Wi-Fi network names.

use std::collections::BTreeMap;

/// Names known to be personal on this machine.
#[derive(Debug, Clone, Default)]
pub struct Secrets {
    pub users: Vec<String>,
    pub hostnames: Vec<String>,
    pub ssids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scrubbed {
    pub text: String,
    /// Placeholder → how many times it was used.
    pub replaced: BTreeMap<&'static str, usize>,
}

pub fn scrub(text: &str, secrets: &Secrets) -> Scrubbed {
    let mut replaced = BTreeMap::new();
    let mut text = home_paths(text, &mut replaced);
    text = addresses(&text, &mut replaced);
    let mut known: Vec<(&str, &'static str)> = Vec::new();
    known.extend(secrets.ssids.iter().map(|s| (s.as_str(), "<ssid>")));
    known.extend(secrets.hostnames.iter().map(|s| (s.as_str(), "<hostname>")));
    known.extend(secrets.users.iter().map(|s| (s.as_str(), "<user>")));
    // Longest first, so "alice-laptop" is replaced before "alice".
    known.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
    for (secret, placeholder) in known {
        if secret.is_empty() {
            continue;
        }
        let (t, n) = replace_words(&text, secret, placeholder);
        if n > 0 {
            *replaced.entry(placeholder).or_default() += n;
        }
        text = t;
    }
    Scrubbed { text, replaced }
}

/// `/home/alice/x` → `~/x`.
fn home_paths(text: &str, replaced: &mut BTreeMap<&'static str, usize>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("/home/") {
        out.push_str(&rest[..i]);
        let after = &rest[i + "/home/".len()..];
        let name_len = after
            .find(|c: char| !(c.is_alphanumeric() || "._-".contains(c)))
            .unwrap_or(after.len());
        if name_len == 0 {
            out.push_str("/home/");
            rest = after;
            continue;
        }
        out.push('~');
        *replaced.entry("~").or_default() += 1;
        rest = &after[name_len..];
    }
    out.push_str(rest);
    out
}

/// Replaces MAC, IPv4 and IPv6 addresses found in runs of address characters.
fn addresses(text: &str, replaced: &mut BTreeMap<&'static str, usize>) -> String {
    let is_addr_char = |c: char| c.is_ascii_hexdigit() || ":.-".contains(c);
    let mut out = String::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if !is_addr_char(c) {
            out.push(c);
            continue;
        }
        let mut end = start + c.len_utf8();
        while let Some(&(i, c)) = chars.peek() {
            if !is_addr_char(c) {
                break;
            }
            end = i + c.len_utf8();
            chars.next();
        }
        let run = &text[start..end];
        // Must not be glued to a word ("lib32-mesa" starts mid-word).
        let glued = text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
            || text[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let trimmed = run.trim_end_matches(['.', ':', '-']);
        let tail = &run[trimmed.len()..];
        let kind = if glued {
            None
        } else if is_mac(trimmed) {
            Some("<mac>")
        } else if is_ipv4(trimmed) || is_ipv6(trimmed) {
            Some("<ip>")
        } else {
            None
        };
        match kind {
            Some(p) => {
                out.push_str(p);
                out.push_str(tail);
                *replaced.entry(p).or_default() += 1;
            }
            None => out.push_str(run),
        }
    }
    out
}

fn is_mac(s: &str) -> bool {
    let sep = if s.contains(':') { ':' } else { '-' };
    let groups: Vec<&str> = s.split(sep).collect();
    groups.len() == 6
        && groups
            .iter()
            .all(|g| g.len() == 2 && g.chars().all(|c| c.is_ascii_hexdigit()))
}

fn is_ipv4(s: &str) -> bool {
    let groups: Vec<&str> = s.split('.').collect();
    groups.len() == 4
        && groups
            .iter()
            .all(|g| !g.is_empty() && g.len() <= 3 && g.parse::<u8>().is_ok())
}

fn is_ipv6(s: &str) -> bool {
    if !s.contains(':') || s.contains('.') || s.contains('-') {
        return false;
    }
    let groups: Vec<&str> = s.split(':').collect();
    let hex_ok = groups
        .iter()
        .all(|g| g.len() <= 4 && g.chars().all(|c| c.is_ascii_hexdigit()));
    let compressed = s.contains("::") && s.matches("::").count() == 1 && groups.len() <= 8;
    hex_ok && (compressed || groups.len() == 8)
}

/// Replaces `word` where it isn't part of a longer word.
fn replace_words(text: &str, word: &str, with: &str) -> (String, usize) {
    let part_of_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut count = 0;
    let mut i = 0;
    while let Some(pos) = text[i..].find(word) {
        let start = i + pos;
        let end = start + word.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        out.push_str(&text[i..start]);
        if before.is_some_and(part_of_word) || after.is_some_and(part_of_word) {
            out.push_str(word);
        } else {
            out.push_str(with);
            count += 1;
        }
        i = end;
    }
    out.push_str(&text[i..]);
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets() -> Secrets {
        Secrets {
            users: vec!["alice".into(), "bob".into()],
            hostnames: vec!["alice-laptop".into()],
            ssids: vec!["Home Sweet Home".into(), "CoffeeShop".into()],
        }
    }

    fn s(text: &str) -> String {
        scrub(text, &secrets()).text
    }

    #[test]
    fn usernames_are_replaced_as_whole_words() {
        assert_eq!(
            s("add alice to video; bob too"),
            "add <user> to video; <user> too"
        );
        assert_eq!(s("malice and alicex stay"), "malice and alicex stay");
    }

    #[test]
    fn hostnames_win_over_the_usernames_inside_them() {
        assert_eq!(s("ssh alice-laptop"), "ssh <hostname>");
    }

    #[test]
    fn home_paths_become_tilde() {
        assert_eq!(
            s("/home/alice/.config/x and /home/carol"),
            "~/.config/x and ~"
        );
        assert_eq!(s("/homework/ok /home/"), "/homework/ok /home/");
    }

    #[test]
    fn ip_addresses_are_replaced() {
        assert_eq!(s("from 192.168.1.20 to 10.0.0.1."), "from <ip> to <ip>.");
        assert_eq!(s("fe80::1c2b:3cff:fe4d:5e6f%wlan0"), "<ip>%wlan0");
        assert_eq!(s("2001:db8:0:0:0:0:2:1 ::1"), "<ip> <ip>");
        assert_eq!(s("999.1.1.1 1.2.3"), "999.1.1.1 1.2.3", "not addresses");
    }

    #[test]
    fn mac_addresses_are_replaced() {
        assert_eq!(s("dev aa:bb:cc:dd:ee:0f up"), "dev <mac> up");
        assert_eq!(s("AA-BB-CC-DD-EE-FF"), "<mac>");
    }

    #[test]
    fn ssids_are_replaced() {
        assert_eq!(
            s("connect to Home Sweet Home or CoffeeShop"),
            "connect to <ssid> or <ssid>"
        );
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        for t in [
            "keychron 3434:0121 and 046d:c52b",
            "vlc 3.0.21-8 on 2026-10-30 at 12:30",
            "lib32-mesa sshd.service fstrim.timer",
            "0003:0000046D:0000C52B",
        ] {
            assert_eq!(s(t), t);
        }
    }

    #[test]
    fn counts_are_reported() {
        let r = scrub("alice at 10.0.0.1 and 10.0.0.2", &secrets());
        assert_eq!(r.replaced.get("<user>"), Some(&1));
        assert_eq!(r.replaced.get("<ip>"), Some(&2));
    }
}
