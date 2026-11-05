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
    /// Agent-initiated dispatch (called by the dispatching-implementers-to-herdr skill).
    Dispatch(Box<DispatchArgs>),
    /// Reopen a finished dispatch's agent session in a headed pane.
    Resume(ResumeArgs),
    /// Human/keybinding-triggered action face.
    Action { name: String },
    /// Workers board pane (declared in the manifest).
    Board,
}

fn status_exit_code(status: &str) -> i32 {
    match status {
        "done" => 0,
        "degraded" => 3,
        "failed" => 2,
        _ => 2, // timeout or anything unexpected
    }
}

/// The folder dispatch records live in: what herdr gave this plugin, or else its usual place.
fn plugin_state_dir() -> Option<std::path::PathBuf> {
    let var = |name: &str| std::env::var(name).ok();
    shepherdr::state::state_dir(var("HERDR_PLUGIN_STATE_DIR"), var("XDG_STATE_HOME"), var("HOME"))
}

fn state_dir_or_here() -> String {
    plugin_state_dir().map_or_else(|| ".".into(), |dir| dir.to_string_lossy().into_owned())
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Dispatch(mut args) => {
            // Every dispatch is recorded, so that the board and resume-last can see it.
            if args.state_dir.is_none() {
                args.state_dir = plugin_state_dir().map(|dir| dir.to_string_lossy().into_owned());
            }
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
            let scratch = state_dir_or_here();
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
            let args = shepherdr::action::second_opinion_args(&ctx, &scratch, &ctx_file.to_string_lossy());
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
            let state_dir = state_dir_or_here();
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
        Cmd::Board => {
            let state_dir = state_dir_or_here();
            shepherdr::board::run_board_loop(std::path::Path::new(&state_dir));
        }
    }
}
