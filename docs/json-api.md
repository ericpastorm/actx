# JSON interface, version 1

## Explicit actions

`actx api` reads one JSON request from stdin (at most 1 MiB) and returns the same versioned
envelope. Requests use these action names:

| Area | Actions |
| --- | --- |
| Catalog and previews | `list`, `preview`, `candidates`, `messages` |
| Conversations | `add`, `delete`, `sync`, `convert`, `resume` |
| Conflicts and recovery | `compare`, `resolve`, `recoveries`, `restore` |
| Desktop administration | `settings`, `configure`, `create_project`, `delete_project`, `diagnostics`, `automatic` |
| Project discovery | `adoption` |
| Local cleanup | `cleanup_scan`, `cleanup` |

Add and delete require an explicit `ids` array of physical `harness:id` values;
there is no implicit "select everything" for these actions. Add accepts
`project` or `general`, and an optional existing `folder` for binding the project on this machine.
Existing repository identities are validated before changing a local project binding.
Delete can be narrowed by `project` or `general`. Original local transcripts and project folders
are preserved when deleting saved copies. Antigravity native shared sync is unsupported.

```sh
echo '{"action":"add","ids":["claude:YOUR-ID"],"general":true}' | actx api
```

By default, add/delete previews return `applied: false`, `operations` and `results`, without writing files.
Each operation includes a `revision`. To execute, resend the request with `apply: true` and
`expected: {"claude:YOUR-ID": "REVISION-FROM-PREVIEW"}` for every selected copy. Execution
takes the shared lock and prepares the selection again before comparing revisions. These
tokens bind the current native content fingerprint, location, stamps and shared metadata.
Treat them as opaque action revisions, not portable content IDs or executable saved plans.
Other reviewed actions return a `data.expected` object to resend unchanged; their sections below
describe the inputs they bind. Shared conflicts block add instead of overwriting.
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
snapshots, including WAL data. Native change indicators remain available; where a content baseline
exists, states also compare versioned fingerprints to detect edits that counts alone would miss.
Legacy state falls back to its existing indicators until a successful sync records content baselines.
Catalog display does not download legacy shared payloads lacking a metadata fingerprint; their shared
weight remains the display indicator. Action planning and confirmation still perform full checks.

## Envelope and exit codes

Every response contains `schema_version: 1`, `command` (`ls` for `list`, `sync_preview` for
`preview`, or the dispatched action name), `status`,
`data` and `diagnostics`. Each diagnostic has a stable `code`, human-readable `message`
and a `details` object. Messages can vary; consumers should branch on codes.

| Exit | Status | Meaning |
| --- | --- | --- |
| 0 | `ok` | Inspection succeeded, including empty results or operations requiring a decision. |
| 1 | `error` | Cannot perform the request; `data` is null. |
| 2 | — | Invalid CLI arguments; no JSON response. |
| 3 | `conflict` | Inspection succeeded and found conflicts; `data` contains the results. |
| 4 | `incomplete` | Partial or obsolete results; inspect diagnostics before using `data`. |
| 5 | `decision_required` | A write needs a fresh preview, confirmation or another explicit decision. |
| 0 | `cancelled` | A request was canceled; running cleanup can include already completed recoverable moves. |

For `ls --json` and `sync --dry-run --json`, an incomplete read takes precedence over conflicts.
These two read-only commands do not prompt or produce cancellation results.
An unavailable configured shared folder yields an incomplete
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

## Conflict comparison and recovery

`compare` takes one physical source ID, optionally `target` for an explicitly linked harness.
It returns recent messages, omitted/truncated counts, common-prefix counts and an exact bounded
native merge preview. `resolve` previews and confirms `choice`: `local`, `remote`, `merge`, or
`defer` for native copies; linked copies support `repair`, `local`, `merge`, `defer`.
Keeping local natively preserves the shared version as a separate branch; keeping shared backs
up local. Linked repair keeps the retired native copy; `repair` refuses independent target edits.
Merges of edited, non-prefix linked targets require a manual decision and remain untouched.
All mutations require the returned `expected`, recheck revisions and protect open agents.

`recoveries` lists local backup/quarantine entries (optionally filtered by `ids`). `restore` takes
the listed opaque `backup` ID and uses preview/confirmation. Existing copies are backed up for
backup restoration; quarantine restoration never overwrites an existing conversation. Listing and
preview are read-only. These services back `actx compare`, `actx resolve`, `actx recoveries` and
`actx restore --backup <id>`; `--json` prints a plan, while the API applies confirmed requests.
Opening a selected harness synchronizes only its native source and selected linked target;
conflicts in other harnesses remain visible and preserved without blocking an otherwise safe copy.

Desktop administration adds read-only `diagnostics` and reviewed `automatic` (`choice`: `on`/`off`)
and `delete_project` (`project`). Automatic-sync changes recheck the hook/timer configuration;
OS failures report `incomplete` with actual component state. Project deletion checks the complete
shared-copy selection again under the store lock and preserves all local histories.

