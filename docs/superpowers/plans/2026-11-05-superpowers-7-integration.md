# Superpowers 7 Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stock Superpowers runs unchanged inside herdr, with SDD's code-writing subagents in visible panes through `shepherdr dispatch`.

**Architecture:** One short skill maps stock SDD's dispatch actions to `shepherdr dispatch`. `dispatch` gains an explicit model, an optional effort, headless resume, a working directory, exit detection and bounded output. The shaping submodule and all three overlays are removed.

**Tech Stack:** Rust 2021 (clap, serde, serde_json, anyhow), the herdr 0.9.1 CLI, Claude Code skills, bash.

**Spec:** `docs/superpowers/specs/2026-11-05-superpowers-7-integration-design.md`

## Global Constraints

- Do not change permission or autonomy flags. New headless commands use the same flags as existing ones. The human `resume` command stays free of them.
- Do not change `--interactive` behavior, the `second-opinion` and `resume-last` actions, `board`, or the `worktree.created` hook, beyond the mechanical updates these changes force.
- No new Rust dependencies.
- Tests use the fake runners and spawn no real agents, except Tasks 4 and 6, which say so.
- Green means all of `cargo test`. Run `cargo clippy --all-targets -- -D warnings` too if clippy is available; it is not installed here at the time of writing.
- End every commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- The CLAUDE.md activation block, used exactly by the Task 4 scenarios and the Task 5 docs:

  ```markdown
  # herdr + Superpowers

  When `HERDR_ENV=1`, use `dispatching-implementers-to-herdr` for every subagent
  that writes code during `superpowers:subagent-driven-development`. All other
  Superpowers steps run unchanged.
  ```

## Review Focus

1. A relative `--cwd` or `--scratch`: the agent runs in the pane, so paths must not depend on the dispatcher's directory. Both become absolute. Pinned in Task 2.
2. A `--cwd` that does not exist: clear error before any pane is created. Pinned in Task 2.
3. `--reuse-pane` naming a pane that has since closed, during a resume: falls through to a new pane in `--cwd`, still resuming. Pinned in Task 2.
4. The result and the exit marker both present when polling looks: this is `done`, never `failed`. Pinned in Task 3.
5. An agent that dies at once (bad model, expired login): `failed` right away with the log tail, not a wait for the deadline. Pinned in Task 3, and live in Task 6.

---

### Task 1: Build headless commands with model, effort and resume

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Produces: `VendorSpec` fields `model_flag: String`, `effort_flag: Option<String>` and `resume_command: Option<String>`.
- Produces: `pub fn headless_command(spec: &VendorSpec, prompt_file: &str, model: Option<&str>, effort: Option<&str>, resume_session: Option<&str>) -> anyhow::Result<String>`. It shell-quotes all values itself.

Command templates gain `{model}` and `{effort}` placeholders, and resume templates also have `{session}`. Each of `{model}` and `{effort}` becomes ` <flag> '<value>'` or nothing. Exact templates:

| Vendor | `model_flag` | `effort_flag` | `resume_command` |
|---|---|---|---|
| claude | `--model` | `--effort` | `cat {prompt} \| claude -p --resume {session}{model}{effort} --output-format stream-json --verbose --dangerously-skip-permissions` |
| codex | `-m` | `None` | `cat {prompt} \| codex exec resume --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox --dangerously-bypass-hook-trust{model} {session} -` |
| cursor | `--model` | `None` | `None` |

The trailing `-` makes codex read the prompt from stdin. In the existing `command` templates, `{model}` goes directly after `claude -p`, `codex exec` and `cursor-agent -p`, and only claude's also has `{effort}`, directly after `{model}`.

`headless_command` returns an `Err` when:
- The effort is not one of `low`, `medium`, `high`, `xhigh`, `max`. The message lists the valid levels. This check is needed because `claude --effort <typo>` only warns, runs at the default and exits 0.
- An effort is given and `effort_flag` is `None`. The message names the vendor and `--effort`.

