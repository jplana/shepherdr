use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long one herdr call may take. These calls normally return in milliseconds,
/// and a wedged herdr must not block a dispatch that is watching its result file.
pub const HERDR_CALL_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ExecResult {
    pub stdout: String,
    pub code: i32,
}

pub trait CommandExec {
    fn exec(&self, args: &[String], capture: bool, timeout: Duration) -> Result<ExecResult>;
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
    fn exec(&self, args: &[String], capture: bool, timeout: Duration) -> Result<ExecResult> {
        let deadline = Instant::now() + timeout;
        let timed_out = || anyhow!("herdr {} timed out after {timeout:?}", args.iter().take(2).cloned().collect::<Vec<_>>().join(" "));
        let mut child = Command::new(&self.bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(if capture { Stdio::piped() } else { Stdio::null() })
            .stderr(Stdio::null())
            .spawn()?;
        // Read on another thread, so that a full pipe cannot block the child and a
        // leftover process holding the pipe open cannot block this call.
        let (sent, received) = mpsc::channel();
        let stdout = child.stdout.take();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut out) = stdout {
                let _ = out.read_to_end(&mut bytes);
            }
            let _ = sent.send(bytes);
        });
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                // Reap on another thread: even a killed process may take a while to go.
                std::thread::spawn(move || child.wait());
                return Err(timed_out());
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let bytes = received
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| timed_out())?;
        Ok(ExecResult {
            stdout: String::from_utf8_lossy(&bytes).into_owned(),
            code: status.code().unwrap_or(-1),
        })
    }
}

pub trait HerdrRunner {
    fn split(&self, pane: &str, direction: &str, focus: bool, cwd: Option<&str>) -> Result<String>;
    fn run(&self, pane: &str, command: &str) -> Result<()>;
    fn wait_output(&self, pane: &str, matcher: &str, timeout_ms: u64) -> Result<bool>;
    fn read(&self, pane: &str, source: &str, lines: u32) -> Result<String>;
    fn close(&self, pane: &str) -> Result<()>;
    fn tab_create(&self, workspace: &str, label: &str) -> Result<String>;
    fn pane_exists(&self, pane: &str) -> Result<bool>;
}

pub struct CliRunner<E: CommandExec> {
    pub exec: E,
    pub call_timeout: Duration,
}
impl<E: CommandExec> CliRunner<E> {
    pub fn new(exec: E) -> Self { CliRunner { exec, call_timeout: HERDR_CALL_TIMEOUT } }

    fn call(&self, args: &[String], capture: bool) -> Result<ExecResult> {
        self.exec.exec(args, capture, self.call_timeout)
    }
}
impl CliRunner<SystemExec> {
    pub fn system() -> Self { CliRunner::new(SystemExec::new()) }
}

fn s(v: &str) -> String { v.to_string() }

