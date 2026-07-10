use clap::{Parser, Subcommand};
use shepherdr::cli::{run_dispatch, run_resume, DispatchArgs, ResumeArgs};
use shepherdr::runner::CliRunner;

#[derive(Parser)]
#[command(name = "shepherdr", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Agent-initiated dispatch (called by the superpowers overlays).
    Dispatch(Box<DispatchArgs>),
    /// Reopen a finished dispatch's agent session in a headed pane.
    Resume(ResumeArgs),
    /// Human/keybinding-triggered action face.
    Action { name: String },
    /// Event hook face (herdr event hooks).
    Event { name: String },
    /// Workers board pane (declared in the manifest).
    Board,
}

fn status_exit_code(status: &str) -> i32 {
    match status {
        "done" => 0,
        "degraded" => 3,
        _ => 2, // timeout or anything unexpected
    }
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Dispatch(args) => {
            let runner = CliRunner::system();
            match run_dispatch(&args, &runner) {
                Ok(result) => {
                    println!("{}", serde_json::to_string(&result).unwrap());
                    std::process::exit(status_exit_code(&result.status));
                }
                Err(e) => {
                    eprintln!("dispatch error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Cmd::Resume(args) => {
            let runner = CliRunner::system();
            match run_resume(&args, &runner) {
                Ok(pane_id) => println!("resumed in pane {pane_id}"),
                Err(e) => {
                    eprintln!("resume error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Cmd::Action { name } if name == "noop" => println!("ok"),
        Cmd::Action { name } if name == "second-opinion" => {
            let ctx_json = std::env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default();
            let ctx = shepherdr::action::parse_context(&ctx_json);
            let scratch = std::env::var("HERDR_PLUGIN_STATE_DIR").unwrap_or_else(|_| ".".into());
            let from_pane = ctx.pane_id.clone().unwrap_or_else(|| "1-1".into());
            let context = shepherdr::action::second_opinion_context(&ctx);
            let ctx_file = std::path::Path::new(&scratch).join("second-opinion.context.md");
            if let Err(e) = std::fs::create_dir_all(&scratch) {
                eprintln!("action error: {e}");
                std::process::exit(1);
            }
            if let Err(e) = std::fs::write(&ctx_file, context) {
                eprintln!("action error: {e}");
                std::process::exit(1);
            }
            let args = DispatchArgs {
                role: "reviewer".into(), context_file: ctx_file.to_string_lossy().into(),
                from_pane, task_id: "second-opinion".into(), scratch: scratch.clone(),
                vendor: None, unavailable: String::new(), timeout_ms: 600_000,
                split_direction: "right".into(), close: false,
                state_dir: Some(scratch), reuse_pane: None, interactive: false,
            };
            let runner = CliRunner::system();
            match run_dispatch(&args, &runner) {
                Ok(r) => {
                    println!("{}", serde_json::to_string(&r).unwrap());
                    std::process::exit(status_exit_code(&r.status));
                }
                Err(e) => { eprintln!("action error: {e}"); std::process::exit(1); }
            }
        }
        Cmd::Action { name } if name == "resume-last" => {
            let state_dir = std::env::var("HERDR_PLUGIN_STATE_DIR").unwrap_or_else(|_| ".".into());
            let records = shepherdr::state::read_records(std::path::Path::new(&state_dir))
                .unwrap_or_default();
            match shepherdr::state::latest_resumable(&records) {
                Some(record) => {
                    let ctx_json = std::env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default();
                    let ctx = shepherdr::action::parse_context(&ctx_json);
                    let from_pane = ctx.pane_id.clone().unwrap_or_else(|| "1-1".into());
                    let args = ResumeArgs {
                        task_id: record.task_id.clone(),
                        state_dir,
                        from_pane,
                        split_direction: "right".into(),
                    };
                    let runner = CliRunner::system();
                    match run_resume(&args, &runner) {
                        Ok(pane_id) => println!("resumed {} in pane {pane_id}", record.task_id),
                        Err(e) => { eprintln!("resume error: {e}"); std::process::exit(1); }
                    }
                }
                None => {
                    eprintln!("no resumable dispatch found");
                    std::process::exit(1);
                }
            }
        }
        Cmd::Action { name } => {
            eprintln!("unknown action: {name}");
            std::process::exit(1);
        }
        Cmd::Event { name } if name == "worktree-created" => {
            let event_json = std::env::var("HERDR_PLUGIN_EVENT_JSON").unwrap_or_default();
            let runner = CliRunner::system();
            match shepherdr::event::handle_worktree_created(&runner, &event_json) {
                Ok(Some(tab)) => println!("created workers tab {tab}"),
                Ok(None) => {}
                Err(e) => { eprintln!("event error: {e}"); std::process::exit(1); }
            }
        }
        Cmd::Event { name } => {
            eprintln!("unknown event: {name}");
            std::process::exit(1);
        }
        Cmd::Board => {
            let state_dir = std::env::var("HERDR_PLUGIN_STATE_DIR").unwrap_or_else(|_| ".".into());
            shepherdr::board::run_board_loop(std::path::Path::new(&state_dir));
        }
    }
}