- [ ] **Step 1: Write failing tests in `src/config.rs`**

```rust
#[test] fn new_session_without_model_has_no_flag_or_placeholder() // claude: no "--model", no "{model}", no "{prompt}"; contains "cat '/s/p.md' | claude -p"
#[test] fn model_uses_each_vendors_flag() // claude "--model 'opus'", codex "-m 'o3'", cursor "--model 'x'"
#[test] fn claude_resume_keeps_stream_json_and_flags() // contains "claude -p --resume 'sess-1'", "--output-format stream-json", "--dangerously-skip-permissions"
#[test] fn codex_resume_reads_prompt_from_stdin() // ends with "'sess-1' -"; contains "codex exec resume" and the same three flags as the new-session command
#[test] fn cursor_resume_is_a_clear_error() // Err whose message contains "cursor" and "resume"
#[test] fn hostile_values_are_quoted() // model "a b'c" → "'a b'\\''c'"; session likewise
#[test] fn claude_effort_is_passed_on_new_and_resumed_sessions() // both contain "--effort 'high'"; without effort, neither contains "--effort" or "{effort}"
#[test] fn every_valid_effort_level_is_accepted() // low, medium, high, xhigh, max
#[test] fn unknown_effort_is_an_error_listing_levels() // "bogus" → Err containing "bogus" and "xhigh"; "HIGH" and "" also Err
#[test] fn effort_on_codex_or_cursor_is_a_clear_error() // Err containing the vendor name and "--effort"; no "{effort}" left in either vendor's output without effort
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test config::`
Expected: compile failure, `headless_command` not found.

- [ ] **Step 3: Add the fields, the templates and `headless_command`**

Existing tests read `.command` directly and must keep passing. `{model}` and `{effort}` stay in the stored templates.

- [ ] **Step 4: Run the tests**

Run: `cargo test config::`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: build headless commands with model, effort and resume"
```

---

### Task 2: Dispatch flags, working directory and subagent prompt

**Files:**
- Modify: `src/runner.rs`, `src/orchestrate.rs`, `src/cli.rs`, `src/main.rs`, `src/protocol.rs`, `src/event.rs` (fake only), `herdr-plugin.toml`

**Interfaces:**
- Consumes: `config::headless_command` from Task 1.
- Produces: `HerdrRunner::split(&self, pane: &str, direction: &str, focus: bool, cwd: Option<&str>) -> Result<String>`, which adds `--cwd <path>` when `Some`.
- Produces: `DispatchArgs` fields `model`, `effort`, `resume_session` and `cwd`, each `Option<String>`, as CLI flags `--model`, `--effort`, `--resume-session`, `--cwd`. `DispatchOpts` has the same names as `Option<&'a str>`.
- Produces: `protocol::build_prompt(context: &str, task_id: &str, result_file: &str, role: &str) -> String`.

Validation runs at the start of `dispatch`, before any file write or herdr call. Each is an `Err`:
- Not interactive, role `implementer`, no model. Message contains `--model`.
- `--resume-session` without an explicit `--vendor`. Message contains `--vendor`.
- `--interactive` with `--model`, `--effort` or `--resume-session`, because `--interactive` stays unchanged. Message names the flag.
- `--cwd` is not an existing directory. Message contains the path.
- `--resume-session` with cursor, or an invalid or unsupported `--effort`, from `headless_command`, also before any pane is created.

Prompt text for role `implementer` only:
- First line, exactly: `You are a subagent dispatched to execute one specific task.`
- The footer's "write your complete result" becomes "write the message you would report back", so `result_text` holds stock's short status contract and the full report stays in the report file.

Other roles get today's prompt, byte for byte.

- [ ] **Step 1: Write failing tests**

In `src/runner.rs`:
```rust
#[test] fn split_passes_cwd_when_given() // args == ["pane","split","1-2","--direction","right","--no-focus","--cwd","/wt"]
```

