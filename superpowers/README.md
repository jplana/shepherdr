# super-shepherdr

**super-shepherdr = [superpowers](https://github.com/obra/superpowers) + shepherdr.**

superpowers gives Claude Code a disciplined workflow (brainstorm → write a spec → write a plan → execute it with a fresh subagent per task, reviewed between tasks). On its own, those delegated subagents run in-process and opaque — you can't easily watch or take over one mid-task.

shepherdr dispatches those same subagents into **visible, auditable herdr panes**. Put them together and every delegated agent in a superpowers workflow becomes a live pane you can read, resume, and steer.

These three overlay skills are the glue. Each one **follows its stock superpowers skill exactly** and adds a single shepherdr step. They activate only when `HERDR_ENV=1`; with the variable unset, stock superpowers runs unchanged.

| Overlay | Stock skill it overrides | What it adds |
|---------|--------------------------|--------------|
| `brainstorming-herdr` | `superpowers:brainstorming` | Before the approval gate, **asks** whether the design merits a codex second opinion; on yes, dispatches one as a live pane (reusing a prior second-opinion pane via `--reuse-pane`). |
| `writing-plans-herdr` | `superpowers:writing-plans` | Same prompt-gated codex review, for the finished plan before the execution handoff. |
| `subagent-driven-development-herdr` | `superpowers:subagent-driven-development` | Runs each **implementer** as a visible pane via `shepherdr dispatch`; adds **no** extra review (the spec + quality review loop is unchanged from stock SDD). |

## Activate

1. Install the plugin (see the [top-level README](../README.md)): `herdr plugin install afogel/shepherdr`.
2. Have the [superpowers](https://github.com/obra/superpowers) plugin installed in Claude Code.
3. Make the overlay skills discoverable to Claude Code — copy or symlink each overlay directory into your skills directory, e.g.:

   ```bash
   for d in brainstorming-herdr writing-plans-herdr subagent-driven-development-herdr; do
     ln -s "$PWD/superpowers/$d" ~/.claude/skills/"$d"
   done
   ```

4. Add this block to your `~/.claude/CLAUDE.md` so Claude prefers the overlays when running inside herdr:

   ```markdown
   # herdr Superpowers Overlays

   When `HERDR_ENV=1`, prefer the herdr overlay variants of these superpowers skills,
   which run delegated subagents as live, interactive herdr panes via the
   `afogel.shepherdr` plugin's `shepherdr` binary:

   - brainstorming → `brainstorming-herdr`
   - writing-plans → `writing-plans-herdr`
   - subagent-driven-development → `subagent-driven-development-herdr`

   Each overlay follows the stock skill exactly and adds a cross-vendor review /
   pane dispatch step. When `HERDR_ENV` is unset, use the stock superpowers skills unchanged.
   ```

5. Run Claude Code inside a herdr session (`HERDR_ENV=1`). Brainstorm/plan gates now offer a codex second opinion on demand, and subagent-driven development runs each implementer in a pane you can watch and resume.
