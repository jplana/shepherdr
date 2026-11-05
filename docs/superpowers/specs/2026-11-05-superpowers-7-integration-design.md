# Superpowers 7 integration for shepherdr

## Intent

The owner wants Superpowers integrated with herdr and is happy with the stock
Superpowers steps. shepherdr should add one thing to them: implementer
subagents run in visible herdr panes. It should not add a pipeline of its own.

The current overlays say they follow stock "exactly", but they have drifted
through Superpowers 6.2 to 7.0. They contradict stock in places and restate
parts of it that have since changed. The fix is to stop restating stock.

## Goals

- Inside herdr (`HERDR_ENV=1`), Superpowers runs its stock steps unchanged:
  brainstorming, writing-plans, then either execution choice.
- When stock subagent-driven development (SDD) dispatches an implementer, the
  implementer runs in a herdr pane through `shepherdr dispatch`.
- `dispatch` can honor what stock SDD requires of a dispatch: an explicit
  model, resuming the implementer for fix rounds, and a short report back to
  the controller.
- An implementer in a pane behaves like a stock subagent and does not restart
  the Superpowers workflow.

## Non-goals

- **Reviewers in panes.** Task, re-review and final reviewers stay native
  Claude Code subagents. Moving them to panes today would add agents running
  with checks bypassed. Revisit after sandboxing.
- **Sandboxing or changing permission flags.** That is the next piece of work.
  New commands in this work keep the existing flags, including the new headless
  resume. The existing human `resume` command stays free of autonomy flags.
- **The other bugs from the review**, unless listed under Scope: board and
  `resume-last` state, path traversal in task IDs, automatically stopping agents
  on timeout, and surfacing herdr errors.
- **Non-Claude implementers in this flow.** The skill dispatches `claude` only.
  The `cursor` vendor stays in the code and keeps its unit tests, but the skill
  offers no vendor fallback and shepherdr gains no out-of-tokens detection.
- **Moving to herdr's `agent start` API** and **packaging as a Claude Code
  plugin.**
- **An overlay for `executing-plans` (Native execution).** It dispatches no
  implementers, so it needs nothing from shepherdr.

## Anti-goals

- A skill body that restates stock SDD's process. Restating it is what caused
  this drift. If stock changes its review flow next release, the new skill
  should still be correct.
- Any step Superpowers does not have, such as a prompt offering a second
  opinion.

## Decisions by the owner

- Remove shaping entirely; it was never used.
- Keep the stock Superpowers steps as they are.
- Only implementers get panes in this work.
- `dispatch` supports selecting the effort for subagents.

## Scope

### Remove

- The `shaping-skills` submodule and `.gitmodules`.
- `superpowers/brainstorming-herdr/`, `superpowers/writing-plans-herdr/` and
  `superpowers/subagent-driven-development-herdr/`.
- The four-step pipeline and its CLAUDE.md block from both READMEs.

### Add: one skill

`superpowers/dispatching-implementers-to-herdr/SKILL.md`. It answers one
question: when stock SDD says to dispatch or resume an implementer, what do you
run instead of the native subagent tool? It requires
`superpowers:subagent-driven-development` as background and does not restate it.

| Stock SDD action | In herdr |
|---|---|
| Compose the implementer dispatch from `implementer-prompt.md` | Write the same composed prompt to a context file in the plan's SDD workspace. |
| Dispatch an implementer on a chosen model | `shepherdr dispatch --role implementer --vendor claude --model <m> --timeout-ms 3600000 --context-file <f> --from-pane <p> --task-id <id> --scratch <workspace> --cwd <worktree>`, run as a background command. Stock's cheap, standard and most capable tiers map to `haiku`, `sonnet` and `opus`. |
| An effort is set for the dispatch, by the plan or the owner | Add `--effort <level>`, on every dispatch and resume it applies to. Otherwise omit it. The flag takes named levels only, not the native tool's `lower` or `higher`. |
| Record the implementer's identity | Record `session_id`, `pane_id` and `agent_exited` from the JSON output. |
| Fix rounds 1-3: resume the implementer; also answering NEEDS_CONTEXT | The same command plus `--resume-session <session_id>`, with the findings or answer as the context file. Add `--reuse-pane <pane_id>` only if the previous dispatch returned `agent_exited: true`; otherwise use a new pane. |
| No resume possible (`session_id` is null) | Stock's own fallback: dispatch a fresh implementer with the brief path, report path and findings. |
| Fix rounds 4-5: fresh implementer on a more capable model | Write a new context file naming the brief path, report path and findings, and pass that as `--context-file`. Then a dispatch without `--resume-session`, with the higher `--model`. |
| Final-review fix subagent | A fresh dispatch, because it writes code. Any stock subagent that writes code counts as an implementer; any that only reads stays native. |
| Wait for the implementer | Stock's waiting rule applies; the dispatch is a background command that reports when it exits. |
| `status: failed` (agent exited without a complete result) | Read `result_text`, then a bounded tail of `log_file` if that is not enough. Handle as stock's BLOCKED. |
| `status: done` with `agent_exited: false` | Read the pane before any further dispatch in that worktree. If the shell prompt has returned, carry on. If the agent is still running, handle it as `status: timeout`. |
| `status: timeout` or `degraded` (agent has not exited) | The agent is still running. Never start another implementer in that worktree while it runs. Read the pane; if it looks stuck, interrupt it with `herdr pane send-keys <pane_id> ctrl+c` and confirm the shell prompt has returned. Then handle as stock's BLOCKED, resuming if a `session_id` exists. |
| A failure or timeout looks like a claude outage or limit | Stop and tell the owner. There is no vendor to fall back to. |