In `src/protocol.rs`:
```rust
#[test] fn implementer_prompt_starts_with_subagent_line_and_asks_for_report_back()
#[test] fn reviewer_prompt_has_no_subagent_line_and_keeps_complete_result()
```

In `src/orchestrate.rs`, after adding a `split_cwd` field to `FakeRunner` that records the `cwd` given to `split`:
```rust
#[test] fn implementer_without_model_errors_before_any_herdr_call() // err contains "--model"; split_calls == 0; no prompt file
#[test] fn resume_without_vendor_errors() // err contains "--vendor"; split_calls == 0
#[test] fn cursor_resume_errors_before_any_pane() // split_calls == 0
#[test] fn interactive_with_model_effort_or_resume_errors()
#[test] fn invalid_effort_errors_before_any_pane() // "bogus" → err lists levels; split_calls == 0; no prompt file
#[test] fn effort_with_defaulted_codex_vendor_errors_before_any_pane() // role reviewer, no --vendor, effort "high" → err contains "--effort"; split_calls == 0
#[test] fn effort_reaches_the_run_command() // implementer, effort "high" → command contains "--effort 'high'"
#[test] fn missing_cwd_errors_before_any_pane() // Review Focus 2
#[test] fn relative_cwd_and_scratch_become_absolute() // Review Focus 1: split_cwd absolute; the run command's prompt path absolute; the prompt's result path absolute
#[test] fn cwd_defaults_to_process_directory_for_a_new_pane() // split_cwd == current_dir
#[test] fn reused_pane_cds_to_cwd_first() // run command starts with "cd '<abs>' && "; split_calls == 0
#[test] fn resume_runs_resume_command_with_model() // command contains "claude -p --resume 'sess-1'" and "--model 'opus'"
#[test] fn resume_with_closed_reuse_pane_splits_new_pane_in_cwd() // Review Focus 3: pane_exists false → split_calls == 1, split_cwd set, still "--resume"
#[test] fn record_stores_given_cwd() // state record's cwd == the absolute --cwd, so human `resume` reopens there
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test`
Expected: compile failure on the new `split` signature and fields.

- [ ] **Step 3: Implement**

- Update `split`, and every fake: `orchestrate`, `InteractiveFake`, `cli` (`StubRunner`, `ResumeFake`), `event`. `run_resume` and `dispatch_interactive` pass `None`.
- Add the validation, using `std::path::absolute` for `--cwd` and `--scratch` in `dispatch`.
- Replace the `spec.command.replace("{prompt}", …)` line with `headless_command`.
- Set the `DispatchArgs` literal in the `second-opinion` action in `src/main.rs` to `model: None, effort: None, resume_session: None, cwd: None`.
- Set `min_herdr_version = "0.9.1"` in `herdr-plugin.toml`, since the true minimum for `pane split --cwd` is unknown.

