---
name: dispatching-implementers-to-herdr
description: Use when executing a plan with superpowers:subagent-driven-development while HERDR_ENV=1, before dispatching or resuming any subagent that writes code.
---

# Dispatching Implementers to herdr

When stock SDD says to dispatch or resume an implementer, run `shepherdr dispatch` instead of the native subagent tool.

**REQUIRED BACKGROUND:** superpowers:subagent-driven-development

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
| A waiting stretch ends and the dispatch has not returned | Find its tab by the `--task-id` label in `herdr tab list --workspace "$HERDR_WORKSPACE_ID"`, its pane by that `tab_id` in `herdr pane list --workspace "$HERDR_WORKSPACE_ID"`, then `herdr pane read <pane_id> --source visible --lines 40`. A running agent's conversation may not show up in this read; look for a prompt or an error. |
| The pane shows a prompt waiting for an answer, such as folder trust or a login | Stop and ask the owner to answer it in that tab. Trust is the owner's decision, so never answer it or send keys to it yourself. The dispatch carries on once it is answered. |
| `status: failed` (the agent stopped without a complete result) | Read `result_text`, then a bounded tail of `log_file` if that is not enough. Handle as stock's BLOCKED. |
| `status: done` with `agent_exited: false` | Read the pane before any further dispatch in that worktree. If the shell prompt has returned, carry on. If the agent is still running, handle it as `status: timeout`. |
| `status: timeout` or `degraded` (agent has not exited) | The agent is still running. Never start another implementer in that worktree while it runs. Read the pane; if it looks stuck, interrupt it with `herdr pane send-keys <pane_id> ctrl+c` and confirm the shell prompt has returned. Then handle as stock's BLOCKED, resuming if a `session_id` exists. |
| A failure or timeout looks like a claude outage or limit | Stop and tell the owner. There is no vendor to fall back to. |

Skill rules that stock does not supply:

- `--task-id` is unique per dispatch, for example `task-3-impl-r2`, because a
  dispatch deletes any earlier result and log with the same ID.
- `--scratch` is the absolute path printed by stock's `sdd-workspace`.
- `--from-pane` is `$HERDR_PANE_ID`. Each agent runs in its own background tab,
  labelled with its `--task-id`. A clean finish closes the tab. Any other outcome
  keeps it.

Reviewers stay native subagents.
