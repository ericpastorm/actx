"""Pure functions: paths, status, parsing Claude Code / opencode conversations, translations."""

import ast
import json
import re
import uuid

from conftest import ACTX, claude_session, opencode_fixture


def test_every_interface_string_has_a_spanish_translation(actx_mod):
    tree = ast.parse(ACTX.read_text())
    calls = [n for n in ast.walk(tree) if isinstance(n, ast.Call) and getattr(n.func, "id", None) == "tr"]
    assert calls
    missing, mismatched = [], []
    for call in calls:
        arg = call.args[0]
        if isinstance(arg, ast.Name):  # tr(HELP)
            text = getattr(actx_mod, arg.id)
        else:
            assert isinstance(arg, ast.Constant), ast.dump(arg)
            text = arg.value
        if text not in actx_mod.ES:
            missing.append(text)
        elif set(re.findall(r"{(\w+)}", text)) != set(re.findall(r"{(\w+)}", actx_mod.ES[text])):
            mismatched.append(text)
    assert not missing, f"untranslated: {missing}"
    assert not mismatched, f"placeholders differ: {mismatched}"


def test_encode_and_resolve_claude_folder_names(actx_mod, tmp_path):
    folder = tmp_path / "Work" / "my-app_v2.0" / "src"
    folder.mkdir(parents=True)
    name = actx_mod.encode_claude(str(folder))
    assert "/" not in name and "_" not in name and "." not in name
    assert actx_mod.resolve_encoded(name) == str(folder)


def test_path_fixer_respects_name_boundaries(actx_mod):
    fix = actx_mod.path_fixer([("/home/a/shop", "/home/b/work/shop2")])
    data = b'"cwd":"/home/a/shop" edited /home/a/shop/cart.ts not /home/a/shop-old or /home/a/shopping'
    assert fix(data) == (b'"cwd":"/home/b/work/shop2" edited /home/b/work/shop2/cart.ts '
                         b"not /home/a/shop-old or /home/a/shopping")
    assert actx_mod.path_fixer([("/same", "/same")]) is None


def test_status_uses_the_last_sync_as_base(actx_mod):
    S, status = actx_mod.Session, actx_mod.status
    local, remote = S("claude", "x", "t", "/p", 0, 100), S("claude", "x", "t", "/p", 0, 100)
    assert status(None, remote, None) == "new"
    assert status(local, None, None) == "local"
    assert status(local, remote, [100, 100]) == "synced"
    assert status(S("claude", "x", "t", "/p", 0, 150), remote, [100, 100]) == "push"
    assert status(local, S("claude", "x", "t", "/p", 0, 150), [100, 100]) == "pull"
    assert status(S("claude", "x", "t", "/p", 0, 120), S("claude", "x", "t", "/p", 0, 150), [100, 100]) == "conflict"
    # sizes differ but nothing changed since the last sync (e.g. paths were rewritten)
    assert status(S("claude", "x", "t", "/p", 0, 90), remote, [90, 100]) == "synced"
    assert status(S("claude", "x", "t", "/p", 0, 150), remote, None) == "push"  # no base: longer wins


def _entry(type_, content, parent=None, **extra):
    u = str(uuid.uuid4())
    message = {"role": type_, "content": content}
    return u, {"type": type_, "uuid": u, "parentUuid": parent, "message": message,
               "timestamp": "2026-01-01T10:00:00.000Z", **extra}


def test_claude_turns_follows_the_active_branch(actx_mod):
    lines = []

    def add(type_, content, parent, **extra):
        u, e = _entry(type_, content, parent, **extra)
        lines.append(json.dumps(e))
        return u

    a = add("user", "old question before compacting", None)
    add("assistant", [{"type": "text", "text": "old answer"}], a)
    lines.append(json.dumps({"type": "system", "subtype": "compact_boundary", "uuid": "b", "parentUuid": None}))
    s = add("user", "Summary of the earlier conversation", "b")
    q = add("user", [{"type": "text", "text": "<system-reminder>ignore me</system-reminder>"},
                     {"type": "text", "text": "fix the cart"}], s)
    call = add("assistant", [{"type": "thinking", "thinking": "hmm"},
                             {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}], q)
    res = add("user", [{"type": "tool_result", "tool_use_id": "t1", "content": "x" * 2000}], call)
    add("assistant", [{"type": "text", "text": "rewound answer (dropped)"}], res)  # a branch left by /rewind
    add("assistant", [{"type": "text", "text": "subagent work"}], res, isSidechain=True)
    final = add("assistant", [{"type": "text", "text": "cart fixed"}], res)
    add("user", "<local-command-stdout>noise</local-command-stdout>", final, isMeta=True)

    turns = actx_mod.claude_turns(lines)
    texts = [(who, text) for who, text, _ts in turns]
    assert texts[0] == ("user", "Summary of the earlier conversation\n\nfix the cart")
    assert texts[1][0] == "assistant"
    assert texts[1][1].startswith("‹Bash› $ ls\n" + "x" * 1500)
    assert "500 more characters" in texts[1][1]
    assert texts[1][1].endswith("cart fixed")
    assert all("old" not in t and "rewound" not in t and "subagent" not in t and "noise" not in t for _w, t in texts)