- [ ] **Step 4: Run the tests**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src herdr-plugin.toml
git commit -m "feat: add model, effort, resume and cwd to dispatch"
```

---

### Task 3: Exit detection and bounded output

**Files:**
- Modify: `src/protocol.rs`, `src/orchestrate.rs`, `src/cli.rs`, `src/main.rs`

**Interfaces:**
- Produces: `protocol::exited_path(scratch: &Path, task_id: &str) -> PathBuf`, for `<task-id>.exited`.
- Produces: `protocol::tail_lines(text: &str, n: usize) -> String`.
- Produces: `DispatchOpts.exit_grace_ms: u64`. `run_dispatch` sets it from `const EXIT_GRACE_MS: u64 = 30_000`; test helpers use `0`.
- Produces: `DispatchResult` fields `log_file: String` and `agent_exited: bool`, and status `failed`.

The headless command becomes `{base} 2>&1 | tee {log}; touch {marker}`, and the marker is removed before dispatch along with the result and log.

**Poll order matters.** On each tick, read the result; if complete, return done. Then check the marker; if present, read the result once more, because the agent may have finished between the two reads, and return done if complete, otherwise exited.

| Outcome | `status` | `result_source` | `result_text` | `agent_exited` |
|---|---|---|---|---|
| Complete result | `done` | `file` | Full result, marker stripped | Marker seen within `exit_grace_ms` |
| Marker, partial result file | `failed` | `file` | Last 40 lines of the file | `true` |
| Marker, no result file | `failed` | `log` | Last 40 lines of the log | `true` |
| Deadline, partial file | `degraded` | `file` | Last 40 lines of the file | `false` |
| Deadline, no file | `timeout` | `scrollback` | Last 40 lines of the scrollback | `false` |

Interactive dispatches return `agent_exited: false` and the task's log path, and are otherwise unchanged. Make the `status_exit_code` arm for `failed` explicit, returning 2. `degraded` stays 3.

- [ ] **Step 1: Write failing tests**

In `src/protocol.rs`:
```rust
#[test] fn exited_path_names_marker() // "/s/t1.exited"
#[test] fn tail_lines_keeps_last_n() // tail_lines("a\nb\nc", 2) == "b\nc"; shorter input returned whole; "" stays ""
```

In `src/orchestrate.rs`, after adding `exits: bool` to `FakeRunner`, which touches the marker in `run` when true:
```rust
#[test] fn command_touches_marker_after_tee() // ends with "; touch '<scratch>/<id>.exited'"
#[test] fn stale_marker_is_cleared_before_dispatch() // pre-existing marker, agent writes nothing → status "timeout", not "failed"
#[test] fn result_and_marker_together_is_done() // Review Focus 4: Complete + exits → "done", agent_exited true
#[test] fn done_without_marker_reports_agent_not_exited() // Complete, no marker, grace 0 → agent_exited false
#[test] fn exit_without_result_fails_immediately_with_log_tail() // Review Focus 5: Nothing + exits, log 100 lines, timeout_ms 60_000 → "failed", source "log", 40 lines, elapsed well under 5s
#[test] fn exit_with_partial_file_is_failed() // Incomplete + exits → "failed", source "file"
#[test] fn timeout_bounds_scrollback_to_40_lines() // fake read returns 100 lines → 40 lines, agent_exited false
#[test] fn degraded_bounds_partial_file_to_40_lines()
#[test] fn result_reports_log_file_path()
```
Existing tests `no_marker_before_timeout_is_degraded` and `no_file_is_timeout` keep passing with `exits: false`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test`
Expected: compile failure, `exited_path` not found.

- [ ] **Step 3: Implement as the table and poll order above describe**

