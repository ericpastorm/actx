"""Codex, checked against the real Codex (its app-server lists and reads what actx writes)."""

import json

from conftest import claude_session, codex_continue, codex_rollout, codex_threads, git_clone, git_repo, needs_codex

pytestmark = needs_codex


def setup(machines):
    a, b = machines("desk"), machines("laptop")
    repo = git_repo(a.home / "code" / "shop")
    clone = git_clone(repo, b.home / "work" / "shop-laptop")
    rollout = codex_rollout(a, repo, [("user", "fix the cart"), ("assistant", f"Done in {repo}/cart.ts")], title="Cart fix")
    sid = rollout.name[-42:-6]
    assert "Cart fix" in a.actx("mk", "shop", pick=[sid])
    return a, b, repo, clone, sid


def rollout_of(m, sid):
    (path,) = (m.home / ".codex/sessions").glob(f"*/*/*/*{sid}.jsonl")
    return path


def test_codex_sync_between_different_paths(machines):
    a, b, repo, clone, sid = setup(machines)
    b.actx("sync")
    path = rollout_of(b, sid)
    first = json.loads(path.read_text().splitlines()[0])
    assert first["payload"]["cwd"] == str(clone)
    assert f"Done in {clone}/cart.ts" in path.read_text()
    assert codex_threads(b)[sid] == ("fix the cart", 1, str(clone))  # Codex itself sees it, in its new folder
    assert "Cart fix" in (b.home / ".codex/session_index.jsonl").read_text()

    codex_continue(path, [("user", "add coupons"), ("assistant", f"Coupons in {clone}/coupons.ts")])
    assert "↑" in b.actx("sync")
    a.actx("sync")
    text = rollout_of(a, sid).read_text()
    assert f"Coupons in {repo}/coupons.ts" in text and str(clone) not in text
    assert "pending" not in a.actx("ls") and "pending" not in b.actx("ls")


def test_codex_conversions(machines):
    a, b, repo, clone, sid = setup(machines)
    b.actx("sync")
    claude_session(b, clone, [("user", "hi"), ("assistant", "hello")])
    out = b.actx("cv", pick=[sid, "claude"])  # Codex → Claude Code
    assert "Cart fix (from Codex)" in out and "claude --resume" in out

    copy = next(p for p in b.claude_dir.glob("*/*.jsonl") if "from Codex" in p.read_text())
    out = b.actx("cv", pick=[copy.stem, "codex"])  # Claude Code → Codex
    assert "codex resume" in out
    new_id = out.split("codex resume ")[1].split()[0]
    preview, turns, cwd = codex_threads(b)[new_id]
    assert "Conversation imported from Claude Code" in preview and turns == 1 and cwd == str(clone)


def test_codex_conflict_merge(machines):
    a, b, repo, clone, sid = setup(machines)
    b.actx("sync")
    codex_continue(rollout_of(a, sid), [("user", "coupons"), ("assistant", "Coupons done on desk")])
    codex_continue(rollout_of(b, sid), [("user", "green button"), ("assistant", "Button green on laptop")])
    b.actx("sync")
    assert "⚠ conflict" in a.actx("ls")

    out = a.actx("sync", "--all", answer="m")
    assert "branch from laptop" in out
    text = rollout_of(a, sid).read_text()
    assert "Coupons done on desk" in text and "[Meanwhile, this conversation continued on laptop" in text
    assert "Button green on laptop" in text
    threads = codex_threads(a)
    assert threads[sid][1] == 3  # the original turn, desk's, and the merge note with its reply
    assert sum(1 for p, _n, _c in threads.values() if p == "fix the cart") == 2  # plus the branch
    b.actx("sync")
    assert "pending" not in a.actx("ls") and "pending" not in b.actx("ls")