Skill rules that stock does not supply:

- `--task-id` is unique per dispatch, for example `task-3-impl-r2`, because a
  dispatch deletes any earlier result and log with the same ID.
- `--scratch` is the absolute path printed by stock's `sdd-workspace`.
- `--from-pane` is the workers tab's root pane. The workspace ID is
  `$HERDR_WORKSPACE_ID`. Create the tab with
  `herdr tab create --workspace "$HERDR_WORKSPACE_ID" --cwd <worktree> --label workers --no-focus`;
  its JSON holds the root pane at `result.root_pane.pane_id`. If
  `herdr tab list --workspace "$HERDR_WORKSPACE_ID"` already shows a tab labelled
  `workers`, reuse it: its root pane is the first pane with that `tab_id` in
  `herdr pane list --workspace "$HERDR_WORKSPACE_ID"`.

**Activation:** one symlink into `~/.claude/skills/` and a short CLAUDE.md block
saying that with `HERDR_ENV=1`, SDD implementer dispatches use this skill. The
description is triggering conditions only: when executing a plan with
`superpowers:subagent-driven-development` while `HERDR_ENV=1`.

### Change: `shepherdr dispatch`

- **`--model <name>`** passes the model to the vendor CLI: `--model` for claude
  and cursor, `-m` for codex. It is required for `--role implementer`, because
  stock requires an explicit model and a required flag cannot be forgotten. It
  stays optional for reviewers, so the `prefix+r` action keeps working.
- **`--effort <level>`** passes `--effort` to claude. Valid levels are `low`,
  `medium`, `high`, `xhigh` and `max`. It is optional for every role: stock SDD
  never sets an effort, so it has nothing to require. Without it, the agent runs
  at claude's configured default, not at the controller's effort as a native
  subagent would. shepherdr checks the level itself, because `claude --effort
  <typo>` only warns, then runs at the default and exits 0. With codex or cursor
  it fails with a clear error: effort for them is not needed by this flow and
  could not be tested here.
- **`--resume-session <id>`** runs the vendor's headless resume instead of a new
  session: `claude -p --resume <id>` and `codex exec resume <id>`, with the same
  output and autonomy flags as a normal headless dispatch. Requires `--vendor`,
  since a session belongs to one vendor; without it, fail with a clear error.
  The returned `session_id` is parsed from this run's log as usual, and the
  caller uses the latest one.
- **`--cwd <path>`** is passed to `herdr pane split --cwd`, and defaults to the
  process's working directory. When a pane is reused, the command is prefixed
  with `cd <path> &&`. Today nothing sets this for headless runs, so an
  implementer could run outside the worktree. It is included because stock's
  worktree isolation depends on it. The `second-opinion` action passes its
  context's worktree when it has one.
