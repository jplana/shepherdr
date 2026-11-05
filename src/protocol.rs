use std::path::{Path, PathBuf};

pub fn sentinel(task_id: &str) -> String {
    format!("DONE::{task_id}")
}

pub fn prompt_path(scratch: &Path, task_id: &str) -> PathBuf {
    scratch.join(format!("{task_id}.prompt.md"))
}

pub fn result_path(scratch: &Path, task_id: &str) -> PathBuf {
    scratch.join(format!("{task_id}.result.md"))
}

pub fn log_path(scratch: &Path, task_id: &str) -> PathBuf {
    scratch.join(format!("{task_id}.log"))
}

/// Single-quote a path/string for safe inclusion in a shell command.
pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

pub fn build_prompt(context: &str, task_id: &str, result_file: &str, role: &str) -> String {
    let implementer = role == "implementer";
    let (head, what) = if implementer {
        ("You are a subagent dispatched to execute one specific task.\n\n", "the message you would report back")
    } else {
        ("", "your complete result")
    };
    format!(
        "{head}{context}\n\n---\nWhen you have finished the task above, write {what} \
to the file `{result_file}` (overwrite it if it exists). The \
very last line of that file must be exactly:\n\n{sentinel}",
        context = context.trim_end(),
        sentinel = sentinel(task_id),
    )
}

/// If `file_text` is a complete result (its last non-empty line equals the
/// sentinel for `task_id`), returns Some(result) with that trailing marker
/// line removed and surrounding whitespace trimmed. Returns None if the
/// marker is absent (partial/incomplete write).
pub fn extract_complete_result(file_text: &str, task_id: &str) -> Option<String> {
    let marker = sentinel(task_id);
    let lines: Vec<&str> = file_text.lines().collect();
    let last_non_empty_index = lines.iter().rposition(|line| !line.trim().is_empty())?;
    if lines[last_non_empty_index].trim() != marker {
        return None;
    }
    Some(lines[..last_non_empty_index].join("\n").trim().to_string())
}

use anyhow::{anyhow, Result};
use serde_json::Value;

fn field<'a>(v: &'a Value, path: &[&str]) -> Result<&'a Value> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key).ok_or_else(|| anyhow!("missing field: {key}"))?;
    }
    Ok(cur)
}

pub fn pane_id_from_split(json: &str) -> Result<String> {
    let v: Value = serde_json::from_str(json)?;
    Ok(field(&v, &["result", "pane", "pane_id"])?
        .as_str().ok_or_else(|| anyhow!("pane_id not a string"))?.to_string())
}

pub fn tab_id_from_create(json: &str) -> Result<String> {
    let v: Value = serde_json::from_str(json)?;
    Ok(field(&v, &["result", "tab", "tab_id"])?
        .as_str().ok_or_else(|| anyhow!("tab_id not a string"))?.to_string())
}

#[derive(Debug)]
pub enum Capture {
    File(String),
    Scrollback(String),
    Missing,
}

pub fn capture_result(result_file: &Path, scrollback: Option<&str>) -> Capture {
    if let Ok(text) = std::fs::read_to_string(result_file) {
        return Capture::File(text);
    }
    match scrollback {
        Some(s) => Capture::Scrollback(s.to_string()),
        None => Capture::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn sentinel_format() {
        assert_eq!(sentinel("rev-1"), "DONE::rev-1");
    }

    #[test]
    fn paths() {
        assert_eq!(prompt_path(Path::new("/s"), "t1"), Path::new("/s/t1.prompt.md"));
        assert_eq!(result_path(Path::new("/s"), "t1"), Path::new("/s/t1.result.md"));
    }

    #[test]
    fn log_path_names_task_log_file() {
        assert_eq!(log_path(Path::new("/s"), "t1"), Path::new("/s/t1.log"));
    }

    #[test]
    fn shell_quote_wraps_and_escapes_single_quotes() {
        assert_eq!(shell_quote("/a b/c"), "'/a b/c'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn implementer_prompt_starts_with_subagent_line_and_asks_for_report_back() {
        let out = build_prompt("Do it.", "t1", "/s/t1.result.md", "implementer");
        assert_eq!(
            out.lines().next().unwrap(),
            "You are a subagent dispatched to execute one specific task."
        );
        assert!(out.contains("write the message you would report back to the file `/s/t1.result.md`"), "was: {out}");
        assert!(!out.contains("complete result"), "was: {out}");
        assert!(out.ends_with("DONE::t1"));
    }

    #[test]
    fn reviewer_prompt_has_no_subagent_line_and_keeps_complete_result() {
        let out = build_prompt("Do it.", "t1", "/s/t1.result.md", "reviewer");
        assert!(!out.contains("subagent"), "was: {out}");
        assert!(out.starts_with("Do it.\n\n---\nWhen you have finished the task above, write your complete result to the file `/s/t1.result.md`"), "was: {out}");
    }

    #[test]
    fn build_prompt_mentions_result_file_and_last_line_marker() {
        let out = build_prompt("Do it.", "t1", "/s/t1.result.md", "reviewer");
        assert!(out.starts_with("Do it."));
        assert!(out.contains("/s/t1.result.md"));
        assert!(out.contains("DONE::t1"));
        assert!(out.contains("last line"));
    }

    #[test]
    fn extract_complete_result_returns_text_without_marker() {
        let out = extract_complete_result("findings here\n\nDONE::t1", "t1");
        assert_eq!(out, Some("findings here".to_string()));
    }

    #[test]
    fn extract_complete_result_none_when_marker_absent() {
        let out = extract_complete_result("partial output", "t1");
        assert_eq!(out, None);
    }

    #[test]
    fn extract_complete_result_none_when_marker_not_last_line() {
        let out = extract_complete_result("DONE::t1\nmore text", "t1");
        assert_eq!(out, None);
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn pane_id_parsed() {
        let j = r#"{"result":{"pane":{"pane_id":"1-3"}}}"#;
        assert_eq!(pane_id_from_split(j).unwrap(), "1-3");
    }

    #[test]
    fn tab_id_parsed() {
        let j = r#"{"result":{"tab":{"tab_id":"1:2"}}}"#;
        assert_eq!(tab_id_from_create(j).unwrap(), "1:2");
    }

    #[test]
    fn capture_reads_file_when_present() {
        let dir = std::env::temp_dir().join("hd-cap-1");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.result.md");
        std::fs::write(&f, "findings").unwrap();
        match capture_result(&f, Some("SCROLL")) {
            Capture::File(s) => assert_eq!(s, "findings"),
            other => panic!("expected File, got {other:?}"),
        }
        std::fs::remove_file(&f).ok();
    }

    #[test]
    fn capture_falls_back_to_scrollback() {
        let f = Path::new("/definitely/missing.result.md");
        match capture_result(f, Some("SCROLL")) {
            Capture::Scrollback(s) => assert_eq!(s, "SCROLL"),
            other => panic!("expected Scrollback, got {other:?}"),
        }
    }

    #[test]
    fn capture_missing_without_reader() {
        let f = Path::new("/definitely/missing.result.md");
        assert!(matches!(capture_result(f, None), Capture::Missing));
    }
}
