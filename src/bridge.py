#!/usr/bin/env python3
"""actx used to be a Python script; it's a native program now. This stands in for the old script
in the shared folder (_bin/actx): `actx update` on a machine with the Python version installs it,
and the first time it runs it puts the program for this machine in its place and runs that."""

import json
import os
import platform
import shutil
import sys
from pathlib import Path


def main() -> None:
    config_dir = Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config")
    try:
        config = json.loads((config_dir / "actx" / "config.json").read_text())
    except (OSError, ValueError):
        config = {}
    lang = os.environ.get("ACTX_LANG") or config.get("lang") or os.environ.get("LANG", "")[:2]

    def die(en: str, es: str) -> None:
        print(f"actx: {es if lang == 'es' else en}", file=sys.stderr)
        sys.exit(1)

    arch = platform.machine().lower()
    arch = {"arm64": "aarch64", "amd64": "x86_64"}.get(arch, arch)
    system = {"darwin": "macos"}.get(sys.platform, sys.platform)
    root = os.environ.get("ACTX_ROOT") or config.get("root")
    if not root:
        die("no shared folder configured", "no hay carpeta compartida configurada")
    new = Path(os.path.expandvars(os.path.expanduser(root))) / "_bin" / f"actx-{arch}-{system}"
    if not new.is_file():
        die(
            f"the shared folder has no actx for {arch}-{system} yet: install it from "
            "https://github.com/ericpastorm/actx",
            f"la carpeta compartida aún no tiene actx para {arch}-{system}: instálalo desde "
            "https://github.com/ericpastorm/actx",
        )
    me = Path(__file__).resolve()
    tmp = me.with_name(".actx.new")
    try:
        shutil.copyfile(new, tmp)
        os.chmod(tmp, 0o755)
        when = new.stat().st_mtime
        os.utime(tmp, (when, when))  # same date as the shared copy, so it doesn't look newer
        os.replace(tmp, me)
    except OSError as e:
        tmp.unlink(missing_ok=True)
        die(f"can't write {me}: {e.strerror}", f"no se puede escribir {me}: {e.strerror}")
    os.execv(me, [str(me), *sys.argv[1:]])


main()
