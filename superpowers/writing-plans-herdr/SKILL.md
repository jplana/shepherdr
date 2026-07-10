---
name: writing-plans-herdr
description: Use INSTEAD OF superpowers:writing-plans when HERDR_ENV=1. Runs stock plan-writing; before the execution handoff it ASKS whether the plan merits a codex second opinion and, only if you say yes, dispatches one as a live herdr pane (reusing a prior second-opinion pane when one is open).
---

# Writing Plans (herdr overlay)

**Precondition:** `HERDR_ENV=1`. If unset, use stock `superpowers:writing-plans`.

Follow `superpowers:writing-plans` exactly, with ONE addition.

**After the plan is drafted and self-reviewed, before the Execution Handoff, ask whether it merits a second opinion.** Do not dispatch automatically. Ask the user a single question, e.g. "Want a codex review of the plan before we execute?" Only if they say yes, do the dispatch below. If they decline, go straight to the Execution Handoff.

If yes:

1. Write to `<scratch>/plan-review.context.md`: "You are a second reviewer of an implementation plan. Check for missing tasks, inconsistent interfaces between tasks, placeholder steps, and untestable steps. Reference task numbers. Be concise." followed by the full plan text.
2. Find your own pane id with `herdr pane list` for `--from-pane`. **Pane reuse:** if a second-opinion pane from earlier this session is still open (e.g. the brainstorming design review), pass its pane id via `--reuse-pane` so this review runs in the same pane. Dispatch:

```bash
shepherdr dispatch \
  --role reviewer \
  --context-file <scratch>/plan-review.context.md \
  --from-pane <your-pane> \
  --task-id plan-review \
  --scratch <scratch> \
  [--reuse-pane <prior-second-opinion-pane-id>]
```

3. It prints JSON `{status, result_text, pane_id, ...}`. **Record `pane_id`** for reuse next time. On `done`, apply valid findings to the plan file inline (add missing tasks, fix interface mismatches); note rejected findings and why. On `timeout`/`degraded`, read the pane and surface it. The codex pane stays open for follow-ups.
4. Proceed to the Execution Handoff. Recommend `subagent-driven-development-herdr` as the execution skill.
