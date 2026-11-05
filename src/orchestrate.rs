use crate::config::{Config, VendorSpec};
use crate::protocol::{self, Capture};
use crate::runner::HerdrRunner;
use crate::state::{write_record, DispatchRecord};
use crate::vendors::resolve;
use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use serde_json::Value;
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
    pub placement: &'a str,
    pub keep_open: bool,
    pub headless: bool,
    pub state_dir: Option<&'a Path>,
    pub reuse_pane: Option<&'a str>,
    pub mode: &'a str,
    pub interactive: bool,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub resume_session: Option<&'a str>,
    pub cwd: Option<&'a str>,
    pub exit_grace_ms: u64,
}

#[derive(Serialize, Debug)]
pub struct DispatchResult {
    pub status: String,
    pub task_id: String,
    pub vendor: String,
    pub pane_id: String,
    pub result_file: String,
    pub result_source: String,
    pub session_id: Option<String>,
    pub log_file: String,
    pub agent_exited: bool,
    // Last, so that the identity fields stay visible when the JSON is cut short.
    pub result_text: String,
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

enum HeadlessOutcome {
    /// Complete result and whether the agent's exit showed up within the grace.
    Done { text: String, exited: bool },
    /// Agent exited (marker present or pane gone) with no complete result.
    Exited,
    /// The real interface ended its turn with no complete result, and was told to quit.
    TurnEnded { text: String, exited: bool },
    Deadline,
}

fn read_complete(result_file: &Path, task_id: &str) -> Option<String> {
    let text = protocol::read_lossy(result_file)?;
    protocol::extract_complete_result(&text, task_id)
}

/// Asking herdr spawns a process, so this checks less often than the file polls.
const PANE_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// Whether the pane has gone away, checking herdr at most once per interval. A
/// herdr error is not evidence that the pane is gone.
struct PaneWatch<'a, R: HerdrRunner> {
    runner: &'a R,
    pane: &'a str,
    next_check: Instant,
}

impl<'a, R: HerdrRunner> PaneWatch<'a, R> {
    fn new(runner: &'a R, pane: &'a str) -> Self {
        PaneWatch { runner, pane, next_check: Instant::now() }
    }

    fn gone(&mut self) -> bool {
        if Instant::now() < self.next_check {
            return false;
        }
        self.next_check = Instant::now() + PANE_CHECK_INTERVAL;
        matches!(self.runner.pane_exists(self.pane), Ok(false))
    }
}

