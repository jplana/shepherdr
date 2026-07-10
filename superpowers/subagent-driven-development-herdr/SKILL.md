---
name: subagent-driven-development-herdr
description: Use INSTEAD OF superpowers:subagent-driven-development when HERDR_ENV=1. Runs each implementer subagent as a live, auditable herdr pane (via shepherdr) instead of the native Agent tool. Does NOT add any codex/second-opinion review — review follows stock SDD.
---

# Subagent-Driven Development (herdr overlay)

**Precondition:** `HERDR_ENV=1`. If unset, use stock `superpowers:subagent-driven-development`.

Follow `superpowers:subagent-driven-development` exactly, with ONE change: dispatch each **implementer** as a live herdr pane via `shepherdr` instead of the native Agent tool, so you can watch and audit it. **Do not add a codex or second-opinion review here** — the review and fix loop is unchanged from stock SDD (second opinions belong only to the brainstorm and plan gates).

## Setup (once)

Create a workers tab (or reuse the one the `worktree.created` hook made):

```bash
herdr tab create --workspace <workspace-id> --label workers
```
Record the workers tab's root pane id (from the `tab create` JSON, `result.root_pane`) as your `--from-pane`.

## Per task

1. **Implement (as a visible pane).** Write the task's isolated context (task text + interfaces + global constraints) to `<scratch>/task-<n>.context.md`, then:

```bash
shepherdr dispatch --role implementer \
  --context-file <scratch>/task-<n>.context.md \
  --from-pane <workers-root-pane> --task-id task-<n>-impl --scratch <scratch>
```
The implementer runs headless in the pane, streaming its work (tool calls + reasoning) so you can audit it live and afterward; the pane stays open. Read its result from the printed `result_file` (or `result_text`).

2. **Review + fix per stock SDD.** Run the task review and any fix loop exactly as `superpowers:subagent-driven-development` prescribes. This overlay does not layer a codex second opinion on top.

3. Mark the task complete and continue. Do not pause between tasks (stock rule).

## Vendor selection

Implementers default to `claude`. Override with `--vendor`. If claude tokens run out, add it to `--unavailable` so dispatch falls back to `cursor`.

## Concurrency

Independent tasks may dispatch in parallel into the workers tab. Cap concurrent worker panes at 3 to keep the tab readable; queue beyond that. Watch progress in the plugin's workers board (`herdr plugin pane open --plugin afogel.shepherdr --entrypoint board`).

## Finish

When all tasks are complete, do the stock SDD final whole-branch review, then use `superpowers:finishing-a-development-branch`.
