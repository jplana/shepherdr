//! Turns claude's stream-json output into text a person can follow in a pane. The
//! raw stream stays in the task's log file; this only decides what the pane shows.

use serde_json::Value;
use std::io::{BufRead, Write};

const MAX_LINE_CHARS: usize = 120;

/// The input field that best says what a tool call is doing, most specific first.
const TOOL_TARGET_KEYS: [&str; 9] = [
    "file_path", "notebook_path", "command", "pattern", "url", "query", "description", "prompt", "path",
];

/// What the pane shows for one line of output, or None if it stays in the log only.
/// A line that isn't a JSON object, such as a shell error, is shown as it is.
pub fn render_line(line: &str) -> Option<String> {
    let event = match serde_json::from_str::<Value>(line) {
        Ok(event) if event.is_object() => event,
        _ => return Some(clean(line)),
    };
    let text = |key: &str| event.get(key).and_then(Value::as_str);
    match text("type")? {
        "system" if text("subtype") == Some("init") => {
            Some(format!("── started {}", clean(text("model").unwrap_or("agent"))))
        }
        "assistant" => joined(event.pointer("/message/content")?, assistant_block),
        "user" => joined(event.pointer("/message/content")?, tool_error),
        "result" => Some(end_summary(&event)),
        _ => None,
    }
}

/// Copies input to output line by line. It reads until the input ends even if the
/// output breaks, because stopping would break the pipe and kill the agent.
pub fn run<R: BufRead, W: Write>(mut input: R, mut output: W) {
    let mut line = Vec::new();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&line);
        if let Some(shown) = render_line(text.trim_end_matches(['\r', '\n'])) {
            let _ = writeln!(output, "{shown}");
            let _ = output.flush();
        }
    }
}

/// Decoded JSON can hold terminal escape sequences that the raw stream kept escaped,
/// and they may come from whatever repo the agent is working in.
fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect()
}

fn one_line(text: &str) -> String {
    let cleaned = clean(text);
    let mut lines = cleaned.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or("");
    let mut shown: String = first.chars().take(MAX_LINE_CHARS).collect();
    if lines.next().is_some() || first.chars().count() > MAX_LINE_CHARS {
        shown.push('…');
    }
    shown
}

fn joined(content: &Value, render: fn(&Value) -> Option<String>) -> Option<String> {
    let parts: Vec<String> = content.as_array()?.iter().filter_map(render).collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn assistant_block(block: &Value) -> Option<String> {
    match block.get("type")?.as_str()? {
        "text" => Some(clean(block.get("text")?.as_str()?)),
        "tool_use" => {
            let name = clean(block.get("name")?.as_str()?);
            let target = TOOL_TARGET_KEYS
                .iter()
                .find_map(|key| block.get("input")?.get(key)?.as_str());
            Some(match target {
                Some(target) => format!("→ {name}: {}", one_line(target)),
                None => format!("→ {name}"),
            })
        }
        _ => None,
    }
}

fn tool_error(block: &Value) -> Option<String> {
    if block.get("is_error")?.as_bool() != Some(true) {
        return None;
    }
    let content = block.get("content")?;
    let text = match content.as_str() {
        Some(text) => text,
        None => content.as_array()?.iter().find_map(|b| b.get("text")?.as_str())?,
    };
    // A failed Bash call puts its exit code first and the message after it.
    let flat = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" · ");
    Some(format!("✗ {}", one_line(&flat)))
}

