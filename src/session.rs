use crate::protocol::shell_quote;
use anyhow::{anyhow, Result};
use serde_json::Value;

/// Strip ANSI CSI escape sequences so text/JSON parses cleanly.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Parse the agent session id from a captured dispatch log.
///
/// codex prints a plain-text `session id: <id>` line; claude and cursor emit
/// structured JSON/NDJSON whose events carry the id under a known key. Returns
/// the first id found, or None (caller fails closed on None).
pub fn extract_session_id(vendor: &str, log: &str) -> Option<String> {
    match vendor {
        "codex" => extract_codex(log),
        "claude" => extract_json_key(log, &["session_id"]),
        "cursor" => extract_json_key(
            log,
            &["chatId", "chat_id", "sessionId", "session_id", "threadId"],
        ),
        _ => None,
    }
}

fn extract_codex(log: &str) -> Option<String> {
    let clean = strip_ansi(log);
    for line in clean.lines() {
        let lower = line.to_lowercase();
        if let Some(pos) = lower.find("session id:") {
            let after = &lower[pos + "session id:".len()..];
            if let Some(token) = after.split_whitespace().next() {
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }
    None
}

fn extract_json_key(log: &str, keys: &[&str]) -> Option<String> {
    let clean = strip_ansi(log);
    for line in clean.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            if let Some(found) = find_string_key(&value, keys) {
                return Some(found);
            }
        }
    }
    None
}

