# LGTM

A fast, keyboard-driven desktop app for reviewing GitHub pull requests on macOS. It diffs PRs from a local mirror (so huge PRs open instantly), lets you write and queue review comments, shows what changed since your last review, and can run AI review skills that leave findings you accept as draft comments.

Built with Tauri 2 (Rust) and React.

## Features

- **Home**: PRs waiting on your review and PRs you need to re-review, plus open PRs per project. Paste any PR link anywhere to open it.
- **Fast diffs**: unified or split view, syntax highlighting, virtualized rendering for PRs with thousands of files, generated files collapsed, per-file "Viewed" marks that reset when the file changes.
- **Reviewing**: comment on a line (gutter `+` or `c`), drag the `+` down to comment on a range, conventional labels (praise, nit, suggestion, question), suggestion blocks, replies, resolve threads. Drafts are saved locally and posted together as one review (Comment, Approve or Request changes).
- **Since your review**: after a force-push or new commits, see only the files whose changes differ from what you last reviewed.
- **Auto review**: run a review skill (`SKILL.md`) through an ACP agent such as Cursor, Claude Code, Gemini or Codex. The agent works in a separate read-only checkout; edits and shell commands are refused. Findings appear on the diff with Accept, Edit and Dismiss.
- **Projects**: add local clones to see their branch and dirty state, and check out a PR into the folder or a separate worktree.

## Requirements

- macOS (Apple Silicon or Intel)
- [git](https://git-scm.com)
- A GitHub login: the [GitHub CLI](https://cli.github.com) (`gh auth login`), or a `GH_TOKEN` / `GITHUB_TOKEN` environment variable. GitHub Enterprise hosts logged in through `gh` work too.
- For Auto review, optionally one ACP agent:

  | Agent | Needs |
  | --- | --- |
  | Cursor | `cursor-agent` on your `PATH`, logged in |
  | Claude Code | `claude` and `npx` |
  | Gemini | `gemini` |
  | Codex | `codex` and `npx` |

## Install on a new Mac

There are no prebuilt releases yet; build it once per machine.

1. Install the toolchain:

   ```sh
   xcode-select --install                                  # Apple's build tools
   curl https://sh.rustup.rs -sSf | sh                     # Rust (stable)
   brew install node pnpm gh                               # or install them however you like
   gh auth login
   ```

2. Build the app:

   ```sh
   git clone https://github.com/UtkarshRahim/LGTM.git
   cd LGTM
   pnpm install
   pnpm tauri build
   ```

3. Copy `target/release/bundle/macos/LGTM.app` to `/Applications`. A `.dmg` is also written to `target/release/bundle/dmg/`.

The app isn't signed, so the first launch may be blocked. Right-click LGTM in Finder and choose **Open**, or run:

```sh
xattr -dr com.apple.quarantine /Applications/LGTM.app
```

To update later: `git pull && pnpm install && pnpm tauri build`, then replace the app.

## Using it

Open a PR from Home, by pasting its URL, or with `⌘1`–`⌘9` for open tabs. Add a project with `⌘O` or by dropping its folder on the window.

### Keyboard

| Where | Key | Action |
| --- | --- | --- |
| Anywhere | `⌘1` / `⌘2`–`⌘9` | Home / open review tabs |
| | `⌘W` | Close the review tab |
| | `⌘O` | Add a project |
| | `⌘+` `⌘−` `⌘0` | Zoom |
| Home | `j` `k` / `↓` `↑`, `↵` | Move and open |
| | `/` | Search |
| Diff | `j` `k` | Next / previous file |
| | `n` `p` | Next / previous hunk |
| | `x` | Collapse / expand the file |
| | `v` | Mark the file viewed |
| | `s` | Unified / split layout |
| | `c` | Comment on the hovered line (`⇧c` extends a range) |
| | `⌘↵` | Check out the PR (in a project) |
| Comments | `⌘↵` / `Esc` | Save / cancel |
| Auto review | `⌘R` | Open the review palette (`↵` runs the pinned review) |
| | `]` `[` | Next / previous finding |
| | `a` | Accept the focused finding as a draft |
| | `d` then `1`–`4` | Dismiss with a reason |

Panels can be resized by dragging their edges; double-click an edge to reset it.

### Auto review

A review is an agent skill: a folder containing a `SKILL.md` with front matter and instructions.

```markdown
---
name: bug-hunt
description: Find correctness bugs and unhandled errors
---
Review the change for correctness bugs, unhandled errors and missing tests.
Report only the issues that matter most.
```

LGTM looks for skills in the reviewed repository and in your home folder:

- `<repo>/.claude/skills/`, `<repo>/.cursor/skills/`, `<repo>/.agents/skills/`
- `~/.claude/skills/`, `~/.cursor/skills/`, `~/.agents/skills/`

A repository skill hides a personal one with the same name. Press `⌘R`, pick a review (star one to pin it), choose the agent at the bottom of the palette, and press `↵`. LGTM checks out the PR head in its own folder, writes the full diff to `.prism/pr.diff` for the agent, and asks for findings as JSON. Findings on changed lines can be accepted as draft comments; ones outside the diff can be copied.

### Settings

Settings live in `~/Library/Application Support/com.utkarshrahim.lgtm/settings.toml`:

```toml
[ui]
diff_layout = "unified"        # or "split"

[github]
default_account = "github.com/your-login"

[skills]
dirs = ["~/.claude/skills", "~/.cursor/skills", "~/.agents/skills"]
agent = "cursor"               # cursor, claude, gemini or codex

[checkout]
when_dirty = "ask"             # or "worktree"
```

The same folder holds the local database (drafts, viewed files, review history) and the git mirrors of reviewed repos. Deleting `mirrors/` is safe; they're fetched again.

## Development

```sh
pnpm install
pnpm tauri dev                          # app with hot reload
cargo test --workspace                  # Rust tests (also regenerates src/bindings.ts)
pnpm typecheck
```

Layout:

| Path | What |
| --- | --- |
| `src/` | React UI |
| `src-tauri/` | Tauri app: commands, state, events |
| `crates/gh` | GitHub REST/GraphQL client and auth discovery |
| `crates/repo` | Git mirrors, diffs, checkouts |
| `crates/store` | SQLite storage and settings |
| `crates/skills` | Skill discovery, prompts and the findings format |
| `crates/agent` | Agent Client Protocol client with a read-only policy |
| `crates/syntax` | Syntax highlighting |
| `crates/env` | Login-shell environment capture |
| `harness/` | Browser harness that renders the UI against recorded fixtures |

Some tests talk to GitHub or run an agent and only run when asked, e.g. `PRISM_SKILL_PR=owner/repo#123 cargo test -p prism gate_skill -- --nocapture`.

## License

MIT
