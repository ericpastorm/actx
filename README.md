# actx

**Agent context, everywhere.** Keep your [Claude Code](https://claude.com/claude-code),
[opencode](https://opencode.ai) and [Codex](https://github.com/openai/codex) conversations in sync
across your machines, grouped into projects, through any shared folder: a NAS, Syncthing, Dropbox…

[Leer en español](README.es.md)

You work on a desktop and a laptop. `git pull` brings the code, but the conversation with your
agent (everything it learned about the task) stays behind. actx brings it along:

```
desktop$ actx mk shop          # create an empty project
desktop$ actx add shop         # pick the conversations to save in it
laptop$  actx sync             # pull them, and push whatever you continued here
laptop$  claude --resume …     # carry on where you left off
```

## Features

- **Projects you name, conversations you choose.** Only conversations you explicitly add travel, in a project or in General.
- **Path independent.** The repo can live at a different path on each machine (another clone, an
  IDE worktree…): actx finds it by its first commit and rewrites the paths inside the conversation.
- **Two-way sync with conflict handling.** It remembers the last sync, so it knows what to push,
  what to pull, and when both sides changed. Then you can **merge**: keep this machine's version
  plus a summary of what was done on the other, and the other version is kept as a separate branch.
- **Automatic, if you want.** `actx auto on`, and conversations sync when you open or close them in
  Claude Code or Codex, and every 10 minutes. It never replaces a conversation that's open in an agent.
- **Claude Code ⇄ opencode ⇄ Codex ⇄ Antigravity.** Copy a conversation into another tool and keep going
  there.
- **Safe.** Backups before overwriting (`actx restore`), atomic writes and a lock in the shared folder,
  and git warnings when the code is out of step (uncommitted changes, a different commit).
- **One small binary.** Written in Rust; it only needs git and fzf. Linux, macOS and Windows. English and Spanish
  interface.

## Install

With [Rust](https://rustup.rs) installed:

```
cargo install --git https://github.com/ericpastorm/actx
```

**Arch Linux:** the AUR package is on its way. Meanwhile you can build it with the PKGBUILD in this repo:

```
git clone https://github.com/ericpastorm/actx
cd actx/packaging/aur && makepkg -si
```

**Windows:** the same `cargo install` works (with the
[Build Tools for Visual Studio](https://visualstudio.microsoft.com/visual-cpp-build-tools/) installed, which Rust
asks for anyway). `git` and `fzf` come from winget: `winget install Git.Git junegunn.fzf`. Conversations move
between Windows, Linux and macOS: the paths inside them are rewritten between `C:\Users\me\shop` and
`/home/me/shop`.

At runtime it needs `git` and [`fzf`](https://github.com/junegunn/fzf). On machines without Rust, copy the
binary from one that has it: actx leaves a copy in the shared folder (`_bin/`), and `actx update` keeps it
current (machines still on the old Python version move to this one too: run `actx update` twice).

Shell completions: `actx completions bash|zsh|fish` (the PKGBUILD installs them), e.g.
`actx completions bash > ~/.local/share/bash-completion/completions/actx`.

Supported tools: **Claude Code**, **opencode** 1.x and 2.x (each machine can have either: a conversation
saved with one is converted when it reaches the other), and **Codex**. **Antigravity CLI** (`agy`)
converts to and from the others, but its conversations don't sync yet.

## Quick start

On every machine, once:

```
actx init            # choose the shared folder (it suggests network mounts and sync folders)
actx auto on         # optional: sync by itself (see "Automatic sync" below)
```

Then:

| Command | What it does |
|---|---|
| `actx mk <project>` | Create an empty project. |
| `actx add [project]` | Pick local conversations to save in an existing project. Without a project, save them in General. |
| `actx delete [project]` | Pick shared copies to delete, across all projects and General or only the named project. Keeps local originals and projects, even when empty. |
| `actx sync [project]` | Push what you continued here, pull what's new. Pick projects and General, or use `--all`. |
| `actx` / `actx ls [project]` | Projects and the state of each conversation: `↑ push`, `↓ pull`, `↓ new`, `⚠ conflict`, `✓ synced`. |
| `actx cv [project]` | Copy a conversation into another tool (Claude Code, opencode, Codex, Antigravity). |
| `actx rm <project>` | Delete the whole project and its shared copies, after confirmation. Keeps local originals on every machine. |
| `actx auto [on\|off]` | Automatic sync: hooks in Claude Code and Codex, and a timer. Plain `actx auto` shows how it's doing. |
| `actx restore` | Put back a conversation from a backup. |
| `actx doctor` | Check the shared folder, the tools and automatic sync. |
| `actx update` | Install the newest actx found in the shared folder (for copies installed by hand). |

Pickers: type to filter, <kbd>Tab</kbd> marks several, <kbd>Ctrl</kbd>+<kbd>A</kbd> all, <kbd>Enter</kbd> confirms.

`mk` now only creates projects; use `add <project>` where you previously used `mk` to save
conversations. Use `delete` to remove individual shared copies; `rm` now deletes a whole project.
Existing saved projects keep working without migration.

### General conversations

Run `actx add` to save selected conversations without creating an actx project or a Git repository.
On another machine, `actx sync` asks for a destination folder for each new General conversation.
Choose an existing folder (including **other folder** to type any path); actx remembers it separately
for each conversation on that machine, even when several came from the same original folder.
Cancel to leave it pending. If a remembered folder no longer exists, sync asks again.

Use `--general` with `sync`, `ls`, `cv` or `delete` to select only General. For example:

```sh
actx add                     # save selected local conversations in General
actx sync --general          # download General conversations; choose destinations when needed
actx ls --general            # see their state
actx delete --general        # select shared copies to remove; keep local originals
```

Automatic sync skips General conversations that still need a destination, with a message in its
log. Run an interactive sync once to choose it. Already present conversations stay in their local
folder. A conversation belongs to one collection: adding it elsewhere moves its shared copy.
All machines need an actx version with General support to sync these conversations; older clients
continue seeing the existing projects.

**Tip:** run `actx sync` inside the project (e.g. in your IDE's terminal): that project comes first,
and new conversations of that repo go straight there.

## How it works

### Where a conversation goes

When a conversation arrives on a machine for the first time, actx looks for:

1. a folder you picked before for that repo;
2. the same path (relative to your home);
3. the folder you're in, if it's the same repo;
4. any clone or worktree of the repo under your home (`ACTX_SCAN`), or next to folders with
   conversations (projects on another disk, say), identified by its **first commit**, so it
   works whatever the path or folder name;
5. otherwise it asks once (with *here* as the first option) and remembers.

Paths inside the conversation are rewritten from the other machine's to this one's.

### Sync and conflicts

Each machine remembers how every conversation looked at its last sync. Changed only here → push.
Changed only in the shared folder → pull. Changed on both → conflict, and you choose: **merge**,
keep this one, take the other one, or skip.

*Merge* keeps this machine's version and appends a message summarizing what was done on the other
one (and an acknowledgement from the agent), so the agent knows about both. The other version is
also kept, whole, as *«title (branch from laptop)»*. Before anything is overwritten, a backup goes
to `~/.local/state/actx/backups/`.

Close the conversation in its agent before merging. If actx detects that it is open, the merge
stays pending until you close it and run `actx sync` again.

### Automatic sync

`actx auto on` sets up, on this machine:

- **hooks in Claude Code** (`~/.claude/settings.json`) **and Codex** (`~/.codex/hooks.json`), next to
  any you already have. When a conversation starts, actx syncs in the background; if the one you're
  resuming has newer messages from another machine, Claude Code or Codex shows you a warning (exit, run
  `actx sync`, resume it again). When it ends, it's pushed. Codex asks you to review new hooks once:
  accept them in `/hooks`.
- **a timer** every 10 minutes (a systemd user timer on Linux, a LaunchAgent on macOS, a Task Scheduler
  task on Windows, run with no console window). It's what
  keeps opencode in sync, and it catches anything the hooks missed.

Each run is `actx sync --auto`: it pushes what changed and pulls what's new, but it never asks, so it
only does what's safe. It doesn't pull over a conversation that's open in an agent (it waits until you
close it), and it leaves conflicts, and new conversations it doesn't know where to put, for `actx sync`.
What it does goes to `~/.local/state/actx/auto.log`; `actx auto` shows the last lines. `actx auto off`
removes everything.

### Backups

Before replacing a conversation on this machine (pulling, merging, restoring), actx keeps a copy in
`~/.local/state/actx/backups/` for 30 days. `actx restore` lists them and puts one back (the current
version is backed up first).

### Converting between tools

`actx cv` writes a copy in the tool you pick. Your messages and the replies are kept; tool calls
become text (what was run and what came out, outputs trimmed to 1500 characters); internal
reasoning and images are dropped. If the conversation was compacted, only the summary and what
followed are converted (that's what the agent had in context). It's an approximation, but enough to
keep working. In opencode, the copy uses the last model you used there; in Codex and Antigravity, your
default one. Antigravity has no import: actx writes the conversation into its database (the steps, the
index and the title), and `agy --conversation <id>` resumes it.

When Codex is installed, actx also registers imported and downloaded conversations through its
local app-server so they appear in the resume picker. This reads the saved thread without starting
an agent turn or making an inference request.

### The shared folder

```
<shared folder>/
  .actx                         marker
  .lock                         who is writing right now (machines take turns)
  <project>/
    claude/<session id>/        meta.json · preview.txt · session.jsonl.zst · extra.tar.zst
    opencode/<session id>/      meta.json · preview.txt · session.json.zst · child-*.json.zst
    codex/<session id>/         meta.json · preview.txt · rollout.jsonl.zst
  _general/<tool>/<session id>/  General conversations, same payload layout
  _bin/actx-<arch>-<os>         a copy of actx, for `actx update` on copies installed by hand
  _bin/actx                     for machines on the old Python version: moves them to the above
```

Anything that all your machines can reach works: an SMB/NFS mount, a Syncthing or Dropbox folder…
On a network share, machines take turns writing (the `.lock` file says who's at it; one left behind by
a run that died expires after 5 minutes), and a sync that waited skips anything pushed meanwhile. A
sync tool that isn't real-time can't offer that: avoid syncing the same project on two machines at the
same moment.

## Configuration

`~/.config/actx/config.json`, written by `actx init`:

```json
{"root": "/mnt/nas/actx", "lang": "en"}
```

| Variable | Meaning |
|---|---|
| `ACTX_ROOT` | Shared folder (overrides the config). |
| `ACTX_LANG` | `en` or `es` (default: from your locale). |
| `ACTX_SCAN` | Where to look for repos, `:`-separated (default: your home). |
| `ACTX_OPENCODE_ARGS` | Extra flags for opencode's export/import/delete, e.g. `--standalone` (2.x). |
| `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `XDG_*` | Respected, like Claude Code, Codex and opencode do. |

## Privacy

Conversations contain your prompts, the agent's replies and **command output**, which may include
secrets (API keys, tokens, `.env` contents). actx stores them as-is in the shared folder: keep it
private, and don't use a public sync service you wouldn't trust with your code.

## Limitations

- actx reads the **internal, undocumented formats** of Claude Code (`~/.claude/projects/*.jsonl`),
  opencode (its SQLite database and its export/import commands), Codex (`~/.codex/sessions` rollouts)
  and Antigravity CLI (`~/.gemini/antigravity-cli`: a SQLite database of protocol buffers per
  conversation). They can change without notice. Tested with Claude Code 2.1, opencode 1.19 and 2.0,
  Codex 0.160 and Antigravity CLI 1.4;
  the test suite runs against the real opencode and Codex (weekly too) to catch format changes early.
  If something breaks, please open an issue.
- Conversion between tools is lossy (see above). Syncing within the same tool is not, except between
  opencode 1.x and 2.x, which goes through the same conversion. Merging needs the same opencode version.
- opencode is driven through its CLI; while you're importing a session, don't have that same session
  open in opencode.
- Linux, macOS and Windows (all three tested in CI). On Windows, actx can't see which folder another
  program is running in, so while Codex or opencode is running at all, it doesn't pull over their
  conversations (Claude Code says exactly which one it has open, so it isn't affected). The hooks run
  in the shell Claude Code and Codex use there (Git Bash).
- Windows sync apps with on-demand files (Synology Drive, OneDrive…) leave files online-only, and
  actx fails with `os error 395`. Set the shared folder to "Always keep on this device" (or
  `attrib +P "<folder>" /S /D`).

## Development

```
cargo test            # opencode and Codex tests run if they are installed, and are skipped otherwise
```

The end-to-end tests build fake machines (each with its own home, Claude Code, opencode and Codex data)
that share one folder, and run the real binary on them: syncs, merges and conversions. They steer it with
`ACTX_HOST` (the machine's name), `ACTX_PICK` (what the pickers choose), `ACTX_ANSWER` (the reply to
every question), `ACTX_LOCK_WAIT` (seconds to wait for the shared folder's lock) and `ACTX_HOOK_WAIT`
(hooks wait for the sync they start). CI runs them on Linux and macOS, plus `cargo fmt --check` and
`cargo clippy -- -D warnings`.

actx 0.3 is a Rust rewrite of the 0.1–0.2 Python script. All versions read and write the same shared
folder and the same local state, so you can mix them while you upgrade your machines (0.4 adds the
lock, which older versions ignore).

## License

[MIT](LICENSE). actx is an independent project, not affiliated with Anthropic, OpenAI or the opencode team.
