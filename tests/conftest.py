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

needs_opencode = pytest.mark.skipif(not shutil.which("opencode"), reason="opencode is not installed")


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
            "ACTX_OPENCODE_ARGS": "--standalone",  # never talk to a real opencode service
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
            try:
                mod.main()
            except SystemExit:
                pass
        return out.getvalue()

    def opencode(self, *args: str) -> subprocess.CompletedProcess:
        with self.active():
            return subprocess.run(["opencode", "session", args[0], "--standalone", *args[1:]],
                                  capture_output=True, cwd=self.home)

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


def opencode_fixture(folder: Path) -> dict:
    exp = json.loads((FIXTURES / "opencode_export.json").read_text())
    exp["info"]["location"]["directory"] = str(folder)
    return exp