impl<E: CommandExec> HerdrRunner for CliRunner<E> {
    fn split(&self, pane: &str, direction: &str, focus: bool, cwd: Option<&str>) -> Result<String> {
        let mut args = vec![s("pane"), s("split"), s(pane), s("--direction"), s(direction)];
        if !focus { args.push(s("--no-focus")); }
        if let Some(dir) = cwd { args.push(s("--cwd")); args.push(s(dir)); }
        Ok(self.call(&args, true)?.stdout)
    }
    fn run(&self, pane: &str, command: &str) -> Result<()> {
        self.call(&[s("pane"), s("run"), s(pane), s(command)], false)?;
        Ok(())
    }
    fn wait_output(&self, pane: &str, matcher: &str, timeout_ms: u64) -> Result<bool> {
        let args = vec![s("wait"), s("output"), s(pane), s("--match"),
                        s(matcher), s("--timeout"), timeout_ms.to_string()];
        // herdr enforces this timeout itself, so the bound only has to outlast it.
        let bound = Duration::from_millis(timeout_ms) + self.call_timeout;
        Ok(self.exec.exec(&args, false, bound)?.code == 0)
    }
    fn read(&self, pane: &str, source: &str, lines: u32) -> Result<String> {
        let args = vec![s("pane"), s("read"), s(pane), s("--source"),
                        s(source), s("--lines"), lines.to_string()];
        Ok(self.call(&args, true)?.stdout)
    }
    fn close(&self, pane: &str) -> Result<()> {
        self.call(&[s("pane"), s("close"), s(pane)], false)?;
        Ok(())
    }
    fn tab_create(&self, workspace: &str, label: &str) -> Result<String> {
        let args = vec![s("tab"), s("create"), s("--workspace"), s(workspace),
                        s("--label"), s(label)];
        Ok(self.call(&args, true)?.stdout)
    }
    fn pane_exists(&self, pane: &str) -> Result<bool> {
        // Ok(false) must mean herdr said the pane is gone, so anything else is an error.
        let out = self.call(&[s("pane"), s("list")], true)?;
        if out.code != 0 {
            bail!("herdr pane list exited with code {}", out.code);
        }
        let parsed: Value = serde_json::from_str(&out.stdout)?;
        let panes = parsed
            .get("result")
            .and_then(|r| r.get("panes"))
            .and_then(|p| p.as_array())
            .ok_or_else(|| anyhow!("herdr pane list gave no panes: {}", out.stdout))?;
        Ok(panes.iter().any(|p| p.get("pane_id").and_then(|id| id.as_str()) == Some(pane)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Recorder {
        calls: RefCell<Vec<(Vec<String>, bool)>>,
        timeouts: RefCell<Vec<Duration>>,
        stdout: String,
        code: i32,
    }
    impl CommandExec for Recorder {
        fn exec(&self, args: &[String], capture: bool, timeout: Duration) -> anyhow::Result<ExecResult> {
            self.calls.borrow_mut().push((args.to_vec(), capture));
            self.timeouts.borrow_mut().push(timeout);
            Ok(ExecResult { stdout: self.stdout.clone(), code: self.code })
        }
    }
    fn rec(stdout: &str, code: i32) -> Recorder {
        Recorder { calls: RefCell::new(vec![]), timeouts: RefCell::new(vec![]), stdout: stdout.into(), code }
    }

    #[test]
    fn split_emits_no_focus_by_default() {
        let r = CliRunner::new(rec(r#"{"result":{"pane":{"pane_id":"1-3"}}}"#, 0));
        let out = r.split("1-2", "right", false, None).unwrap();
        assert!(out.contains("1-3"));
        let call = &r.exec.calls.borrow()[0];
        assert_eq!(call.0, vec!["pane","split","1-2","--direction","right","--no-focus"]);
        assert!(call.1); // captured
    }

    #[test]
    fn split_passes_cwd_when_given() {
        let r = CliRunner::new(rec(r#"{"result":{"pane":{"pane_id":"1-3"}}}"#, 0));
        r.split("1-2", "right", false, Some("/wt")).unwrap();
        let call = &r.exec.calls.borrow()[0];
        assert_eq!(call.0, vec!["pane","split","1-2","--direction","right","--no-focus","--cwd","/wt"]);
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
    fn pane_exists_errors_instead_of_reporting_gone_when_herdr_fails() {
        assert!(CliRunner::new(rec("", 1)).pane_exists("w1:p1").is_err());
        assert!(CliRunner::new(rec("not json", 0)).pane_exists("w1:p1").is_err());
        assert!(CliRunner::new(rec(r#"{"error":{"code":"x"}}"#, 0)).pane_exists("w1:p1").is_err());
        assert!(CliRunner::new(rec(r#"{"result":{"panes":[{"pane_id":"w1:p1"}]}}"#, 1)).pane_exists("w1:p1").is_err());
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

    /// A herdr stand-in: a shell script with the given body.
    fn stub(name: &str, body: &str) -> (std::path::PathBuf, SystemExec) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("hd-runner-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("herdr");
        std::fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let exec = SystemExec { bin: bin.to_string_lossy().into_owned() };
        (dir, exec)
    }

    #[test]
    fn a_wedged_call_is_killed_at_its_timeout() {
        let (dir, exec) = stub("wedged", "echo $$ > \"$(dirname \"$0\")/pid\"; exec sleep 6");
        let start = std::time::Instant::now();
        // Long enough for the stub to start and write its pid while other tests run.
        assert!(exec.exec(&[s("pane"), s("list")], true, Duration::from_secs(1)).is_err());
        assert!(start.elapsed() < Duration::from_secs(4), "took {:?}", start.elapsed());

        let pid = std::fs::read_to_string(dir.join("pid")).unwrap();
        let alive = || Command::new("kill").args(["-0", pid.trim()]).status().unwrap().success();
        let gone_by = std::time::Instant::now() + Duration::from_secs(2);
        while alive() && std::time::Instant::now() < gone_by {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(), "the wedged process is still running");
    }

    #[test]
    fn a_leftover_child_holding_stdout_does_not_outlast_the_timeout() {
        let (_dir, exec) = stub("leftover", "sleep 6 &\nexit 0");
        let start = std::time::Instant::now();
        let _ = exec.exec(&[s("pane"), s("list")], true, Duration::from_millis(300));
        assert!(start.elapsed() < Duration::from_secs(3), "took {:?}", start.elapsed());
    }

    #[test]
    fn output_larger_than_a_pipe_buffer_arrives_whole_with_its_exit_code() {
        let (_dir, exec) = stub("big", "head -c 200000 /dev/zero | tr '\\0' x\nexit 7");
        let out = exec.exec(&[s("pane"), s("list")], true, Duration::from_secs(5)).unwrap();
        assert_eq!(out.stdout.len(), 200_000);
        assert_eq!(out.code, 7);
    }

    #[test]
    fn a_long_wait_outlasts_the_ordinary_call_bound() {
        let r = CliRunner::new(rec("", 0));
        r.wait_output("1-3", "x", 60_000).unwrap();
        r.read("1-3", "visible", 1).unwrap();
        let timeouts = r.exec.timeouts.borrow();
        assert!(timeouts[0] > Duration::from_secs(60), "wait got {:?}", timeouts[0]);
        assert!(timeouts[1] < Duration::from_secs(60), "read got {:?}", timeouts[1]);
    }

    #[test]
    fn a_complete_result_comes_back_while_pane_list_is_wedged() {
        let dir = std::env::temp_dir().join(format!("hd-runner-e2e-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let result = dir.join("t.result.md");
        let body = format!(
            "case \"$1 $2\" in\n\
             \"pane split\") echo '{{\"result\":{{\"pane\":{{\"pane_id\":\"w1:p9\"}}}}}}' ;;\n\
             \"pane run\") printf 'finished\\nDONE::t\\n' > '{}' ;;\n\
             \"pane list\") exec sleep 6 ;;\n\
             esac",
            result.display(),
        );
        let (_stub_dir, exec) = stub("e2e", &body);
        let mut runner = CliRunner::new(exec);
        runner.call_timeout = Duration::from_secs(1);
        let opts = crate::orchestrate::DispatchOpts {
            context: "x", role: "reviewer", from_pane: "w1:p1", scratch: &dir, task_id: "t",
            vendor: None, unavailable: Default::default(), timeout_ms: 60_000,
            split_direction: "right", close: false, state_dir: None, reuse_pane: None,
            mode: "headless", interactive: false, model: None, effort: None,
            resume_session: None, cwd: None, exit_grace_ms: 500,
        };
        let start = std::time::Instant::now();
        let out = crate::orchestrate::dispatch(&runner, &crate::config::default_config(), &opts).unwrap();
        assert!(start.elapsed() < Duration::from_secs(5), "took {:?}", start.elapsed());
        assert_eq!(out.status, "done");
        assert_eq!(out.result_text, "finished");
        assert!(!out.agent_exited, "a wedged herdr is no evidence that the agent exited");
    }
}
