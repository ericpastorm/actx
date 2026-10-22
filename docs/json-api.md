# JSON interface, version 1

## Explicit actions

`actx api` reads one JSON request from stdin (at most 1 MiB) and returns the same versioned
envelope. `action` can be `list`, `preview`, `add` or `delete`. For writes, `ids` is an explicit
array of physical `harness:id` values; there is no implicit "select everything". Add accepts
`project` or `general`, and an optional existing `folder` for binding the project on this machine.
Existing repository identities are validated before changing a local project binding.
Delete can be narrowed by `project` or `general`. Original local transcripts and project folders
are preserved when deleting saved copies. Antigravity native shared sync is unsupported.

```sh
echo '{"action":"add","ids":["claude:YOUR-ID"],"general":true}' | actx api
```

By default, actions return `applied: false`, `operations` and `results`, without writing files.
Each operation includes a `revision`. To execute, resend the request with `apply: true` and
`expected: {"claude:YOUR-ID": "REVISION-FROM-PREVIEW"}` for every selected copy. Execution
takes the shared lock and prepares the selection again before comparing revisions. These
tokens detect changes in native sizes/timestamps and shared metadata; they are not durable
content hashes or executable saved plans. Shared conflicts block add instead of overwriting.
`cancel: true` returns `status: cancelled`, exit 0, without inspecting or changing data.

Missing/stale confirmations return `status: decision_required`, exit 5, with current operations.
Busy folders return `shared_folder_busy` promptly, without blocking a frontend. Partial writes
or state-save failures return `incomplete` with per-copy results; successful copies are not rolled
back. The caller must refresh after any applied action. No binaries are published by this API.

The `sync`, `convert` and `resume` actions also use preview/confirmation. Send the returned
`data.expected` object unchanged in the confirmed request. `sync` accepts physical `ids`, a
project/General scope, `folder` or per-copy `folders: {"harness:id":"/destination"}`. An empty
selection syncs the scope. It rechecks native and linked changes, uses adapter backups and the
same shared locks, and returns per-copy results plus `remaining` operations. Interactive conflict
resolution is deliberately left pending; this action never silently chooses local or remote.
Retention and binary publication are excluded from explicit frontend sync actions.

`convert` requires one physical source ID and `target: "claude"|"codex"|"opencode"|"antigravity"`.
It accepts `folder` when the source's work folder is unavailable. It reuses explicitly identified
existing copies, preserving the source; a new conversion is local and is not implicitly published.
Shared sources can be converted without installing the original harness. Conversion also works
without configuring shared storage. `target_id` identifies the resulting copy.

`resume` accepts one conversation ID and an optional target harness. It checks/syncs shared
copies before opening and refuses pending conflicts or active agents. With `launch: false`
(default), it returns a structured `launch: {program,args,folder}` after syncing. `launch: true`
opens an agent in a separate system terminal: Windows Terminal on Windows, Terminal on macOS,
or an available terminal on Linux. The frontend remains responsive and receives `terminal_pid`.
Launching a terminal successfully does not guarantee the harness subsequently starts successfully.
Managed copies cannot be resumed through this API while their configured shared storage is offline.
Enable an unmaintained target copy in the harness profile before synchronized resume.

```sh
actx ls --json
actx ls myproject --json
actx ls --general --json
actx ls --all --json
actx sync --dry-run --json
actx sync --dry-run myproject --json
actx sync --dry-run --general --auto --json
```

These commands return one JSON object followed by a newline on stdout. They never prompt,
start an agent or emit terminal formatting. Runtime diagnostics are included in the object;
failure to write stdout is reported on stderr. Invalid CLI arguments use clap's stderr output
and exit code 2; `sync --json` requires `--dry-run`.

Listing reads maintained conversations by default; `ls --all` includes local conversations.
Preview uses the same project inference as `sync`: the project bound to the current folder,
or all collections when outside a bound project. `--all` and `--auto` inspect all collections;
an explicit project or `--general` narrows the scope. JSON never opens a project picker.

Both operations are read-only: no state migration, history cache writes, locks, backups,
conversation edits, binary updates or opencode CLI calls. SQLite reads use private temporary
snapshots, including WAL data. Filesystem timestamps and native counts still determine native
sync states; this API does not replace them with content fingerprints.

## Envelope and exit codes

Every response contains `schema_version: 1`, `command` (`ls` or `sync_preview`), `status`,
`data` and `diagnostics`. Each diagnostic has a stable `code`, human-readable `message`
and a `details` object. Messages can vary; consumers should branch on codes.

| Exit | Status | Meaning |
| --- | --- | --- |
| 0 | `ok` | Inspection succeeded, including empty results or operations requiring a decision. |
| 1 | `error` | Cannot perform the request; `data` is null. |
| 2 | — | Invalid CLI arguments; no JSON response. |
| 3 | `conflict` | Inspection succeeded and found conflicts; `data` contains the results. |
| 4 | `incomplete` | Partial or obsolete results; inspect diagnostics before using `data`. |

An incomplete read takes precedence over conflicts. There is no cancellation result because
these commands are noninteractive. An unavailable configured shared folder yields an incomplete
local listing; preview requires a reachable shared folder and returns an error instead.

Diagnostic codes: `invalid_config`, `config_unreadable`, `project_not_found`,
`shared_folder_not_configured`, `shared_folder_unavailable`, `inspection_failed`, `snapshot_changed`.
`project_not_found` includes `details.project`.

## Listing data

