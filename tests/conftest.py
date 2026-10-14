"""Test helpers: fake machines that share one store, plus Claude Code transcripts and git repos."""

from __future__ import annotations

import contextlib
import importlib.machinery
import importlib.util
import io
import json
import os
import re
import shutil
import subprocess
import sys
import uuid
from pathlib import Path

import pytest

sys.dont_write_bytecode = True  # keep the repo free of __pycache__
ACTX = Path(os.environ.get("ACTX_UNDER_TEST") or Path(__file__).resolve().parent.parent / "actx")
FIXTURES = Path(__file__).resolve().parent / "fixtures"
GIT_ENV = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}



def _opencode_major() -> int | None:
    """The installed opencode's major version (1 or 2), or None."""
    if not shutil.which("opencode"):
        return None
    try:
        res = subprocess.run(["opencode", "session", "--help"], capture_output=True, text=True, timeout=120)
    except (OSError, subprocess.SubprocessError):
        return None
    return 2 if "import" in res.stdout + res.stderr else 1


OPENCODE_MAJOR = _opencode_major()
needs_opencode = pytest.mark.skipif(OPENCODE_MAJOR is None, reason="opencode is not installed")
needs_codex = pytest.mark.skipif(not shutil.which("codex"), reason="codex is not installed")


def load_actx(env: dict):
    """A fresh copy of the actx module, loaded with `env` (its paths are read at import time)."""
    saved = os.environ.copy()
    os.environ.update(env)
    try:
        name = f"actx_{uuid.uuid4().hex[:8]}"
        loader = importlib.machinery.SourceFileLoader(name, str(ACTX))
        spec = importlib.util.spec_from_loader(name, loader)
        mod = importlib.util.module_from_spec(spec)
        sys.modules[name] = mod
        loader.exec_module(mod)
        return mod
    finally:
        os.environ.clear()
        os.environ.update(saved)


class Machine:
    """A fake computer: its own HOME, Claude Code and opencode data, actx state and config."""

    def __init__(self, base: Path, name: str, store: Path, lang: str = "en"):
        self.name = name
        self.home = base / name
        self.home.mkdir(parents=True)
        self.env = GIT_ENV | {
            "HOME": str(self.home),
            "XDG_CONFIG_HOME": str(self.home / ".config"),
            "XDG_STATE_HOME": str(self.home / ".local/state"),
            "XDG_DATA_HOME": str(self.home / ".local/share"),
            "XDG_CACHE_HOME": str(self.home / ".cache"),
            "CLAUDE_CONFIG_DIR": str(self.home / ".claude"),
            "ACTX_ROOT": str(store),
            "ACTX_SCAN": str(self.home),
            "ACTX_LANG": lang,
            # opencode 2.x: never talk to a real opencode service (1.x has no service)
            "ACTX_OPENCODE_ARGS": "--standalone" if OPENCODE_MAJOR == 2 else "",
            "CODEX_HOME": str(self.home / ".codex"),
            "NO_COLOR": "1",
        }
        self.mod = load_actx(self.env)
        self.mod.HOST = name

    @property
    def claude_dir(self) -> Path:
        return self.home / ".claude" / "projects"

    @contextlib.contextmanager
    def active(self, cwd: Path | None = None):
        saved_env, saved_cwd = os.environ.copy(), os.getcwd()
        os.environ.update(self.env)
        os.chdir(cwd or self.home)
        try:
            yield
        finally:
            os.chdir(saved_cwd)
            os.environ.clear()
            os.environ.update(saved_env)

    def actx(self, *argv: str, cwd: Path | None = None, pick: list[str] | None = None, answer: str | None = None) -> str:
        """Run an actx command on this machine. `pick`: keys the pickers choose; `answer`: reply to questions."""
        mod = self.mod
        if pick is not None:
            mod.fzf = lambda rows, header, multi=False, preview=True: [k for k, *_ in rows if any(p in k for p in pick)]
        if answer is not None:
            mod.ask = lambda question, lower=True: answer
            mod.sys = _TtyStdin(sys)  # questions are only asked on a terminal
        out = io.StringIO()
        with self.active(cwd), contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            sys.argv = ["actx", *argv]
            mod.git_id.cache_clear()
            mod.local_repos.cache_clear()
            mod.oc_db.cache_clear()
            mod.oc_version.cache_clear()
            try:
                mod.main()
            except SystemExit:
                pass
        return out.getvalue()

    def call(self, fn, *args, cwd: Path | None = None):
        """Call one of actx's functions on this machine (e.g. to import a test conversation)."""
        with self.active(cwd):
            self.mod.oc_db.cache_clear()
            self.mod.oc_version.cache_clear()
            return fn(*args)

    def opencode_db(self) -> Path:
        return self.home / ".local/share/opencode/opencode.db"


