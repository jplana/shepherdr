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

pub fn exited_path(scratch: &Path, task_id: &str) -> PathBuf {
    scratch.join(format!("{task_id}.exited"))
}

pub fn settings_path(scratch: &Path, task_id: &str) -> PathBuf {
    scratch.join(format!("{task_id}.settings.json"))
}

/// Written by the real interface's Stop hook each time a turn ends.
pub fn turn_ended_path(scratch: &Path, task_id: &str) -> PathBuf {
    scratch.join(format!("{task_id}.stop.json"))
}

/// Written by the real interface's StopFailure hook when a turn ends in an error.
pub fn turn_failed_path(scratch: &Path, task_id: &str) -> PathBuf {
    scratch.join(format!("{task_id}.stopfailure.json"))
}

const REFUSAL: &str = "No one is available to approve this. Do not retry it or look for a way \
around it. Carry on if you can; otherwise report status BLOCKED and name what was refused.";

/// Claude settings whose hooks save each turn's outcome to the given files and refuse
/// every permission prompt. The rename means a reader never sees half a file.
pub fn hook_settings(turn_ended: &Path, turn_failed: &Path) -> String {
    let save = |path: &Path| {
        let target = shell_quote(&path.to_string_lossy());
        let partial = shell_quote(&format!("{}.partial", path.to_string_lossy()));
        serde_json::json!([{ "hooks": [{ "type": "command", "command": format!("cat > {partial} && mv {partial} {target}") }] }])
    };
    let refusal = serde_json::json!({ "hookSpecificOutput": {
        "hookEventName": "PermissionRequest",
        "decision": { "behavior": "deny", "message": REFUSAL },
    }})
    .to_string();
    // Nobody is watching a background tab, so a prompt would otherwise wait forever.
    let refuse = serde_json::json!([{ "hooks": [{
        "type": "command",
        "command": format!("printf %s {}", shell_quote(&refusal)),
    }] }]);
    serde_json::json!({ "hooks": {
        "Stop": save(turn_ended),
        "StopFailure": save(turn_failed),
        "PermissionRequest": refuse,
    } })
    .to_string()
}

/// What the real interface is told to do. The task itself stays in the prompt file,
/// so the command line stays short.
pub fn first_message(prompt_file: &Path) -> String {
    format!("Read {} and follow all instructions in it.", prompt_file.display())
}

/// Last `n` lines of `text`; shorter input is returned whole.
pub fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

const OUTPUT_TAIL_LINES: usize = 40;
const OUTPUT_TAIL_BYTES: usize = 8192;
pub const OUTPUT_TRUNCATED_MARKER: &str = "[output truncated to its last 8 KiB] ";

/// Bound failure output for `result_text`: the last 40 lines and at most 8 KiB,
/// marker included. The marker sits at the start of the first kept line, so a
/// byte cut never adds a line. A cut made only by the line cap carries no marker.
pub fn bound_output(text: &str) -> String {
    let tail = tail_lines(text, OUTPUT_TAIL_LINES);
    if tail.len() <= OUTPUT_TAIL_BYTES {
        return tail;
    }
    let mut start = tail.len() - (OUTPUT_TAIL_BYTES - OUTPUT_TRUNCATED_MARKER.len());
    while !tail.is_char_boundary(start) {
        start += 1;
    }
    format!("{OUTPUT_TRUNCATED_MARKER}{}", &tail[start..])
}

