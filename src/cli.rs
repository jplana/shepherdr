use crate::config::default_config;
use crate::orchestrate::{dispatch, DispatchOpts, DispatchResult};
use crate::runner::HerdrRunner;
use crate::{protocol, session, state};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::path::Path;

#[derive(Parser)]
pub struct DispatchArgs {
    #[arg(long)]
    pub role: String,
    #[arg(long)]
    pub context_file: String,
    #[arg(long)]
    pub from_pane: String,
    #[arg(long)]
    pub task_id: String,
    #[arg(long, default_value = ".")]
    pub scratch: String,
    #[arg(long)]
    pub vendor: Option<String>,
    #[arg(long, default_value = "")]
    pub unavailable: String,
    #[arg(long, default_value_t = 600_000)]
    pub timeout_ms: u64,
    #[arg(long, default_value = "right")]
    pub split_direction: String,
    #[arg(long)]
    pub close: bool,
    #[arg(long)]
    pub state_dir: Option<String>,
    #[arg(long)]
    pub reuse_pane: Option<String>,
    #[arg(long)]
    pub interactive: bool,
}

pub fn run_dispatch<R: HerdrRunner>(args: &DispatchArgs, runner: &R) -> Result<DispatchResult> {
    let context = std::fs::read_to_string(&args.context_file)?;
    let unavailable = args.unavailable.split(',').filter(|s| !s.is_empty())
        .map(String::from).collect();
    let state_dir = args.state_dir.as_deref().map(Path::new);
    let opts = DispatchOpts {
        context: &context,
        role: &args.role,
        from_pane: &args.from_pane,
        scratch: Path::new(&args.scratch),
        task_id: &args.task_id,
        vendor: args.vendor.as_deref(),
        unavailable,
        timeout_ms: args.timeout_ms,
        split_direction: &args.split_direction,
        close: args.close,
        state_dir,
        reuse_pane: args.reuse_pane.as_deref(),
        mode: if args.interactive { "interactive" } else { "headless" },
        interactive: args.interactive,
    };
    dispatch(runner, &default_config(), &opts)
}

#[derive(Parser)]
pub struct ResumeArgs {
    #[arg(long)]
    pub task_id: String,
    #[arg(long)]
    pub state_dir: String,
    #[arg(long)]
    pub from_pane: String,
    #[arg(long, default_value = "right")]
    pub split_direction: String,
}

