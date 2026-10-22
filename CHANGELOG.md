# Changelog

## Unreleased

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

The existing [CI workflow](.github/workflows/test.yml) tests default CLI features on Linux, macOS and
Windows. It does **not** enable the optional GUI feature. Native Windows/macOS desktop dialogs,
terminal launching, cancellation and filesystem recovery behavior still require validation.

See the [desktop guide](docs/desktop.md) for installation and platform limits, or the
[JSON API contract](docs/json-api.md) for actions, revisions, scopes and exit codes.