class _TtyStdin:
    """Proxy for the sys module whose stdin claims to be a terminal."""

    def __init__(self, real):
        self._real = real

    def __getattr__(self, name):
        if name == "stdin":
            return _FakeTty()
        return getattr(self._real, name)


class _FakeTty:
    def isatty(self) -> bool:
        return True


@pytest.fixture
def machines(tmp_path):
    store = tmp_path / "store"
    store.mkdir()

    def make(name: str, lang: str = "en") -> Machine:
        return Machine(tmp_path, name, store, lang)

    make.store = store
    return make


@pytest.fixture
def actx_mod(tmp_path):
    """The actx module with an empty, temporary environment."""
    return Machine(tmp_path, "solo", tmp_path / "store").mod


def git_repo(path: Path) -> Path:
    path.mkdir(parents=True)
    subprocess.run(["git", "init", "-q", "-b", "main", str(path)], check=True)
    subprocess.run(["git", "-C", str(path), "commit", "-q", "--allow-empty", "-m", "init"], check=True,
                   env=os.environ | GIT_ENV)
    return path


def git_clone(src: Path, dest: Path) -> Path:
    dest.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "clone", "-q", str(src), str(dest)], check=True, capture_output=True)
    return dest


def _line(obj: dict) -> str:
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":")) + "\n"


def claude_session(m: Machine, folder: Path, turns: list[tuple[str, str]], title: str | None = None,
                   sid: str | None = None) -> Path:
    """Write a Claude Code transcript the way Claude Code does (compact JSON, a uuid chain)."""
    sid = sid or str(uuid.uuid4())
    d = m.claude_dir / re.sub(r"[^a-zA-Z0-9]", "-", str(folder))
    d.mkdir(parents=True, exist_ok=True)
    path = d / f"{sid}.jsonl"
    path.write_text("")
    claude_continue(path, turns, folder=folder, sid=sid)
    if title:
        with open(path, "a") as fo:
            fo.write(_line({"type": "ai-title", "aiTitle": title, "sessionId": sid}))
    return path


def claude_continue(path: Path, turns: list[tuple[str, str]], folder: Path | None = None, sid: str | None = None) -> None:
    """Append turns to a transcript, chained to its last message (as if you kept talking)."""
    lines = [json.loads(x) for x in path.read_text().splitlines() if x.strip()]
    chained = [e for e in lines if e.get("uuid")]
    parent = chained[-1]["uuid"] if chained else None
    cwd = str(folder) if folder else chained[-1]["cwd"]
    sid = sid or path.stem
    with open(path, "a") as fo:
        for n, (who, text) in enumerate(turns):
            u = str(uuid.uuid4())
            if who == "user":
                message = {"role": "user", "content": text}
            else:
                message = {"role": "assistant", "type": "message", "id": f"msg_{u[:8]}", "model": "test",
                           "content": [{"type": "text", "text": text}]}
            fo.write(_line({"parentUuid": parent, "isSidechain": False, "type": who, "message": message, "uuid": u,
                            "timestamp": f"2026-01-01T10:{len(chained) + n:02d}:00.000Z", "cwd": cwd,
                            "sessionId": sid}))
            parent = u


def opencode_fixture(folder: Path, major: int | None = None) -> dict:
    """The test opencode conversation, in the format of opencode `major` (default: the installed one)."""
    major = major or OPENCODE_MAJOR or 2
    exp = json.loads((FIXTURES / ("opencode1_export.json" if major == 1 else "opencode_export.json")).read_text())
    if major == 1:
        exp["info"]["directory"] = str(folder)
    else:
        exp["info"]["location"]["directory"] = str(folder)
    return exp


