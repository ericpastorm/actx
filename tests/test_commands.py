"""init and update."""

import json
import os
import shutil
import time

from conftest import ACTX, Machine, load_actx


def test_init_saves_the_shared_folder(tmp_path):
    m = Machine(tmp_path, "pc", tmp_path / "unused")
    del m.env["ACTX_ROOT"]
    target = tmp_path / "nas" / "actx"
    assert "Carpeta compartida" in m.actx("init", str(target), "--lang", "es")
    config = json.loads((m.home / ".config/actx/config.json").read_text())
    assert config == {"root": str(target), "lang": "es"}
    assert (target / ".actx").exists()
    later = load_actx({k: v for k, v in m.env.items() if k != "ACTX_LANG"})
    assert later.store_root() == target and later.SPANISH


def _installed_copy(tmp_path, machines):
    m = machines("pc")
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    shutil.copy(ACTX, bin_dir / "actx")
    m.mod.SELF = bin_dir / "actx"
    shared = machines.store / "_bin" / "actx"
    shared.parent.mkdir()
    return m, bin_dir / "actx", shared


def test_update_installs_a_newer_shared_copy(tmp_path, machines):
    m, mine, shared = _installed_copy(tmp_path, machines)
    shared.write_text(ACTX.read_text() + "\n# newer\n")
    future = time.time() + 60
    os.utime(shared, (future, future))
    assert "updated" in m.actx("update")
    assert mine.read_text().endswith("# newer\n") and os.access(mine, os.X_OK)
    assert "latest version" in m.actx("update")


def test_update_uploads_when_this_copy_is_newer(tmp_path, machines):
    m, mine, shared = _installed_copy(tmp_path, machines)
    shared.write_text("# old\n")
    os.utime(shared, (time.time() - 600, time.time() - 600))
    assert "uploaded" in m.actx("update")
    assert shared.read_text() == mine.read_text()


def test_update_refuses_a_broken_shared_copy(tmp_path, machines):
    m, mine, shared = _installed_copy(tmp_path, machines)
    shared.write_text("def broken(:\n")
    os.utime(shared, (time.time() + 60, time.time() + 60))
    assert "broken" in m.actx("update")
    assert mine.read_text() == ACTX.read_text()


def test_update_defers_to_git_and_packages(tmp_path, machines):
    m = machines("pc")
    m.mod.SELF = ACTX
    if (ACTX.parent / ".git").exists():
        assert "git checkout" in m.actx("update")
    m.mod.install_kind = lambda: "package"
    assert "package manager" in m.actx("update")
