---
name: brainstorming-herdr
description: Use INSTEAD OF superpowers:brainstorming when HERDR_ENV=1. Runs stock brainstorming; before the approval gate it ASKS whether the design merits a codex second opinion and, only if you say yes, dispatches one as a live herdr pane (reusing a prior second-opinion pane when one is open).
---

# Brainstorming (herdr overlay)

**Precondition:** `HERDR_ENV=1`. If unset, use stock `superpowers:brainstorming`.

Follow `superpowers:brainstorming` exactly, with ONE addition.

**Before presenting the design for approval, ask whether it merits a second opinion.** Do not dispatch automatically. Ask the user a single question, e.g. "Want a codex second opinion on this design before you approve?" Only if they say yes, do the dispatch below. If they decline, go straight to the approval gate.

If yes:

1. Write the design you are about to present to `<scratch>/brainstorm-review.context.md`, prefaced with: "You are a second reviewer. Critique this design for gaps, risks, and unstated assumptions. Be concise and specific."
2. Find your own pane id with `herdr pane list` (the focused pane) for `--from-pane`. **Pane reuse:** if you dispatched a second opinion earlier this session and that pane is still open, pass its pane id via `--reuse-pane` so this review runs in the same pane instead of opening another. Dispatch:

```bash
shepherdr dispatch \
  --role reviewer \
  --context-file <scratch>/brainstorm-review.context.md \
  --from-pane <your-pane> \
  --task-id brainstorm-review \
  --scratch <scratch> \
  [--reuse-pane <prior-second-opinion-pane-id>]
```

3. It prints JSON `{status, result_text, pane_id, ...}`. **Record `pane_id`** — that is the second-opinion pane to `--reuse-pane` next time. On `done`, fold `result_text` into what you present (agree → adjust the design; disagree → note why). On `timeout`/`degraded`, read the pane (`herdr pane read <pane_id> --source recent-unwrapped`) and surface the problem instead of proceeding silently. The codex pane stays open so the user can read its reasoning.
4. Proceed to the normal approval gate. The transition target after the spec is `writing-plans-herdr`.