def codex_rollout(m: Machine, folder: Path, turns: list[tuple[str, str]], title: str | None = None) -> Path:
    """Write a Codex rollout the way Codex does: session_meta, then per turn what the model saw
    (response_item) and what Codex shows (event_msg)."""
    sid = str(uuid.UUID(int=(1790000000000 << 80) | (0x7 << 76) | (0b10 << 62) | uuid.uuid4().int % (1 << 62)))
    d = m.home / ".codex/sessions/2026/01/01"
    d.mkdir(parents=True, exist_ok=True)
    path = d / f"rollout-2026-01-01T10-00-00-{sid}.jsonl"
    line = lambda kind, payload: _line({"timestamp": "2026-01-01T10:00:00.000Z", "type": kind, "payload": payload})
    out = [line("session_meta", {"id": sid, "session_id": sid, "timestamp": "2026-01-01T10:00:00.000Z",
                                 "cwd": str(folder), "originator": "codex-tui", "cli_version": "0.160.0",
                                 "source": "cli", "model_provider": "openai"}),
           line("response_item", {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "<permissions>...</permissions>"}]})]
    path.write_text("".join(out))
    codex_continue(path, turns)
    if title:
        with open(m.home / ".codex/session_index.jsonl", "a") as fo:
            fo.write(_line({"id": sid, "thread_name": title, "updated_at": "2026-01-01T10:00:00Z"}))
    return path


def codex_continue(path: Path, turns: list[tuple[str, str]], tool: tuple[str, str] | None = None) -> None:
    """Append turns to a rollout (as if you kept talking in Codex). `tool`: a command and its output."""
    sid = path.name[-42:-6]
    line = lambda kind, payload: _line({"timestamp": "2026-01-01T10:05:00.000Z", "type": kind, "payload": payload})
    with open(path, "a") as fo:
        for who, text in turns:
            turn = str(uuid.uuid4())
            if who == "user":
                fo.write(line("event_msg", {"type": "task_started", "turn_id": turn, "started_at": 1790000000}))
                fo.write(line("response_item", {"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]}))
                fo.write(line("event_msg", {"type": "item_completed", "thread_id": sid, "turn_id": turn, "item": {
                    "type": "UserMessage", "id": turn, "content": [{"type": "text", "text": text, "text_elements": []}]},
                    "started_at_ms": 1790000000000, "completed_at_ms": 1790000000000}))
            else:
                if tool:
                    fo.write(line("response_item", {"type": "function_call", "name": "shell", "call_id": "c1",
                                                    "arguments": json.dumps({"command": ["bash", "-lc", tool[0]]})}))
                    fo.write(line("response_item", {"type": "function_call_output", "call_id": "c1", "output": tool[1]}))
                fo.write(line("response_item", {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}))
                fo.write(line("event_msg", {"type": "item_completed", "thread_id": sid, "turn_id": turn, "item": {
                    "type": "AgentMessage", "id": turn, "content": [{"type": "Text", "text": text}], "phase": "final_answer"},
                    "started_at_ms": 1790000001000, "completed_at_ms": 1790000001000}))
                fo.write(line("event_msg", {"type": "task_complete", "turn_id": turn, "last_agent_message": text,
                                            "started_at": 1790000000, "completed_at": 1790000001, "duration_ms": 1000}))


def codex_threads(m: Machine) -> dict:
    """What Codex itself sees on this machine, through its app-server: {id: (preview, turn count)}."""
    import select
    import time

    env = os.environ | m.env
    p = subprocess.Popen(["codex", "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.DEVNULL, env=env, text=True)

    def call(n: int, method: str, params: dict) -> dict:
        p.stdin.write(json.dumps({"id": n, "method": method, "params": params}) + "\n")
        p.stdin.flush()
        end = time.time() + 60
        while time.time() < end:
            if select.select([p.stdout], [], [], 1)[0]:
                msg = json.loads(p.stdout.readline() or "{}")
                if msg.get("id") == n:
                    return msg.get("result") or {}
        return {}

    try:
        call(0, "initialize", {"clientInfo": {"name": "actx-tests", "version": "0"}})
        p.stdin.write(json.dumps({"method": "initialized"}) + "\n")
        listed = call(1, "thread/list", {}).get("data", [])
        out = {}
        for n, t in enumerate(listed, 2):
            thread = call(n, "thread/read", {"threadId": t["id"], "includeTurns": True}).get("thread") or {}
            out[t["id"]] = (t.get("preview"), len(thread.get("turns") or []), thread.get("cwd"))
        return out
    finally:
        p.kill()
