# Dispatch skill scenarios

Each scenario is a `## N` section. `run.sh` sends everything above the
`Expected:` line as the scenario text. Facts common to all scenarios: the plan
is being run with superpowers:subagent-driven-development, the worktree is
`/work/demo-wt`, the SDD workspace is `/work/demo-wt/.superpowers/sdd/demo`,
`$HERDR_PANE_ID` is `w1-2`, the controller is about to act.

## 1
You are the controller. Task 3 is ready and is a standard-tier task. The task
brief is at `/work/demo-wt/.superpowers/sdd/demo/task-3-brief.md`. No implementer
has been dispatched for it yet. Dispatch the implementer.

Expected: `shepherdr dispatch --role implementer --vendor claude --model sonnet --timeout-ms 3600000`, with `--cwd`, absolute `--scratch`, no `--resume-session`.

## 2
Task 3's reviewer found issues. This is fix round 2. The previous implementer
dispatch returned `{"status":"done","session_id":"abc-123","pane_id":"w1-5","agent_exited":true}`.
Findings are in `/work/demo-wt/.superpowers/sdd/demo/task-3-findings-r2.md`. Send the fixes to the implementer.

Expected: `--resume-session abc-123` and `--reuse-pane w1-5`.

## 3
Task 3's reviewer found issues. This is fix round 2. The previous implementer
dispatch returned `{"status":"done","session_id":"abc-123","pane_id":"w1-5","agent_exited":false}`.
Findings are in `/work/demo-wt/.superpowers/sdd/demo/task-3-findings-r2.md`. Send the fixes to the implementer.

Expected: `--resume-session abc-123`, no `--reuse-pane`.

## 4
Task 3's reviewer found issues. This is fix round 2. The previous implementer
dispatch returned `{"status":"done","session_id":null,"pane_id":"w1-5","agent_exited":true}`.
Findings are in `/work/demo-wt/.superpowers/sdd/demo/task-3-findings-r2.md`. The brief is
`task-3-brief.md` and the implementer's report is `task-3-report.md` in the same directory. Send the fixes.

Expected: fresh dispatch (no `--resume-session`) whose context carries the brief, report and findings.

## 5
Task 3's reviewer found issues. This is fix round 4 (rounds 1-3 used the
standard-tier `sonnet` implementer). The previous dispatch returned
`{"status":"done","session_id":"abc-123","pane_id":"w1-5","agent_exited":true}`.
Findings are in `/work/demo-wt/.superpowers/sdd/demo/task-3-findings-r4.md`. Send the fixes.

Expected: fresh dispatch, no `--resume-session`, higher model (`opus`), and a context file naming the brief, report and findings.

## 6
All tasks are done. The final whole-implementation review found issues that need
code changes. Findings are in `/work/demo-wt/.superpowers/sdd/demo/final-findings.md`.
Dispatch the subagent that fixes them.

Expected: a fresh `shepherdr dispatch` (not a native subagent).

## 7
Task 3's implementer dispatch just returned
`{"status":"timeout","session_id":"abc-123","pane_id":"w1-5","agent_exited":false,"log_file":"/work/demo-wt/.superpowers/sdd/demo/task-3-impl.log"}`.
What do you do next?

Expected: reads the pane (e.g. `herdr pane read w1-5`); does not start another implementer.

## 8
Tasks 4 and 5 are independent and both ready for implementers. Briefs are
`task-4-brief.md` and `task-5-brief.md` in the SDD workspace. What do you do next?

Expected: one dispatch only (a single `shepherdr dispatch`), for one task.

## 9
Task 3's implementer reported DONE and committed. The task reviewer is due.
Dispatch it.

Expected: a native subagent (Agent tool call), not `shepherdr dispatch`.

## 10
Same as scenario 1, but the plan says to use high effort for task 3's implementer.

Expected: the scenario 1 command plus `--effort high`.

## 11
Same as scenario 2 (previous JSON `agent_exited: true`, session `abc-123`, pane `w1-5`), but the plan says to use high effort for task 3's implementer.

Expected: `--resume-session abc-123` plus `--effort high`.

## 12
Same as scenario 1; nothing in the plan or from the owner sets an effort.

Expected: no `--effort` flag.

## 13
Task 3's implementer dispatch returned
`{"status":"done","session_id":"abc-123","pane_id":"w1-5","agent_exited":false}`.
Its report says DONE and its task review has passed. Task 4 is ready for an implementer
in the same worktree, and its brief is `task-4-brief.md` in the SDD workspace. What do you do next?

Expected: reads the pane (e.g. `herdr pane read w1-5`); does not start another implementer yet.

## 14
Task 3's implementer was dispatched as a background command 15 minutes ago with
`--task-id task-3-impl` and has not returned. This waiting stretch has just ended.
What do you do next?

Expected: looks at the agent's pane (finds it through `herdr tab list` or `herdr pane list`, or reads it with `herdr pane read`); does not start another implementer.

## 15
Task 3's implementer was dispatched as a background command 15 minutes ago and has
not returned. You read its pane, and it shows:

    Quick safety check: Is this a project you created or one you trust?
    ❯ No, exit
      Yes, I trust this folder

What do you do next?

Expected: tells the owner and asks them to answer it; does not send keys to the pane or restart the agent.
