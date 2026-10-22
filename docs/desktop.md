# Desktop preview

The desktop is an optional native frontend using [egui/eframe](https://docs.rs/eframe/0.37.0/eframe/)
and [rfd](https://docs.rs/rfd/0.17.2/rfd/) folder dialogs. It shares the CLI's configuration,
Rust services, adapters and backups. It does not install a timer or run a web server.

```sh
cargo build --release --features gui --locked
./target/release/actx gui
```

GUI builds also produce `actx-gui` (`actx-gui.exe` on Windows), a launcher that can be opened
without a terminal. Keep it beside `actx`/`actx.exe`. `cargo install --path . --features gui --locked`
installs both. A Linux desktop entry is provided in `packaging/actx.desktop` for packaging.

A regular build keeps GUI dependencies disabled and reports build instructions if `gui` is
requested. Linux needs a working OpenGL desktop and X11 or Wayland. Folder dialogs use the
desktop portal, with Zenity as a fallback; see rfd's platform requirements. Windows/macOS
use system dialogs. Optional GUI dependencies are not needed to build or run the CLI alone.

## Workflow

1. **Settings** chooses an existing shared folder, language and harness profile. Review the
   preview and choose **Apply**. You can enter the converter without shared storage instead.
   `ACTX_ROOT`, when set, takes precedence over saved configuration.
2. **Create project** asks for a name and work folder. After confirmation, the app offers local
   conversations from that folder/repository, including conversations that mention its path in
   their opening messages. Select a conversation and use **Add**, then confirm its destination.
3. The normal view shows maintained conversations, with projects and General, search, activity,
   native copies and state. **Add conversations** discovers candidates for the destination project.
   **Show local conversations** also provides the full inventory; disable/re-enable it to leave
   candidate selection and browse all local conversations.
4. Choose a physical source copy and target harness. **Sync**, **Convert**, **Resume** and
   **Delete** first show what will happen. Choose a work folder when the destination is unresolved.
   Delete removes shared copies, preserving native originals and projects. Conversion is local,
   reuses identified existing copies and does not implicitly publish them.
5. Results show failures and remaining operations. Conflicts and active agents remain pending;
   resolve conflicts with the CLI for this first preview. The GUI does not silently choose a winner.

Discovery, history reads and actions run in isolated API subprocesses, outside the UI thread.
A spinner stays active while they work. Confirmations include current revisions and are checked
again under locks before writes. Closing during an apply is deferred until it completes.
Canceling a confirmation performs no writes.

Wide windows have three columns; narrow windows use conversation/detail tabs. **Ctrl+1 / Ctrl+2**
(**Cmd** on macOS) switch views. Tab navigation uses egui widgets. The bundled OFL font covers
common CJK characters without requiring a separate Chinese font install.

Resume opens a separate terminal: an available supported terminal on Linux, Terminal on macOS,
or Windows Terminal (`wt`, required) on Windows. Windows uses `cmd /k` for npm harness shims.
A terminal launch can succeed even if the harness later fails; its terminal displays that error.

## Current limits

This is a first desktop preview for review. Native startup, rendering and compact keyboard
navigation were checked on Linux in Spanish and Chinese using isolated fixture data. Action
services were checked with simulated machines. Windows/macOS native windows, dialogs, terminal
launching and accessibility still need hands-on review. Chinese welcomes native-speaker review.

The desktop does not yet expose conflict resolution, backup/quarantine restoration, automatic-sync
setup or project deletion. These remain in the CLI. Frontend sync excludes retention and binary
publication. Tray icons and notifications are outside this version. See [JSON API](json-api.md).
