# workz

**The environment engine for agent worktrees.** Your agent creates the worktree —
workz makes it *run*: dependencies linked, `.env` copied, a unique port range, its own
database and compose project. Zero config.

![workz CLI demo](demo.gif)

## The problem

Claude Code, Cursor, and Codex all create git worktrees for parallel work now. But a
fresh worktree is inert:

```bash
git worktree add ../my-feature feature/login
cd ../my-feature
# Where's .env? Gone.
# node_modules? Gone — reinstall and wait, or waste 2GB duplicating it.
# What port does the dev server use? The same one as every other worktree.
# The database? Shared — one agent's migration breaks the others.
```

Every one of those tools ships a *worktree-create hook* and tells you to write the
setup command yourself. **workz is that command.**

## The fix

```bash
workz sync --isolated
# node_modules symlinked, .env copied, PORT=3010-3019 + DB_NAME + COMPOSE_PROJECT_NAME
# written to .env.local — idempotent, safe to run again.
```

Use it standalone, or wire it into your agent's hook so every new worktree is runnable
the moment it exists.

## Use it as a hook

Let your editor/agent create worktrees natively; point its setup hook at `workz sync`.

**Cursor** — `.cursor/worktrees.json`:

```json
{ "setup-worktree": ["workz", "sync", "--isolated", "--quiet"] }
```

**worktrunk** (`wt`) — in your worktrunk config:

```toml
[hooks]
create = "workz sync --isolated --quiet"
```

**Claude Code** — add a `WorktreeCreate` hook in `.claude/settings.json` that runs:

```bash
workz sync --isolated --quiet "$WORKTREE_PATH"
```

**Anything else** — any tool that runs a command after creating a worktree:

```bash
workz sync --isolated --quiet <path>
```

`--json` makes the output machine-readable for scripting; `--quiet` keeps success
output silent (warnings still go to stderr).

## Use it standalone

```bash
workz start feature/login             # create worktree + auto-sync deps & env
workz start feature/auth --isolated   # + assign PORT range, DB_NAME, COMPOSE_PROJECT_NAME
workz start feature/api --docker      # + docker compose up
workz start feature/ui --ai           # + launch Claude Code (or --ai-tool cursor/aider/codex/…)
```

What `start` does:
1. Creates `../myrepo--feature-login` as a git worktree.
2. Symlinks `node_modules`, `target`, `.venv`, … (project-aware, never duplicated).
3. Copies `.env*` and other untracked config into the worktree.
4. With `--isolated`, allocates a PORT range + DB + compose project into `.env.local`.

## Install

```bash
# Homebrew (macOS / Linux)
brew tap rohansx/tap
brew install workz

# Cargo
cargo install workz
```

Build from source:

```bash
git clone https://github.com/rohansx/workz.git
cd workz && cargo install --path .
```

## Shell setup

Adds a `cd`-on-switch wrapper and tab completions:

```bash
# zsh (~/.zshrc) or bash (~/.bashrc)
eval "$(workz shell-init zsh)"

# fish (~/.config/fish/config.fish)
workz shell-init fish | source
```

## Commands

| Command | Does |
|---------|------|
| `workz` / `workz status` | Status of every worktree: branch, dirty state, size, port range |
| `workz start <branch>` | Create a worktree + sync (`--isolated`, `--docker`, `--ai`, `--base`, `--no-sync`) |
| `workz sync [path]` | Make a worktree runnable — the hook command (`--isolated`, `--json`, `--quiet`, `--no-install`) |
| `workz switch [query]` | Fuzzy-switch between worktrees (aliased `s`) |
| `workz list` | List worktrees with size and status (aliased `ls`) |
| `workz done [branch]` | Remove a worktree (`--force`, `--delete-branch`, `--cleanup-db`) |
| `workz clean` | Prune stale worktrees (`--merged` also removes merged branches) |
| `workz doctor` | Diagnose broken symlinks, orphaned ports, stale config (`--fix` repairs) |
| `workz mcp` | Run the MCP server (see below) |
| `workz shell-init <shell>` | Print shell integration for zsh/bash/fish |

## Environment isolation

`--isolated` gives each worktree its own port range, database, and compose project — no
collisions between parallel worktrees.

