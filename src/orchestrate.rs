use crate::config::{Config, VendorSpec};
use crate::protocol::{self, Capture};
use crate::runner::HerdrRunner;
use crate::state::{write_record, DispatchRecord};
use crate::vendors::resolve;
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

pub struct DispatchOpts<'a> {
    pub context: &'a str,
    pub role: &'a str,
    pub from_pane: &'a str,
    pub scratch: &'a Path,
    pub task_id: &'a str,
    pub vendor: Option<&'a str>,
    pub unavailable: HashSet<String>,
    pub timeout_ms: u64,
    pub split_direction: &'a str,
    pub close: bool,
    pub state_dir: Option<&'a Path>,
    pub reuse_pane: Option<&'a str>,
    pub mode: &'a str,
    pub interactive: bool,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub resume_session: Option<&'a str>,
    pub cwd: Option<&'a str>,
}

#[derive(Serialize, Debug)]
pub struct DispatchResult {
    pub status: String,
    pub task_id: String,
    pub vendor: String,
    pub pane_id: String,
    pub result_file: String,
    pub result_source: String,
    pub result_text: String,
    pub session_id: Option<String>,
}

fn record(
    opts: &DispatchOpts,
    vendor: &str,
    pane_id: &str,
    status: &str,
    created_at: &str,
    session_id: Option<&str>,
    cwd: &str,
) {
    if let Some(dir) = opts.state_dir {
        let cwd = cwd.to_string();
        let _ = write_record(dir, &DispatchRecord {
            task_id: opts.task_id.into(), role: opts.role.into(),
            vendor: vendor.into(), pane_id: pane_id.into(), status: status.into(),
            session_id: session_id.map(String::from),
            mode: opts.mode.into(),
            cwd,
            created_at: created_at.into(),
        });
    }
}