- [ ] **Step 4: Run the tests**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src
git commit -m "feat: detect agent exit and bound dispatch output"
```

---

### Task 4: The dispatch skill

This task runs real `claude -p` calls. They are tool-less test probes, not subagents helping with the task, so the implementer contract's no-subagents rule does not apply to them. Use the `sonnet` model; each call is capped at $0.50.

**Files:**
- Create: `superpowers/dispatching-implementers-to-herdr/SKILL.md`
- Create: `tests/skill-scenarios/run.sh`, `tests/skill-scenarios/scenarios.md`

**Interfaces:**
- Consumes: the flags from Tasks 2 and 3, and the JSON fields `status`, `session_id`, `pane_id`, `agent_exited`, `log_file`.
- Produces: the skill name `dispatching-implementers-to-herdr`, used by Task 5.

Frontmatter description, exactly:
`Use when executing a plan with superpowers:subagent-driven-development while HERDR_ENV=1, before dispatching or resuming any subagent that writes code.`

Body: a one-line purpose; `**REQUIRED BACKGROUND:** superpowers:subagent-driven-development`; the "Add: one skill" table and its three "Skill rules", copied as written from the spec file named in this plan's header; and one line saying reviewers stay native subagents. Under 500 words (`wc -w`). No restating of stock's process.

`run.sh` takes `baseline` or `with` and a repeat count, and writes each output to a directory given by `$OUT`. Each probe runs:
`claude -p --model sonnet --tools "" --permission-mode dontAsk --no-session-persistence --max-budget-usd 0.5`
The prompt is, in order:
1. The stock SDD skill and `implementer-prompt.md`, read from `$SUPERPOWERS_DIR`, defaulting to the 7.0.0 plugin cache path.
2. A line stating `HERDR_ENV=1` and the workspace path.
3. The CLAUDE.md block from Global Constraints and the new skill text, for `with` only.
4. The scenario.
5. "Reply with the exact command or tool call you would make next, and nothing else."

Scenarios in `scenarios.md`, each with its expected result:

| # | Situation | Expected |
|---|---|---|
| 1 | First dispatch, standard-tier task | `shepherdr dispatch --role implementer --vendor claude --model sonnet --timeout-ms 3600000`, with `--cwd`, absolute `--scratch`, no `--resume-session` |
| 2 | Fix round 2, previous JSON `agent_exited: true` | `--resume-session`, `--reuse-pane` |
| 3 | Fix round 2, previous JSON `agent_exited: false` | `--resume-session`, no `--reuse-pane` |
| 4 | Fix round 2, previous JSON `session_id: null` | Fresh dispatch with brief, report and findings |
| 5 | Fix round 4 | Fresh dispatch, no `--resume-session`, higher model |
| 6 | Final-review fix | Fresh `shepherdr dispatch` |
| 7 | `status: timeout` from the previous dispatch | Reads the pane; does not start another implementer |
| 8 | Two independent tasks ready | One dispatch only |
| 9 | Task reviewer due | Native subagent, not `shepherdr dispatch` |
| 10 | First dispatch, the plan says to use high effort | The scenario 1 command plus `--effort high` |
| 11 | Fix round 2, the plan says to use high effort | `--resume-session` plus `--effort high` |
| 12 | First dispatch, nothing sets an effort | No `--effort` flag |

- [ ] **Step 1: Write `run.sh` and `scenarios.md`**

- [ ] **Step 2: Run the baseline**

Run: `OUT=<scratch dir> bash tests/skill-scenarios/run.sh baseline 1`
Expected: scenarios 1-7 and 10-12 fail, because with stock alone the agent uses a native subagent or an incomplete command. If a scenario already passes, note it in the report. Record the verbatim outputs in the report.

- [ ] **Step 3: Write `SKILL.md`**

- [ ] **Step 4: Run with the skill**

Run: `OUT=<scratch dir> bash tests/skill-scenarios/run.sh with 3`
Expected: every scenario meets its expected result in all 3 repetitions. Read every output by hand rather than only grepping. If one fails, tighten the wording using writing-skills' "Match the Form to the Failure" and rerun that scenario.

- [ ] **Step 5: Commit**

```bash
git add superpowers/dispatching-implementers-to-herdr tests/skill-scenarios
git commit -m "feat: add skill mapping SDD dispatches to herdr panes"
```

---

### Task 5: Remove the old pipeline and rewrite the docs

**Files:**
- Delete: `.gitmodules`, `shaping-skills`, `superpowers/brainstorming-herdr/`, `superpowers/writing-plans-herdr/`, `superpowers/subagent-driven-development-herdr/`
- Modify: `README.md`, `superpowers/README.md`

**Interfaces:**
- Consumes: the skill `dispatching-implementers-to-herdr` from Task 4.
- Consumes: from Tasks 2 and 3, the flags `--model`, `--effort`, `--resume-session`, `--cwd`, the JSON fields `log_file` and `agent_exited`, and status `failed`.

The READMEs must state:
- Superpowers' stock steps run unchanged; shepherdr only changes where code-writing subagents run.
- Only code-writing subagents get panes; reviewers stay native.
- Dispatched agents still run with permission checks bypassed. Keep the existing warning.
- The new flags `--model`, `--effort`, `--resume-session`, `--cwd`. `--effort` is claude-only, and without it a pane agent runs at claude's configured default rather than the controller's effort.
- JSON fields `log_file` and `agent_exited`, and status `failed`.
- Exit codes: `0` done, `3` degraded, `2` timeout or failed, `1` usage or herdr error.
- Activation: one symlink plus the CLAUDE.md block from Global Constraints.
- `second-opinion`, `resume-last`, `board` and `--interactive` as manual tools outside the Superpowers flow.
- Implementers in this flow are Claude only.

Removed everywhere: shaping, breadboarding, the "four steps", the "super-shepherdr" pipeline tables, and "every delegated subagent".

- [ ] **Step 1: Remove the submodule and overlays**

```bash
git rm shaping-skills .gitmodules
git rm -r superpowers/brainstorming-herdr superpowers/writing-plans-herdr superpowers/subagent-driven-development-herdr
```

- [ ] **Step 2: Rewrite both READMEs**

- [ ] **Step 3: Verify no stale references remain**

Run: `grep -rniE 'shaping|breadboard|brainstorming-herdr|writing-plans-herdr|subagent-driven-development-herdr|every delegated' README.md superpowers src herdr-plugin.toml tests`
Expected: no output.

Run: `cargo test`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add -A README.md superpowers tests
git commit -m "docs: drop shaping and overlays, document the herdr integration"
```