/// Depth-first search for the first candidate key holding a non-empty string.
/// Candidate keys at a given object level take priority in the order given.
fn find_string_key(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(Value::String(found)) = map.get(*key) {
                    if !found.is_empty() {
                        return Some(found.clone());
                    }
                }
            }
            for (_key, child) in map {
                if let Some(found) = find_string_key(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => {
            for item in items {
                if let Some(found) = find_string_key(item, keys) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// Build the plain (non-bypass) resume command for a vendor, run in `cwd`.
/// Per the trust-posture-at-launch policy, this carries NO autonomy flags:
/// the human is present and approves tool calls in the resumed session.
pub fn resume_command(vendor: &str, session_id: &str, cwd: &str, claude_config_dir: Option<&str>) -> Result<String> {
    let id = shell_quote(session_id);
    let dir = shell_quote(cwd);
    let resume = match vendor {
        "codex" => format!("codex resume {id}"),
        "claude" => format!("{}claude --resume {id}", crate::protocol::claude_env(claude_config_dir)),
        "cursor" => format!("cursor-agent --resume {id}"),
        other => return Err(anyhow!("cannot resume unknown vendor: {other}")),
    };
    Ok(format!("cd {dir} && {resume}"))
}

/// A random (version 4) UUID, for handing a new agent session its ID up front.
pub fn new_session_id() -> std::io::Result<String> {
    use std::io::Read;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_id_from_plain_text_line_even_after_huge_output() {
        let mut log = String::from("codex session id: 4f3c2b1a-1111-2222-3333-444455556666\n");
        for i in 0..5000 {
            log.push_str(&format!("thinking line {i} ...........................\n"));
        }
        assert_eq!(
            extract_session_id("codex", &log),
            Some("4f3c2b1a-1111-2222-3333-444455556666".to_string())
        );
    }

    #[test]
    fn codex_id_tolerates_ansi_escapes() {
        let log = "\u{1b}[2mSession id: \u{1b}[0mabc-123-def\u{1b}[0m\n";
        assert_eq!(extract_session_id("codex", log), Some("abc-123-def".to_string()));
    }

    #[test]
    fn codex_id_survives_non_ascii_before_marker() {
        // U+0130 lowercases to a different byte length; a byte offset taken
        // from the lowercased string must not be used to slice the original.
        let log = "İ session id: 4f3c2b1a-1111-2222-3333-444455556666\n";
        assert_eq!(
            extract_session_id("codex", log),
            Some("4f3c2b1a-1111-2222-3333-444455556666".to_string())
        );
        // Two U+0130 shift the lowercased offset by 2 bytes, enough that the
        // buggy `&line[pos..]` slice drops the leading hex digit (or lands on a
        // non-char boundary and panics). Guards that the token comes from the
        // same string that was searched.
        let log = "İİ session id: 4f3c2b1a-1111-2222-3333-444455556666\n";
        assert_eq!(
            extract_session_id("codex", log),
            Some("4f3c2b1a-1111-2222-3333-444455556666".to_string())
        );
    }

    #[test]
    fn claude_id_from_init_ndjson_event() {
        let log = "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-9\",\"tools\":[]}\n\
                   {\"type\":\"assistant\",\"session_id\":\"sess-9\"}\n";
        assert_eq!(extract_session_id("claude", log), Some("sess-9".to_string()));
    }

    #[test]
    fn cursor_id_from_chat_id_field() {
        let log = "{\"type\":\"result\",\"chatId\":\"chat_777\",\"ok\":true}\n";
        assert_eq!(extract_session_id("cursor", log), Some("chat_777".to_string()));
    }

    #[test]
    fn malformed_json_lines_are_skipped_until_a_good_one() {
        let log = "not json at all\n{oops\n{\"session_id\":\"sess-late\"}\n";
        assert_eq!(extract_session_id("claude", log), Some("sess-late".to_string()));
    }

    #[test]
    fn multiple_ids_return_the_first() {
        let log = "session id: first-id\nsession id: second-id\n";
        assert_eq!(extract_session_id("codex", log), Some("first-id".to_string()));
    }

    #[test]
    fn missing_id_returns_none() {
        assert_eq!(extract_session_id("codex", "no id here\n"), None);
        assert_eq!(extract_session_id("claude", "{\"type\":\"assistant\"}\n"), None);
        assert_eq!(extract_session_id("unknown-vendor", "session id: x\n"), None);
    }

    #[test]
    fn resume_command_per_vendor_cds_and_resumes() {
        let codex = resume_command("codex", "id-1", "/repo/wt", None).unwrap();
        assert!(codex.contains("cd '/repo/wt' &&"), "was: {codex}");
        assert!(codex.contains("codex resume 'id-1'"), "was: {codex}");

        let claude = resume_command("claude", "id-2", "/repo/wt", None).unwrap();
        assert!(claude.contains("claude --resume 'id-2'"), "was: {claude}");

        let cursor = resume_command("cursor", "id-3", "/repo/wt", None).unwrap();
        assert!(cursor.contains("cursor-agent --resume 'id-3'"), "was: {cursor}");
    }

    #[test]
    fn resume_command_omits_all_autonomy_flags() {
        for vendor in ["codex", "claude", "cursor"] {
            let cmd = resume_command(vendor, "id", "/wt", None).unwrap();
            assert!(!cmd.contains("--dangerously"), "{vendor}: {cmd}");
            assert!(!cmd.contains("--force"), "{vendor}: {cmd}");
            assert!(!cmd.contains("bypass"), "{vendor}: {cmd}");
            assert!(!cmd.contains("--skip-git-repo-check"), "{vendor}: {cmd}");
        }
    }

    #[test]
    fn a_hostile_config_dir_stays_one_word_in_the_claude_resume_command() {
        let cmd = resume_command("claude", "id", "/wt", Some("/a b'; touch x")).unwrap();
        assert!(cmd.contains(r"env CLAUDE_CONFIG_DIR='/a b'\''; touch x' claude --resume 'id'"), "was: {cmd}");
    }

    #[test]
    fn only_claude_resume_commands_carry_the_config_dir() {
        for vendor in ["codex", "cursor"] {
            let cmd = resume_command(vendor, "id", "/wt", Some("/cfg")).unwrap();
            assert!(!cmd.contains("CLAUDE_CONFIG_DIR"), "{vendor}: {cmd}");
        }
    }

    #[test]
    fn resume_command_rejects_unknown_vendor() {
        assert!(resume_command("nope", "id", "/wt", None).is_err());
    }
}
