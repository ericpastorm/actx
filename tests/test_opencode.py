"""opencode, against the real opencode CLI installed here (1.x or 2.x; skipped when there's none).

These double as an early warning: if opencode changes its export/import format, they break here
before they break for users."""

import json
import shutil
import sqlite3
import time

from conftest import OPENCODE_MAJOR, claude_session, git_clone, git_repo, needs_opencode, opencode_fixture

pytestmark = needs_opencode
SID = "ses_e5a1b2c3d4e5Fx7Qk2LmNp9RsT"


def import_fixture(m, folder, major=None):
    data = json.dumps(opencode_fixture(folder, major)).encode()
    m.call(m.mod.oc_import, data, str(folder), cwd=folder)


def sessions(m):
    """{id: (title, directory, number of messages)} straight from opencode's database."""
    con = sqlite3.connect(f"file:{m.opencode_db()}?mode=ro", uri=True)
    if OPENCODE_MAJOR == 2:
        sql = ("select s.id, s.title, s.directory, (select count(*) from session_message m where m.session_id = s.id) "
               "from session_v2 s")
    else:
        sql = "select s.id, s.title, s.directory, (select count(*) from message m where m.session_id = s.id) from session s"
    return {sid: (title, directory, n) for sid, title, directory, n in con.execute(sql)}


def continue_opencode(m, folder, text, reply):
    """Add a question and an answer, as if you kept talking in opencode."""
    with m.active(folder):
        m.mod.oc_db.cache_clear()
        m.mod.oc_version.cache_clear()
        s = next(s for s in m.mod.opencode_local() if s.id == SID)
        m.mod._oc_append(s, text, reply)


def test_opencode_sync_and_convert(machines):
    a, b = machines("desk"), machines("laptop")
    repo = git_repo(a.home / "code" / "web")
    clone = git_clone(repo, b.home / "src" / "web-laptop")
    import_fixture(a, repo)
    assert "Fix the login form" in a.actx("mk", "web", pick=[SID])

    b.actx("sync")
    assert sessions(b)[SID][:2] == ("Fix the login form", str(clone))
    assert sessions(b)[SID][2] == 3

    claude = claude_session(b, clone, [("user", "hi"), ("assistant", "hello")])  # so "convert to" has a choice
    out = b.actx("cv", pick=[SID, "claude"])  # opencode → Claude Code
    assert "Fix the login form (from opencode)" in out and "claude --resume" in out
    copy = next(p for p in b.claude_dir.glob("*/*.jsonl") if p != claude)
    lines = [json.loads(x) for x in copy.read_text().splitlines()]
    assert lines[0]["cwd"] == str(clone)
    assert "Conversation imported from opencode (test-model)" in lines[0]["message"]["content"]
    assert "$ grep -n submit src/login.js" in lines[1]["message"]["content"][0]["text"]

    out = b.actx("cv", pick=[copy.stem, "opencode"])  # and back: Claude Code → opencode
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


def test_a_conversation_from_the_other_opencode_version_is_converted(machines):
    """A machine with opencode 1.x receives one saved by 2.x (or the other way round)."""
    other = 1 if OPENCODE_MAJOR == 2 else 2
    a, b = machines("desk"), machines("laptop")
    repo = git_repo(a.home / "code" / "web")
    clone = git_clone(repo, b.home / "src" / "web")
    import_fixture(a, repo)
    a.actx("mk", "web", pick=[SID])
    # make the stored copy look like it came from the other version
    stored = machines.store / "web" / "opencode" / SID
    meta = json.loads((stored / "meta.json").read_text())
    meta["files"]["format"] = f"opencode-v{other}"
    (stored / "meta.json").write_text(json.dumps(meta))
    data = json.dumps(opencode_fixture(repo, other)).encode()
    with a.mod.copen(stored / meta["files"]["session"], "wb") as fo:
        fo.write(data)

    out = b.actx("sync")
    assert "converted" in out
    title, directory, n = sessions(b)[SID]
    assert (title, directory) == ("Fix the login form", str(clone)) and n == 2  # question + flattened answer