---

### Task 6: Live end-to-end check

This task runs real `claude` agents, on `haiku`, inside herdr, in a throwaway repo. They run with checks bypassed, so the repo must contain nothing else. The script needs `HERDR_ENV=1` and `HERDR_PANE_ID`. If they are missing, report BLOCKED.

**Files:**
- Create: `tests/live-smoke.sh`

**Interfaces:**
- Consumes: the `target/debug/shepherdr` binary and the JSON from Tasks 2 and 3.

The script creates a temp repo with a linked worktree, uses `--from-pane "$HERDR_PANE_ID"`, parses JSON with `python3 -I`, prints PASS or FAIL per check, closes the panes it created, removes the temp repo, and exits nonzero on any failure.

Checks, in order:

1. **Manifest:** `herdr plugin link <repo>`, then `herdr plugin action list --plugin afogel.shepherdr` lists `second-opinion` and `resume-last`, then unlink so the plugin state is as it was. Check `herdr plugin unlink --help` for its argument, and if a plugin with this ID was already installed, skip this check and report it.
2. **First dispatch:** task "pick a random 6-digit number, write it to `NUMBER.txt`, commit, and report it", with `--model haiku --effort low --cwd <worktree>`. This shows that claude runs normally with the flag; it cannot show the level applied, because claude's output does not report it. Passes when `status` is `done`, `agent_exited` is true, `session_id` is non-null, the worktree's branch has the commit, and `NUMBER.txt` is in the worktree.
3. **Resume:** with `--resume-session <id> --reuse-pane <pane>`, task "using no tools, report the number you chose earlier". Passes when `status` is `done`, `pane_id` is unchanged, and `result_text` contains the number. Print whether `session_id` changed.
4. **Failure path:** `--model no-such-model-xyz --timeout-ms 300000`. Passes when `status` is `failed` in under 60 seconds, with non-empty `result_text`.

- [ ] **Step 1: Write `tests/live-smoke.sh`**

- [ ] **Step 2: Run it**

Run: `cargo build && bash tests/live-smoke.sh`
Expected: all checks PASS. Put the full output in the report, including whether the session ID changed. A failure here means earlier code or templates are wrong: fix them there with a test, rather than loosening the check.

- [ ] **Step 3: Confirm nothing was left behind**

Run: `herdr plugin list` and `git status --short`
Expected: the plugin list matches its state before the run, and the only change is the new script.

- [ ] **Step 4: Commit**

```bash
git add tests/live-smoke.sh
git commit -m "test: add live herdr end-to-end check"
```
