# Claude Code Editor

A native desktop GUI for browsing and editing Claude Code / Claude Desktop
session JSONL files. Written in Rust with [egui](https://github.com/emilk/egui).

Point it at your `~/.claude/projects` directory (auto-detected on first
launch) and it lists every project as a collapsible group, every session
inside sorted by last-message time. Pick a session and you get a rendered
transcript where you can edit any message (user, assistant, or attachment),
delete records with automatic `parentUuid` rewiring, insert new attachment
records, and copy message text to the clipboard.

## Features

- **Universal project browser.** Scans `~/.claude/projects`, groups
  conversations by project folder, remembers which groups you collapsed.
- **Full message editing.** Edit assistant text blocks, user messages, and
  attachment `rendered[i].content` — including embedded system tags such as
  `<system-reminder>` or `<command-name>`, which are highlighted as their own
  visual blocks in view mode.
- **Delete with chain repair.** Removing a message rewrites `parentUuid` on
  its direct children so the transcript chain stays connected — no orphaned
  siblings, no Claude Desktop weirdness on next open.
- **Insert attachments.** Every message has a `+` button that inserts a new
  attachment record right below it, with a fresh v4 UUID, correct
  `parentUuid`, and inherited `sessionId` / `gitBranch` / `cwd` / etc.
- **Windows-style text editing.** Ctrl+Backspace, Ctrl+Delete,
  Ctrl+Arrow (+Shift), and double-click word-select all follow Notepad-style
  rules where whitespace is the sole word separator (or, for double-click,
  alphanumerics form the word). egui's default class-boundary behaviour is
  bypassed.
- **Fine-grained undo.** Ctrl+Z steps back short bursts of typing rather
  than swallowing whole paragraphs.
- **Rewind detection.** Older siblings of the same first-user-message UUID
  are flagged as `rewind` and can be hidden.
- **State that persists.** Window position, size, maximised flag, and
  collapsed project list survive restarts (stored in
  `%APPDATA%\claude-session-editor\window.json` on Windows).

## Screenshots

<!-- Add screenshots here. Suggested paths: docs/screenshot-main.png -->

## Requirements

- Rust toolchain (stable). Install via [rustup](https://rustup.rs).
- Tested on Windows 11. Should build on Linux (Wayland/X11) and macOS via
  eframe's glow backend; no macOS-specific polish yet.

## Build & run

```bash
git clone https://github.com/Whitebrim/Claude-Code-Editor.git
cd Claude-Code-Editor
cargo build --release
./target/release/claude-code-editor
```

On first launch the app looks for `%USERPROFILE%\.claude\projects` (Windows)
or `~/.claude/projects` (Linux/macOS). Use **Change folder…** in the toolbar
to point it at a different location — either the root folder holding several
project subdirectories, or a single project folder directly.

## Usage cheatsheet

| Action                      | How                                          |
|-----------------------------|----------------------------------------------|
| Open a session              | Click the card in the left sidebar           |
| Collapse a project group    | Click the header row                         |
| Edit a message              | **Edit** on the card, **Save edit** to write |
| Copy message text           | **Copy** on the card                         |
| Delete a message            | **Delete** → **Confirm delete**              |
| Insert a new attachment     | **+** on the card                            |
| Show hidden bits            | Toolbar checkboxes: sidechain / tool / thinking / rewinds |
| Save current file           | Ctrl+S                                       |
| Search sessions             | Type in the search field above the list      |

Every write goes through the same JSONL flow: raw lines are rewritten
in-place, then reloaded from disk to keep the in-memory model honest. The
file's mtime is absorbed after each self-write so the scroll doesn't jump.

## File format assumptions

- Each session lives in one `.jsonl` file where each line is a JSON object
  with `type` ∈ `{"user", "assistant", "attachment", ...}`, plus at least
  `uuid`, `parentUuid`, `timestamp`, `sessionId`, `gitBranch`.
- Assistant messages carry `message.content` as a mixed array of `text`,
  `thinking`, `tool_use`, `tool_result` blocks.
- Attachment records carry text under `rendered[i].content` — that's where
  `<system-reminder>` blocks live.
- A sidecar `<uuid>.desktop-released.json` next to a `.jsonl` marks a
  released rewind/delete; the editor skips those.

## Not a Claude / Anthropic product

Unofficial community tool. Uses the on-disk session format written by
Claude Code / Claude Desktop but is not affiliated with, endorsed by, or
supported by Anthropic. Use at your own risk — always back up sessions you
care about before editing them.

## License

[MIT](LICENSE).