/// Reopen a finished dispatch's agent session in a fresh, focused, headed
/// pane, in the recorded working directory. Fails closed if the record has
/// no captured session id.
pub fn run_resume<R: HerdrRunner>(args: &ResumeArgs, runner: &R) -> Result<String> {
    let record = state::read_record(Path::new(&args.state_dir), &args.task_id)?
        .ok_or_else(|| anyhow!("no dispatch record for {}", args.task_id))?;
    let session_id = record.session_id.as_deref().ok_or_else(|| {
        anyhow!("no captured session id for {} — cannot resume", args.task_id)
    })?;
    let command = session::resume_command(&record.vendor, session_id, &record.cwd)?;
    let pane_id = protocol::pane_id_from_split(
        &runner.split(&args.from_pane, &args.split_direction, true)?,
    )?;
    runner.run(&pane_id, &command)?;
    Ok(pane_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::result_path;

    struct StubRunner { scratch: std::path::PathBuf, task_id: String }
    impl HerdrRunner for StubRunner {
        fn split(&self, _p: &str, _d: &str, _f: bool) -> anyhow::Result<String> {
            Ok(r#"{"result":{"pane":{"pane_id":"9-9"}}}"#.into())
        }
        fn run(&self, _p: &str, cmd: &str) -> anyhow::Result<()> {
            if cmd.contains(".prompt.md") {
                // completion is now detected by polling the result file for
                // its trailing sentinel line, so the stub must write one
                let content = format!("no findings\n{}", crate::protocol::sentinel(&self.task_id));
                std::fs::write(result_path(&self.scratch, &self.task_id), content).unwrap();
            }
            Ok(())
        }
        fn wait_output(&self, _p: &str, _m: &str, _t: u64) -> anyhow::Result<bool> { Ok(true) }
        fn read(&self, _p: &str, _s: &str, _l: u32) -> anyhow::Result<String> { Ok("x".into()) }
        fn close(&self, _p: &str) -> anyhow::Result<()> { Ok(()) }
        fn tab_create(&self, _w: &str, _l: &str) -> anyhow::Result<String> { Ok("{}".into()) }
        fn pane_exists(&self, _pane: &str) -> anyhow::Result<bool> { Ok(true) }
    }

    use crate::state::{write_record, DispatchRecord};
    use std::cell::RefCell;

    struct ResumeFake {
        commands: RefCell<Vec<String>>,
        split_focus: RefCell<Vec<bool>>,
    }
    impl HerdrRunner for ResumeFake {
        fn split(&self, _p: &str, _d: &str, focus: bool) -> anyhow::Result<String> {
            self.split_focus.borrow_mut().push(focus);
            Ok(r#"{"result":{"pane":{"pane_id":"r-1"}}}"#.into())
        }
        fn run(&self, _p: &str, cmd: &str) -> anyhow::Result<()> {
            self.commands.borrow_mut().push(cmd.into());
            Ok(())
        }
        fn wait_output(&self, _p: &str, _m: &str, _t: u64) -> anyhow::Result<bool> { Ok(true) }
        fn read(&self, _p: &str, _s: &str, _l: u32) -> anyhow::Result<String> { Ok(String::new()) }
        fn close(&self, _p: &str) -> anyhow::Result<()> { Ok(()) }
        fn tab_create(&self, _w: &str, _l: &str) -> anyhow::Result<String> { Ok("{}".into()) }
        fn pane_exists(&self, _p: &str) -> anyhow::Result<bool> { Ok(true) }
    }

    fn resumable_record(task_id: &str, session_id: Option<&str>) -> DispatchRecord {
        DispatchRecord {
            task_id: task_id.into(), role: "reviewer".into(), vendor: "codex".into(),
            pane_id: "1-3".into(), status: "done".into(),
            session_id: session_id.map(String::from), mode: "headless".into(),
            cwd: "/repo/wt".into(), created_at: "1".into(),
        }
    }

    #[test]
    fn run_resume_splits_focused_pane_and_runs_plain_resume() {
        let dir = std::env::temp_dir().join("hd-resume-ok");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        write_record(&dir, &resumable_record("t1", Some("sess-1"))).unwrap();
        let args = ResumeArgs {
            task_id: "t1".into(), state_dir: dir.to_string_lossy().into(),
            from_pane: "1-2".into(), split_direction: "right".into(),
        };
        let fake = ResumeFake { commands: RefCell::new(vec![]), split_focus: RefCell::new(vec![]) };
        let pane = run_resume(&args, &fake).unwrap();
        assert_eq!(pane, "r-1");
        assert_eq!(fake.split_focus.borrow().as_slice(), [true]);
        let cmd = &fake.commands.borrow()[0];
        assert!(cmd.contains("cd '/repo/wt' &&"), "was: {cmd}");
        assert!(cmd.contains("codex resume 'sess-1'"), "was: {cmd}");
        assert!(!cmd.contains("--dangerously"), "was: {cmd}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_resume_fails_closed_without_session_id() {
        let dir = std::env::temp_dir().join("hd-resume-none");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        write_record(&dir, &resumable_record("t1", None)).unwrap();
        let args = ResumeArgs {
            task_id: "t1".into(), state_dir: dir.to_string_lossy().into(),
            from_pane: "1-2".into(), split_direction: "right".into(),
        };
        let fake = ResumeFake { commands: RefCell::new(vec![]), split_focus: RefCell::new(vec![]) };
        let err = run_resume(&args, &fake).unwrap_err().to_string();
        assert!(err.contains("no captured session id"), "was: {err}");
        assert!(fake.commands.borrow().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_dispatch_captures_result() {
        let dir = std::env::temp_dir().join("hd-cli-1");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = dir.join("ctx.md");
        std::fs::write(&ctx, "Review the plan.").unwrap();
        let args = DispatchArgs {
            role: "reviewer".into(), context_file: ctx.to_string_lossy().into(),
            from_pane: "1-2".into(), task_id: "rev-1".into(),
            scratch: dir.to_string_lossy().into(), vendor: None, unavailable: String::new(),
            timeout_ms: 1000, split_direction: "right".into(),
            close: false, state_dir: None, reuse_pane: None, interactive: false,
        };
        let out = run_dispatch(&args, &StubRunner { scratch: dir.clone(), task_id: "rev-1".into() }).unwrap();
        assert_eq!(out.status, "done");
        assert_eq!(out.result_text, "no findings");
        assert_eq!(out.pane_id, "9-9");
        std::fs::remove_dir_all(&dir).ok();
    }
}
