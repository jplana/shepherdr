use anyhow::Result;
use serde_json::Value;
use std::process::Command;

pub struct ExecResult {
    pub stdout: String,
    pub code: i32,
}

pub trait CommandExec {
    fn exec(&self, args: &[String], capture: bool) -> Result<ExecResult>;
}

pub struct SystemExec {
    bin: String,
}
impl SystemExec {
    pub fn new() -> Self {
        SystemExec {
            bin: std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into()),
        }
    }
}
impl Default for SystemExec {
    fn default() -> Self { Self::new() }
}
impl CommandExec for SystemExec {
    fn exec(&self, args: &[String], capture: bool) -> Result<ExecResult> {
        let out = Command::new(&self.bin).args(args).output()?;
        let stdout = if capture {
            String::from_utf8_lossy(&out.stdout).into_owned()
        } else {
            String::new()
        };
        Ok(ExecResult { stdout, code: out.status.code().unwrap_or(-1) })
    }
}

pub trait HerdrRunner {
    fn split(&self, pane: &str, direction: &str, focus: bool) -> Result<String>;
    fn run(&self, pane: &str, command: &str) -> Result<()>;
    fn wait_output(&self, pane: &str, matcher: &str, timeout_ms: u64) -> Result<bool>;
    fn read(&self, pane: &str, source: &str, lines: u32) -> Result<String>;
    fn close(&self, pane: &str) -> Result<()>;
    fn tab_create(&self, workspace: &str, label: &str) -> Result<String>;
    fn pane_exists(&self, pane: &str) -> Result<bool>;
}

pub struct CliRunner<E: CommandExec> {
    pub exec: E,
}
impl<E: CommandExec> CliRunner<E> {
    pub fn new(exec: E) -> Self { CliRunner { exec } }
}
impl CliRunner<SystemExec> {
    pub fn system() -> Self { CliRunner::new(SystemExec::new()) }
}

fn s(v: &str) -> String { v.to_string() }

impl<E: CommandExec> HerdrRunner for CliRunner<E> {
    fn split(&self, pane: &str, direction: &str, focus: bool) -> Result<String> {
        let mut args = vec![s("pane"), s("split"), s(pane), s("--direction"), s(direction)];
        if !focus { args.push(s("--no-focus")); }
        Ok(self.exec.exec(&args, true)?.stdout)
    }
    fn run(&self, pane: &str, command: &str) -> Result<()> {
        self.exec.exec(&[s("pane"), s("run"), s(pane), s(command)], false)?;
        Ok(())
    }
    fn wait_output(&self, pane: &str, matcher: &str, timeout_ms: u64) -> Result<bool> {
        let args = vec![s("wait"), s("output"), s(pane), s("--match"),
                        s(matcher), s("--timeout"), timeout_ms.to_string()];
        Ok(self.exec.exec(&args, false)?.code == 0)
    }
    fn read(&self, pane: &str, source: &str, lines: u32) -> Result<String> {
        let args = vec![s("pane"), s("read"), s(pane), s("--source"),
                        s(source), s("--lines"), lines.to_string()];
        Ok(self.exec.exec(&args, true)?.stdout)
    }
    fn close(&self, pane: &str) -> Result<()> {
        self.exec.exec(&[s("pane"), s("close"), s(pane)], false)?;
        Ok(())
    }
    fn tab_create(&self, workspace: &str, label: &str) -> Result<String> {
        let args = vec![s("tab"), s("create"), s("--workspace"), s(workspace),
                        s("--label"), s(label)];
        Ok(self.exec.exec(&args, true)?.stdout)
    }
    fn pane_exists(&self, pane: &str) -> Result<bool> {
        let stdout = self.exec.exec(&[s("pane"), s("list")], true)?.stdout;
        let parsed: Value = match serde_json::from_str(&stdout) {
            Ok(v) => v,
            Err(_) => return Ok(false),
        };
        let panes = match parsed.get("result").and_then(|r| r.get("panes")).and_then(|p| p.as_array()) {
            Some(panes) => panes,
            None => return Ok(false),
        };
        Ok(panes.iter().any(|p| p.get("pane_id").and_then(|id| id.as_str()) == Some(pane)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Recorder {
        calls: RefCell<Vec<(Vec<String>, bool)>>,
        stdout: String,
        code: i32,
    }
    impl CommandExec for Recorder {
        fn exec(&self, args: &[String], capture: bool) -> anyhow::Result<ExecResult> {
            self.calls.borrow_mut().push((args.to_vec(), capture));
            Ok(ExecResult { stdout: self.stdout.clone(), code: self.code })
        }
    }
    fn rec(stdout: &str, code: i32) -> Recorder {
        Recorder { calls: RefCell::new(vec![]), stdout: stdout.into(), code }
    }

    #[test]
    fn split_emits_no_focus_by_default() {
        let r = CliRunner::new(rec(r#"{"result":{"pane":{"pane_id":"1-3"}}}"#, 0));
        let out = r.split("1-2", "right", false).unwrap();
        assert!(out.contains("1-3"));
        let call = &r.exec.calls.borrow()[0];
        assert_eq!(call.0, vec!["pane","split","1-2","--direction","right","--no-focus"]);
        assert!(call.1); // captured
    }

    #[test]
    fn run_sends_command_uncaptured() {
        let r = CliRunner::new(rec("", 0));
        r.run("1-3", "codex").unwrap();
        assert_eq!(r.exec.calls.borrow()[0].0, vec!["pane","run","1-3","codex"]);
    }

    #[test]
    fn wait_output_true_on_zero_exit() {
        let r = CliRunner::new(rec("", 0));
        assert!(r.wait_output("1-3", "DONE::t1", 5000).unwrap());
        assert_eq!(
            r.exec.calls.borrow()[0].0,
            vec!["wait","output","1-3","--match","DONE::t1","--timeout","5000"]
        );
    }

    #[test]
    fn wait_output_false_on_nonzero_exit() {
        let r = CliRunner::new(rec("", 1));
        assert!(!r.wait_output("1-3", "x", 10).unwrap());
    }

    #[test]
    fn read_uses_recent_unwrapped() {
        let r = CliRunner::new(rec("text", 0));
        assert_eq!(r.read("1-3", "recent-unwrapped", 200).unwrap(), "text");
        assert_eq!(
            r.exec.calls.borrow()[0].0,
            vec!["pane","read","1-3","--source","recent-unwrapped","--lines","200"]
        );
    }

    #[test]
    fn tab_create_emits_workspace_and_label() {
        let r = CliRunner::new(rec(r#"{"result":{"tab":{"tab_id":"1:2"}}}"#, 0));
        let out = r.tab_create("1", "workers").unwrap();
        assert!(out.contains("1:2"));
        assert_eq!(
            r.exec.calls.borrow()[0].0,
            vec!["tab","create","--workspace","1","--label","workers"]
        );
    }

    #[test]
    fn pane_exists_checks_pane_list() {
        let r = CliRunner::new(rec(
            r#"{"result":{"panes":[{"pane_id":"w1:p1"},{"pane_id":"w1:p3"}]}}"#, 0,
        ));
        assert!(r.pane_exists("w1:p3").unwrap());
        assert!(!r.pane_exists("w9:p9").unwrap());
        assert_eq!(r.exec.calls.borrow()[0].0, vec!["pane", "list"]);
    }
}
