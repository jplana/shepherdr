# shepherdr

**shepherdr** (shep·herdr) is a [herdr](https://herdr.dev) plugin that runs the code-writing subagents of [superpowers](https://github.com/obra/superpowers) in herdr terminal panes, so you can watch, resume, and take over their work instead of facing opaque in-process subagents. Superpowers' stock steps run unchanged; shepherdr only changes where code-writing subagents run. Reviewers stay native.

Plugin id: `afogel.shepherdr`. macOS.

## What it does

- **`dispatch`** (agent-initiated, called by the `dispatching-implementers-to-herdr` skill): splits a pane, runs a headless agent against a constructed prompt, and captures the agent's result. Completion is detected by **polling the result file** for a trailing sentinel marker (headless agents echo their prompt, so a scrollback sentinel is unreliable). The agent's combined output is teed to a durable `<task-id>.log`, from which the agent **session id** is parsed and stored so the run can be resumed later. The pane streams the agent's work for live/after-the-fact auditing and stays open when done.
- **`resume`** (CLI): reopens a finished dispatch's captured session in a fresh headed pane, in its original working directory — plain interactive, no autonomy flags. Fails closed if no session id was captured.
- **`action second-opinion`** (keybinding-triggered, `prefix+r`): dispatches a codex review of the focused worktree's git diff.
- **`action resume-last`** (keybinding-triggered, `prefix+shift+r`): reopens the newest resumable dispatch (latest completed dispatch with a captured session id) in a fresh headed pane, in its original working directory.
- **`event worktree-created`**: auto-creates a `workers` tab.
- **`board`**: an overlay pane rendering a live table of active dispatches from the plugin state dir, including a SESSION column so you can see at a glance which dispatches are resumable.

## Install

Requires the Rust toolchain and `~/.cargo/bin` on `PATH` — the plugin builds a native binary on install.

**From the marketplace (recommended):**

```bash
herdr plugin install afogel/shepherdr
```

herdr clones the repo and runs the manifest's build step (`cargo install --path .`), placing the `shepherdr` binary on `~/.cargo/bin`.

**Local development (from a clone):**

```bash
cd path/to/shepherdr
cargo install --path .            # local `link` does not build, so build it yourself
herdr plugin link "$PWD"
herdr plugin action list --plugin afogel.shepherdr   # sanity check
```

## The dispatch CLI

```
shepherdr dispatch \
  --role implementer|reviewer \
  --context-file <file>        # the constructed task/review context
  --from-pane <pane-id>        # pane to split off (find via `herdr pane list`)
  --task-id <id>               # names the prompt/result files and the sentinel
  --scratch <dir>              # where prompt/result files live
  [--vendor claude|codex|cursor]        # override the role's default
  [--unavailable codex,claude]          # skip vendors (e.g. out of tokens); falls back down the chain
  [--reuse-pane <pane-id>]              # run in an existing pane instead of splitting (if it still exists)
  [--interactive]              # opt-in: start a HEADED TUI you drive from the
                               # start (bare TUI + gate-bypass flags, prompt
                               # delivered as one line, pane left live)
  [--model <m>]                # required for --role implementer
  [--effort low|medium|high|xhigh|max]  # optional, claude only
  [--resume-session <id>]      # headless resume of a captured session; requires --vendor
  [--cwd <dir>]                # working directory for the agent (e.g. a worktree)
  [--timeout-ms N]             # poll deadline for the result file (default 600000)
  [--split-direction right|down]
  [--close]                    # dispose of the pane when done (default: keep open for auditing)
  [--state-dir <dir>]          # write a DispatchRecord here for the board
```

Prints JSON: `{status, task_id, vendor, pane_id, result_file, result_source, result_text, session_id, log_file, agent_exited}`.
`session_id` is the captured agent session (null when none was found); it is what `resume` and `--resume-session` reuse.
`log_file` is the durable `<task-id>.log` tee of the agent's output. `agent_exited` is true when shepherdr saw the agent process exit.
`status` is `done` (result file complete), `degraded` (partial file at timeout), `timeout` (no file; `result_text` carries the pane scrollback for diagnosis), or `failed` (the agent exited without a complete result).

Exit codes: `0` done, `3` degraded, `2` timeout or failed, `1` usage or herdr error.

`--effort` is claude-only; other values and other vendors are rejected. Without it, a pane agent runs at claude's configured default effort, not the controller's. `--model` is required for `--role implementer`. `--resume-session` requires `--vendor`.

## Resuming a finished dispatch

```
shepherdr resume \
  --task-id <id>            # the dispatch to reopen
  --state-dir <dir>         # where the DispatchRecord was written
  --from-pane <pane-id>     # pane to split the headed session off
  [--split-direction right|down]
```

Splits a fresh, focused, headed pane, `cd`s to the dispatch's recorded working
directory, and runs the vendor's plain resume (`codex resume <id>` /
`claude --resume <id>` / `cursor-agent --resume <id>`) with **no** autonomy
flags — you are present and approve tool calls yourself. Fails closed if the
record has no captured session id.

