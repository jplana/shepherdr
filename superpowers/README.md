# super-shepherdr

**super-shepherdr = [superpowers](https://github.com/obra/superpowers) + shepherdr.**

The `HERDR_ENV=1` pipeline is: **shape a spec → breadboard it → write a plan → execute it with a fresh subagent per task, reviewed between tasks.** The spec and breadboard steps use the stock [shaping-skills](https://github.com/rjs/shaping-skills) (`shaping`, `breadboarding`) — bundled here as the `../shaping-skills` submodule. The plan and execute steps use the overlays below. On its own, superpowers' delegated subagents run in-process and opaque — you can't easily watch or take over one mid-task; shepherdr dispatches those same subagents into **visible, auditable herdr panes** you can read, resume, and steer.

The overlay skills are the glue. Each one **follows its stock superpowers skill exactly** and adds a single shepherdr step. They activate only when `HERDR_ENV=1`; with the variable unset, stock superpowers runs unchanged.

| Pipeline step | Skill | Codex second opinion | What it adds |
|---------------|-------|----------------------|--------------|
| 1. Spec | `shaping` (stock, submodule) | No | — (stock skill, unchanged) |
| 2. Breadboard | `breadboarding` (stock, submodule) | No | — (stock skill, unchanged) |
| 3. Plan | `writing-plans-herdr` | **Asks** | Before the execution handoff, asks whether the finished plan merits a codex second opinion; on yes, dispatches one as a live pane (reusing a prior second-opinion pane via `--reuse-pane`). |
| 4. Execute | `subagent-driven-development-herdr` | Never | Runs each **implementer** as a visible pane via `shepherdr dispatch`; adds **no** extra review (the spec + quality review loop is unchanged from stock SDD). |

`brainstorming-herdr` also ships here — same prompt-gated codex review, for `superpowers:brainstorming`'s approval gate — but is **not** part of the default pipeline; `shaping` replaces it as the entry point. Map it in only if you prefer brainstorming over shaping for a given project.

## Activate

1. Install the plugin (see the [top-level README](../README.md)): `herdr plugin install afogel/shepherdr`. Clone with `--recurse-submodules` (or run `git submodule update --init`) so the bundled `../shaping-skills` submodule is populated.
2. Have the [superpowers](https://github.com/obra/superpowers) plugin installed in Claude Code.
3. Make the overlay skills **and** the stock shaping skills discoverable to Claude Code — symlink each into your skills directory, e.g.:

   ```bash
   # herdr overlays (plan + execute steps)
   for d in writing-plans-herdr subagent-driven-development-herdr; do
     ln -s "$PWD/superpowers/$d" ~/.claude/skills/"$d"
   done
   # stock shaping skills (spec + breadboard steps) from the submodule
   for d in shaping breadboarding framing-doc kickoff-doc; do
     ln -s "$PWD/shaping-skills/$d" ~/.claude/skills/"$d"
   done
   # optional: brainstorming-herdr, only if you prefer brainstorming over shaping
   # ln -s "$PWD/superpowers/brainstorming-herdr" ~/.claude/skills/brainstorming-herdr
   ```

4. Add this block to your `~/.claude/CLAUDE.md` so Claude runs the pipeline in order when inside herdr:

   ```markdown
   # herdr Superpowers Overlays

   When `HERDR_ENV=1`, run the herdr subagent-driven-development pipeline in this
   order. Steps 1–2 are stock skills unchanged; steps 3–4 are herdr overlay
   variants that run delegated subagents as live, interactive herdr panes via the
   `afogel.shepherdr` plugin's `shepherdr` binary:

   1. **Spec** → `shaping` (stock). Initial spec planning. No codex second opinion.
   2. **Breadboard** → `breadboarding` (stock). Detail the shaped parts into
      affordances. No codex second opinion.
   3. **Plan** → `writing-plans-herdr`. Asks whether the plan merits a codex second
      opinion before the execution handoff.
   4. **Execute** → `subagent-driven-development-herdr`. Runs each implementer as a
      live pane; never asks for codex.

   When `HERDR_ENV` is unset, use the stock superpowers skills unchanged.
   `brainstorming-herdr` is no longer part of this pipeline — `shaping` replaces it
   as the entry point.
   ```

5. Run Claude Code inside a herdr session (`HERDR_ENV=1`). Shaping and breadboarding run stock (no codex), the plan gate offers a codex second opinion on demand, and subagent-driven development runs each implementer in a pane you can watch and resume.
