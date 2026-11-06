#!/usr/bin/env python3
"""The pane command must leave its .exited marker however the agent ends.

Usage: cargo build && python3 -I tests/exit-marker-pty.py

Runs the exact command shepherdr sends to a pane, inside real interactive zsh and
bash shells on a pty. No real agent runs: `herdr` and `claude` are stubs, and the
stub agent only sleeps. The scratch directory name holds a space and an apostrophe,
so every case also checks the command's quoting.
"""
import os
import pty
import re
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "target", "debug", "shepherdr")
SHELLS = (
    ("zsh", ["/bin/zsh", "-f"]),
    ("bash", ["/bin/bash", "--norc", "--noprofile"]),
)
WAIT_S = 6.0

HERDR_STUB = """#!/bin/sh
case "$1 $2" in
  "pane split") echo '{"result":{"pane":{"pane_id":"stub:p9"}}}' ;;
  "pane list") echo '{"result":{"panes":[{"pane_id":"stub:p9"}]}}' ;;
  "pane run") printf '%s' "$4" > "$STUB_COMMAND_FILE" ;;
esac
"""

# $$ survives the exec, so agent.pid names the sleeping process.
AGENT_STUB = """#!/bin/sh
stub_dir="$(dirname "$0")"
if [ -t 0 ]; then printf '%s' "$*" > "$stub_dir/agent.stdin"; else cat > "$stub_dir/agent.stdin"; fi
echo '{"type":"system","session_id":"stub-session"}'
printf '%s' "$CLAUDE_CONFIG_DIR" > "$stub_dir/agent.config"
echo $$ > "$stub_dir/agent.pid"
case "$STUB_AGENT" in
  exit0) exit 0 ;;
  exit7) exit 7 ;;
  *) exec sleep 60 ;;
esac
"""


def write_stub(path, text):
    with open(path, "w") as f:
        f.write(text)
    os.chmod(path, 0o755)


class Fixture:
    def __init__(self, base):
        self.base = base
        self.stubs = os.path.join(base, "stubs")
        self.scratch = os.path.join(base, "scratch dir it's")
        self.worktree = os.path.join(base, "work tree it's")
        os.makedirs(self.stubs)
        os.makedirs(self.scratch)
        os.makedirs(self.worktree)
        write_stub(os.path.join(self.stubs, "herdr"), HERDR_STUB)
        write_stub(os.path.join(self.stubs, "claude"), AGENT_STUB)
        self.pid_file = os.path.join(self.stubs, "agent.pid")
        self.stdin_file = os.path.join(self.stubs, "agent.stdin")
        self.config_file = os.path.join(self.stubs, "agent.config")

    def pane_command(self, task_id, reuse, headless=True, config_dir=None):
        """Ask the real binary for the command it would send to the pane."""
        command_file = os.path.join(self.base, "command")
        context = os.path.join(self.scratch, f"{task_id}.context.md")
        with open(context, "w") as f:
            f.write("stub task\n")
        args = [
            BIN, "dispatch", "--role", "implementer", "--vendor", "claude",
            "--model", "stub", "--timeout-ms", "1", "--from-pane", "stub:p1",
            "--task-id", task_id, "--context-file", context,
            "--scratch", self.scratch, "--cwd", self.worktree, "--placement", "split",
        ]
        if headless:
            args += ["--headless"]
        if reuse:
            args += ["--reuse-pane", "stub:p9"]
        env = dict(os.environ, HERDR_BIN_PATH=os.path.join(self.stubs, "herdr"),
                   STUB_COMMAND_FILE=command_file,
                   # Without this, these fake dispatches land on the real board.
                   HERDR_PLUGIN_STATE_DIR=os.path.join(self.base, "state"))
        env.pop("CLAUDE_CONFIG_DIR", None)
        if config_dir:
            env["CLAUDE_CONFIG_DIR"] = config_dir
        subprocess.run(args, env=env, capture_output=True, cwd=self.base)
        with open(command_file) as f:
            return f.read()