`data` contains `scope: {project, general, all}`, `shared_available`, `complete`, `projects`
and `conversations`. `projects` includes empty projects. Each entry has a `collection` and
`local_folder` (the remembered binding on this machine, or null).

A collection is `{ "kind": "project", "id": "myproject" }` or
`{ "kind": "general", "id": null }`. The translated General label is never an identifier.

Each conversation contains:

- `id`: canonical conversation identity, shared by its explicitly linked harness copies.
- `title`, `managed`, `state`, `updated_at`, `collections`, `desired_harnesses`, `copies`.
- Each copy has `id` (`harness:native-id`), `harness`, `state`, `local` and `shared`.
- Each present location has `folder`, `updated_at`, `weight`, `weight_unit`, `host`.
  Missing locations and local host values are null. Shared folders/hosts describe the source
  machine; they are not a resolved destination on this machine.

Timestamps are Unix seconds, possibly fractional. Weight units are `bytes` for Claude/Codex,
`messages` for opencode and `steps` for Antigravity. Weights are adapter change indicators,
not portable content hashes. State values are `local`, `new`, `synced`, `push`, `pull`,
`conflict`, `unknown`, `missing`, `unmanaged`; the aggregate conversation state uses the
highest-priority pending state. Offline maintained conversations are `unknown`.
Listings contain metadata, not message bodies or rendered previews. Matching titles alone
never group conversations.

## Preview data

`data` contains `scope`, `mode` (`manual` or `auto`), `read_only: true`, `valid`, `obsolete`,
`counts` and `operations`. Counts contain only operation kinds that occur. `valid` means the
inspection completed without errors or changing inputs; it does not mean there are no conflicts,
deferred operations or pending decisions. An obsolete snapshot has `valid: false` and exit 4.

Each operation contains `kind`, `session_id`, `conversation_id`, `title`, `collection`,
`source_harness`, `target_harness`, `target_id`, `reason`, `requirements` and `destination`.
`session_id` identifies the maintained physical source. For linked operations `target_harness`
names its counterpart and `target_id` is set once an existing copy is identified. Otherwise
those fields are null. Direction is encoded by `kind`: `linked_push` imports from the linked
counterpart into the source; `linked_pull` updates the counterpart. Native operations move
between local and shared copies of the same harness.

Kinds: `push`, `pull`, `new`, `synced`, `conflict`, `skip`, `check`, `convert`, `link`,
`linked_push`, `linked_pull`, `retention`.

Reasons are stable codes, independent of the UI language:

| Group | Reasons |
| --- | --- |
| Native copies | `local_changed`, `shared_changed`, `local_copy_missing`, `both_changed`, `copies_match`, `conversation_active`, `opencode_cli_required` |
| Linked copies | `recheck_after_sync`, `source_download_required`, `source_sync_required`, `source_active`, `work_folder_required`, `no_turns`, `linked_copy_missing`, `linked_copies_match`, `history_edited`, `histories_diverged`, `baselines_need_update`, `incomplete_turn`, `destination_active`, `continuation_ready`, `harness_not_installed`, `multiple_copies`, `copy_history_differs`, `matching_copy_found`, `linked_copy_required`, `linked_source_changed`, `linked_state_unreadable`, `linked_history_unreadable` |
| Retention | `retention_expired`, `retention_pending_changes`, `retention_linked_copy_not_ready` |

Requirements may include `destination_required`, `close_agent`, `restore_linked_copy`,
`install_harness`, `recheck_after_sync`, `resolve_conflict`. A null destination may mean no
destination is involved or it is unresolved; use the reason and requirements to distinguish them.

A preview is a snapshot, not an executable saved plan. Downloading a source, resolving a
conflict or importing a linked continuation can change later operations. A real sync always
recalculates operations and checks its locks and current inputs before writing.

Consumers should ignore additional fields and handle unknown codes rather than assume an
exhaustive enum. Breaking structural changes will increment `schema_version`. The older
`profile --json` retains its existing format and is not part of this envelope.

## Desktop support requests

- `settings` reads the configured root, language, harnesses, retention and installed tools.
  `root_from_env` indicates that `ACTX_ROOT` overrides saved configuration.
- `configure` accepts optional `root` (existing directory), `lang` (`en`, `es`, `zh-CN`) and
  `harnesses` (array). It uses preview/`data.expected`/`apply`, preserves unrelated settings,
  does not move storage and does not install hooks or timers.
- `create_project` requires `project` and an existing work `folder`. It uses the same confirmation
  flow and binds the folder/repository under the shared lock. It never adds conversations implicitly.
- `candidates` lists conversations for explicit selection. `project` narrows discovery using the
  bound folder/repository and bounded native history path references; `all: true` bypasses that
  filter. Without a project it returns the full inventory. It never adds conversations.
- `messages` requires one physical ID and returns at most the latest 20 normalized messages,
  each clipped to 4,000 Unicode scalar values, with truncation flags and total count. Local
  data is preferred; shared-only sources use the stored adapter. It is read-only and does not
  execute a harness CLI to export messages.

New statuses: `decision_required` (exit 5) and `cancelled` (exit 0). Existing list/preview statuses
and the older profile JSON format retain their behavior.

Content baselines are stored alongside legacy size baselines. New uploads include a versioned
content fingerprint; local fingerprints ignore auxiliary Claude/Codex records. Native task boundaries
keep unfinished Codex turns out of published snapshots. Old state remains readable and gains content
baselines on a successful sync; changes predating that first baseline cannot be inferred retroactively.
A new Codex compaction can replace an unchanged linked conversion while preserving the old native
copy as a retired local conversation. Independent target edits remain conflicts.
