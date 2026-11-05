#!/usr/bin/env bash
# Live end-to-end check: real herdr, real claude agents on haiku, throwaway repo.
# Agents run with permission checks bypassed, so everything happens under a fresh mktemp -d.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/debug/shepherdr"
PLUGIN_ID="afogel.shepherdr"

if [ "${HERDR_ENV:-}" != "1" ] || [ -z "${HERDR_PANE_ID:-}" ]; then
  echo "BLOCKED: needs HERDR_ENV=1 and HERDR_PANE_ID"; exit 2
fi
[ -x "$BIN" ] || { echo "BLOCKED: build first (cargo build)"; exit 2; }

TMP="$(mktemp -d)"
REPO="$TMP/repo"; WT="$TMP/wt"; STATE="$TMP/state"
CREATED_PANES=""
LINKED=0
FAILS=0

cleanup() {
  for p in $CREATED_PANES; do herdr pane close "$p" >/dev/null 2>&1; done
  [ "$LINKED" = 1 ] && herdr plugin unlink "$PLUGIN_ID" >/dev/null 2>&1
  rm -rf "$TMP"
}
trap cleanup EXIT

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILS=$((FAILS+1)); }
jget() { python3 -I -c 'import sys,json
d=json.load(sys.stdin); v=d.get(sys.argv[1])
print("" if v is None else (str(v).lower() if isinstance(v,bool) else v))' "$1"; }
note_pane() { [ -n "$1" ] && CREATED_PANES="$CREATED_PANES $1"; }

mkdir -p "$STATE"
git init -q -b main "$REPO"
git -C "$REPO" config user.email smoke@example.com
git -C "$REPO" config user.name smoke
echo smoke > "$REPO/README"
git -C "$REPO" add README && git -C "$REPO" commit -q -m init
git -C "$REPO" worktree add -q -b smoke-branch "$WT"
git -C "$WT" config user.email smoke@example.com
git -C "$WT" config user.name smoke

# 1. Manifest
if herdr plugin list 2>/dev/null | grep -q "$PLUGIN_ID"; then
  echo "SKIP: manifest check ($PLUGIN_ID already installed)"
else
  if herdr plugin link "$ROOT" >/dev/null 2>&1; then
    LINKED=1
    out="$(herdr plugin action list --plugin "$PLUGIN_ID" 2>&1)"
    if echo "$out" | grep -q second-opinion && echo "$out" | grep -q resume-last; then
      pass "manifest lists second-opinion and resume-last"
    else fail "manifest actions: $out"; fi
    herdr plugin unlink "$PLUGIN_ID" >/dev/null 2>&1 && LINKED=0
  else fail "herdr plugin link"; fi
fi

dispatch() { # task-id context-text extra-args...
  local id="$1" text="$2"; shift 2
  printf '%s\n' "$text" > "$TMP/$id.md"
  "$BIN" dispatch --role implementer --vendor claude --from-pane "$HERDR_PANE_ID" \
    --task-id "$id" --context-file "$TMP/$id.md" --scratch "$TMP" --state-dir "$STATE" \
    --cwd "$WT" "$@" 2>"$TMP/$id.err"
}

# 2. First dispatch
OUT1="$(dispatch t1 "Pick a random 6-digit number, write it to NUMBER.txt, commit, and report it." \
  --model haiku --effort low --timeout-ms 300000)"
echo "$OUT1" | head -c 1500; echo
S1="$(echo "$OUT1" | jget status)"; EX1="$(echo "$OUT1" | jget agent_exited)"
SID1="$(echo "$OUT1" | jget session_id)"; PANE1="$(echo "$OUT1" | jget pane_id)"
note_pane "$PANE1"
NUM=""; [ -f "$WT/NUMBER.txt" ] && NUM="$(tr -dc '0-9' < "$WT/NUMBER.txt")"
COMMITS="$(git -C "$WT" log --oneline main..HEAD 2>/dev/null | wc -l | tr -d ' ')"
if [ "$S1" = done ] && [ "$EX1" = true ] && [ -n "$SID1" ] && [ "$COMMITS" -ge 1 ] && [ -n "$NUM" ]; then
  pass "first dispatch (number=$NUM, session=$SID1, pane=$PANE1)"
else fail "first dispatch status=$S1 exited=$EX1 session=$SID1 commits=$COMMITS number=$NUM"; cat "$TMP/t1.err"; fi

# 3. Resume
OUT2="$(dispatch t2 "Using no tools, report the number you chose earlier." \
  --model haiku --effort low --timeout-ms 300000 --resume-session "$SID1" --reuse-pane "$PANE1")"
echo "$OUT2" | head -c 1500; echo
S2="$(echo "$OUT2" | jget status)"; PANE2="$(echo "$OUT2" | jget pane_id)"
SID2="$(echo "$OUT2" | jget session_id)"; R2="$(echo "$OUT2" | jget result_text)"
note_pane "$PANE2"
if [ "$S2" = done ] && [ "$PANE2" = "$PANE1" ] && [ -n "$NUM" ] && echo "$R2" | grep -q "$NUM"; then
  pass "resume (same pane, result contains number)"
else fail "resume status=$S2 pane=$PANE2 (was $PANE1) result=$R2"; cat "$TMP/t2.err"; fi
if [ "$SID2" = "$SID1" ]; then echo "INFO: session_id unchanged ($SID2)"; else echo "INFO: session_id changed ($SID1 -> $SID2)"; fi

# 4. Failure path
T0=$(date +%s)
OUT3="$(dispatch t3 "Say hello." --model no-such-model-xyz --timeout-ms 300000)"
T1=$(date +%s)
echo "$OUT3" | head -c 1500; echo
S3="$(echo "$OUT3" | jget status)"; R3="$(echo "$OUT3" | jget result_text)"
note_pane "$(echo "$OUT3" | jget pane_id)"
if [ "$S3" = failed ] && [ $((T1-T0)) -lt 60 ] && [ -n "$R3" ]; then
  pass "failure path (failed in $((T1-T0))s)"
else fail "failure path status=$S3 elapsed=$((T1-T0))s result=$R3"; cat "$TMP/t3.err"; fi

echo "failures: $FAILS"
[ "$FAILS" = 0 ]
