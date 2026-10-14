"""Syncing Claude Code conversations between fake machines with different homes and repo paths."""

import json

from conftest import claude_continue, claude_session, git_clone, git_repo


def transcript(m, sid):
    """The texts of a machine's copy of a conversation, and the cwds it mentions."""
    (path,) = m.claude_dir.glob(f"*/{sid}.jsonl")
    lines = [json.loads(x) for x in path.read_text().splitlines()]
    texts = []
    for e in lines:
        content = (e.get("message") or {}).get("content")
        if isinstance(content, str):
            texts.append(content)
        elif isinstance(content, list):
            texts += [b["text"] for b in content if b.get("type") == "text"]
    return path, texts, {e["cwd"] for e in lines if e.get("cwd")}


def setup_two_machines(machines):
    a, b = machines("desk"), machines("laptop")
    repo = git_repo(a.home / "code" / "shop")
    clone = git_clone(repo, b.home / "work" / "shop-laptop")
    session = claude_session(a, repo, [("user", "fix the cart"), ("assistant", f"Done in {repo}/cart.ts")],
                             title="Cart fix")
    out = a.actx("mk", "shop", pick=[session.stem])
    assert "Project shop created" in out and "Cart fix" in out
    return a, b, repo, clone, session.stem


def test_roundtrip_between_different_paths(machines):
    a, b, repo, clone, sid = setup_two_machines(machines)

    out = b.actx("sync")  # no terminal: all projects
    assert "Cart fix" in out and str(clone).replace(str(b.home), "~") in out
    path, texts, cwds = transcript(b, sid)
    assert path.parent.name == b.mod.encode_claude(str(clone))
    assert cwds == {str(clone)} and f"Done in {clone}/cart.ts" in texts
    assert "✓ synced" in b.actx("ls")

    claude_continue(path, [("user", "add coupons"), ("assistant", f"Coupons in {clone}/coupons.ts")])
    assert "↑ push" in b.actx("ls")
    assert "↑" in b.actx("sync")

    assert "↓ pull" in a.actx("ls")
    a.actx("sync")
    _path, texts, cwds = transcript(a, sid)
    assert cwds == {str(repo)} and f"Coupons in {repo}/coupons.ts" in texts
    assert "pending" not in a.actx("ls") and "pending" not in b.actx("ls")


def test_conflict_merge_keeps_both_sides(machines):
    a, b, repo, clone, sid = setup_two_machines(machines)
    b.actx("sync")
    claude_continue(next(a.claude_dir.glob(f"*/{sid}.jsonl")), [("user", "coupons"), ("assistant", "Coupons done on desk")])
    claude_continue(next(b.claude_dir.glob(f"*/{sid}.jsonl")), [("user", "green button"), ("assistant", "Button green on laptop")])
    b.actx("sync")
    assert "⚠ conflict" in a.actx("ls")

    out = a.actx("sync", "--all", answer="m")
    assert "branch from laptop" in out
    _path, texts, _cwds = transcript(a, sid)
    assert "Coupons done on desk" in texts
    summary = next(t for t in texts if t.startswith("[Meanwhile"))
    assert "green button" in summary and "Button green on laptop" in summary and "Coupons" not in summary
    assert texts[-1].startswith("Got it")

    b.actx("sync")  # the laptop gets the merged version and the branch
    _path, texts, _cwds = transcript(b, sid)
    assert any(t.startswith("[Meanwhile") for t in texts)
    titles = b.actx("ls")
    assert "Cart fix (branch from laptop)" in titles
    assert "pending" not in titles and "pending" not in a.actx("ls")


def test_running_inside_the_project_places_it_there(machines):
    a, b, repo, _clone, sid = setup_two_machines(machines)
    c = machines("third")
    one = git_clone(repo, c.home / "old" / "shop-a")
    two = git_clone(repo, c.home / "ide" / "workspaces" / "shop" / "main")  # e.g. an IDE worktree

    assert "skipping" in c.actx("sync")  # two clones, same branch: ambiguous, and no terminal to ask
    c.actx("sync", cwd=two)
    path, _texts, cwds = transcript(c, sid)
    assert cwds == {str(two)} and str(one) not in path.parent.name


def test_sync_one_project_leaves_the_rest_pending(machines):
    a, b, repo, clone, sid = setup_two_machines(machines)
    notes = git_repo(a.home / "code" / "notes")
    other = claude_session(a, notes, [("user", "sort notes"), ("assistant", "sorted")], title="Notes")
    a.actx("mk", "notes", pick=[other.stem])
    out = b.actx("sync", "shop")
    assert "Cart fix" in out and "Notes" not in out and "1 pending in projects you didn't pick" in out
    assert "Notes" in b.actx("sync", "--all")


def test_rm_removes_from_the_store_only(machines):
    a, b, repo, clone, sid = setup_two_machines(machines)
    a.mod.confirm = lambda question: True
    out = a.actx("rm", "shop", pick=[sid])
    assert "Project shop deleted" in out
    assert not (machines.store / "shop").exists()
    assert list(a.claude_dir.glob(f"*/{sid}.jsonl"))  # still on the machine


def test_spanish_interface(machines):
    a = machines("desk", lang="es")
    assert "Aún no hay proyectos" in a.actx("ls")