/// Poll the result file until its trailing sentinel appears or the deadline
/// passes. Returns the marker-stripped result, or None on timeout.
fn poll_result_file(result_file: &Path, task_id: &str, timeout_ms: u64) -> Option<String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let poll = Duration::from_millis(200);
    loop {
        if let Ok(text) = std::fs::read_to_string(result_file) {
            if let Some(result) = protocol::extract_complete_result(&text, task_id) {
                return Some(result);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(poll);
    }
}

pub fn dispatch<R: HerdrRunner>(
    runner: &R,
    config: &Config,
    opts: &DispatchOpts,
) -> Result<DispatchResult> {
    // All validation happens first, before any file write or herdr call.
    if opts.interactive {
        for (flag, given) in [
            ("--model", opts.model.is_some()),
            ("--effort", opts.effort.is_some()),
            ("--resume-session", opts.resume_session.is_some()),
        ] {
            if given {
                bail!("{flag} is not supported with --interactive");
            }
        }
    }
    if opts.resume_session.is_some() && opts.vendor.is_none() {
        bail!("--resume-session requires an explicit --vendor");
    }
    if !opts.interactive && opts.role == "implementer" && opts.model.is_none() {
        bail!("role implementer requires --model");
    }
    let cwd_path = match opts.cwd {
        Some(c) => {
            let abs = std::path::absolute(c)?;
            if !abs.is_dir() {
                bail!("--cwd {c} is not an existing directory");
            }
            abs
        }
        None => std::env::current_dir()?,
    };
    let cwd = cwd_path.to_string_lossy().into_owned();
    let scratch = std::path::absolute(opts.scratch)?;

    let (vendor, spec) = resolve(opts.role, config, opts.vendor, &opts.unavailable)?;
    let vendor = vendor.to_string();
    let created_at = crate::state::now_created_at();

    let prompt_file = protocol::prompt_path(&scratch, opts.task_id);
    let result_file = protocol::result_path(&scratch, opts.task_id);
    let log_file = protocol::log_path(&scratch, opts.task_id);
    let base_command = if opts.interactive {
        None
    } else {
        Some(crate::config::headless_command(
            spec,
            &prompt_file.to_string_lossy(),
            opts.model,
            opts.effort,
            opts.resume_session,
        )?)
    };
    std::fs::create_dir_all(&scratch)?;
    // A prior run/retry with the same task_id (e.g. a fallback vendor after
    // --unavailable) may have left a result file behind. Clear it so this
    // dispatch can only ever capture a file the current agent writes;
    // otherwise capture_result would read stale content and mask a real
    // degraded/timeout outcome. Absence is fine, hence the ignored error.
    let _ = std::fs::remove_file(&result_file);
    // A prior run with the same task_id may have left a log behind; clear it
    // so id capture can only ever read this run's agent output.
    let _ = std::fs::remove_file(&log_file);
    std::fs::write(
        &prompt_file,
        protocol::build_prompt(opts.context, opts.task_id, &result_file.to_string_lossy(), opts.role),
    )?;
    record(opts, &vendor, "", "dispatched", &created_at, None, &cwd);

    let Some(base) = base_command else {
        return dispatch_interactive(runner, spec, opts, &vendor, &prompt_file, &result_file, &created_at, &cwd);
    };

    let mut reused = false;
    let pane_id = match opts.reuse_pane {
        Some(p) if runner.pane_exists(p)? => {
            reused = true;
            p.to_string()
        }
        _ => protocol::pane_id_from_split(
                 &runner.split(opts.from_pane, opts.split_direction, false, Some(&cwd))?,
             )?,
    };

    let mk = |status: &str, source: &str, text: String| DispatchResult {
        status: status.into(), task_id: opts.task_id.into(), vendor: vendor.clone(),
        pane_id: pane_id.clone(), result_file: result_file.to_string_lossy().into(),
        result_source: source.into(), result_text: text, session_id: None,
    };

    record(opts, &vendor, &pane_id, "working", &created_at, None, &cwd);

    // Run the headless agent command; it writes its complete result to
    // result_file, whose last line becomes the sentinel marker. Prompt echo
    // in headless agent output can spoof a scrollback-matched sentinel, so
    // completion is detected by polling the result file itself rather than
    // watching the pane.
    let quoted_log = protocol::shell_quote(&log_file.to_string_lossy());
    let cd = if reused {
        format!("cd {} && ", protocol::shell_quote(&cwd))
    } else {
        String::new()
    };
    let command = format!("{cd}{base} 2>&1 | tee {quoted_log}");
    runner.run(&pane_id, &command)?;

    let completed = poll_result_file(&result_file, opts.task_id, opts.timeout_ms);

    let mut result = match completed {
        Some(text) => mk("done", "file", text),
        None => {
            let scrollback = runner.read(&pane_id, "recent-unwrapped", 200)?;
            match protocol::capture_result(&result_file, Some(&scrollback)) {
                Capture::File(t) => mk("degraded", "file", t),
                Capture::Scrollback(t) => mk("timeout", "scrollback", t),
                Capture::Missing => mk("timeout", "scrollback", String::new()),
            }
        }
    };
    let session_id = std::fs::read_to_string(&log_file)
        .ok()
        .and_then(|log| crate::session::extract_session_id(&vendor, &log));
    result.session_id = session_id.clone();
    record(opts, &vendor, &pane_id, &result.status, &created_at, session_id.as_deref(), &cwd);

    if opts.close {
        runner.close(&pane_id)?;
    }
    Ok(result)
}

const READY_TIMEOUT_MS: u64 = 30_000;
const READY_POLL_MS: u64 = 300;

/// Opt-in headed dispatch (Option A). Launch the bare TUI with gate-bypass
/// flags, wait for readiness, deliver the prompt as one line, verify it
/// registered, then file-poll for completion. The pane is left live; this is
/// the fragile, per-vendor path, isolated behind `--interactive`.
#[allow(clippy::too_many_arguments)]
fn dispatch_interactive<R: HerdrRunner>(
    runner: &R,
    spec: &VendorSpec,
    opts: &DispatchOpts,
    vendor: &str,
    prompt_file: &Path,
    result_file: &Path,
    created_at: &str,
    cwd: &str,
) -> Result<DispatchResult> {
    let pane_id = protocol::pane_id_from_split(
        &runner.split(opts.from_pane, opts.split_direction, true, None)?,
    )?;
    record(opts, vendor, &pane_id, "working", created_at, None, cwd);

    let mk = |status: &str, source: &str, text: String| DispatchResult {
        status: status.into(), task_id: opts.task_id.into(), vendor: vendor.into(),
        pane_id: pane_id.clone(), result_file: result_file.to_string_lossy().into(),
        result_source: source.into(), result_text: text, session_id: None,
    };

    // 1. Launch the bare TUI in the target dir with gate-bypass flags.
    let launch = format!("cd {} && {}", protocol::shell_quote(cwd), spec.interactive_command);
    runner.run(&pane_id, &launch)?;

    // 2. Poll `read --source visible` for readiness (cue if set, else any
    //    non-empty visible output), bounded by a timeout fallback.
    let ready_deadline =
        Instant::now() + Duration::from_millis(READY_TIMEOUT_MS.min(opts.timeout_ms));
    let mut ready = false;
    loop {
        let visible = runner.read(&pane_id, "visible", 200).unwrap_or_default();
        let seen = if spec.ready_cue.is_empty() {
            !visible.trim().is_empty()
        } else {
            visible.contains(&spec.ready_cue)
        };
        if seen {
            ready = true;
            break;
        }
        if Instant::now() >= ready_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(READY_POLL_MS));
    }

    // 3. Deliver the task as one short line.
    let deliver = format!(
        "read {} and follow all instructions in it",
        protocol::shell_quote(&prompt_file.to_string_lossy()),
    );
    runner.run(&pane_id, &deliver)?;

    // 4. Verify delivery registered.
    let after = runner.read(&pane_id, "visible", 200).unwrap_or_default();
    let delivery_verified = !after.trim().is_empty();

    if !ready && !delivery_verified {
        let result = mk(
            "degraded",
            "interactive",
            "interactive TUI never showed a ready cue and delivery could not be verified".into(),
        );
        record(opts, vendor, &pane_id, &result.status, created_at, None, cwd);
        return Ok(result); // leave the pane live for manual inspection
    }

    // 5. File-poll for completion (same mechanism as headless).
    let result = match poll_result_file(result_file, opts.task_id, opts.timeout_ms) {
        Some(text) => mk("done", "file", text),
        None => mk(
            "timeout",
            "interactive",
            "not completed — the interactive session may need manual nudging".into(),
        ),
    };
    // 6. Leave the pane live (never close): you continue interactively.
    record(opts, vendor, &pane_id, &result.status, created_at, None, cwd);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_config;
    use crate::protocol::result_path;
    use std::cell::RefCell;
    use std::collections::HashSet;

    #[derive(Clone, Copy)]
    enum WriteMode {
        /// simulates the agent finishing: content + trailing sentinel line
        Complete,
        /// simulates a still-in-progress write: content, no sentinel
        Incomplete,
        /// simulates an agent that never wrote a result file
        Nothing,
    }

    struct FakeRunner {
        pane_id: String,
        write_mode: WriteMode,
        result_text: String,
        scratch: std::path::PathBuf,
        task_id: String,
        closed: RefCell<Vec<String>>,
        run_calls: RefCell<u32>,
        split_calls: RefCell<u32>,
        split_cwd: RefCell<Option<String>>,
        pane_exists_result: bool,
        commands: RefCell<Vec<String>>,
    }
    impl HerdrRunner for FakeRunner {
        fn split(&self, _p: &str, _d: &str, _f: bool, cwd: Option<&str>) -> anyhow::Result<String> {
            *self.split_cwd.borrow_mut() = cwd.map(String::from);
            *self.split_calls.borrow_mut() += 1;
            Ok(format!(r#"{{"result":{{"pane":{{"pane_id":"{}"}}}}}}"#, self.pane_id))
        }
        fn run(&self, _pane: &str, command: &str) -> anyhow::Result<()> {
            self.commands.borrow_mut().push(command.to_string());
            // the headless command (`cat <...>.prompt.md | ...`) triggers the simulated write
            if command.contains(".prompt.md") {
                *self.run_calls.borrow_mut() += 1;
                let path = result_path(&self.scratch, &self.task_id);
                match self.write_mode {
                    WriteMode::Complete => {
                        let content = format!(
                            "{}\n{}", self.result_text, protocol::sentinel(&self.task_id),
                        );
                        std::fs::write(path, content).unwrap();
                    }
                    WriteMode::Incomplete => {
                        std::fs::write(path, &self.result_text).unwrap();
                    }
                    WriteMode::Nothing => {}
                }
                let log = crate::protocol::log_path(&self.scratch, &self.task_id);
                std::fs::write(
                    log,
                    "session id: 11111111-2222-3333-4444-555555555555\nwork output\n",
                ).unwrap();
            }
            Ok(())
        }
        // no longer exercised by dispatch (completion is now polled from the
        // result file), kept trivial to satisfy the trait
        fn wait_output(&self, _p: &str, _matcher: &str, _t: u64) -> anyhow::Result<bool> {
            Ok(true)
        }
        fn read(&self, _p: &str, _s: &str, _l: u32) -> anyhow::Result<String> {
            Ok("scrollback".into())
        }
        fn close(&self, pane: &str) -> anyhow::Result<()> {
            self.closed.borrow_mut().push(pane.into()); Ok(())
        }
        fn tab_create(&self, _w: &str, _l: &str) -> anyhow::Result<String> { Ok("{}".into()) }
        fn pane_exists(&self, _pane: &str) -> anyhow::Result<bool> { Ok(self.pane_exists_result) }
    }

    fn opts<'a>(scratch: &'a std::path::Path, task_id: &'a str) -> DispatchOpts<'a> {
        DispatchOpts {
            context: "Review the diff.", role: "reviewer", from_pane: "1-2",
            scratch, task_id, vendor: None, unavailable: HashSet::new(),
            timeout_ms: 400, split_direction: "right",
            close: false, state_dir: None, reuse_pane: None, mode: "headless",
            interactive: false,
            model: None, effort: None, resume_session: None, cwd: None,
        }
    }

    fn fake(scratch: &std::path::Path, task_id: &str, write_mode: WriteMode,
            result_text: &str) -> FakeRunner {
        FakeRunner {
            pane_id: "1-3".into(), write_mode,
            result_text: result_text.into(),
            scratch: scratch.to_path_buf(), task_id: task_id.into(),
            closed: RefCell::new(vec![]), run_calls: RefCell::new(0),
            split_calls: RefCell::new(0), split_cwd: RefCell::new(None), pane_exists_result: true,
            commands: RefCell::new(vec![]),
        }
    }

    #[test]
    fn happy_path_captures_from_file() {
        let dir = std::env::temp_dir().join("hd-orch-1");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake(&dir, "rev-1", WriteMode::Complete, "LGTM");
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-1")).unwrap();
        assert_eq!(out.status, "done");
        assert_eq!(out.vendor, "codex");
        assert_eq!(out.pane_id, "1-3");
        assert_eq!(out.result_source, "file");
        assert_eq!(out.result_text, "LGTM");
        // the headless command was run exactly once
        assert_eq!(*r.run_calls.borrow(), 1);
        assert!(dir.join("rev-1.prompt.md").exists());
        assert!(r.closed.borrow().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn happy_path_captures_session_id_from_log() {
        let dir = std::env::temp_dir().join("hd-orch-sess");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake(&dir, "rev-sess", WriteMode::Complete, "LGTM");
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-sess")).unwrap();
        assert_eq!(out.status, "done");
        assert_eq!(out.session_id, Some("11111111-2222-3333-4444-555555555555".to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_marker_before_timeout_is_degraded() {
        let dir = std::env::temp_dir().join("hd-orch-2");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake(&dir, "rev-2", WriteMode::Incomplete, "partial output");
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-2")).unwrap();
        assert_eq!(out.status, "degraded");
        assert_eq!(out.result_source, "file");
        assert!(out.result_text.contains("partial output"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_file_is_timeout() {
        let dir = std::env::temp_dir().join("hd-orch-3");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake(&dir, "rev-3", WriteMode::Nothing, "");
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-3")).unwrap();
        assert_eq!(out.status, "timeout");
        assert_eq!(out.result_source, "scrollback");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn close_flag_closes_pane() {
        let dir = std::env::temp_dir().join("hd-orch-4");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake(&dir, "rev-4", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "rev-4"); o.close = true;
        dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(r.closed.borrow().as_slice(), ["1-3"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stale_result_file_is_cleared_before_dispatch() {
        // A prior run/retry with the same task_id (e.g. a fallback vendor
        // after --unavailable) can leave a result file behind. If dispatch
        // doesn't clear it, an agent that writes nothing this run would
        // wrongly report the stale file's content as a false `done`.
        let dir = std::env::temp_dir().join("hd-orch-stale");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            result_path(&dir, "rev-stale"),
            format!("STALE\n{}", protocol::sentinel("rev-stale")),
        ).unwrap();

        // this run's agent never writes a result file, so the only way to
        // see stale content (or a false `done`) is if dispatch failed to
        // clear the leftover file from the previous run.
        let r = fake(&dir, "rev-stale", WriteMode::Nothing, "");
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-stale")).unwrap();

        assert_eq!(out.status, "timeout");
        assert!(!out.result_text.contains("STALE"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reuse_pane_skips_split_when_pane_exists() {
        let dir = std::env::temp_dir().join("hd-orch-reuse");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake(&dir, "rev-reuse", WriteMode::Complete, "reused ok");
        let mut o = opts(&dir, "rev-reuse");
        o.reuse_pane = Some("w1:p9");
        let out = dispatch(&r, &default_config(), &o).unwrap();

        assert_eq!(out.status, "done");
        assert_eq!(out.pane_id, "w1:p9");
        assert_eq!(*r.split_calls.borrow(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn command_tees_output_to_task_log() {
        let dir = std::env::temp_dir().join("hd-orch-tee");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake(&dir, "rev-tee", WriteMode::Complete, "ok");
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-tee")).unwrap();

        assert_eq!(out.status, "done");
        let commands = r.commands.borrow();
        assert_eq!(commands.len(), 1);
        let log_file = protocol::log_path(&dir, "rev-tee");
        let quoted_log = format!("'{}'", log_file.to_string_lossy());
        assert!(commands[0].contains("| tee "), "command was: {}", commands[0]);
        assert!(commands[0].contains(&quoted_log), "command was: {}", commands[0]);
        std::fs::remove_dir_all(&dir).ok();
    }

    struct InteractiveFake {
        pane_id: String,
        scratch: std::path::PathBuf,
        task_id: String,
        visible: String,
        commands: RefCell<Vec<String>>,
        split_focus: RefCell<Vec<bool>>,
    }
    impl HerdrRunner for InteractiveFake {
        fn split(&self, _p: &str, _d: &str, focus: bool, _c: Option<&str>) -> anyhow::Result<String> {
            self.split_focus.borrow_mut().push(focus);
            Ok(format!(r#"{{"result":{{"pane":{{"pane_id":"{}"}}}}}}"#, self.pane_id))
        }
        fn run(&self, _pane: &str, command: &str) -> anyhow::Result<()> {
            self.commands.borrow_mut().push(command.to_string());
            // the delivery line ("follow all instructions") triggers the
            // simulated in-context completion
            if command.contains("follow all instructions") {
                let path = result_path(&self.scratch, &self.task_id);
                let content = format!("done in context\n{}", protocol::sentinel(&self.task_id));
                std::fs::write(path, content).unwrap();
            }
            Ok(())
        }
        fn wait_output(&self, _p: &str, _m: &str, _t: u64) -> anyhow::Result<bool> { Ok(true) }
        fn read(&self, _p: &str, _s: &str, _l: u32) -> anyhow::Result<String> {
            Ok(self.visible.clone())
        }
        fn close(&self, _p: &str) -> anyhow::Result<()> { Ok(()) }
        fn tab_create(&self, _w: &str, _l: &str) -> anyhow::Result<String> { Ok("{}".into()) }
        fn pane_exists(&self, _p: &str) -> anyhow::Result<bool> { Ok(true) }
    }

    #[test]
    fn interactive_launches_delivers_and_completes() {
        let dir = std::env::temp_dir().join("hd-orch-interactive");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let fake = InteractiveFake {
            pane_id: "i-1".into(), scratch: dir.clone(), task_id: "task-i".into(),
            visible: "codex ready >".into(),
            commands: RefCell::new(vec![]), split_focus: RefCell::new(vec![]),
        };
        let mut o = opts(&dir, "task-i");
        o.interactive = true;
        o.mode = "interactive";
        let out = dispatch(&fake, &default_config(), &o).unwrap();

        assert_eq!(out.status, "done");
        assert_eq!(out.pane_id, "i-1");
        assert_eq!(out.result_text, "done in context");
        assert_eq!(fake.split_focus.borrow().as_slice(), [true]); // headed
        let commands = fake.commands.borrow();
        // launch (bare TUI, no `cat`/`tee`), then delivery line
        assert!(commands[0].contains("codex --dangerously-bypass-approvals-and-sandbox"), "was: {}", commands[0]);
        assert!(!commands[0].contains("| tee"), "launch should not tee: {}", commands[0]);
        assert!(commands.iter().any(|c| c.contains("follow all instructions")),
            "no delivery line in {commands:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn command_quotes_prompt_path_with_spaces() {
        let dir = std::env::temp_dir().join("hd cleanup dir");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake(&dir, "rev-quote", WriteMode::Complete, "ok");
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-quote")).unwrap();

        assert_eq!(out.status, "done");
        let commands = r.commands.borrow();
        assert_eq!(commands.len(), 1);
        let prompt_file = protocol::prompt_path(&dir, "rev-quote");
        let quoted_prompt = format!("'{}'", prompt_file.to_string_lossy());
        assert!(commands[0].contains(&quoted_prompt));
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tdir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn implementer_without_model_errors_before_any_herdr_call() {
        let dir = tdir("hd-orch-nomodel");
        let r = fake(&dir, "imp-1", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "imp-1");
        o.role = "implementer";
        let err = dispatch(&r, &default_config(), &o).unwrap_err().to_string();
        assert!(err.contains("--model"), "was: {err}");
        assert_eq!(*r.split_calls.borrow(), 0);
        assert!(!dir.join("imp-1.prompt.md").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resume_without_vendor_errors() {
        let dir = tdir("hd-orch-resume-novendor");
        let r = fake(&dir, "res-1", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "res-1");
        o.resume_session = Some("sess-1");
        let err = dispatch(&r, &default_config(), &o).unwrap_err().to_string();
        assert!(err.contains("--vendor"), "was: {err}");
        assert_eq!(*r.split_calls.borrow(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cursor_resume_errors_before_any_pane() {
        let dir = tdir("hd-orch-cursor-resume");
        let r = fake(&dir, "cur-1", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "cur-1");
        o.vendor = Some("cursor");
        o.resume_session = Some("sess-1");
        let err = dispatch(&r, &default_config(), &o).unwrap_err().to_string();
        assert!(err.contains("cannot resume"), "was: {err}");
        assert_eq!(*r.split_calls.borrow(), 0);
        assert!(!dir.join("cur-1.prompt.md").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn interactive_with_model_effort_or_resume_errors() {
        let dir = tdir("hd-orch-int-flags");
        let r = fake(&dir, "int-1", WriteMode::Complete, "ok");
        for flag in ["--model", "--effort", "--resume-session"] {
            let mut o = opts(&dir, "int-1");
            o.interactive = true;
            o.vendor = Some("claude");
            match flag {
                "--model" => o.model = Some("opus"),
                "--effort" => o.effort = Some("high"),
                _ => o.resume_session = Some("s"),
            }
            let err = dispatch(&r, &default_config(), &o).unwrap_err().to_string();
            assert!(err.contains(flag), "{flag}: {err}");
        }
        assert_eq!(*r.split_calls.borrow(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_effort_errors_before_any_pane() {
        let dir = tdir("hd-orch-bad-effort");
        let r = fake(&dir, "eff-1", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "eff-1");
        o.vendor = Some("claude");
        o.effort = Some("bogus");
        let err = dispatch(&r, &default_config(), &o).unwrap_err().to_string();
        assert!(err.contains("valid levels"), "was: {err}");
        assert_eq!(*r.split_calls.borrow(), 0);
        assert!(!dir.join("eff-1.prompt.md").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn effort_with_defaulted_codex_vendor_errors_before_any_pane() {
        let dir = tdir("hd-orch-codex-effort");
        let r = fake(&dir, "eff-2", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "eff-2");
        o.effort = Some("high");
        let err = dispatch(&r, &default_config(), &o).unwrap_err().to_string();
        assert!(err.contains("--effort"), "was: {err}");
        assert_eq!(*r.split_calls.borrow(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn effort_reaches_the_run_command() {
        let dir = tdir("hd-orch-effort-ok");
        let r = fake(&dir, "eff-3", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "eff-3");
        o.role = "implementer";
        o.model = Some("opus");
        o.effort = Some("high");
        dispatch(&r, &default_config(), &o).unwrap();
        let commands = r.commands.borrow();
        assert!(commands[0].contains("--effort 'high'"), "was: {}", commands[0]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_cwd_errors_before_any_pane() {
        let dir = tdir("hd-orch-nocwd");
        let r = fake(&dir, "cwd-1", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "cwd-1");
        o.cwd = Some("/definitely/not/here");
        let err = dispatch(&r, &default_config(), &o).unwrap_err().to_string();
        assert!(err.contains("/definitely/not/here"), "was: {err}");
        assert_eq!(*r.split_calls.borrow(), 0);
        assert!(!dir.join("cwd-1.prompt.md").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn relative_cwd_and_scratch_become_absolute() {
        let base = tdir("hd-orch-rel");
        std::fs::create_dir_all(base.join("wt")).unwrap();
        // express both as paths relative to the process directory
        let proc_dir = std::env::current_dir().unwrap();
        let rel = |p: std::path::PathBuf| {
            let mut out = std::path::PathBuf::new();
            for _ in proc_dir.components().skip(1) { out.push(".."); }
            out.join(p.strip_prefix("/").unwrap())
        };
        let rel_wt = rel(base.join("wt"));
        let rel_scratch = rel(base.join("scratch"));
        let scratch_abs = base.join("scratch");
        std::fs::create_dir_all(&scratch_abs).unwrap();
        let r = fake(&scratch_abs, "rel-1", WriteMode::Complete, "ok");
        let mut o = opts(&rel_scratch, "rel-1");
        o.cwd = Some(rel_wt.to_str().unwrap());
        dispatch(&r, &default_config(), &o).unwrap();
        let cwd = r.split_cwd.borrow().clone().unwrap();
        assert!(std::path::Path::new(&cwd).is_absolute(), "was: {cwd}");
        let commands = r.commands.borrow();
        let prompt_arg = commands[0].split('\'').nth(1).unwrap();
        assert!(std::path::Path::new(prompt_arg).is_absolute(), "was: {prompt_arg}");
        let prompt = std::fs::read_to_string(scratch_abs.join("rel-1.prompt.md")).unwrap();
        let result_line = prompt.lines().find(|l| l.contains("rel-1.result.md")).unwrap();
        assert!(result_line.contains("`/"), "was: {result_line}");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn cwd_defaults_to_process_directory_for_a_new_pane() {
        let dir = tdir("hd-orch-cwd-default");
        let r = fake(&dir, "cwd-2", WriteMode::Complete, "ok");
        dispatch(&r, &default_config(), &opts(&dir, "cwd-2")).unwrap();
        let expected = std::env::current_dir().unwrap().to_string_lossy().into_owned();
        assert_eq!(r.split_cwd.borrow().as_deref(), Some(expected.as_str()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reused_pane_cds_to_cwd_first() {
        let dir = tdir("hd-orch-reuse-cd");
        let wt = dir.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let r = fake(&dir, "cwd-3", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "cwd-3");
        o.reuse_pane = Some("w1:p9");
        o.cwd = Some(wt.to_str().unwrap());
        dispatch(&r, &default_config(), &o).unwrap();
        let commands = r.commands.borrow();
        let prefix = format!("cd '{}' && ", wt.to_string_lossy());
        assert!(commands[0].starts_with(&prefix), "was: {}", commands[0]);
        assert_eq!(*r.split_calls.borrow(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resume_runs_resume_command_with_model() {
        let dir = tdir("hd-orch-resume-cmd");
        let r = fake(&dir, "res-2", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "res-2");
        o.role = "implementer";
        o.vendor = Some("claude");
        o.model = Some("opus");
        o.resume_session = Some("sess-1");
        dispatch(&r, &default_config(), &o).unwrap();
        let commands = r.commands.borrow();
        assert!(commands[0].contains("claude -p --resume 'sess-1'"), "was: {}", commands[0]);
        assert!(commands[0].contains("--model 'opus'"), "was: {}", commands[0]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resume_with_closed_reuse_pane_splits_new_pane_in_cwd() {
        let dir = tdir("hd-orch-resume-closed");
        let mut r = fake(&dir, "res-3", WriteMode::Complete, "ok");
        r.pane_exists_result = false;
        let mut o = opts(&dir, "res-3");
        o.role = "implementer";
        o.vendor = Some("claude");
        o.model = Some("opus");
        o.resume_session = Some("sess-1");
        o.reuse_pane = Some("w1:p9");
        dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(*r.split_calls.borrow(), 1);
        assert!(r.split_cwd.borrow().is_some());
        assert!(r.commands.borrow()[0].contains("--resume"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_stores_given_cwd() {
        let dir = tdir("hd-orch-record-cwd");
        let wt = dir.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let state = dir.join("state");
        let r = fake(&dir, "rec-1", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "rec-1");
        o.cwd = Some(wt.to_str().unwrap());
        o.state_dir = Some(&state);
        dispatch(&r, &default_config(), &o).unwrap();
        let rec = crate::state::read_record(&state, "rec-1").unwrap().unwrap();
        assert_eq!(rec.cwd, wt.to_string_lossy());
        std::fs::remove_dir_all(&dir).ok();
    }
}