def test_claude_turns_without_uuids_uses_file_order(actx_mod):
    lines = [json.dumps({"type": "user", "message": {"content": "hi"}}),
             json.dumps({"type": "assistant", "message": {"content": [{"type": "text", "text": "hello"}]}})]
    assert [(w, t) for w, t, _ in actx_mod.claude_turns(lines)] == [("user", "hi"), ("assistant", "hello")]


def test_oc_turns_starts_at_the_last_compaction(actx_mod, tmp_path):
    exp = opencode_fixture(tmp_path)
    turns = actx_mod.oc_turns(exp)
    assert [w for w, _t, _ts in turns] == ["user", "assistant"]
    assert "‹shell› $ grep -n submit src/login.js\n12:  form.onsubmit = null" in turns[1][1]
    assert "reset somewhere" not in turns[1][1]  # reasoning is dropped
    exp["messages"].insert(2, {"type": "compaction", "summary": "We found the bug.", "time": {"created": 1}})
    turns = actx_mod.oc_turns(exp)
    assert turns[0][1].endswith("We found the bug.")
    assert "doesn't submit" not in turns[0][1]


def test_fit_drops_the_beginning_and_keeps_one_note(actx_mod, monkeypatch):
    monkeypatch.setattr(actx_mod, "CONV_BUDGET", 50)
    turns = [("user", "a" * 40, 1), ("assistant", "b" * 30, 2), ("user", "c" * 20, 3)]
    fitted = actx_mod._fit(list(turns), "opencode", "some-model")
    assert fitted[0][0] == "user" and "Conversation imported from opencode (some-model)" in fitted[0][1]
    assert "The first 1 messages" in fitted[0][1]
    again = actx_mod._fit([("user", fitted[0][1], 1)], "claude", None)  # a copy of a copy
    assert again[0][1].count("Conversation imported") == 1 and "from Claude Code" in again[0][1]


def test_render_keeps_your_questions_visible(actx_mod):
    msgs = [("user", "q1"), ("assistant", "a1"), ("assistant", "a2"), ("user", "q2"), ("assistant", "a3")]
    text = actx_mod.render(msgs, "claude")
    assert "a1" not in text and "a2" in text and "q1" in text and "q2" in text and "a3" in text


def test_claude_scan_titles_paths_and_empty_sessions(tmp_path, machines):
    m = machines("pc")
    app = m.home / "code" / "my_app"
    app.mkdir(parents=True)
    claude_session(m, app, [("user", "first prompt\nmore"), ("assistant", "ok")])
    titled = claude_session(m, app, [("user", "hi"), ("assistant", "ok")], title="Named by Claude")
    with open(titled, "a") as fo:
        fo.write(json.dumps({"type": "custom-title", "customTitle": "Renamed by me"}, separators=(",", ":")) + "\n")
    claude_session(m, app, [("user", "only a question")])  # never answered: not a conversation
    with m.active():
        found = {s.title: s for s in m.mod.claude_local()}
    assert set(found) == {"first prompt", "Renamed by me"}
    assert {s.path for s in found.values()} == {str(app)}


def test_opencode_1_is_reported_not_crashed(machines):
    import sqlite3

    m = machines("pc")
    db = m.home / ".local/share/opencode/opencode.db"
    db.parent.mkdir(parents=True)
    sqlite3.connect(db).execute("create table session (id text primary key)").connection.commit()
    out = m.actx("mk", "x", pick=[])
    assert "Your opencode is 1.x" in out and "Can't read" not in out