fn end_summary(event: &Value) -> String {
    let text = |key: &str| event.get(key).and_then(Value::as_str);
    if event.get("is_error").and_then(Value::as_bool) == Some(true) {
        let why = text("result").or(text("subtype")).unwrap_or("unknown error");
        return format!("── failed: {}", one_line(why));
    }
    let mut summary = String::from("── finished");
    if let Some(ms) = event.get("duration_ms").and_then(Value::as_u64) {
        summary.push_str(&format!(" in {}s", ms / 1000));
    }
    if let Some(turns) = event.get("num_turns").and_then(Value::as_u64) {
        summary.push_str(&format!(", {turns} turns"));
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn tool(name: &str, input: &str) -> String {
        format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"{name}","input":{input}}}]}}}}"#)
    }

    #[test]
    fn start_shows_the_model() {
        let out = render_line(r#"{"type":"system","subtype":"init","model":"claude-haiku-5-5","session_id":"abc"}"#).unwrap();
        assert!(out.contains("claude-haiku-5-5"), "was: {out}");
    }

    #[test]
    fn assistant_text_is_shown_as_written() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"First line\nSecond line"}]}}"#;
        assert_eq!(render_line(line).unwrap(), "First line\nSecond line");
    }

    #[test]
    fn a_tool_call_shows_its_name_and_target() {
        for (name, input, target) in [
            ("Edit", r#"{"file_path":"src/a.rs","old_string":"secret body"}"#, "src/a.rs"),
            ("Bash", r#"{"command":"cargo test","description":"Run tests"}"#, "cargo test"),
            ("Grep", r#"{"pattern":"fn main"}"#, "fn main"),
        ] {
            let out = render_line(&tool(name, input)).unwrap();
            assert!(out.contains(name) && out.contains(target), "was: {out}");
            assert!(!out.contains("secret body"), "was: {out}");
        }
    }

    #[test]
    fn a_tool_with_no_known_input_still_shows_its_name() {
        assert!(render_line(&tool("TodoWrite", r#"{"todos":[]}"#)).unwrap().contains("TodoWrite"));
    }

    #[test]
    fn a_long_or_multiline_command_stays_on_one_line() {
        let long = "x".repeat(500);
        let out = render_line(&tool("Bash", &format!(r#"{{"command":"{long}"}}"#))).unwrap();
        assert!(out.chars().count() < 200, "was {} chars", out.chars().count());
        let out = render_line(&tool("Bash", r#"{"command":"first\nsecond"}"#)).unwrap();
        assert!(out.contains("first") && !out.contains("second") && !out.contains('\n'), "was: {out}");
    }

    #[test]
    fn truncation_is_safe_on_multibyte_text() {
        let long = "é".repeat(500);
        assert!(render_line(&tool("Bash", &format!(r#"{{"command":"{long}"}}"#))).is_some());
    }

    #[test]
    fn noise_is_left_to_the_log() {
        for line in [
            r#"{"type":"system","subtype":"hook_started","hook_name":"SessionStart"}"#,
            r#"{"type":"system","subtype":"thinking_tokens"}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":false,"content":"file contents"}]}}"#,
        ] {
            assert_eq!(render_line(line), None, "for: {line}");
        }
    }

    #[test]
    fn a_tool_error_is_shown_in_both_content_shapes() {
        let text = r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":true,"content":"cat: no such file\nmore"}]}}"#;
        assert!(render_line(text).unwrap().contains("cat: no such file"));
        let blocks = r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":true,"content":[{"type":"text","text":"denied"}]}]}}"#;
        assert!(render_line(blocks).unwrap().contains("denied"));
    }

    #[test]
    fn a_tool_error_keeps_the_message_after_an_exit_code_line() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":true,"content":"Exit code 1\ncat: no such file"}]}}"#;
        let out = render_line(line).unwrap();
        assert!(out.contains("Exit code 1") && out.contains("cat: no such file"), "was: {out}");
        assert!(!out.contains('\n'), "was: {out}");
    }

    #[test]
    fn a_successful_end_gives_a_summary_without_repeating_the_answer() {
        let out = render_line(r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":12300,"num_turns":4,"result":"the final answer"}"#).unwrap();
        assert!(out.contains("12") && out.contains('4'), "was: {out}");
        assert!(!out.contains("the final answer"), "was: {out}");
    }

    #[test]
    fn a_failed_end_says_why() {
        let out = render_line(r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"model not found"}"#).unwrap();
        assert!(out.contains("model not found"), "was: {out}");
        let out = render_line(r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#).unwrap();
        assert!(out.contains("error_max_turns"), "was: {out}");
    }

    #[test]
    fn anything_that_is_not_a_json_object_is_passed_through() {
        for line in ["claude: command not found", "{broken json", "123", "[1,2]", r#""text""#] {
            assert_eq!(render_line(line).as_deref(), Some(line));
        }
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        let esc = r"\u001b]52;c;aGk=\u0007";
        for line in [
            format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"a{esc}b"}}]}}}}"#),
            tool("Bash", &format!(r#"{{"command":"a{esc}b"}}"#)),
            format!(r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","is_error":true,"content":"a{esc}b"}}]}}}}"#),
            format!(r#"{{"type":"result","is_error":true,"result":"a{esc}b"}}"#),
            format!(r#"{{"type":"system","subtype":"init","model":"a{esc}b"}}"#),
            "raw \u{1b}]52;c;aGk=\u{7} line".to_string(),
        ] {
            let out = render_line(&line).unwrap();
            assert!(!out.chars().any(|c| c.is_control() && c != '\n' && c != '\t'), "was: {out:?}");
        }
    }

    #[test]
    fn run_shows_each_line_and_survives_invalid_utf8() {
        let mut input = b"first\n\xff\xfe\n".to_vec();
        input.extend_from_slice(br#"{"type":"rate_limit_event"}"#);
        input.extend_from_slice(b"\nlast");
        let mut out = Vec::new();
        run(Cursor::new(input), &mut out);
        let out = String::from_utf8(out).unwrap();
        assert!(out.starts_with("first\n"), "was: {out}");
        assert!(out.ends_with("last\n"), "was: {out}");
        assert!(!out.contains("rate_limit"), "was: {out}");
    }

    struct ClosedPane;
    impl Write for ClosedPane {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    /// If this stopped reading, `tee` would get SIGPIPE and take the agent with it.
    #[test]
    fn run_keeps_reading_after_its_output_breaks() {
        let input = b"one\ntwo\nthree\n".to_vec();
        let len = input.len() as u64;
        let mut cursor = Cursor::new(input);
        run(&mut cursor, ClosedPane);
        assert_eq!(cursor.position(), len);
    }
}
