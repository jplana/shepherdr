# shepherdr

**shepherdr** (shep·herdr) is a [herdr](https://herdr.dev) plugin that runs the code-writing subagents of [superpowers](https://github.com/obra/superpowers) in herdr terminal panes, so you can watch, resume, and take over their work instead of facing opaque in-process subagents. Superpowers' stock steps run unchanged; shepherdr only changes where code-writing subagents run. Reviewers stay native.

Plugin id: `jplana.shepherdr`. macOS.

## What it does

- **`dispatch`** (agent-initiated, called by the `dispatching-implementers-to-herdr` skill): opens a background tab, runs an agent against a constructed prompt, and captures its result. Completion is detected by **polling the result file** for a trailing sentinel marker. Claude runs in its **real interface**, with its usual colours and formatting, and you can type into it to take over. Its session id is chosen up front, and hooks report when each turn ends, so a turn that ends without a result or in an API error is reported as `failed` right away. Other vendors run headless, with their output teed to a durable `<task-id>.log` from which the session id is parsed. A clean finish closes the tab; any other outcome keeps it.
- **`resume`** (CLI): reopens a finished dispatch's captured session in a fresh headed pane, in its original working directory — plain interactive, no autonomy flags. Fails closed if no session id was captured.
- **`action second-opinion`** (keybinding-triggered, `prefix+r`): dispatches a codex review of the focused worktree's git diff, in a read-only sandbox, from the directory the action was launched from. It reads the diff with `git -C`, which honors the reviewed repo's git config. In a test, the sandbox stopped a hostile git config from writing outside the repo. Code from that config still runs and can read files, and network access was not tested, so use it only on worktrees you trust.
- **`action resume-last`** (keybinding-triggered, `prefix+shift+r`): reopens the newest resumable dispatch (latest completed dispatch with a captured session id) in a fresh headed pane, in its original working directory.
- **`board`**: an overlay pane rendering a live table of the 20 newest dispatches from the plugin state dir, including a SESSION column so you can see at a glance which dispatches are resumable.

## Install

This is a fork of [afogel/shepherdr](https://github.com/afogel/shepherdr). Install it from a clone: `herdr plugin install afogel/shepherdr` installs the upstream code, which doesn't have the Superpowers 7 integration.

Requires the Rust toolchain.

```bash
git clone https://github.com/jplana/shepherdr.git
cd shepherdr
cargo install --path .            # `link` does not build, so build it yourself
herdr plugin link "$PWD"
herdr plugin action list --plugin jplana.shepherdr   # sanity check
```

herdr and your shell both run `shepherdr` by name. If `~/.cargo/bin` isn't on their `PATH`, symlink the binary into a directory that is, such as `ln -s ~/.cargo/bin/shepherdr ~/.local/bin/shepherdr`.

After pulling changes, run `cargo install --path .` again. Keep the clone in place, because the plugin link points into it.

## The dispatch CLI

```
shepherdr dispatch \
  --role implementer|reviewer \
  --context-file <file>        # the constructed task/review context
  --from-pane <pane-id>        # the agent's tab opens in this pane's workspace, or this pane is split
  --task-id <id>               # names the prompt/result files and the sentinel
  --scratch <dir>              # where prompt/result files live
  [--vendor claude|codex|cursor]        # override the role's default
  [--unavailable codex,claude]          # skip vendors (e.g. out of tokens); falls back down the chain
  [--reuse-pane <pane-id>]              # run in an existing pane instead (if it still exists)
  [--interactive]              # opt-in: start a HEADED TUI you drive from the
                               # start (bare TUI, with your usual permission
                               # settings, prompt delivered as one line, pane
                               # left live)
  [--model <m>]                # required for headless --role implementer; rejected with --interactive
  [--effort low|medium|high|xhigh|max]  # optional, claude only
  [--resume-session <id>]      # headless resume of a captured session; requires --vendor
  [--cwd <dir>]                # working directory for the agent (e.g. a worktree)
  [--timeout-ms N]             # poll deadline for the result file (default 600000)
  [--placement tab|split]      # a background tab of its own (default), or a split of --from-pane
  [--split-direction right|down]        # with --placement split
  [--keep-open]                # keep the tab or pane after a clean finish
  [--headless]                 # run claude with -p instead of its real interface
  [--state-dir <dir>]          # where to write the DispatchRecord (default: the plugin's state folder,
                               # ~/.local/state/herdr/plugins/jplana.shepherdr, which the board and
                               # resume-last read)
```

Prints JSON: `{status, task_id, vendor, pane_id, result_file, result_source, session_id, log_file, agent_exited, result_text}`.
`session_id` is the agent session, chosen up front for claude's real interface and otherwise captured from the log (null when none was found); it is what `resume` and `--resume-session` reuse.
`log_file` is the durable `<task-id>.log` tee of a headless agent's output. For claude's real interface it is the session transcript, or a path that doesn't exist if no turn ended. `agent_exited` is true when the pane command left its `<task-id>.exited` marker or herdr reports the pane gone.
`result_text` holds the agent's result on `done`. On `failed`, `timeout` and `degraded` it is bounded to the last 40 lines and at most 8 KiB, with a leading marker when the byte cap dropped text; the rest is in `log_file`.
`status` is `done` (result file complete), `degraded` (partial file at timeout), `timeout` (no file; `result_text` carries the pane scrollback, which for the real interface may hold little), or `failed` (the agent exited, or its turn ended, without a complete result).

Exit codes: `0` done, `3` degraded, `2` timeout or failed, `1` usage or herdr error.

`--effort` is claude-only; other values and other vendors are rejected. Without it, a pane agent runs at claude's configured default effort, not the controller's. `--model` is required for `--role implementer`, unless `--interactive`. `--resume-session` requires `--vendor`. `--model`, `--effort` and `--resume-session` are rejected with `--interactive`.

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

## Roles, vendors, and permissions

Vendor selection is a preference chain, overridable per dispatch:

| Role | Default chain | Command (autonomous) |
|------|---------------|----------------------|
| implementer | claude → cursor | `claude --session-id {session} --settings {settings} --permission-mode auto {first}`; with `--headless`, `cat {prompt} \| claude -p --output-format stream-json --verbose --permission-mode auto` |
| reviewer | codex → cursor | `cat {prompt} \| codex exec --skip-git-repo-check --sandbox read-only --output-last-message {result}` |
| (backup) | cursor | `cat {prompt} \| cursor-agent -p --output-format json --force` |

The headless claude and cursor commands emit a structured JSON/NDJSON stream (rather than plain text) so the agent session id can be parsed from the run and stored on the dispatch record; codex prints it as a plain `session id:` line. That captured id is what `resume` / `resume-last` reopen.

shepherdr never passes a bypass flag to claude or codex:

- **Claude** runs in auto mode, where a classifier reviews its actions. A hook answers any permission prompt with "no", so an agent in a background tab is told and carries on or reports `BLOCKED` instead of waiting. Claude's OS sandbox is not enabled, so what auto mode allows runs with your user's access.
- **Codex as a reviewer** runs in a `read-only` sandbox, and codex saves its final message as the result.
- **Codex as an implementer** (`--vendor codex`) runs in a `workspace-write` sandbox. It **can't commit**, because git's files live outside what it may write, so something else has to commit its work. Opening up the git directory is deliberately avoided, since write access to its hooks or config could escape the sandbox.
- **Cursor** still runs with `--force`, which skips its checks. It is a backup that has not been tested here.
- **`--interactive`** starts the bare CLI with your usual settings, for you to drive.

Claude's real interface stops at its trust prompt in a folder Claude has not been told to trust. In our testing, git worktrees of a trusted repo did not prompt. Run `claude` once in a new repo and answer the prompt, or use `--headless` if nobody will be there to answer it. A dispatch waiting at the prompt carries on once it is answered, and otherwise runs until its timeout.

### Interactive dispatch (opt-in)

`--interactive` launches the bare vendor TUI (with your usual permission settings)
in a headed pane, waits for it to be ready, delivers the task as a single
`read <prompt> and follow all instructions in it` line, and file-polls for
completion — leaving the pane live so you stay in the loop. This is the
fragile, per-vendor path (readiness cues are tuned by the live smoke test);
headless remains the reliable default. Interactive dispatches are not resumable
via `resume`/`resume-last`: the vendor TUI renders to the alt-screen so no
session id is captured and `session_id` stays null.

## The completion protocol

`dispatch` writes `<scratch>/<task-id>.prompt.md` = your context + a footer telling the agent to write its result to `<scratch>/<task-id>.result.md` with the final line being exactly `DONE::<task-id>`. It then polls that file until the marker appears (or the timeout), reads the result (marker stripped), and reports. A stale result file from a prior run is removed before dispatch.

The pane command runs the agent under its own `sh -c`, with traps that create `<scratch>/<task-id>.exited` when the agent pipeline ends, whether by a normal or nonzero exit, SIGINT, SIGTERM or SIGHUP, or a failed `cd`. That marker with no complete result gives `failed` at once instead of waiting for the timeout. So does a pane that herdr reports gone; a herdr error does not count. A complete result is always `done`.

## Superpowers integration

shepherdr ships one skill, [`dispatching-implementers-to-herdr`](superpowers/dispatching-implementers-to-herdr/SKILL.md). When `HERDR_ENV=1`, it maps each place where `superpowers:subagent-driven-development` dispatches or resumes an implementer onto `shepherdr dispatch`. Every other Superpowers step runs unchanged, and only code-writing subagents get panes; reviewers stay native. Implementers in this flow are Claude only.

See [`superpowers/README.md`](superpowers/README.md) to activate it.

### Manual tools

Outside the Superpowers flow you can use `second-opinion` (`prefix+r`), `resume-last` (`prefix+shift+r`), `board`, and `dispatch --interactive` by hand.

## Development

```bash
cargo test        # unit tests with an injected fake HerdrRunner (no live socket)
cargo clippy --all-targets -- -D warnings
cargo build && python3 -I tests/exit-marker-pty.py   # the pane command under zsh and bash on a pty (stub agent, no herdr)
```

The `HerdrRunner` trait (`src/runner.rs`) is the seam over the `herdr` CLI (`HERDR_BIN_PATH`); tests inject a recording/fake implementation.
