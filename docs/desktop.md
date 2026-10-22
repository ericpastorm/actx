# Desktop preview

The **actX** desktop follows Pneuma's terminal style: black panels, square borders,
JetBrains Mono typography and white/gray controls. The wordmark uses a white `act`
and a red uppercase `X`; color in the interface indicates status and destructive actions.
The wordmark uses original bitmap lettering. Settings offers **Pixel 5x7** (default),
**Terminal stencil** and **ASCII matrix**, with a red X pulse/scan animation that can be disabled.
These local appearance choices are saved immediately, independently of sync configuration.
Compare the [three lettering proposals](../assets/actx-wordmark-proposals.svg).

A standalone vector wordmark is available in [assets/actx.svg](../assets/actx.svg).

The desktop is an optional native frontend using [egui/eframe](https://docs.rs/eframe/0.37.0/eframe/)
and [rfd](https://docs.rs/rfd/0.17.2/rfd/) folder dialogs. It shares the CLI's configuration,
Rust services, adapters and backups. Opening it does not install a timer or run a web server. Automatic-sync configuration is explicit.

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

On Debian/Ubuntu with X11, install the graphics runtime libraries before opening the desktop:

```sh
sudo apt-get install libxkbcommon-x11-0 libegl1 libgl1 libgl1-mesa-dri
```

The [X11 keyboard library](https://packages.ubuntu.com/noble/libxkbcommon-x11-0) is a separate
package from `libxkbcommon`; having the latter installed alone does not provide X11 keyboard support.

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
5. **Compare histories** shows local/shared or selected linked histories, omitted-message counts
   and the native merge preview. Choose a resolution and review **Apply**; the alternative stays
   recoverable. Independent edits are never silently replaced. A Codex compaction with an intact
   target can repair the conversion while retaining the old native conversation.
6. **Recovery** lists backups, quarantined conversations and interrupted shared publications, with
   preview and confirmation. Restoring a backup protects the current history; quarantine recovery
   does not overwrite it. Replacing a shared publication retains its previous shared version.
7. A selected project's **Delete project** reviews all shared copies and keeps local originals.
   **Settings** shows automatic-sync components and lets you enable or disable them through a
   reviewed operation. **Diagnostics** shows storage availability, installed tools and recent
   automatic-sync logs. Partial OS setup failures are reported with actual component state.

8. **Clean local conversations** opens criteria and an explicit candidate checklist. Analyze first,
   review reasons/previews, select entries and confirm moving them to recoverable quarantine.
   Progress and cancellation are available; a running move finishes the current conversation before
   stopping. **Recovery** restores completed moves. Nothing is selected by default.
9. **Automatic additions** on a selected project edits its opt-in shared rule, harnesses, subfolders,
   clone handling and exclusions. **Preview** lists current matches before saving. **Sync project
   scope** applies the selected project, General or all projects; eligible new conversations are
   included when their project rule is enabled. A conversation-only Sync keeps its explicit scope.
   Rule activation applies on future syncs, and deleted conversations remain excluded until manually added.

Discovery, history reads and actions run in isolated API subprocesses, outside the UI thread.
Task handling and local catalog checks continue when the window is hidden or minimized.
A spinner stays active while they work. Confirmations include current revisions and are checked
again under locks before writes. Closing during an apply is deferred until it completes.
Canceling a confirmation performs no writes.

App-owned dialogs open centered horizontally and vertically. Vector action icons retain text
labels and keyboard interaction; they do not depend on system emoji fonts.
Action icons have their own column so wrapped labels stay aligned. Official harness logos identify
Claude, Codex, opencode and Antigravity in selectors and configuration. Conversation titles use
the full row width, with small logos beside status and age underneath; hovering reveals the tool names.
Artwork sources are recorded in [assets/harnesses](../assets/harnesses/README.md).

Wide windows have three columns; narrow windows use conversation/detail tabs. **Ctrl+1 / Ctrl+2**
(**Cmd** on macOS) switch views. Tab navigation uses egui widgets. The bundled OFL font covers
common CJK characters without requiring a separate Chinese font install.

Resume opens a separate terminal: an available supported terminal on Linux, Terminal on macOS,
or Windows Terminal (`wt`, required) on Windows. Windows uses `cmd /k` for npm harness shims.
A terminal launch can succeed even if the harness later fails; its terminal displays that error.

## Startup and local index

The desktop stores the last complete normal catalog in the local XDG cache folder
(`~/.cache/actx/gui-index-v1.json` by default on Core). It includes project work folders,
conversation IDs/titles, native/shared locations and last checked states; it contains no message
history. This file is specific to this computer, shared-folder configuration and harness locations.
It is disposable and is rebuilt if missing, corrupt, from another configuration or version.

Startup displays this snapshot with its last-check time and a pending-verification label, then
checks the actual catalog in a separate background subprocess. Settings and selected-message
reads run independently of that catalog check. There is no automatic conversation selection:
message history is loaded only after selection, and superseded selections/results are discarded.
An unchanged selected copy reuses the messages already loaded in memory.

Successful normal refreshes atomically replace the index. Full local inventory and candidate
searches do not enter it. Partial/offline refreshes preserve the last complete snapshot with an
unverified label and diagnostics. CLI/API changes notify an open desktop via local cache/state
metadata; it checks those cheaply every two seconds. Returning focus also requests verification.
Changes made on another computer appear on the next catalog refresh; no filesystem watcher is
installed on shared storage. GUI actions refresh the catalog afterwards. Settings changes discard
old on-screen data before loading the new configuration.

For older shared publications without a stored content fingerprint, the catalog uses their legacy
weight indicator rather than downloading compressed histories just to display a row. Current
publications supply their content hash in metadata. This display fallback never authorizes writes.

All previews and writes still re-read the real histories and check revisions/locks. The index
is only for display and never supplies synchronization decisions or work-folder bindings.
Opening the GUI creates the index; read-only CLI/JSON queries do not create or update it.

## Current limits

This is a first desktop preview for review. Native startup, rendering and compact keyboard
navigation were checked on Linux in Spanish and Chinese using isolated fixture data. Action
services were checked with simulated machines. Windows/macOS native windows, dialogs, terminal
launching and accessibility still need hands-on review. Chinese welcomes native-speaker review.

Frontend sync excludes retention and binary publication. Tray icons and notifications are outside this version. See [JSON API](json-api.md).

### Platform validation

| Coverage | Current state |
| --- | --- |
| CLI tests | The repository CI matrix runs default-feature builds/tests on Linux, macOS and Windows. Check the run for the relevant commit before assuming it passed. |
| GUI builds and frontend tests | A separate CI matrix enables `--features gui` on Linux, macOS and Windows: Clippy, unit tests, compact/wide egui rendering, and builds of both `actx` and `actx-gui`. |
| Native Linux desktop | CI starts a real window under Xvfb with software OpenGL and isolated conversations. It verifies API-driven project/add/delete/General catalog refreshes, offline snapshot retention and unchanged original history. Startup, hidden-window handling and cleanup cancellation also have focused regressions. |
| Native Windows/macOS desktop | Window behavior, folder dialogs, terminal launching and cancellation still require hands-on validation. |

Code review and simulated machines do not establish native platform parity. In particular, Windows
discovery invalidation uses file size/mtime where Unix also uses change time/inode. Publication
payloads are flushed before swapping; directory syncing is Unix-specific. Crash recovery and cache
invalidation still need native filesystem testing on Windows/macOS.

Run the frontend checks locally with:

```sh
cargo clippy --locked --features gui --all-targets -- -D warnings
cargo test --locked --features gui --bin actx --bin actx-gui
cargo build --locked --features gui --bins
# Linux, with Xvfb, xauth and Mesa installed:
xvfb-run -a cargo test --locked --features gui --test desktop -- --ignored --nocapture
```

The native window test uses temporary homes/storage and is ignored by ordinary test runs.
It tests catalog updates through the reviewed API; it does not click every native dialog or launch
an interactive coding agent. Headless egui tests render ten views/dialogs at compact and wide sizes.

## Advanced discovery performance

Discovery keeps disposable native-file metadata and bounded cleanup summaries in the local
`discovery-v1.json` index. File size, modification time and Unix change time/inode invalidate
individual entries; SQLite WAL changes invalidate database-derived entries. Directory enumeration
still finds newly created conversations and ignores removed files. Codex custom titles participate
in their metadata signature. Missing/corrupt/foreign indexes fall back to native reads.

GUI subprocesses return index deltas through the private stderr transport; the desktop persists
those atomically. Ordinary CLI/JSON queries can reuse them and do not write them. Full native
history fingerprints are calculated only for explicit cleanup selections, then checked again
before and after creating the recovery copy. Active/maintained/parent protections stay live.
Discovery and cleanup never trust a display index as authorization to overwrite or delete data.

Project candidate listing reuses one catalog, including repository/path-mention suggestions.
opencode is not opened for a managed-key query containing no opencode conversations. Unchanged
opencode metadata can be reused without copying its database for each query; changed databases
continue to use isolated read snapshots.

## Interrupted publication recovery

**Recovery** includes shared publication remnants alongside local backups and quarantine. It labels
previous/staged versions, incomplete remnants and active writers, and can refresh the scoped list.
Reviewing a recoverable entry validates its payload and shows recent messages. Applying can restore
a missing shared copy or replace it while preserving the current shared version as a new recovery
entry. Native local histories are kept. Incomplete entries remain visible with their reason and
are never silently deleted. **Diagnostics** and `actx doctor` report pending shared publications.

Inspection jobs have cancellable subprocesses and private scratch directories. Superseded message
or catalog reads are stopped; cancelled SQLite inspections have their scratch files removed by the
parent. Cleanup cancellation finishes its current durable move and reports completed/quarantined
items before stopping. Ordinary publication/recovery writes finish their reviewed operation rather
than being killed mid-swap.
