# Changelog

## Unreleased

### Faster synchronization and cleanup fixes

- Fingerprint legacy shared snapshots (without `content_hash`) once and cache them by file stamp,
  instead of downloading and rehashing them up to four times per preview and again on every sync.
  Previews still write nothing; real and automatic syncs persist the cache.
- Reuse cached fingerprints for desktop review tokens, skip baselines that are already current,
  and decide unchanged linked copies from the linked-history cache without reading both histories.
- Read a conversation twice instead of four times when uploading it; the final check only rehashes
  if the file changed, so active conversations fail less with "changed while preparing the upload".
- Remove the staged files of a failed upload, and sweep incomplete staged files left by runs that
  died (recoverable staged versions are kept). Previously each failure left a full copy behind.
- Keep one automatic sync running and at most one queued per machine; extra hook or timer
  triggers exit at once and leave the conversation they were opening protected for the queued run.
- Don't remember an unknown opencode version detected during inspection, which made a later import
  use opencode 1.x arguments against opencode 2.x.

### Desktop conversation moves and project renames

- Add searchable multiple selection for moving saved conversations between projects and General,
  including confirmed creation of a destination project. The detail panel can preselect a conversation.
- Add project renaming in the sidebar. Keep drafts after cancellation or failed/stale confirmations;
  use the confirmed destination when refreshing the view after success.
- Reuse the CLI's move/rename plans through reviewed `move` and `rename_project` API actions.
  Expand explicitly linked shared copies, preserve snapshots and native edits, and recheck tokens
  and destination collisions under the shared lock. Preview and cancellation remain read-only.
- Center organization dialogs, adapt their lists to short windows and make the sidebar scrollable.
  Extend GUI rendering checks and the native Linux smoke flow to cover organization and index refresh.

### Moving saved conversations and renaming projects

- Add `actx mv [destination]`, `--general` and `--all` with source detection from the current
  project, grouped conversation selection, a destination picker and confirmed project creation.
- Move existing shared snapshots and all linked harness copies together, preserving native
  histories, pending local edits, identities, preferences and sync baselines.
- Add `actx mv --project OLD NEW`. Keep work folders and project settings, and carry former names
  so other computers can retain non-Git folder bindings across renames.
- Recheck reviewed inputs and destination collisions under the shared lock. Roll back completed
  conversation moves after I/O failures; shared snapshots remain intact if the process is interrupted.
- Provide English, Spanish and Simplified Chinese interface texts. Desktop controls are planned
  separately; the operation logic is shared for that integration.

### Conversion from the current folder

- `actx cv` discovers conversations in the current folder/project, including when launched from
  a Git subdirectory or a bound actx project's subfolder. Shared-only copies follow the local
  project binding, even when published from a different computer.
- Use `actx cv --all` for all folders and folderless conversations. Explicit project and General
  selectors still work from any folder; conversion still works without setup or a shared store.

### Desktop CI and workflow fixes

- Build and lint the GUI and launcher on Linux, macOS and Windows. Run frontend state and
  compact/wide egui rendering tests, plus a real Linux window under Xvfb with isolated histories.
- Verify project/add/delete/General catalog updates, offline snapshot retention and preservation
  of the original conversation while the native window is running.
- Keep project-creation folders separate from conversation destinations. Preserve settings drafts
  when saving or automatic-sync setup fails, and freeze those editors during reviewed actions.
- After adding a conversation, show its confirmed destination even if the editor selection changed.
  Show the active shared folder in the sidebar and release controls when a worker disconnects.

### Cross-machine sync and CI fixes

- Rewrite conversation paths using the latest publishing computer's metadata when syncing back
  to another machine. Keep the project binding as a fallback for conversations started at home.
- Inspect opencode databases through private SQLite snapshots so the first discovery does not
  hide sessions or invalidate content fingerprints by creating WAL sidecars in the source database.
- Fix Windows-specific compiler warnings and update native opencode fixtures for its current
  import timestamps. Scoped-sync tests verify that unselected conversations stay undownloaded.
- Isolate PowerShell startup caches from the inspected Windows profile, prefer PowerShell 7
  when available and retain Windows PowerShell as a fallback. Make cross-platform fixtures use
  explicit timestamps and compare native paths. CI stops when the first test target fails.

### Codex activity, growing replies and desktop appearance

- Identify Codex activity by the actual writable rollout or an explicit resume ID, rather than
  treating every conversation in the same work folder as open. Stopped launchers without an open
  writer and idle indexing app-servers no longer block resume. A server with an unfinished task
  remains protected; Linux and macOS file discovery distinguish writers from readers.
