"""opencode, against the real opencode CLI (skipped when it isn't installed).

These double as an early warning: if opencode changes its export/import format, they break here
before they break for users."""

import json
import sqlite3
import time

from conftest import claude_session, git_clone, git_repo, needs_opencode, opencode_fixture

pytestmark = needs_opencode
SID = "ses_e5a1b2c3d4e5Fx7Qk2LmNp9RsT"


def import_fixture(m, folder):
    path = m.home / "fixture.json"
    path.write_text(json.dumps(opencode_fixture(folder)))
    res = m.opencode("import", "--directory", str(folder), str(path))
    assert b"Imported session" in res.stdout + res.stderr, res


def sessions(m):
    con = sqlite3.connect(f"file:{m.opencode_db()}?mode=ro", uri=True)
    return {sid: (title, directory, n) for sid, title, directory, n in con.execute(
        "select s.id, s.title, s.directory, (select count(*) from session_message m where m.session_id = s.id) "
        "from session_v2 s")}


def continue_opencode(m, folder, text, reply):
    """Add a question and an answer to the fixture session, as if you kept talking in opencode."""
    out = m.home / "export.json"
    with m.active(), open(out, "wb") as fo:
        import subprocess
        subprocess.run(["opencode", "session", "export", "--standalone", SID], stdout=fo, check=True, cwd=m.home)
    exp = json.loads(out.read_text())
    now = int(time.time() * 1000)
    model = exp["info"]["model"]
    exp["messages"] += [
        {"id": m.mod._oc_id("msg", now, 1), "time": {"created": now}, "text": text, "files": [], "agents": [],
         "type": "user"},
        {"id": m.mod._oc_id("msg", now, 2), "time": {"created": now, "streamed": now, "completed": now},
         "type": "assistant", "agent": "build", "model": model, "content": [{"type": "text", "text": reply}],
         "finish": "stop", "rawFinish": "stop", "cost": 0,
         "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}}},
    ]
    out.write_text(json.dumps(exp))
    m.opencode("delete", SID)
    res = m.opencode("import", "--directory", str(folder), str(out))
    assert b"Imported session" in res.stdout + res.stderr, res


def test_opencode_sync_and_convert(machines):
    a, b = machines("desk"), machines("laptop")
    repo = git_repo(a.home / "code" / "web")
    clone = git_clone(repo, b.home / "src" / "web-laptop")
    import_fixture(a, repo)
    assert "Fix the login form" in a.actx("mk", "web", pick=[SID])

    b.actx("sync")
    assert sessions(b)[SID][:2] == ("Fix the login form", str(clone))

    out = b.actx("cv", pick=[SID])  # opencode → Claude Code
    assert "Fix the login form (from opencode)" in out and "claude --resume" in out
    (copy,) = b.claude_dir.glob("*/*.jsonl")
    lines = [json.loads(x) for x in copy.read_text().splitlines()]
    assert lines[0]["cwd"] == str(clone)
    assert "Conversation imported from opencode (test-model)" in lines[0]["message"]["content"]
    assert "‹shell› $ grep -n submit src/login.js" in lines[1]["message"]["content"][0]["text"]

    out = b.actx("cv", pick=[copy.stem])  # and back: Claude Code → opencode
    assert "opencode " in out and " -s ses_" in out
    titles = {t for t, _d, _n in sessions(b).values()}
    assert "Fix the login form (from Claude Code)" in titles
    assert "web" in b.actx("ls") and "pending" not in b.actx("ls")  # the copies travel with the project


def test_opencode_conflict_merge(machines):
    a, b = machines("desk"), machines("laptop")
    repo = git_repo(a.home / "code" / "web")
    clone = git_clone(repo, b.home / "src" / "web")
    import_fixture(a, repo)
    a.actx("mk", "web", pick=[SID])
    b.actx("sync")
    continue_opencode(a, repo, "and on Windows?", "Use the Task Manager (desk)")
    continue_opencode(b, clone, "how do I prevent it?", "Disable the daemon (laptop)")
    b.actx("sync")
    assert "⚠ conflict" in a.actx("ls")

    out = a.actx("sync", "--all", answer="m")
    assert "branch from laptop" in out
    found = sessions(a)
    assert found[SID][2] == 3 + 2 + 2  # fixture + desk's turn + the merge note and its reply
    assert any(t == "Fix the login form (branch from laptop)" for t, _d, _n in found.values())
    b.actx("sync")
    assert "pending" not in a.actx("ls") and "pending" not in b.actx("ls")
