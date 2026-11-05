#!/usr/bin/env bash
# Usage: OUT=<dir> run.sh baseline|with <repeats>
# Runs each scenario in scenarios.md through a tool-less `claude -p` probe and
# writes the raw reply to $OUT/<mode>-<scenario>-<rep>.txt.
set -u
mode="${1:?baseline|with}"
reps="${2:?repeat count}"
: "${OUT:?set OUT to an output directory}"
here="$(cd "$(dirname "$0")" && pwd)"
sp="${SUPERPOWERS_DIR:-$HOME/.claude/plugins/cache/claude-plugins-official/superpowers/7.0.0}"
skill="$here/../../superpowers/dispatching-implementers-to-herdr/SKILL.md"
workspace="/work/demo-wt/.superpowers/sdd/demo"
mkdir -p "$OUT"

claude_md='# herdr + Superpowers

When `HERDR_ENV=1`, use `dispatching-implementers-to-herdr` for every subagent
that writes code during `superpowers:subagent-driven-development`. All other
Superpowers steps run unchanged.'

scenario_text() { # $1 = number
  awk -v n="$1" '
    $0 == "## " n { on=1; next }
    /^## / { on=0 }
    /^Expected:/ { on=0 }
    on { print }' "$here/scenarios.md"
}

nums=$(grep -E '^## [0-9]+$' "$here/scenarios.md" | sed 's/## //')
for n in $nums; do
  [ -n "${ONLY:-}" ] && [ "$n" != "$ONLY" ] && continue
  for rep in $(seq 1 "$reps"); do
    {
      echo "<stock-sdd-skill>"; cat "$sp/skills/subagent-driven-development/SKILL.md"; echo "</stock-sdd-skill>"
      echo "<implementer-prompt.md>"; cat "$sp/skills/subagent-driven-development/implementer-prompt.md"; echo "</implementer-prompt.md>"
      echo
      echo "HERDR_ENV=1. The workspace path is $workspace."
      if [ "$mode" = with ]; then
        echo; echo "<CLAUDE.md>"; echo "$claude_md"; echo "</CLAUDE.md>"
        echo "<skill name=\"dispatching-implementers-to-herdr\">"; cat "$skill"; echo "</skill>"
      fi
      echo; scenario_text "$n"
      echo; echo "Reply with the exact command or tool call you would make next, and nothing else."
    } | claude -p --model sonnet --tools "" --permission-mode dontAsk \
          --no-session-persistence --max-budget-usd 0.5 \
          > "$OUT/$mode-$n-$rep.txt" 2>&1
  done
done