## Automatic additions (opt-in per project)

`{"action":"adoption","project":"Work"}` inspects the shared rule and current matching
candidates. Passing `adoption` proposes a replacement rule; use the returned `expected.adoption`
with `apply:true` to save it. Saving a rule does not immediately add conversations.

```json
{"action":"adoption","project":"Work","adoption":{"enabled":true,"subfolders":true,"include_clones":false,"harnesses":["claude","codex"],"exclude":["scratch/**"],"exclude_titles":["*private*"]}}
```

Rules default to disabled and live in project metadata, so each machine uses its own resolved
project folder. Enabled rules run during project/all sync (including the existing automatic
sync hooks/timer); explicit conversation-ID sync and General do not discover new histories.
Normal catalog browsing does not scan the local inventory. Only selected native harnesses are
scanned when an enabled, resolvable project needs discovery. Discovery uses session work folders;
opening-message path mentions remain a manual discovery suggestion.

`subfolders` defaults to true; other clones are excluded unless `include_clones` is enabled.
Exclusions are case-insensitive glob patterns (`*`, `?`, `**`), relative to the project folder;
`scratch/**` excludes both that folder and descendants. Title exclusions use the same syntax.
Matching more than one enabled rule yields a `deferred` operation with `ambiguous_project`;
active sessions/unfinished Codex turns are deferred. Subagents, linked/retired copies, known aliases,
and already shared IDs are excluded. Delete records a suppression in `.auto-add-ignored.json`;
manual Add clears it after a successful save. Disabling a rule preserves previously added histories.

Sync preview/dry-run includes `adopt` operations; applying sync rechecks its candidate snapshot.
The desktop exposes the same rule editor and candidate preview. CLI configuration uses
`actx auto-add Work --on --exclude 'scratch/**' --exclude-title '*private*'`; `--off` disables,
`--clear-exclusions` clears patterns, and `--json` returns a read-only plan.

## Reviewed local cleanup

`cleanup_scan` lists unmaintained local candidates with reasons, size, folder, bounded preview and
revision. Optional `cleanup` criteria include `days` (7), `max_messages` (4), `max_chars` (500),
`min_similar` (3), `harnesses`, `rules` (`agents`, `repeated`, `tests`, `short`; optionally `old`),
`folder` and `contains`. Nothing is selected or moved by analysis.

`cleanup` requires explicit physical `ids` and the same criteria. Preview returns quarantine
operations and `expected.cleanup`; use that revision with `apply:true`. Changed/protected/missing
selections are rejected. Shared storage must be reachable when configured. The existing cleanup
engine rechecks fingerprints, live agents, parent chains and maintained identities under locks,
creates durable compressed recovery copies, then moves only the reviewed local originals.
`recoveries`/`restore` expose the resulting quarantine. There is no GUI permanent-purge action.

Desktop workers use opt-in JSON progress events on stderr; stdout remains one API envelope.
Canceling analysis stops its read-only subprocess. Canceling cleanup signals a cooperative stop
between conversations, preserving any completed moves in quarantine and reporting their results.
Closing a read-only desktop job also cancels its subprocess. A write confirmation can be canceled
without creating quarantine entries or changing histories.

## Interrupted shared publications

`recoveries` also lists `publication:<opaque-id>` entries for recognized `.tmp-*`/`.old-*`
directories, including legacy PID names. Rows distinguish staged/previous versions, incomplete
payloads and active shared writers. `can_restore` means the entry can be reviewed; full payload
validation takes place in the restore preview. `pending` distinguishes interrupted work from a
previous version intentionally retained after a successful recovery. Local backup/quarantine
entries remain accessible if shared storage is unavailable; the response marks that partial read.

Use `restore` with the publication ID to preview its source, destination and recent messages.
Applying requires `expected.restore`, then rechecks both candidate and current shared bytes under
the local action/shared-folder locks. It validates metadata identity, relative payload paths,
compressed JSON and attachments, plus recorded fingerprints/checksums where available. Incomplete,
changed, linked-file or actively written candidates are retained and refused. Recovery changes
only the shared publication; local native histories are preserved.

When the destination is absent, the candidate becomes its shared copy. When it exists, confirmation
explicitly states that its current version will be retained as another recoverable publication.
The previous version can subsequently be restored through the same reviewed operation.
`actx recoveries`, `actx restore --backup <id>`, the desktop Recovery window and Diagnostics expose
this workflow; `actx doctor` reports pending publications.

New uploads use unique transaction names and a synced `.publish-<uuid>.json` journal. Staged files
are synced before the shared swap; lock ownership and staged checksums are checked again before
publishing. Failure after moving the previous destination attempts to put it back. Failed stages
and alternative versions are never deleted by reusing a PID-based temporary name. Normal successful
uploads remove only their own completed temporary/previous data. Unknown remnants and incomplete
unrecoverable fragments are left in place for inspection; journals without remaining data are not
listed as recoverable conversations. Directory sync is used where the platform supports it.