```bash
workz start feat/auth --isolated
# PORT=3010  PORT_END=3019  DB_NAME=feat_auth  COMPOSE_PROJECT_NAME=feat_auth

workz start feat/api --isolated
# PORT=3020  PORT_END=3029  DB_NAME=feat_api   COMPOSE_PROJECT_NAME=feat_api
```

Values are written into a **managed block** in `.env.local`, so isolation vars sit
alongside — and never overwrite — your own copied secrets:

```dotenv
API_KEY=your-real-secret          # preserved, untouched

# >>> workz managed — do not edit between these markers >>>
PORT=3010
PORT_END=3019
DB_NAME=feat_auth
DATABASE_URL=postgres://localhost/feat_auth
COMPOSE_PROJECT_NAME=feat_auth
REDIS_URL=redis://localhost:3011
# <<< workz managed <<<
```

Re-running `--isolated` only rewrites the managed block. Port ranges are tracked in
`~/.config/workz/ports.json` and released on `workz done`. workz also detects common
web frameworks and writes their port var (`VITE_PORT`, `SERVER_PORT`, `FLASK_RUN_PORT`,
`UVICORN_PORT`, …).

## What gets synced

**Symlinked** (project-type aware — only what's relevant is linked):

| Project | Directories |
|---------|------------|
| Node.js | `node_modules`, `.next`, `.nuxt`, `.svelte-kit`, `.turbo`, `.parcel-cache`, `.angular` |
| Rust | `target` |
| Python | `.venv`, `venv`, `__pycache__`, `.mypy_cache`, `.pytest_cache`, `.ruff_cache` |
| Go | `vendor` |
| Java/Kotlin | `.gradle`, `build` |
| General | `.direnv`, `.cache` |
| IDE | `.vscode`, `.idea`, `.cursor`, `.claude`, `.zed` |

**Copied**: `.env`, `.env.*`, `.envrc`, `.tool-versions`, `.node-version`, `.npmrc`,
`.yarnrc.yml`, `docker-compose.override.yml`, `.secrets*`, and more.

**Auto-installed** from the detected lockfile (`bun` / `pnpm` / `yarn` / `npm ci` /
`uv` / `poetry` / `pipenv` / `pip`). Skip with `--no-install`.

## Configuration

Two layers — project overrides global:

1. **Global** — `~/.config/workz/config.toml`
2. **Project** — `.workz.toml` in repo root

```toml
[sync]
# Extend the built-in defaults (recommended — keeps node_modules, target, .venv, …):
symlink_add = ["my-large-cache"]
copy_add    = ["config/local.settings.json"]
ignore_add  = ["logs", "tmp"]

# …or replace the defaults wholesale:
# symlink = ["node_modules", "target"]
# copy    = [".env*", ".envrc"]

# Per-directory strategy override — the escape hatch when symlinked
# node_modules breaks Vite / Vitest / a pnpm monorepo:
[sync.overrides]
node_modules = "copy"      # copy instead of symlink
".vscode"    = "ignore"    # skip entirely

[hooks]
post_start = "pnpm install --frozen-lockfile"
pre_done = "docker compose down"

[isolation]
port_range_size = 10   # ports per worktree (default: 10)
base_port = 3000       # first port (default: 3000)
```

`symlink`/`copy`/`ignore` **replace** the built-in defaults; the `*_add` variants
**extend** them. `[sync.overrides]` sets a per-entry strategy (`symlink` / `copy` /
`ignore`). Project `.workz.toml` overrides `~/.config/workz/config.toml`.

Zero config works out of the box for Node, Rust, Python, Go, and Java.

## MCP server

workz ships an MCP server so agents can manage worktrees themselves:

```bash
claude mcp add workz -- workz mcp
```

Exposes `workz_start`, `workz_sync`, `workz_list`, `workz_status`, `workz_done`, and
`workz_conflicts` (files modified in more than one worktree — merge conflicts before
they happen).

## Docker

```bash
workz start feature/api --docker   # worktree + docker compose up -d
workz done feature/api             # stops containers + removes worktree
```

Supports `docker compose` and `podman-compose`.

---

MIT OR Apache-2.0. Where workz is headed and why it's shaped this way: [`V2.md`](V2.md).