- **Exit detection for headless runs.** The pane command runs the agent pipeline
  under its own `sh -c`, with traps that create `<scratch>/<task-id>.exited` when
  that `sh` exits: on a normal or nonzero exit, on SIGINT, SIGTERM or SIGHUP, and
  when a reused pane's `cd` fails, whichever shell the pane runs. The `sh` acts on
  a signal only after the pipeline has ended, so the marker always means the agent
  is gone. Dispatch clears that file beforehand as it does the result and log.
  While polling:
  - The marker with no complete result gives a new `status: failed`, with exit
    code 2, immediately instead of at the deadline.
  - A pane that herdr reports gone counts the same as the marker, so it also gives
    `failed` with `agent_exited: true`. An error from herdr does not count as gone.
  - The result file is re-read before either of those is believed: a complete
    result is always `done`.
  - After a complete result, dispatch waits up to 30 seconds for the marker or the
    pane going away, because an agent can write its result before it exits.
  - The JSON gains `agent_exited`, which says whether a pane is safe to reuse:
    `pane run` on a pane with a live agent would go to that agent's input.
  - A `degraded` or `timeout` status now always means the marker never appeared
    and the pane was still there.
  This is included because implementer timeouts must be long, and without it a
  dead agent would cost the full hour. `--interactive` is unchanged.
- **Bounded output.** `result_text` holds the agent's short status contract on
  `done`. On `failed`, `timeout` and `degraded`, it holds no more than the last
  40 lines and at most 8 KiB, cut on a character boundary, with a leading marker
  when the byte cap dropped text. The JSON gains `log_file` so the rest stays out
  of the controller's context, and `session_id`, `log_file` and `agent_exited`
  come before `result_text`. The JSON's other fields stay the same.
- **Prompt preamble for implementers.** The prompt gains a leading line stating
  that the agent is a subagent dispatched to execute one specific task. That is
  the condition `using-superpowers` uses to stand down; headless `claude -p`
  does load that skill at startup. For implementers only, the footer's "when you
  have finished" also covers stopping early to ask a question or report being
  blocked, so such a stop comes back as `done` with the message in `result_text`.

`HERDR_ENV` is not unset in panes, because herdr's own agent integration may use
it. With the pipeline block gone from CLAUDE.md, it no longer starts a workflow.

### Keep

The `second-opinion` and `resume-last` actions and keybindings, `board`, the
`worktree.created` hook, the human `resume` command, and `--interactive`. These
remain as manual tools outside the Superpowers flow. Codex stays the default
vendor for `--role reviewer`, used only by `prefix+r` now.

### Docs

Rewrite both READMEs around the above. They must be accurate that only
implementers get panes, that dispatched agents still run with checks bypassed,
and that `degraded` exits with code 3.

## Verification

- Rust changes follow TDD using the existing fake runner.
- The new skill follows `superpowers:writing-skills`: a baseline scenario first
  (an agent running SDD with `HERDR_ENV=1` and no skill), then with the skill.
  Scenarios must show it choosing the right command for a first dispatch, a fix
  round 2, a fix round 4 and the final-review fix, handling a timeout without
  starting a second implementer, and never dispatching implementers in parallel.
- One live end-to-end check in a throwaway repo and worktree inside herdr: a
  first dispatch to a real `claude` implementer that commits, then a resume in
  the same pane that sees its earlier context. This is also the first real test
  of the plugin manifest against herdr 0.9.1, and it must confirm that the
  resume keeps the conversation, and whether the session ID changes.

## Calls made by the author, not the owner

- Removing `brainstorming-herdr` and `writing-plans-herdr`, and with them the
  in-flow prompt for a codex second opinion. The owner approved keeping stock
  steps and did not object when this was stated.
- Requiring `--model` for implementers rather than warning.
- `--effort` is optional, claude-only and checked by shepherdr, and the skill
  adds no rule of its own for choosing a level, because stock has none.
- Claude-only implementers in this flow: `cursor-agent` is not installed here,
  and stock's reviewers are already Claude.
- A 60-minute implementer timeout, set by the skill. The 10-minute default stays
  for other uses.
- Adding exit detection, with the `failed` status and `agent_exited`. The first
  draft left this for later, but the timeout handling depends on it.
- Treating the final-review fix subagent as an implementer, so it gets a pane.
- The skill name, the 40-line and 8 KiB caps, the 30-second wait and the
  `log_file` field.
- Activation by symlink and CLAUDE.md block, kept simple until plugin packaging.

## Left to the builder

- How model and resume are represented in `VendorSpec` and the command
  templates.
- Whether `min_herdr_version` needs raising for `pane split --cwd`; raise it to
  the tested 0.9.1 if the true minimum cannot be determined.
- Cursor's headless resume form. `cursor-agent` is not installed here, so it
  cannot be verified: make `--resume-session` with cursor fail with a clear
  error, unless its form can be confirmed from its docs.