- Recognize an exact extension of the last synchronized assistant reply as a continuation. Existing
  saved hashes must match its full previous prefix; independent edits and divergent continuations
  remain conflicts. Append the new reply text without duplicating the user prompt or changing IDs.
- Keep catalog display from downloading legacy shared histories that lack a fingerprint in their
  metadata; use their existing weight indicator for display and keep full checks in action planning.
- Add labeled vector action icons, centered dialogs and original pixel/ASCII wordmark proposals.
  Settings chooses Pixel 5x7, Terminal stencil or ASCII matrix; the red X animation can be disabled.
  Appearance preferences stay on this computer.
- Use official harness logos in selectors and settings, and small logos beside conversation status
  and age. Titles span the full row; hovering reveals harness names. Wrapped action labels reserve
  a separate icon column, and Settings uses a gear icon. Bundled artwork needs no network requests.

These changes extend the CLI and optional desktop preview. Existing configuration and legacy size
baselines remain readable. This section describes source changes; it does not announce a tagged
release or downloadable binaries.

### Conversation integrity and conflict recovery

- Add versioned content baselines so edits can be detected even when message counts or file sizes
  stay the same. Successful syncs establish baselines for legacy state.
- Publish Codex snapshots only through complete task boundaries. A compaction can rebuild an
  unchanged linked copy while retaining its previous native history; independent edits remain conflicts.
- Add `actx compare`, `actx resolve` and shared JSON services for reviewed conflict decisions.
  Alternatives remain available as backups, branches or retired native copies.
- Add `actx recoveries` and reviewed restore requests for local backups and cleanup quarantine.
  Resuming a chosen harness checks only the source and selected target; conflicts in other copies
  remain visible without preventing an otherwise safe resume.

### Desktop workflow

- Add comparison, resolution, recovery, project deletion, automatic-sync settings and diagnostics.
  Mutations share CLI/API services and recheck the reviewed inputs before writing.
- Display the last complete normal catalog from `gui-index-v1.json`, then verify it in the
  background. Preserve that snapshot when shared storage is unavailable; label it as unverified.
- Load messages on explicit selection, coalesce superseded reads and ignore outdated results.
  Keep task processing active while the window is hidden or minimized.
- Add local cleanup with criteria, candidate reasons/previews and an empty initial selection.
  Selected conversations move to recoverable quarantine after confirmation. Progress and cancellation
  are available; cancellation finishes the current conversation and leaves completed moves recoverable.

### Project discovery and performance

- Add opt-in `actx auto-add` project rules, including harness selection, title/path exclusions,
  subfolder controls and optional matching of other clones. Saving a rule does not add conversations
  immediately; eligible project/all syncs apply it using each machine's local project folder.
- Keep manual selection as the default. Defer ambiguous, active or unfinished conversations, and
  remember deletions so automatic additions do not immediately re-add a removed shared copy.
- Reuse native metadata and bounded cleanup summaries in disposable `discovery-v1.json` entries.
  File changes, SQLite WAL changes and Codex custom titles invalidate their relevant entries.
- Reuse one catalog for project candidate discovery; avoid opening opencode for managed selections
  containing no opencode IDs. Calculate full cleanup fingerprints for explicit selections and
  recheck their native data before moving it.
- Keep ordinary read-only CLI/JSON queries free of cache writes. Desktop subprocesses return
  discovery deltas for the desktop to persist locally.

### Interrupted shared publications

- Use unique staging/previous-version names, a synced publication journal and staged-file checksums.
  Recheck lock ownership before publishing and attempt rollback if the previous destination was moved
  but the replacement failed. Normal successful publication removes only its own completed remnants.
- Expose recognized legacy/new `.tmp-*` and `.old-*` publications through Recovery, Diagnostics and
  `actx doctor`. Preview validates payloads and shows recent messages before restoring a shared version.
- Preserve the current shared version as another recoverable entry when replacing it. Native local
  histories stay intact. Incomplete, unsafe or actively written candidates are retained and refused.

### Compatibility and validation

Focused Linux checks cover content edits, compaction, reviewed actions, cleanup/restore, interrupted
publication failures and GUI catalog behavior. An isolated native cleanup cancellation check confirms
that completed moves remain recoverable and unprocessed originals remain present.

The [CI workflow](.github/workflows/test.yml) tests default CLI features and separately enables the
GUI on Linux, macOS and Windows, with an additional native Linux window check. Windows/macOS desktop dialogs,
terminal launching, cancellation and filesystem recovery behavior still require validation.

See the [desktop guide](docs/desktop.md) for installation and platform limits, or the
[JSON API contract](docs/json-api.md) for actions, revisions, scopes and exit codes.