/// Read a file as text, replacing invalid UTF-8; None if it cannot be read.
pub fn read_lossy(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Single-quote a path/string for safe inclusion in a shell command.
pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The start of a command that runs claude under `config_dir`, or nothing without one.
pub fn claude_env(config_dir: Option<&str>) -> String {
    config_dir.map_or(String::new(), |dir| format!("env CLAUDE_CONFIG_DIR={} ", shell_quote(dir)))
}

/// The command typed into an agent's pane. `clear` removes the typed command from
/// view. `log_file` is for headless agents only, because piping would stop the real
/// interface from running in a terminal. It runs under its own `sh`,
/// so whether the marker appears does not depend on how the pane's shell treats
/// an interrupted command list. The EXIT trap leaves the marker once the pipeline
/// has ended, however it ended, and also when `cd` fails. The signal traps turn a
/// signal into a normal exit, because a shell killed by a signal may skip its EXIT
/// trap. `sh` runs them only after the foreground pipeline has ended, so a marker
/// always means the agent is gone. The marker path is passed as `$1`.
pub fn pane_command(cd_to: Option<&str>, base: &str, log_file: Option<&str>, marker: &str) -> String {
    let cd = cd_to.map_or(String::new(), |dir| format!("cd {} && ", shell_quote(dir)));
    let logged = log_file.map_or(String::new(), |log| format!(" 2>&1 | tee {}", shell_quote(log)));
    let script = format!(
        "trap 'touch \"$1\"' EXIT; trap 'exit 129' HUP; trap 'exit 130' INT; trap 'exit 143' TERM; \
{cd}{base}{logged}",
    );
    format!("clear; sh -c {} sh {}", shell_quote(&script), shell_quote(marker))
}

/// The prompt for an agent that can't write files. Its final message becomes the result.
pub fn build_read_only_prompt(context: &str) -> String {
    format!(
        "{}\n\n---\nYou are in a read-only sandbox. Reply with your complete result as your \
final message, and do not try to write any file.",
        context.trim_end(),
    )
}

pub fn build_prompt(context: &str, task_id: &str, result_file: &str, role: &str) -> String {
    let implementer = role == "implementer";
    let (head, also_when, what) = if implementer {
        (
            "You are a subagent dispatched to execute one specific task.\n\n",
            ", or when you stop early to ask a question or report being blocked",
            "the message you would report back",
        )
    } else {
        ("", "", "your complete result")
    };
    format!(
        "{head}{context}\n\n---\nWhen you have finished the task above{also_when}, write {what} \
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


#[derive(Debug)]
pub enum Capture {
    File(String),
    Scrollback(String),
    Missing,
}

pub fn capture_result(result_file: &Path, scrollback: Option<&str>) -> Capture {
    if let Some(text) = read_lossy(result_file) {
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
    fn exited_path_names_marker() {
        assert_eq!(exited_path(Path::new("/s"), "t1"), Path::new("/s/t1.exited"));
    }

    #[test]
    fn tail_lines_keeps_last_n() {
        assert_eq!(tail_lines("a\nb\nc", 2), "b\nc");
        assert_eq!(tail_lines("a\nb", 5), "a\nb");
        assert_eq!(tail_lines("", 3), "");
    }

    #[test]
    fn bound_output_leaves_short_text_unmarked() {
        assert_eq!(bound_output("a\nb"), "a\nb");
        assert_eq!(bound_output(""), "");
    }

    #[test]
    fn bound_output_keeps_last_40_lines_without_marker_when_under_the_byte_cap() {
        let text = (1..=100).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let out = bound_output(&text);
        assert_eq!(out.lines().count(), 40);
        assert!(out.starts_with("line 61\n"), "was: {out}");
        assert!(out.ends_with("line 100"));
    }

    #[test]
    fn bound_output_caps_a_very_long_line_with_a_leading_marker() {
        let text = format!("{}END", "x".repeat(100_000));
        let out = bound_output(&text);
        assert!(out.len() <= 8192, "was {} bytes", out.len());
        assert!(out.starts_with(OUTPUT_TRUNCATED_MARKER), "was: {}", &out[..60]);
        assert!(out.ends_with("xxEND"));
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn bound_output_cuts_on_a_character_boundary() {
        // Each of these is 3 bytes, so some cut offsets fall inside a character.
        for extra in ["", "a", "ab"] {
            let text = format!("{}{extra}", "\u{20ac}".repeat(10_000));
            let out = bound_output(&text);
            assert!(out.len() <= 8192, "was {} bytes", out.len());
            let body = out.strip_prefix(OUTPUT_TRUNCATED_MARKER).expect("marker");
            assert!(body.len() > 8000, "kept only {} bytes", body.len());
            assert_eq!(body.trim_end_matches(extra).trim_start_matches('\u{20ac}'), "");
        }
    }

    #[test]
    fn bound_output_applies_both_caps_to_many_long_lines() {
        let text = (1..=100).map(|i| format!("{i:04} {}", "y".repeat(500))).collect::<Vec<_>>().join("\n");
        let out = bound_output(&text);
        assert!(out.len() <= 8192, "was {} bytes", out.len());
        assert!(out.lines().count() <= 40);
        assert!(out.starts_with(OUTPUT_TRUNCATED_MARKER));
        assert!(out.lines().last().unwrap().starts_with("0100 "));
    }

    #[test]
    fn bound_output_adds_no_extra_line_when_the_first_of_40_lines_is_cut() {
        let text = format!("{}\n{}", "z".repeat(20_000), vec!["s"; 39].join("\n"));
        let out = bound_output(&text);
        assert_eq!(out.lines().count(), 40);
        assert!(out.len() <= 8192);
    }

    #[test]
    fn read_lossy_replaces_invalid_utf8_and_is_none_when_missing() {
        let dir = std::env::temp_dir().join("hd-proto-lossy");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("bad.log");
        std::fs::write(&f, b"before \xff after").unwrap();
        assert_eq!(read_lossy(&f).unwrap(), "before \u{fffd} after");
        assert!(read_lossy(&dir.join("missing")).is_none());
        std::fs::remove_dir_all(&dir).ok();
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

    /// Run a pane command under a real `sh`; the stand-in agent is plain shell.
    fn run_in_sh(command: &str) {
        std::process::Command::new("sh").arg("-c").arg(command).output().unwrap();
    }

    fn pane_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hd proto it's {name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn pane_command_wraps_everything_in_sh_with_the_marker_as_an_argument() {
        let out = pane_command(Some("/w t"), "cat '/p' | agent", Some("/l og"), "/m k");
        assert!(out.starts_with("clear; sh -c '"), "was: {out}");
        assert!(out.ends_with(" sh '/m k'"), "was: {out}");
        for trap in [r#"touch "$1"'\'' EXIT"#, r"exit 129'\'' HUP", r"exit 130'\'' INT", r"exit 143'\'' TERM"] {
            assert!(out.contains(trap), "no {trap} in: {out}");
        }
    }

    #[test]
    fn pane_command_leaves_marker_and_log_on_normal_and_nonzero_exit() {
        for (name, agent) in [("zero", "echo agent-output"), ("nonzero", "echo agent-output; exit 7")] {
            let dir = pane_dir(name);
            let (log, marker) = (dir.join("t.log"), dir.join("t.exited"));
            let base = format!("printf x | sh -c {}", shell_quote(agent));
            run_in_sh(&pane_command(None, &base, Some(log.to_str().unwrap()), marker.to_str().unwrap()));
            assert!(marker.exists(), "{name}: no marker");
            assert!(std::fs::read_to_string(&log).unwrap().contains("agent-output"), "{name}");
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn pane_command_without_a_log_leaves_the_agent_on_the_terminal() {
        let dir = pane_dir("no-log");
        let marker = dir.join("t.exited");
        let command = pane_command(None, "if [ -t 1 ]; then :; else echo piped; fi", None, marker.to_str().unwrap());
        assert!(!command.contains("tee"), "was: {command}");
        run_in_sh(&command);
        assert!(marker.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pane_command_runs_the_agent_in_the_given_directory() {
        let dir = pane_dir("cd");
        let (log, marker) = (dir.join("t.log"), dir.join("t.exited"));
        run_in_sh(&pane_command(Some(dir.to_str().unwrap()), "pwd", Some(log.to_str().unwrap()), marker.to_str().unwrap()));
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(logged.trim_end().ends_with(dir.file_name().unwrap().to_str().unwrap()), "was: {logged}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pane_command_leaves_marker_when_cd_fails_and_skips_the_agent() {
        let dir = pane_dir("cd-fail");
        let (log, marker, ran) = (dir.join("t.log"), dir.join("t.exited"), dir.join("ran"));
        let base = format!("touch {}", shell_quote(ran.to_str().unwrap()));
        run_in_sh(&pane_command(Some("/definitely/not/here"), &base, Some(log.to_str().unwrap()), marker.to_str().unwrap()));
        assert!(marker.exists());
        assert!(!ran.exists());
        std::fs::remove_dir_all(&dir).ok();
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
    fn implementer_footer_covers_stopping_early() {
        let out = build_prompt("Do it.", "t1", "/s/t1.result.md", "implementer");
        assert!(
            out.contains("When you have finished the task above, or when you stop early to ask a question or report being blocked, write the message you would report back to the file `/s/t1.result.md`"),
            "was: {out}"
        );
    }

    #[test]
    fn non_implementer_prompt_is_byte_for_byte_unchanged() {
        let want = "Do it.\n\n---\nWhen you have finished the task above, write your complete result \
to the file `/s/t1.result.md` (overwrite it if it exists). The very last line of that file must be exactly:\n\nDONE::t1";
        assert_eq!(build_prompt("Do it.", "t1", "/s/t1.result.md", "reviewer"), want);
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
    fn capture_reads_invalid_utf8_file_lossily() {
        let dir = std::env::temp_dir().join("hd-cap-lossy");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.result.md");
        std::fs::write(&f, b"partial \xff").unwrap();
        match capture_result(&f, Some("SCROLL")) {
            Capture::File(s) => assert_eq!(s, "partial \u{fffd}"),
            other => panic!("expected File, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
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