class Pane:
    """An interactive shell on a pty, like the one in a herdr pane."""

    def __init__(self, argv, fixture, agent_mode):
        env = dict(os.environ, STUB_AGENT=agent_mode, PS1="prompt> ", TERM="dumb")
        # A new herdr pane doesn't inherit the caller's value, so neither does this one.
        env.pop("CLAUDE_CONFIG_DIR", None)
        env.pop("ENV", None)
        env.pop("BASH_ENV", None)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.execve(argv[0], argv, env)
        self.output = b""
        self.closed = False
        # A system rc file may reset PATH, so set it from inside the shell and
        # refuse to go on unless `claude` resolves to the stub.
        self.send(f'export PATH="{fixture.stubs}:$PATH"; hash -r; '
                  'echo "WHICH:$(command -v claude):END"\n')
        found = self.wait_output(re.compile(rb"WHICH:(/[^:\r\n]*):END"))
        expected = os.path.join(fixture.stubs, "claude").encode()
        if not found or found.group(1) != expected:
            raise SystemExit(f"refusing to run: claude is not the stub: {found}")

    def send(self, text):
        os.write(self.fd, text.encode() if isinstance(text, str) else text)

    def pump(self, seconds):
        ready, _, _ = select.select([self.fd], [], [], seconds)
        if ready:
            try:
                self.output += os.read(self.fd, 65536)
            except OSError:
                time.sleep(seconds)

    def wait_output(self, pattern):
        deadline = time.time() + WAIT_S
        while time.time() < deadline:
            found = pattern.search(self.output)
            if found:
                return found
            self.pump(0.05)
        return None

    def wait_path(self, path, seconds=WAIT_S):
        deadline = time.time() + seconds
        while time.time() < deadline:
            if os.path.exists(path):
                return True
            if self.closed:
                time.sleep(0.05)
            else:
                self.pump(0.05)
        return False

    def close_master(self):
        os.close(self.fd)
        self.closed = True

    def destroy(self):
        if not self.closed:
            self.close_master()
        try:
            os.kill(self.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        os.waitpid(self.pid, 0)


def agent_pid(fixture, pane):
    if not pane.wait_path(fixture.pid_file):
        raise SystemExit(f"stub agent never started; pane said: {pane.output!r}")
    with open(fixture.pid_file) as f:
        return int(f.read().strip())


def parent_of(pid):
    out = subprocess.run(["ps", "-o", "ppid=", "-p", str(pid)], capture_output=True, text=True)
    return int(out.stdout.strip())


def signal_group(sig):
    def act(fixture, pane):
        os.killpg(os.getpgid(agent_pid(fixture, pane)), sig)
    return act


def ctrl_c(fixture, pane):
    agent_pid(fixture, pane)
    pane.send(b"\x03")


def term_agent_only(fixture, pane):
    os.kill(agent_pid(fixture, pane), signal.SIGTERM)


def close_pane(fixture, pane):
    agent_pid(fixture, pane)
    pane.close_master()


def nothing(fixture, pane):
    pass


# name, stub agent behaviour, action once the command runs
CASES = [
    ("normal exit", "exit0", nothing),
    ("nonzero exit", "exit7", nothing),
    ("Ctrl-C at the pty", "sleep", ctrl_c),
    ("SIGINT to the group", "sleep", signal_group(signal.SIGINT)),
    ("SIGTERM to the group", "sleep", signal_group(signal.SIGTERM)),
    ("SIGTERM to the agent", "sleep", term_agent_only),
    ("SIGHUP to the group", "sleep", signal_group(signal.SIGHUP)),
    ("pty closed", "sleep", close_pane),
]


def run_case(fixture, shell, argv, name, mode, action, number, headless):
    task_id = f"{shell}-{number}-{'headless' if headless else 'real'}"
    command = fixture.pane_command(task_id, reuse=False, headless=headless)
    marker = os.path.join(fixture.scratch, f"{task_id}.exited")
    log = os.path.join(fixture.scratch, f"{task_id}.log")
    for path in (fixture.pid_file, fixture.stdin_file):
        if os.path.exists(path):
            os.remove(path)
    pane = Pane(argv, fixture, mode)
    try:
        pane.send(command + "\n")
        if mode == "sleep":
            agent_pid(fixture, pane)
        action(fixture, pane)
        problems = []
        if not pane.wait_path(marker):
            problems.append("no marker")
        pane.wait_path(fixture.stdin_file, 1.0)
        with open(fixture.stdin_file) as f:
            # The real interface gets a pointer to the prompt file, on a terminal.
            expected = f"DONE::{task_id}" if headless else f"{task_id}.prompt.md"
            if expected not in f.read():
                problems.append("prompt did not reach the agent")
        if headless:
            if os.path.exists(log):
                with open(log) as f:
                    if "stub-session" not in f.read():
                        problems.append("session line missing from the log")
            else:
                problems.append("no log")
        if action in (ctrl_c, nothing):
            pane.output = b""
            pane.send('echo "ALIVE:$((40+2))"\n')
            if not pane.wait_output(re.compile(rb"ALIVE:42")):
                problems.append("shell prompt did not return")
        return problems
    finally:
        pane.destroy()


def run_failed_cd(fixture, shell, argv, headless):
    task_id = f"{shell}-cd-{'headless' if headless else 'real'}"
    command = fixture.pane_command(task_id, reuse=True, headless=headless)
    if "cd " not in command:
        return ["reuse-pane command has no cd"]
    marker = os.path.join(fixture.scratch, f"{task_id}.exited")
    moved = fixture.worktree + ".moved"
    os.rename(fixture.worktree, moved)
    pane = Pane(argv, fixture, "exit0")
    try:
        if os.path.exists(fixture.stdin_file):
            os.remove(fixture.stdin_file)
        pane.send(command + "\n")
        problems = []
        if not pane.wait_path(marker):
            problems.append("no marker")
        if os.path.exists(fixture.stdin_file):
            problems.append("agent ran despite the failed cd")
        return problems
    finally:
        pane.destroy()
        os.rename(moved, fixture.worktree)


def run_term_to_wrapper_only(fixture, shell, argv, headless):
    """A signal to the wrapper alone leaves the agent running, so no marker yet."""
    task_id = f"{shell}-wrapper-{'headless' if headless else 'real'}"
    command = fixture.pane_command(task_id, reuse=False, headless=headless)
    marker = os.path.join(fixture.scratch, f"{task_id}.exited")
    if os.path.exists(fixture.pid_file):
        os.remove(fixture.pid_file)
    pane = Pane(argv, fixture, "sleep")
    try:
        pane.send(command + "\n")
        agent = agent_pid(fixture, pane)
        os.kill(parent_of(agent), signal.SIGTERM)
        problems = []
        if pane.wait_path(marker, 1.5):
            problems.append("marker appeared while the agent was still running")
        os.kill(agent, signal.SIGTERM)
        if not pane.wait_path(marker):
            problems.append("no marker after the agent exited")
        return problems
    finally:
        pane.destroy()


def run_config_dir(fixture, shell, argv, headless):
    """The caller's CLAUDE_CONFIG_DIR reaches an agent in a pane that doesn't have it."""
    task_id = f"{shell}-config-{'headless' if headless else 'real'}"
    wanted = os.path.join(fixture.base, "my config it's")
    command = fixture.pane_command(task_id, reuse=False, headless=headless, config_dir=wanted)
    marker = os.path.join(fixture.scratch, f"{task_id}.exited")
    if os.path.exists(fixture.config_file):
        os.remove(fixture.config_file)
    pane = Pane(argv, fixture, "exit0")
    try:
        pane.send(command + "\n")
        if not pane.wait_path(marker):
            return ["no marker"]
        if not os.path.exists(fixture.config_file):
            return ["agent never ran"]
        with open(fixture.config_file) as f:
            seen = f.read()
        return [] if seen == wanted else [f"agent saw {seen!r}"]
    finally:
        pane.destroy()


def main():
    if not os.access(BIN, os.X_OK):
        print("BLOCKED: build first (cargo build)")
        return 2
    base = tempfile.mkdtemp(prefix="shepherdr-pty-")
    failures = 0

    def report(shell, name, problems):
        nonlocal failures
        if problems:
            failures += 1
            print(f"FAIL {shell:4} {name}: {', '.join(problems)}")
        else:
            print(f"PASS {shell:4} {name}")

    try:
        fixture = Fixture(base)
        print("pane command:", fixture.pane_command("sample", reuse=True))
        for headless, kind in ((True, "headless"), (False, "real interface")):
            for shell, argv in SHELLS:
                for number, (name, mode, action) in enumerate(CASES):
                    report(shell, f"{kind}: {name}",
                           run_case(fixture, shell, argv, name, mode, action, number, headless))
                report(shell, f"{kind}: caller's CLAUDE_CONFIG_DIR reaches the agent",
                       run_config_dir(fixture, shell, argv, headless))
                report(shell, f"{kind}: reuse-pane cd fails", run_failed_cd(fixture, shell, argv, headless))
                report(shell, f"{kind}: SIGTERM to the wrapper alone waits for the agent",
                       run_term_to_wrapper_only(fixture, shell, argv, headless))
    finally:
        shutil.rmtree(base, ignore_errors=True)
    print(f"failures: {failures}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