## Roles, vendors, and autonomy

Vendor selection is a preference chain, overridable per dispatch:

| Role | Default chain | Command (autonomous) |
|------|---------------|----------------------|
| implementer | claude → cursor | `cat {prompt} \| claude -p --output-format stream-json --verbose --dangerously-skip-permissions` |
| reviewer | codex → cursor | `cat {prompt} \| codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox --dangerously-bypass-hook-trust` |
| (backup) | cursor | `cat {prompt} \| cursor-agent -p --output-format json --force` |

The claude/cursor commands emit a structured JSON/NDJSON stream (rather than plain text) so the agent session id can be parsed from the run and stored on the dispatch record; codex prints it as a plain `session id:` line. That captured id is what `resume` / `resume-last` reopen.

Dispatched agents run with permission/sandbox bypass so they can actually do the work (run tools, write files). This is deliberate: an autonomous multi-agent harness needs its agents to act. Only run this on repos and tasks you're comfortable letting an agent operate on unattended.

### Interactive dispatch (opt-in)

`--interactive` launches the bare vendor TUI (with the same gate-bypass flags)
in a headed pane, waits for it to be ready, delivers the task as a single
`read <prompt> and follow all instructions in it` line, and file-polls for
completion — leaving the pane live so you stay in the loop. This is the
fragile, per-vendor path (readiness cues are tuned by the live smoke test);
headless remains the reliable default. Interactive dispatches are not resumable
via `resume`/`resume-last`: the vendor TUI renders to the alt-screen so no
session id is captured and `session_id` stays null.

## The completion protocol

`dispatch` writes `<scratch>/<task-id>.prompt.md` = your context + a footer telling the agent to write its result to `<scratch>/<task-id>.result.md` with the final line being exactly `DONE::<task-id>`. It then polls that file until the marker appears (or the timeout), reads the result (marker stripped), and reports. A stale result file from a prior run is removed before dispatch.

## Superpowers integration

shepherdr ships one skill, [`dispatching-implementers-to-herdr`](superpowers/dispatching-implementers-to-herdr/SKILL.md). When `HERDR_ENV=1`, it maps each place where `superpowers:subagent-driven-development` dispatches or resumes an implementer onto `shepherdr dispatch`. Every other Superpowers step runs unchanged, and only code-writing subagents get panes; reviewers stay native. Implementers in this flow are Claude only.

See [`superpowers/README.md`](superpowers/README.md) to activate it.

### Manual tools

Outside the Superpowers flow you can use `second-opinion` (`prefix+r`), `resume-last` (`prefix+shift+r`), `board`, and `dispatch --interactive` by hand.

## Development

```bash
cargo test        # unit tests with an injected fake HerdrRunner (no live socket)
cargo clippy --all-targets -- -D warnings
```

The `HerdrRunner` trait (`src/runner.rs`) is the seam over the `herdr` CLI (`HERDR_BIN_PATH`); tests inject a recording/fake implementation.