/// Wait up to `grace_ms` for the exit marker or the pane going away (always
/// checks at least once).
fn exited_within_grace<R: HerdrRunner>(
    panes: &mut PaneWatch<R>,
    marker: &Path,
    grace_ms: u64,
) -> bool {
    let deadline = Instant::now() + Duration::from_millis(grace_ms);
    loop {
        if marker.exists() || panes.gone() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Poll for a complete result, the agent's exit, or the deadline. The agent has
/// exited if the marker appeared or its pane is gone.
fn poll_headless<R: HerdrRunner>(
    runner: &R,
    pane: &str,
    result_file: &Path,
    marker: &Path,
    task_id: &str,
    timeout_ms: u64,
    grace_ms: u64,
) -> HeadlessOutcome {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let poll = Duration::from_millis(200);
    let mut panes = PaneWatch::new(runner, pane);
    loop {
        if let Some(text) = read_complete(result_file, task_id) {
            let exited = exited_within_grace(&mut panes, marker, grace_ms);
            return HeadlessOutcome::Done { text, exited };
        }
        if marker.exists() || panes.gone() {
            // The agent may have finished between the two reads.
            return match read_complete(result_file, task_id) {
                Some(text) => HeadlessOutcome::Done { text, exited: true },
                None => HeadlessOutcome::Exited,
            };
        }
        if Instant::now() >= deadline {
            return HeadlessOutcome::Deadline;
        }
        std::thread::sleep(poll);
    }
}

fn read_hook(path: &Path) -> Option<Value> {
    serde_json::from_str(&protocol::read_lossy(path)?).ok()
}

/// The Stop hook's report, unless the agent is only pausing for background work.
fn finished_turn(path: &Path) -> Option<Value> {
    let report = read_hook(path)?;
    let waiting = report.get("background_tasks")
        .and_then(Value::as_array)
        .is_some_and(|tasks| !tasks.is_empty());
    (!waiting).then_some(report)
}

fn last_words(report: &Value) -> Option<&str> {
    report.get("last_assistant_message")?.as_str()
}

struct TuiFiles<'a> {
    result: &'a Path,
    marker: &'a Path,
    turn_ended: &'a Path,
    turn_failed: &'a Path,
}

/// The real interface stays open after its turn, so tell it to quit. A failed send
/// only means that the agent is reported as still running.
fn quit_agent<R: HerdrRunner>(
    runner: &R,
    panes: &mut PaneWatch<R>,
    pane: &str,
    marker: &Path,
    grace_ms: u64,
) -> bool {
    let _ = runner.run(pane, "/exit");
    exited_within_grace(panes, marker, grace_ms)
}

/// Poll the real interface for a complete result, the end of its turn, its exit or
/// the deadline. It is only told to quit once its turn is over.
fn poll_tui<R: HerdrRunner>(
    runner: &R,
    pane: &str,
    files: &TuiFiles,
    task_id: &str,
    timeout_ms: u64,
    grace_ms: u64,
) -> HeadlessOutcome {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut panes = PaneWatch::new(runner, pane);
    loop {
        if let Some(text) = read_complete(files.result, task_id) {
            let settle = Instant::now() + Duration::from_millis(grace_ms);
            loop {
                if files.marker.exists() || panes.gone() {
                    return HeadlessOutcome::Done { text, exited: true };
                }
                if finished_turn(files.turn_ended).is_some() || files.turn_failed.exists() {
                    let exited = quit_agent(runner, &mut panes, pane, files.marker, grace_ms);
                    return HeadlessOutcome::Done { text, exited };
                }
                if Instant::now() >= settle {
                    return HeadlessOutcome::Done { text, exited: false };
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        if files.marker.exists() || panes.gone() {
            return match read_complete(files.result, task_id) {
                Some(text) => HeadlessOutcome::Done { text, exited: true },
                None => HeadlessOutcome::Exited,
            };
        }
        let turn_over = read_hook(files.turn_failed)
            .map(|report| match report.get("error").and_then(Value::as_str) {
                Some(error) => format!("{error}: {}", last_words(&report).unwrap_or("")),
                None => last_words(&report).unwrap_or("the turn ended in an error").to_string(),
            })
            .or_else(|| {
                finished_turn(files.turn_ended).map(|report| {
                    last_words(&report).unwrap_or("the turn ended with no result").to_string()
                })
            });
        if let Some(text) = turn_over {
            // The agent writes its result before its turn ends, so check once more.
            return match read_complete(files.result, task_id) {
                Some(result) => {
                    let exited = quit_agent(runner, &mut panes, pane, files.marker, grace_ms);
                    HeadlessOutcome::Done { text: result, exited }
                }
                None => {
                    let exited = quit_agent(runner, &mut panes, pane, files.marker, grace_ms);
                    HeadlessOutcome::TurnEnded { text, exited }
                }
            };
        }
        if Instant::now() >= deadline {
            return HeadlessOutcome::Deadline;
        }
        std::thread::sleep(Duration::from_millis(200));
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
    if !matches!(opts.placement, "tab" | "split") {
        bail!("unknown placement {:?}; use tab or split", opts.placement);
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
    let exited_file = protocol::exited_path(&scratch, opts.task_id);
    let turn_ended_file = protocol::turn_ended_path(&scratch, opts.task_id);
    let turn_failed_file = protocol::turn_failed_path(&scratch, opts.task_id);
    let settings_file = protocol::settings_path(&scratch, opts.task_id);
    // The real interface takes its session ID up front, so there is no log to parse it from.
    let tui_session = if !opts.interactive && !opts.headless && spec.tui_command.is_some() {
        Some(match opts.resume_session {
            Some(id) => id.to_string(),
            None => crate::session::new_session_id()?,
        })
    } else {
        None
    };
    // The CLI saves a read-only agent's final message, so there is no result marker.
    let read_only_base = if opts.interactive || opts.role != "reviewer" {
        None
    } else {
        crate::config::read_only_command(
            spec,
            &prompt_file.to_string_lossy(),
            &result_file.to_string_lossy(),
            opts.model,
            opts.effort,
            opts.resume_session,
        )
        .transpose()?
    };
    let read_only = read_only_base.is_some();
    let base_command = match (&tui_session, opts.interactive) {
        (_, true) => None,
        _ if read_only => read_only_base,
        (Some(session), _) => Some(crate::config::tui_command(spec, &crate::config::TuiRun {
            first_message: &protocol::first_message(&prompt_file),
            settings_file: &settings_file.to_string_lossy(),
            model: opts.model,
            effort: opts.effort,
            session,
            resuming: opts.resume_session.is_some(),
        })?),
        (None, _) => Some(crate::config::headless_command(
            spec,
            &prompt_file.to_string_lossy(),
            opts.model,
            opts.effort,
            opts.resume_session,
        )?),
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
    // A stale exit marker would make this run look like it already exited.
    let _ = std::fs::remove_file(&exited_file);
    // A stale turn report would make this run look like its turn already ended.
    let _ = std::fs::remove_file(&turn_ended_file);
    let _ = std::fs::remove_file(&turn_failed_file);
    if tui_session.is_some() {
        std::fs::write(&settings_file, protocol::hook_settings(&turn_ended_file, &turn_failed_file))?;
    }
    let prompt = if read_only {
        protocol::build_read_only_prompt(opts.context)
    } else {
        protocol::build_prompt(opts.context, opts.task_id, &result_file.to_string_lossy(), opts.role)
    };
    std::fs::write(&prompt_file, prompt)?;
    record(opts, &vendor, "", "dispatched", &created_at, None, &cwd);

    let Some(base) = base_command else {
        return dispatch_interactive(runner, spec, opts, &vendor, &prompt_file, &result_file, &log_file, &created_at, &cwd);
    };

    let mut reused = false;
    let pane_id = match opts.reuse_pane {
        Some(p) if runner.pane_exists(p)? => {
            reused = true;
            p.to_string()
        }
        _ if opts.placement == "tab" => runner.open_tab(opts.from_pane, opts.task_id, Some(&cwd))?,
        _ => protocol::pane_id_from_split(
                 &runner.split(opts.from_pane, opts.split_direction, false, Some(&cwd))?,
             )?,
    };

    let mk = |status: &str, source: &str, text: String| DispatchResult {
        status: status.into(), task_id: opts.task_id.into(), vendor: vendor.clone(),
        pane_id: pane_id.clone(), result_file: result_file.to_string_lossy().into(),
        result_source: source.into(), result_text: text, session_id: None,
        log_file: log_file.to_string_lossy().into(), agent_exited: false,
    };

    record(opts, &vendor, &pane_id, "working", &created_at, tui_session.as_deref(), &cwd);

    // Run the agent command; it writes its complete result to
    // result_file, whose last line becomes the sentinel marker. Prompt echo
    // in headless agent output can spoof a scrollback-matched sentinel, so
    // completion is detected by polling the result file itself rather than
    // watching the pane.
    let command = protocol::pane_command(
        reused.then_some(cwd.as_str()),
        &base,
        tui_session.is_none().then(|| log_file.to_string_lossy()).as_deref(),
        &exited_file.to_string_lossy(),
    );
    // herdr may have delivered the command before it stopped answering.
    runner.run(&pane_id, &command).map_err(|e| {
        anyhow!("{e}; the agent may already be running in pane {pane_id}, so check that pane before dispatching again")
    })?;

    let tail = protocol::bound_output;
    let outcome = if tui_session.is_some() {
        let files = TuiFiles {
            result: &result_file,
            marker: &exited_file,
            turn_ended: &turn_ended_file,
            turn_failed: &turn_failed_file,
        };
        poll_tui(runner, &pane_id, &files, opts.task_id, opts.timeout_ms, opts.exit_grace_ms)
    } else {
        poll_headless(runner, &pane_id, &result_file, &exited_file, opts.task_id, opts.timeout_ms, opts.exit_grace_ms)
    };
    let mut result = match outcome {
        HeadlessOutcome::Done { text, exited } => {
            let mut r = mk("done", "file", text);
            r.agent_exited = exited;
            r
        }
        HeadlessOutcome::Exited => {
            let saved = protocol::read_lossy(&result_file);
            let mut r = match saved.filter(|t| !read_only || !t.trim().is_empty()) {
                Some(t) if read_only => mk("done", "file", t.trim().to_string()),
                Some(t) => mk("failed", "file", tail(&t)),
                // The real interface has no log, so the pane is all there is to show.
                None if tui_session.is_some() => {
                    let shown = runner.read(&pane_id, "recent-unwrapped", 200).unwrap_or_default();
                    let text = if shown.trim().is_empty() {
                        "the agent exited without writing a result, and its pane shows nothing".to_string()
                    } else {
                        tail(&shown)
                    };
                    mk("failed", "pane", text)
                }
                None => {
                    let log = protocol::read_lossy(&log_file).unwrap_or_default();
                    mk("failed", "log", tail(&log))
                }
            };
            r.agent_exited = true;
            r
        }
        HeadlessOutcome::TurnEnded { text, exited } => {
            let mut r = match protocol::read_lossy(&result_file) {
                Some(partial) => mk("failed", "file", tail(&partial)),
                None => mk("failed", "hook", tail(&text)),
            };
            r.agent_exited = exited;
            r
        }
        HeadlessOutcome::Deadline => {
            // A herdr failure here must not lose the file or leave the record at working.
            let scrollback = runner.read(&pane_id, "recent-unwrapped", 200).ok();
            match protocol::capture_result(&result_file, scrollback.as_deref()) {
                Capture::File(t) => mk("degraded", "file", tail(&t)),
                Capture::Scrollback(t) => mk("timeout", "scrollback", tail(&t)),
                Capture::Missing => mk("timeout", "scrollback", String::new()),
            }
        }
    };
    let session_id = tui_session.clone().or_else(|| {
        protocol::read_lossy(&log_file).and_then(|log| crate::session::extract_session_id(&vendor, &log))
    });
    result.session_id = session_id.clone();
    if tui_session.is_some() {
        // The real interface has no log of its own; its transcript takes that place.
        let transcript = [&turn_failed_file, &turn_ended_file].into_iter()
            .filter_map(|path| read_hook(path))
            .find_map(|report| report.get("transcript_path")?.as_str().map(String::from));
        if let Some(path) = transcript {
            result.log_file = path;
        }
    }
    record(opts, &vendor, &pane_id, &result.status, &created_at, session_id.as_deref(), &cwd);

    // Only a clean finish goes away. Anything else may need reading, or may still be running.
    if !opts.keep_open && result.status == "done" && result.agent_exited {
        // The work is done and recorded, so a pane left open is only a warning.
        if let Err(e) = runner.close(&pane_id) {
            eprintln!("warning: could not close pane {pane_id}: {e}");
        }
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
    log_file: &Path,
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
        log_file: log_file.to_string_lossy().into(), agent_exited: false,
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
        /// from this `pane_exists` call on (1-based), the pane is gone
        pane_gone_from_call: Option<u32>,
        pane_exists_errors: bool,
        /// the agent completes its result just as the pane goes away
        completes_as_pane_goes: bool,
        pane_exists_calls: RefCell<u32>,
        commands: RefCell<Vec<String>>,
        exits: bool,
        log_text: String,
        log_bytes: Option<Vec<u8>>,
        result_bytes: Option<Vec<u8>>,
        scrollback: String,
        read_errors: bool,
        close_errors: bool,
        run_errors: bool,
        tab_opens: RefCell<Vec<(String, String, Option<String>)>>,
        /// JSON the real interface's Stop hook writes when the turn ends
        turn_end: Option<String>,
        /// JSON its StopFailure hook writes when the turn ends in an error
        turn_failure: Option<String>,
        /// the agent leaves its exit marker when it is sent /exit
        quits: bool,
        /// what a read-only CLI saves to the result file when it exits
        final_message: Option<String>,
    }
    impl HerdrRunner for FakeRunner {
        fn split(&self, _p: &str, _d: &str, _f: bool, cwd: Option<&str>) -> anyhow::Result<String> {
            *self.split_cwd.borrow_mut() = cwd.map(String::from);
            *self.split_calls.borrow_mut() += 1;
            Ok(format!(r#"{{"result":{{"pane":{{"pane_id":"{}"}}}}}}"#, self.pane_id))
        }
        fn run(&self, _pane: &str, command: &str) -> anyhow::Result<()> {
            if self.run_errors {
                anyhow::bail!("herdr pane run timed out");
            }
            self.commands.borrow_mut().push(command.to_string());
            if command == "/exit" && self.quits {
                std::fs::write(protocol::exited_path(&self.scratch, &self.task_id), "").unwrap();
            }
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
                if let Some(bytes) = &self.result_bytes {
                    std::fs::write(result_path(&self.scratch, &self.task_id), bytes).unwrap();
                }
                let log = crate::protocol::log_path(&self.scratch, &self.task_id);
                match &self.log_bytes {
                    Some(bytes) => std::fs::write(log, bytes).unwrap(),
                    None => std::fs::write(log, &self.log_text).unwrap(),
                }
                if let Some(text) = &self.final_message {
                    std::fs::write(result_path(&self.scratch, &self.task_id), text).unwrap();
                }
                if let Some(payload) = &self.turn_end {
                    std::fs::write(self.scratch.join(format!("{}.stop.json", self.task_id)), payload).unwrap();
                }
                if let Some(payload) = &self.turn_failure {
                    std::fs::write(self.scratch.join(format!("{}.stopfailure.json", self.task_id)), payload).unwrap();
                }
                if self.exits {
                    std::fs::write(protocol::exited_path(&self.scratch, &self.task_id), "").unwrap();
                }
            }
            Ok(())
        }
        // no longer exercised by dispatch (completion is now polled from the
        // result file), kept trivial to satisfy the trait
        fn wait_output(&self, _p: &str, _matcher: &str, _t: u64) -> anyhow::Result<bool> {
            Ok(true)
        }
        fn read(&self, _p: &str, _s: &str, _l: u32) -> anyhow::Result<String> {
            if self.read_errors {
                anyhow::bail!("herdr pane read timed out");
            }
            Ok(self.scrollback.clone())
        }
        fn close(&self, pane: &str) -> anyhow::Result<()> {
            if self.close_errors {
                anyhow::bail!("herdr pane close timed out");
            }
            self.closed.borrow_mut().push(pane.into()); Ok(())
        }
        fn open_tab(&self, from: &str, label: &str, cwd: Option<&str>) -> anyhow::Result<String> {
            self.tab_opens.borrow_mut().push((from.into(), label.into(), cwd.map(String::from)));
            Ok(self.pane_id.clone())
        }
        fn pane_exists(&self, _pane: &str) -> anyhow::Result<bool> {
            *self.pane_exists_calls.borrow_mut() += 1;
            if self.pane_exists_errors {
                anyhow::bail!("herdr unreachable");
            }
            if self.pane_gone_from_call.is_some_and(|n| *self.pane_exists_calls.borrow() >= n) {
                if self.completes_as_pane_goes {
                    let content = format!("{}\n{}", self.result_text, protocol::sentinel(&self.task_id));
                    std::fs::write(result_path(&self.scratch, &self.task_id), content).unwrap();
                }
                return Ok(false);
            }
            Ok(self.pane_exists_result)
        }
    }

    fn opts<'a>(scratch: &'a std::path::Path, task_id: &'a str) -> DispatchOpts<'a> {
        DispatchOpts {
            context: "Review the diff.", role: "reviewer", from_pane: "1-2",
            scratch, task_id, vendor: None, unavailable: HashSet::new(),
            timeout_ms: 400, split_direction: "right",
            placement: "split", keep_open: true, headless: false, state_dir: None, reuse_pane: None, mode: "headless",
            interactive: false,
            model: None, effort: None, resume_session: None, cwd: None,
            exit_grace_ms: 0,
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
            pane_gone_from_call: None, pane_exists_errors: false, completes_as_pane_goes: false,
            pane_exists_calls: RefCell::new(0),
            commands: RefCell::new(vec![]),
            exits: false,
            log_text: "session id: 11111111-2222-3333-4444-555555555555\nwork output\n".into(),
            log_bytes: None,
            result_bytes: None,
            scrollback: "scrollback".into(),
            read_errors: false,
            close_errors: false,
            run_errors: false,
            tab_opens: RefCell::new(vec![]),
            turn_end: None,
            turn_failure: None,
            quits: false,
            final_message: None,
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
        let mut r = r; r.exits = true;
        let mut o = opts(&dir, "rev-4"); o.keep_open = false;
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
        let inner = script(&commands[0]);
        assert!(inner.contains("| tee "), "command was: {}", commands[0]);
        assert!(inner.contains(&quoted_log), "command was: {}", commands[0]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The script a pane command hands to `sh -c`, with one quoting level removed.
    fn script(command: &str) -> String {
        let body = command.strip_prefix("clear; sh -c '").unwrap_or_else(|| panic!("was: {command}"));
        let end = body.rfind("' sh '").unwrap_or_else(|| panic!("was: {command}"));
        body[..end].replace(r"'\''", "'")
    }

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn command_touches_marker_after_tee() {
        let dir = std::env::temp_dir().join("hd-orch-touch");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake(&dir, "rev-touch", WriteMode::Complete, "ok");
        dispatch(&r, &default_config(), &opts(&dir, "rev-touch")).unwrap();
        let marker = protocol::exited_path(&dir, "rev-touch");
        let want = format!(" sh '{}'", marker.to_string_lossy());
        let commands = r.commands.borrow();
        assert!(commands[0].ends_with(&want), "command was: {}", commands[0]);
        // The trap is set before the pipeline starts, so it covers the whole run.
        let inner = script(&commands[0]);
        assert!(inner.find(r#"trap 'touch "$1"' EXIT"#).unwrap() < inner.find("| tee ").unwrap(), "was: {inner}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stale_marker_is_cleared_before_dispatch() {
        let dir = std::env::temp_dir().join("hd-orch-stale-marker");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(protocol::exited_path(&dir, "rev-sm"), "").unwrap();
        let r = fake(&dir, "rev-sm", WriteMode::Nothing, "");
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-sm")).unwrap();
        assert_eq!(out.status, "timeout");
        assert!(!out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn result_and_marker_together_is_done() {
        let dir = std::env::temp_dir().join("hd-orch-both");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mut r = fake(&dir, "rev-both", WriteMode::Complete, "fine");
        r.exits = true;
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-both")).unwrap();
        assert_eq!(out.status, "done");
        assert_eq!(out.result_text, "fine");
        assert!(out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn done_without_marker_reports_agent_not_exited() {
        let dir = std::env::temp_dir().join("hd-orch-nomarker");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake(&dir, "rev-nm", WriteMode::Complete, "fine");
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-nm")).unwrap();
        assert_eq!(out.status, "done");
        assert!(!out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exit_without_result_fails_immediately_with_log_tail() {
        let dir = std::env::temp_dir().join("hd-orch-exit-nores");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mut r = fake(&dir, "rev-en", WriteMode::Nothing, "");
        r.exits = true;
        r.log_text = numbered(100);
        let mut o = opts(&dir, "rev-en");
        o.timeout_ms = 60_000;
        let t = Instant::now();
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert!(t.elapsed() < Duration::from_secs(5));
        assert_eq!(out.status, "failed");
        assert_eq!(out.result_source, "log");
        assert!(out.agent_exited);
        assert_eq!(out.result_text.lines().count(), 40);
        assert!(out.result_text.ends_with("line 100"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exit_with_partial_file_is_failed() {
        let dir = std::env::temp_dir().join("hd-orch-exit-partial");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mut r = fake(&dir, "rev-ep", WriteMode::Incomplete, "half done");
        r.exits = true;
        let mut o = opts(&dir, "rev-ep");
        o.vendor = Some("cursor");
        o.timeout_ms = 60_000;
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(out.status, "failed");
        assert_eq!(out.result_source, "file");
        assert_eq!(out.result_text, "half done");
        assert!(out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn timeout_bounds_scrollback_to_40_lines() {
        let dir = std::env::temp_dir().join("hd-orch-sb40");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mut r = fake(&dir, "rev-sb", WriteMode::Nothing, "");
        r.scrollback = numbered(100);
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-sb")).unwrap();
        assert_eq!(out.status, "timeout");
        assert_eq!(out.result_text.lines().count(), 40);
        assert!(out.result_text.ends_with("line 100"));
        assert!(!out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn degraded_bounds_partial_file_to_40_lines() {
        let dir = std::env::temp_dir().join("hd-orch-deg40");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake(&dir, "rev-d40", WriteMode::Incomplete, &numbered(100));
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-d40")).unwrap();
        assert_eq!(out.status, "degraded");
        assert_eq!(out.result_text.lines().count(), 40);
        assert!(!out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failed_bounds_a_very_long_log_line_to_8_kib() {
        let dir = tdir("hd-orch-long-log");
        let mut r = fake(&dir, "rev-ll", WriteMode::Nothing, "");
        r.exits = true;
        r.log_text = format!("{}END", "x".repeat(100_000));
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-ll")).unwrap();
        assert_eq!(out.status, "failed");
        assert!(out.result_text.len() <= 8192, "was {} bytes", out.result_text.len());
        assert!(out.result_text.starts_with(protocol::OUTPUT_TRUNCATED_MARKER));
        assert!(out.result_text.ends_with("xEND"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn timeout_bounds_a_very_long_scrollback_line_to_8_kib() {
        let dir = tdir("hd-orch-long-sb");
        let mut r = fake(&dir, "rev-ls", WriteMode::Nothing, "");
        r.scrollback = "s".repeat(100_000);
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-ls")).unwrap();
        assert_eq!(out.status, "timeout");
        assert!(out.result_text.len() <= 8192, "was {} bytes", out.result_text.len());
        assert!(out.result_text.starts_with(protocol::OUTPUT_TRUNCATED_MARKER));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn degraded_bounds_a_very_long_partial_file_line_to_8_kib() {
        let dir = tdir("hd-orch-long-deg");
        let r = fake(&dir, "rev-ld", WriteMode::Incomplete, &"p".repeat(100_000));
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-ld")).unwrap();
        assert_eq!(out.status, "degraded");
        assert!(out.result_text.len() <= 8192, "was {} bytes", out.result_text.len());
        assert!(out.result_text.starts_with(protocol::OUTPUT_TRUNCATED_MARKER));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_utf8_log_still_gives_text_and_session_id() {
        let dir = tdir("hd-orch-bad-log");
        let mut r = fake(&dir, "rev-bl", WriteMode::Nothing, "");
        r.exits = true;
        r.log_bytes = Some(
            b"session id: 11111111-2222-3333-4444-555555555555\nbad \xff byte\nthe error\n".to_vec(),
        );
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-bl")).unwrap();
        assert_eq!(out.status, "failed");
        assert_eq!(out.result_source, "log");
        assert!(out.result_text.contains("the error"), "was: {}", out.result_text);
        assert_eq!(out.session_id.as_deref(), Some("11111111-2222-3333-4444-555555555555"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn complete_result_with_invalid_utf8_is_done() {
        let dir = tdir("hd-orch-bad-result");
        let mut r = fake(&dir, "rev-br", WriteMode::Nothing, "");
        r.result_bytes = Some(b"DONE \xff report\nDONE::rev-br".to_vec());
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-br")).unwrap();
        assert_eq!(out.status, "done");
        assert_eq!(out.result_text, "DONE \u{fffd} report");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn json_puts_identity_fields_before_result_text() {
        let dir = tdir("hd-orch-json-order");
        let r = fake(&dir, "rev-jo", WriteMode::Complete, "ok");
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-jo")).unwrap();
        let json = serde_json::to_string(&out).unwrap();
        let at = |key: &str| json.find(&format!("\"{key}\":")).unwrap();
        for key in ["session_id", "log_file", "agent_exited"] {
            assert!(at(key) < at("result_text"), "{key} after result_text: {json}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pane_gone_without_result_is_failed_and_exited() {
        let dir = tdir("hd-orch-gone");
        let mut r = fake(&dir, "rev-g", WriteMode::Nothing, "");
        r.pane_gone_from_call = Some(1);
        r.log_text = "the last words\n".into();
        let mut o = opts(&dir, "rev-g");
        o.timeout_ms = 1500;
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(out.status, "failed");
        assert_eq!(out.result_source, "log");
        assert_eq!(out.result_text, "the last words");
        assert!(out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pane_gone_with_partial_file_is_failed_from_the_file() {
        let dir = tdir("hd-orch-gone-partial");
        let mut r = fake(&dir, "rev-gp", WriteMode::Incomplete, "half done");
        r.pane_gone_from_call = Some(1);
        let mut o = opts(&dir, "rev-gp");
        o.vendor = Some("cursor");
        o.timeout_ms = 1500;
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(out.status, "failed");
        assert_eq!(out.result_source, "file");
        assert_eq!(out.result_text, "half done");
        assert!(out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pane_going_away_later_in_the_run_is_noticed() {
        let dir = tdir("hd-orch-gone-later");
        let mut r = fake(&dir, "rev-gl", WriteMode::Nothing, "");
        r.pane_gone_from_call = Some(2);
        let mut o = opts(&dir, "rev-gl");
        o.timeout_ms = 3000;
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(out.status, "failed");
        assert!(out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn result_completed_as_the_pane_goes_is_done() {
        let dir = tdir("hd-orch-gone-done");
        let mut r = fake(&dir, "rev-gd", WriteMode::Nothing, "made it");
        r.pane_gone_from_call = Some(1);
        r.completes_as_pane_goes = true;
        let mut o = opts(&dir, "rev-gd");
        o.timeout_ms = 1500;
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(out.status, "done");
        assert_eq!(out.result_text, "made it");
        assert!(out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pane_gone_during_the_grace_wait_counts_as_exited() {
        let dir = tdir("hd-orch-gone-grace");
        let mut r = fake(&dir, "rev-gg", WriteMode::Complete, "fine");
        r.pane_gone_from_call = Some(1);
        let mut o = opts(&dir, "rev-gg");
        o.exit_grace_ms = 1500;
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(out.status, "done");
        assert!(out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pane_exists_error_does_not_count_as_gone() {
        let dir = tdir("hd-orch-pane-err");
        let mut r = fake(&dir, "rev-pe", WriteMode::Nothing, "");
        r.pane_exists_errors = true;
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-pe")).unwrap();
        assert_eq!(out.status, "timeout");
        assert!(!out.agent_exited);
        assert!(*r.pane_exists_calls.borrow() >= 1, "polling never checked the pane");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn herdr_failing_at_the_deadline_still_reports_timeout_and_updates_the_record() {
        let dir = tdir("hd-orch-read-err");
        let state = dir.join("state");
        let mut r = fake(&dir, "rev-re", WriteMode::Nothing, "");
        r.read_errors = true;
        let mut o = opts(&dir, "rev-re");
        o.state_dir = Some(&state);
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(out.status, "timeout");
        assert!(!out.agent_exited);
        assert_eq!(crate::state::read_record(&state, "rev-re").unwrap().unwrap().status, "timeout");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn herdr_failing_at_the_deadline_keeps_a_partial_result_file() {
        let dir = tdir("hd-orch-read-err-partial");
        let mut r = fake(&dir, "rev-rp", WriteMode::Incomplete, "half done");
        r.read_errors = true;
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-rp")).unwrap();
        assert_eq!(out.status, "degraded");
        assert_eq!(out.result_text, "half done");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_close_does_not_lose_the_result() {
        let dir = tdir("hd-orch-close-err");
        let mut r = fake(&dir, "rev-ce", WriteMode::Complete, "LGTM");
        r.close_errors = true;
        r.exits = true;
        let mut o = opts(&dir, "rev-ce");
        o.keep_open = false;
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(out.status, "done");
        assert_eq!(out.result_text, "LGTM");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_run_names_the_pane_the_agent_may_be_running_in() {
        let dir = tdir("hd-orch-run-err");
        let mut r = fake(&dir, "rev-ru", WriteMode::Nothing, "");
        r.run_errors = true;
        let err = dispatch(&r, &default_config(), &opts(&dir, "rev-ru")).unwrap_err().to_string();
        assert!(err.contains("1-3"), "was: {err}");
        assert!(err.contains("timed out"), "the cause was dropped: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    fn claude_opts<'a>(dir: &'a std::path::Path, task: &'a str) -> DispatchOpts<'a> {
        let mut o = opts(dir, task);
        o.role = "implementer";
        o.vendor = Some("claude");
        o.model = Some("haiku");
        o.exit_grace_ms = 300;
        o.keep_open = false;
        o
    }

    const TURN_ENDED: &str = r#"{"hook_event_name":"Stop","last_assistant_message":"I have a question.","background_tasks":[],"transcript_path":"/t/session.jsonl"}"#;
    const TURN_FAILED: &str = r#"{"hook_event_name":"StopFailure","error":"model_not_found","last_assistant_message":"There's an issue with the selected model.","transcript_path":"/t/session.jsonl"}"#;

    #[test]
    fn claude_runs_its_real_interface_with_a_session_chosen_up_front() {
        let dir = tdir("hd-orch-tui");
        let mut r = fake(&dir, "t", WriteMode::Complete, "ok");
        r.turn_end = Some(TURN_ENDED.into());
        r.quits = true;
        let out = dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap();
        let commands = r.commands.borrow();
        assert!(commands[0].starts_with("clear; "), "the sh line should not stay on screen: {}", commands[0]);
        let c = &script(&commands[0]);
        assert!(c.contains("claude --model 'haiku'"), "was: {c}");
        assert!(!c.contains(" -p"), "was: {c}");
        assert!(!c.contains("tee "), "piping would stop it being a terminal: {c}");
        assert!(!c.contains("cat "), "stdin must stay the terminal: {c}");
        assert!(c.contains("--permission-mode auto"), "was: {c}");
        assert!(!c.contains("dangerously"), "was: {c}");
        let session = out.session_id.clone().unwrap();
        assert_eq!(session.len(), 36, "was: {session}");
        assert!(c.contains(&session), "the id reported must be the one used: {c}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn two_dispatches_get_different_sessions() {
        let dir = tdir("hd-orch-tui-ids");
        let mut r = fake(&dir, "t", WriteMode::Complete, "ok");
        r.turn_end = Some(TURN_ENDED.into());
        r.quits = true;
        let a = dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap().session_id;
        let b = dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap().session_id;
        assert_ne!(a, b);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_resumed_claude_dispatch_reuses_its_session_in_the_real_interface() {
        let dir = tdir("hd-orch-tui-resume");
        let mut r = fake(&dir, "t", WriteMode::Complete, "ok");
        r.turn_end = Some(TURN_ENDED.into());
        r.quits = true;
        let mut o = claude_opts(&dir, "t");
        o.resume_session = Some("sess-1");
        let out = dispatch(&r, &default_config(), &o).unwrap();
        let commands = r.commands.borrow();
        let c = script(&commands[0]);
        assert!(c.contains("claude --resume 'sess-1'"), "was: {c}");
        assert!(!c.contains("--session-id"), "was: {c}");
        assert_eq!(out.session_id.as_deref(), Some("sess-1"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hostile_values_stay_quoted_in_the_real_interface_command() {
        let dir = tdir("hd-orch it's tui");
        let mut r = fake(&dir, "t", WriteMode::Complete, "ok");
        r.turn_end = Some(TURN_ENDED.into());
        r.quits = true;
        let mut o = claude_opts(&dir, "t");
        o.model = Some("a b'c");
        dispatch(&r, &default_config(), &o).unwrap();
        // Parsed by a real shell, the whole command must stay a single `sh -c` word.
        let commands = r.commands.borrow();
        let wrapped = commands[0].trim_start_matches("clear; ");
        let script = std::process::Command::new("sh").arg("-c")
            .arg(format!("set -- {wrapped}; printf '%s' \"$3\""))
            .output().unwrap().stdout;
        let script = String::from_utf8_lossy(&script);
        assert!(script.contains(r"--model 'a b'\''c'"), "was: {script}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_settings_file_makes_claude_report_how_each_turn_ended() {
        let dir = tdir("hd-orch-tui-hooks");
        let mut r = fake(&dir, "t", WriteMode::Complete, "ok");
        r.turn_end = Some(TURN_ENDED.into());
        r.quits = true;
        dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap();
        let commands = r.commands.borrow();
        assert!(commands[0].contains("t.settings.json"), "was: {}", commands[0]);
        let settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("t.settings.json")).unwrap()).unwrap();
        for (event, file) in [("Stop", "t.stop.json"), ("StopFailure", "t.stopfailure.json")] {
            let hook = settings["hooks"][event][0]["hooks"][0]["command"].as_str().unwrap();
            std::fs::remove_file(dir.join(file)).ok();
            let mut child = std::process::Command::new("sh").arg("-c").arg(hook)
                .stdin(std::process::Stdio::piped()).spawn().unwrap();
            use std::io::Write;
            child.stdin.take().unwrap().write_all(b"{\"payload\":1}").unwrap();
            child.wait().unwrap();
            assert_eq!(std::fs::read_to_string(dir.join(file)).unwrap(), "{\"payload\":1}", "{event}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_permission_prompt_is_answered_no_so_the_agent_never_waits_on_it() {
        let dir = tdir("hd-orch-tui-deny");
        let mut r = fake(&dir, "t", WriteMode::Complete, "ok");
        r.turn_end = Some(TURN_ENDED.into());
        r.quits = true;
        dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap();
        let settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("t.settings.json")).unwrap()).unwrap();
        let hook = settings["hooks"]["PermissionRequest"][0]["hooks"][0]["command"].as_str().unwrap();
        let out = std::process::Command::new("sh").arg("-c").arg(hook).output().unwrap();
        assert!(out.status.success());
        let answer: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(answer["hookSpecificOutput"]["hookEventName"], "PermissionRequest");
        assert_eq!(answer["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert!(answer["hookSpecificOutput"]["decision"]["message"].as_str().unwrap().contains("BLOCKED"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_codex_reviewer_runs_read_only_with_no_file_to_write() {
        let dir = tdir("hd-orch-ro");
        let mut r = fake(&dir, "t", WriteMode::Nothing, "");
        r.final_message = Some("The bug is at calc.py:2".into());
        r.exits = true;
        let mut o = opts(&dir, "t");
        o.vendor = Some("codex");
        let out = dispatch(&r, &default_config(), &o).unwrap();
        let commands = r.commands.borrow();
        let c = script(&commands[0]);
        assert!(c.contains("--sandbox read-only"), "was: {c}");
        assert!(c.contains("--output-last-message"), "was: {c}");
        let prompt = std::fs::read_to_string(protocol::prompt_path(&dir, "t")).unwrap();
        assert!(prompt.contains("Review the diff."), "was: {prompt}");
        assert!(!prompt.contains("DONE::"), "it can't write a file: {prompt}");
        assert!(!prompt.contains(".result.md"), "it can't write a file: {prompt}");
        assert_eq!(out.status, "done", "the saved answer is the result, with no marker to look for");
        assert_eq!(out.result_text, "The bug is at calc.py:2");
        assert_eq!(out.result_source, "file");
        assert!(out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_read_only_reviewer_that_saved_nothing_has_failed() {
        for saved in [None, Some("  \n")] {
            let dir = tdir("hd-orch-ro-empty");
            let mut r = fake(&dir, "t", WriteMode::Nothing, "");
            r.final_message = saved.map(String::from);
            r.exits = true;
            let out = dispatch(&r, &default_config(), &opts(&dir, "t")).unwrap();
            assert_eq!(out.status, "failed", "saved: {saved:?}");
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn a_codex_implementer_writes_its_result_file_from_the_workspace_sandbox() {
        let dir = tdir("hd-orch-cx-impl");
        let mut r = fake(&dir, "t", WriteMode::Complete, "ok");
        r.exits = true;
        let mut o = opts(&dir, "t");
        o.role = "implementer";
        o.vendor = Some("codex");
        o.model = Some("x");
        let out = dispatch(&r, &default_config(), &o).unwrap();
        let commands = r.commands.borrow();
        assert!(script(&commands[0]).contains("--sandbox workspace-write"), "was: {}", commands[0]);
        assert!(std::fs::read_to_string(protocol::prompt_path(&dir, "t")).unwrap().contains("DONE::t"));
        assert_eq!(out.status, "done");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_finished_turn_is_quit_and_its_tab_closed() {
        let dir = tdir("hd-orch-tui-done");
        let mut r = fake(&dir, "t", WriteMode::Complete, "LGTM");
        r.turn_end = Some(TURN_ENDED.into());
        r.quits = true;
        let out = dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap();
        assert_eq!(out.status, "done");
        assert_eq!(out.result_text, "LGTM");
        assert!(out.agent_exited);
        assert_eq!(out.log_file, "/t/session.jsonl", "the transcript stands in for the log");
        assert_eq!(r.commands.borrow().last().unwrap(), "/exit");
        assert_eq!(r.closed.borrow().as_slice(), ["1-3"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn keep_open_leaves_a_clean_finish_on_screen() {
        let dir = tdir("hd-orch-tui-keep");
        let mut r = fake(&dir, "t", WriteMode::Complete, "LGTM");
        r.turn_end = Some(TURN_ENDED.into());
        r.quits = true;
        let mut o = claude_opts(&dir, "t");
        o.keep_open = true;
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(out.status, "done");
        assert!(r.closed.borrow().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_result_written_mid_turn_is_never_quit_out_from_under_the_agent() {
        let dir = tdir("hd-orch-tui-midturn");
        let r = fake(&dir, "t", WriteMode::Complete, "LGTM");
        let out = dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap();
        assert_eq!(out.status, "done");
        assert!(!out.agent_exited);
        assert!(!r.commands.borrow().iter().any(|c| c == "/exit"));
        assert!(r.closed.borrow().is_empty(), "the agent is still running in that tab");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_api_error_fails_at_once_with_the_reason_and_keeps_the_tab() {
        let dir = tdir("hd-orch-tui-fail");
        let mut r = fake(&dir, "t", WriteMode::Nothing, "");
        r.turn_failure = Some(TURN_FAILED.into());
        r.quits = true;
        let mut o = claude_opts(&dir, "t");
        o.timeout_ms = 60_000;
        let start = std::time::Instant::now();
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "took {:?}", start.elapsed());
        assert_eq!(out.status, "failed");
        assert!(out.result_text.contains("model_not_found"), "was: {}", out.result_text);
        assert!(out.result_text.contains("issue with the selected model"), "was: {}", out.result_text);
        assert!(out.agent_exited, "the session was quit, so a resume is safe");
        assert_eq!(r.commands.borrow().last().unwrap(), "/exit");
        assert!(r.closed.borrow().is_empty(), "a failure stays on screen");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ending_a_turn_with_no_result_fails_at_once_with_the_agents_last_words() {
        let dir = tdir("hd-orch-tui-noresult");
        let mut r = fake(&dir, "t", WriteMode::Nothing, "");
        r.turn_end = Some(TURN_ENDED.into());
        r.quits = true;
        let mut o = claude_opts(&dir, "t");
        o.timeout_ms = 60_000;
        let start = std::time::Instant::now();
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "took {:?}", start.elapsed());
        assert_eq!(out.status, "failed");
        assert_eq!(out.result_text, "I have a question.");
        assert!(r.closed.borrow().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_turn_paused_on_background_work_has_not_ended() {
        let dir = tdir("hd-orch-tui-bg");
        let mut r = fake(&dir, "t", WriteMode::Nothing, "");
        r.turn_end = Some(r#"{"last_assistant_message":"waiting","background_tasks":[{"id":"x"}]}"#.into());
        r.quits = true;
        let out = dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap();
        assert_eq!(out.status, "timeout");
        assert!(!out.agent_exited);
        assert!(!r.commands.borrow().iter().any(|c| c == "/exit"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_timed_out_agent_is_left_running_in_its_tab() {
        let dir = tdir("hd-orch-tui-timeout");
        let mut r = fake(&dir, "t", WriteMode::Nothing, "");
        r.quits = true;
        let out = dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap();
        assert_eq!(out.status, "timeout");
        assert!(!r.commands.borrow().iter().any(|c| c == "/exit"));
        assert!(r.closed.borrow().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_agent_quit_by_hand_with_no_result_is_failed() {
        let dir = tdir("hd-orch-tui-quit");
        let mut r = fake(&dir, "t", WriteMode::Nothing, "");
        r.exits = true;
        let out = dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap();
        assert_eq!(out.status, "failed");
        assert!(out.agent_exited);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_agent_that_exits_with_no_result_reports_what_the_pane_shows() {
        let dir = tdir("hd-orch-tui-exit-text");
        let mut r = fake(&dir, "t", WriteMode::Nothing, "");
        r.exits = true;
        r.scrollback = "Error: session not found".into();
        let out = dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap();
        assert_eq!(out.status, "failed");
        assert_eq!(out.result_text, "Error: session not found");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_agent_that_exits_with_nothing_to_show_still_says_so() {
        let dir = tdir("hd-orch-tui-exit-empty");
        let mut r = fake(&dir, "t", WriteMode::Nothing, "");
        r.exits = true;
        r.read_errors = true;
        let out = dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap();
        assert_eq!(out.status, "failed");
        assert!(!out.result_text.trim().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_stale_turn_report_from_an_earlier_run_is_ignored() {
        let dir = tdir("hd-orch-tui-stale");
        std::fs::write(dir.join("t.stop.json"), TURN_ENDED).unwrap();
        std::fs::write(dir.join("t.stopfailure.json"), TURN_FAILED).unwrap();
        let r = fake(&dir, "t", WriteMode::Nothing, "");
        let out = dispatch(&r, &default_config(), &claude_opts(&dir, "t")).unwrap();
        assert_eq!(out.status, "timeout");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn headless_keeps_claude_on_dash_p_and_its_log() {
        let dir = tdir("hd-orch-headless");
        let mut r = fake(&dir, "t", WriteMode::Complete, "ok");
        r.exits = true;
        let mut o = claude_opts(&dir, "t");
        o.headless = true;
        let out = dispatch(&r, &default_config(), &o).unwrap();
        let commands = r.commands.borrow();
        assert!(commands[0].contains("claude -p"), "was: {}", commands[0]);
        assert!(commands[0].contains("tee "), "was: {}", commands[0]);
        assert!(out.log_file.ends_with("t.log"), "was: {}", out.log_file);
        assert!(!commands.iter().any(|c| c == "/exit"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_vendor_with_no_real_interface_mode_stays_headless() {
        let dir = tdir("hd-orch-codex-headless");
        let r = fake(&dir, "t", WriteMode::Complete, "ok");
        let out = dispatch(&r, &default_config(), &opts(&dir, "t")).unwrap();
        assert_eq!(out.vendor, "codex");
        assert!(r.commands.borrow()[0].contains("tee "), "was: {}", r.commands.borrow()[0]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_headless_failure_and_timeout_keep_their_panes() {
        for (name, exits) in [("failed", true), ("timeout", false)] {
            let dir = tdir(&format!("hd-orch-keep-{name}"));
            let mut r = fake(&dir, "t", WriteMode::Nothing, "");
            r.exits = exits;
            let mut o = opts(&dir, "t");
            o.keep_open = false;
            let out = dispatch(&r, &default_config(), &o).unwrap();
            assert_eq!(out.status, name);
            assert!(r.closed.borrow().is_empty(), "{name} closed its pane");
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn a_new_tab_is_the_default_placement_and_named_after_the_task() {
        let dir = tdir("hd-orch-tab");
        let r = fake(&dir, "task-3", WriteMode::Complete, "ok");
        let wt = tdir("hd-orch-tab-wt");
        let mut o = opts(&dir, "task-3");
        o.placement = "tab";
        o.cwd = Some(wt.to_str().unwrap());
        dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(*r.split_calls.borrow(), 0);
        let opens = r.tab_opens.borrow();
        assert_eq!(opens[0].0, "1-2");
        assert_eq!(opens[0].1, "task-3");
        assert_eq!(opens[0].2.as_deref(), wt.to_str());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&wt).ok();
    }

    #[test]
    fn a_reused_pane_opens_no_tab() {
        let dir = tdir("hd-orch-tab-reuse");
        let r = fake(&dir, "t", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "t");
        o.placement = "tab";
        o.reuse_pane = Some("1-7");
        dispatch(&r, &default_config(), &o).unwrap();
        assert!(r.tab_opens.borrow().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_placement_errors_before_any_herdr_call() {
        let dir = tdir("hd-orch-tab-bad");
        let r = fake(&dir, "t", WriteMode::Complete, "ok");
        let mut o = opts(&dir, "t");
        o.placement = "window";
        let err = dispatch(&r, &default_config(), &o).unwrap_err().to_string();
        assert!(err.contains("window"), "was: {err}");
        assert_eq!(*r.split_calls.borrow(), 0);
        assert!(r.tab_opens.borrow().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pane_exists_error_during_the_grace_wait_is_not_an_exit() {
        let dir = tdir("hd-orch-pane-err-grace");
        let mut r = fake(&dir, "rev-peg", WriteMode::Complete, "fine");
        r.pane_exists_errors = true;
        let mut o = opts(&dir, "rev-peg");
        o.exit_grace_ms = 200;
        let out = dispatch(&r, &default_config(), &o).unwrap();
        assert_eq!(out.status, "done");
        assert!(!out.agent_exited);
        assert!(*r.pane_exists_calls.borrow() >= 1, "grace wait never checked the pane");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn result_reports_log_file_path() {
        let dir = std::env::temp_dir().join("hd-orch-logpath");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake(&dir, "rev-lp", WriteMode::Complete, "ok");
        let out = dispatch(&r, &default_config(), &opts(&dir, "rev-lp")).unwrap();
        assert_eq!(out.log_file, protocol::log_path(&dir, "rev-lp").to_string_lossy());
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
        fn open_tab(&self, _f: &str, _l: &str, _c: Option<&str>) -> anyhow::Result<String> { Ok("1-3".into()) }
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
        assert!(commands[0].ends_with("&& codex"), "was: {}", commands[0]);
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
        assert!(script(&commands[0]).contains(&quoted_prompt));
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
        assert!(script(&commands[0]).contains("--effort 'high'"), "was: {}", commands[0]);
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
        o.vendor = Some("cursor");
        o.cwd = Some(rel_wt.to_str().unwrap());
        dispatch(&r, &default_config(), &o).unwrap();
        let cwd = r.split_cwd.borrow().clone().unwrap();
        assert!(std::path::Path::new(&cwd).is_absolute(), "was: {cwd}");
        let commands = r.commands.borrow();
        let inner = script(&commands[0]);
        let prompt_arg = inner.split("cat '").nth(1).unwrap().split('\'').next().unwrap();
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
        let inner = script(&commands[0]);
        assert!(inner.contains(&format!("; {prefix}")), "was: {inner}");
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
        o.headless = true;
        o.model = Some("opus");
        o.resume_session = Some("sess-1");
        dispatch(&r, &default_config(), &o).unwrap();
        let commands = r.commands.borrow();
        let inner = script(&commands[0]);
        assert!(inner.contains("claude -p --resume 'sess-1'"), "was: {inner}");
        assert!(inner.contains("--model 'opus'"), "was: {inner}");
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
