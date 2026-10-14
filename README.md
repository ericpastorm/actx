# actx

**Agent context, everywhere.** Keep your [Claude Code](https://claude.com/claude-code) and
[opencode](https://opencode.ai) conversations in sync across your machines, grouped into projects,
through any shared folder: a NAS, Syncthing, Dropbox…

[Leer en español](README.es.md)

You work on a desktop and a laptop. `git pull` brings the code, but the conversation with your
agent (everything it learned about the task) stays behind. actx brings it along:

```
desktop$ actx mk shop          # create a project, pick the conversations that belong to it
laptop$  actx sync             # pull them, and push whatever you continued here
laptop$  claude --resume …     # carry on where you left off
```

## Features

- **Projects you name, conversations you choose.** Only what you put in a project travels.
- **Path independent.** The repo can live at a different path on each machine (another clone, an
  IDE worktree…): actx finds it by its first commit and rewrites the paths inside the conversation.
- **Two-way sync with conflict handling.** It remembers the last sync, so it knows what to push,
  what to pull, and when both sides changed. Then you can **merge**: keep this machine's version
  plus a summary of what was done on the other, and the other version is kept as a separate branch.
- **Claude Code ⇄ opencode.** Copy a conversation from one tool to the other and keep going there.
- **Safe.** Backups before overwriting, atomic writes in the shared folder, and git warnings when the
  code is out of step (uncommitted changes, a different commit).
- **One file, no dependencies.** Python 3.12+, git and fzf. English and Spanish interface.

## Install

**Arch Linux (AUR):**

```
yay -S actx-git
```

**Anywhere else:** it's a single script.

```
curl -fsSLo ~/.local/bin/actx https://raw.githubusercontent.com/ericpastorm/actx/main/actx
chmod +x ~/.local/bin/actx
```

Requires Python 3.12+ (3.14+ recommended: conversations are compressed with zstd, gzip otherwise),
`git` and [`fzf`](https://github.com/junegunn/fzf). All your machines should use the same
compression, so either all on 3.14+ or all below.

## Quick start

On every machine, once:

```
actx init            # choose the shared folder (it suggests network mounts and sync folders)
```

Then:

| Command | What it does |
|---|---|
| `actx mk <project>` | Create a project (or open one) and pick conversations to save in it. Those that look like they belong (same folder, name in the title) come first. |
| `actx sync [project]` | Push what you continued here, pull what's new. Pick the projects, or `--all`. |
| `actx` / `actx ls [project]` | Projects and the state of each conversation: `↑ push`, `↓ pull`, `↓ new`, `⚠ conflict`, `✓ synced`. |
| `actx cv [project]` | Copy a conversation to the other tool (Claude Code ⇄ opencode). |
| `actx rm <project>` | Remove conversations from a project (nothing is deleted on your machines). |
| `actx update` | Install the newest actx found in the shared folder (for script installs). |

Pickers: type to filter, <kbd>Tab</kbd> marks several, <kbd>Ctrl</kbd>+<kbd>A</kbd> all, <kbd>Enter</kbd> confirms.

**Tip:** run `actx sync` inside the project (e.g. in your IDE's terminal): that project comes first,
and new conversations of that repo go straight there.

## How it works

### Where a conversation goes

When a conversation arrives on a machine for the first time, actx looks for:

1. a folder you picked before for that repo;
2. the same path (relative to your home);
3. the folder you're in, if it's the same repo;
4. any clone or worktree of the repo under your home (`ACTX_SCAN`), identified by its **first
   commit**, so it works whatever the path or folder name;
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

### Converting between tools

`actx cv` writes a copy in the other tool. Your messages and the replies are kept; tool calls
become text (what was run and what came out, outputs trimmed to 1500 characters); internal
reasoning and images are dropped. If the conversation was compacted, only the summary and what
followed are converted (that's what the agent had in context). It's an approximation, but enough to
keep working. In opencode, the copy uses the last model you used there.

### The shared folder

```
<shared folder>/
  .actx                         marker
  <project>/
    claude/<session id>/        meta.json · preview.txt · session.jsonl.zst · extra.tar.zst
    opencode/<session id>/      meta.json · preview.txt · session.json.zst · child-*.json.zst
  _bin/actx                     a copy of actx, for `actx update` on script installs
```

Anything that all your machines can reach works: an SMB/NFS mount, a Syncthing or Dropbox folder…
Avoid editing the same project on two machines at the same moment if your sync tool isn't
real-time.

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
| `ACTX_OPENCODE_ARGS` | Extra flags for `opencode session …`, e.g. `--standalone`. |
| `CLAUDE_CONFIG_DIR`, `XDG_*` | Respected, like Claude Code and opencode do. |

## Privacy

Conversations contain your prompts, the agent's replies and **command output**, which may include
secrets (API keys, tokens, `.env` contents). actx stores them as-is in the shared folder: keep it
private, and don't use a public sync service you wouldn't trust with your code.

## Limitations

- actx reads the **internal, undocumented formats** of Claude Code (`~/.claude/projects/*.jsonl`) and
  opencode (its SQLite database and `opencode session export/import`). They can change without notice.
  Tested with Claude Code 2.1 and opencode 2.0; the test suite runs against the real opencode CLI to
  catch format changes early. If something breaks, please open an issue.
- Conversion between tools is lossy (see above). Syncing within the same tool is not.
- opencode is driven through its CLI; while you're importing a session, don't have that same session
  open in opencode.
- Linux is the target. macOS will likely work; Windows isn't supported.

## Development

```
pip install pytest
pytest                # opencode tests run if opencode is installed, and are skipped otherwise
```

The tests build fake machines (each with its own home, Claude Code and opencode data) that share one
folder, and run real syncs, merges and conversions between them.

## License

[MIT](LICENSE). actx is an independent project, not affiliated with Anthropic or the opencode team.
