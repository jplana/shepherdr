# Superpowers integration

shepherdr changes one thing about [superpowers](https://github.com/obra/superpowers): where code-writing subagents run. Superpowers' stock steps run unchanged. During `superpowers:subagent-driven-development`, each subagent that writes code runs in a visible herdr pane via `shepherdr dispatch`, which you can watch, resume, and take over. Reviewers stay native. Implementers in this flow are Claude only.

The skill that does this is `dispatching-implementers-to-herdr`. It maps each stock SDD dispatch or resume of an implementer onto `shepherdr dispatch` with `--model` (required), optional `--effort`, `--resume-session` and `--cwd`. Read its JSON output: `session_id`, `pane_id`, `log_file` and `agent_exited`. Status `failed` means the agent exited without a complete result.

`--effort` is claude-only. Without it, a pane agent runs at claude's configured default effort, not the controller's.

Dispatched agents still run with permission checks bypassed (`--dangerously-skip-permissions` for claude). This is deliberate so they can do the work, but only run it on repos and tasks you are comfortable letting an agent operate on unattended.

Exit codes: `0` done, `3` degraded, `2` timeout or failed, `1` usage or herdr error.

## Activate

1. Install the plugin (see the [top-level README](../README.md)): `herdr plugin install afogel/shepherdr`.
2. Have the superpowers plugin installed in Claude Code.
3. Symlink the skill into your skills directory. This step assumes a clone of this repo: run it from the root of a clone, cloning first if you only ran `herdr plugin install`, and keep the clone in place because the symlink points into it.

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
