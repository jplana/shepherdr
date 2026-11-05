# Superpowers integration

shepherdr changes one thing about [superpowers](https://github.com/obra/superpowers): where code-writing subagents run. Superpowers' stock steps run unchanged. During `superpowers:subagent-driven-development`, each subagent that writes code runs in a visible herdr pane via `shepherdr dispatch`, which you can watch, resume, and take over. Reviewers stay native. Implementers in this flow are Claude only.

The skill that does this is `dispatching-implementers-to-herdr`. It maps each stock SDD dispatch or resume of an implementer onto `shepherdr dispatch` with `--model` (required), optional `--effort`, `--resume-session` and `--cwd`. Each implementer runs Claude's real interface in its own background tab, which closes after a clean finish. Read the JSON output: `session_id`, `pane_id`, `log_file` and `agent_exited`. Status `failed` means the agent stopped without a complete result.

`--effort` is claude-only. Without it, a pane agent runs at claude's configured default effort, not the controller's.

Implementers run in Claude's auto mode, with no bypass flag. A hook answers any permission prompt with "no", so a blocked agent reports `BLOCKED` through the normal flow instead of waiting in a background tab. Claude's OS sandbox is not enabled, so what auto mode allows runs with your user's access. A folder Claude has not been told to trust stops at a prompt until you answer it; git worktrees of a trusted repo did not prompt in our testing.

Exit codes: `0` done, `3` degraded, `2` timeout or failed, `1` usage or herdr error.

A herdr call that takes longer than 10 seconds counts as a herdr error. While watching for a result, `dispatch` keeps going through these errors, so a hung herdr delays a result by up to 10 seconds per check instead of blocking it. If herdr hangs while the agent is being started, `dispatch` exits `1` and names the pane, because the agent may already be running there: check that pane before dispatching again.

## Activate

1. Install the plugin from a clone (see the [top-level README](../README.md)).
2. Have the superpowers plugin installed in Claude Code.
3. Symlink the skill into your skills directory. Run this from the root of the clone, and keep the clone in place because the symlink points into it.

   ```bash
   ln -s "$PWD/superpowers/dispatching-implementers-to-herdr" ~/.claude/skills/dispatching-implementers-to-herdr
   ```

4. Add this block to your `~/.claude/CLAUDE.md`:

   ```markdown
   # herdr + Superpowers

   When `HERDR_ENV=1`, use `dispatching-implementers-to-herdr` for every subagent
   that writes code during `superpowers:subagent-driven-development`. All other
   Superpowers steps run unchanged.
   ```

5. Run Claude Code inside a herdr session (`HERDR_ENV=1`).

Upgrading from an older shepherdr: remove its overlay symlinks from `~/.claude/skills/` (every `*-herdr` link except `dispatching-implementers-to-herdr`) and delete the old herdr block from your CLAUDE.md, which the block above replaces.

## Manual tools

Outside the Superpowers flow, `second-opinion`, `resume-last`, `board` and `dispatch --interactive` remain available as manual tools; see the top-level README.
