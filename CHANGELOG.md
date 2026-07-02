# Changelog

All notable changes to workz are documented here. This project adheres to
[Semantic Versioning](https://semver.org/).

## [0.11.0] — unreleased ("The Wizard")

### Added
- **`--create-db` / `--from-db`** on `workz start` and `workz sync`. With `--isolated`,
  workz can now actually create the per-worktree Postgres database (via `createdb`), and
  `--from-db <template>` clones it from an existing database. Pairs with the existing
  `workz done --cleanup-db`. Status goes to stderr so `--json` output stays clean.
- **`workz init`** — a short setup wizard. It detects the project (language, package
  manager, framework, docker-compose, monorepo), asks a few skippable questions, writes a
  minimal `.workz.toml` (only your deviations from the zero-config defaults), and can
  install the worktree hook for your agent tool in one step. `workz init -y` runs
  non-interactively with detected defaults.

### Changed
- **`init` is now the setup wizard.** `workz init <shell>` (the old way to print shell
  integration) still works but prints a deprecation note pointing at
  `workz shell-init <shell>`.
- **MCP `workz_sync` retargeted.** It now accepts `isolated` and returns the same
  structured JSON as the CLI (`worktree`, `branch`, `symlinked`, `copied`, `installed`,
  `isolation`, `warnings`) so agents can allocate an isolated environment in one call.

## [0.10.0] ("The Hook Release")

### Added
- **`workz conflicts`** — show files modified in more than one worktree (potential merge
  conflicts before they happen), promoted from MCP-only to a first-class CLI command.
- **`workz hook <host>`** — print (or `--install`) the worktree-create hook recipe that
  wires `workz sync --isolated` into a host tool: `claude`, `cursor`, `codex`,
  `conductor`, `worktrunk`, or `generic`. `--install` writes the dedicated config file
  for hosts that have one (e.g. Cursor's `.cursor/worktrees.json`) and never overwrites
  an existing file.
- **`workz doctor`** — diagnose the things that quietly break worktree setups:
  dangling symlinks inside worktrees, orphaned port allocations (worktree gone),
  stale worktree refs, and unparseable `.workz.toml` / global config. Exit code 1 when
  problems are found (CI-friendly). `workz doctor --fix` applies the safe repairs:
  release orphaned ports, remove dead symlinks, prune stale worktrees.
- **Config v2 — additive keys and per-directory strategies.**
  - `symlink_add` / `copy_add` / `ignore_add` extend the built-in defaults instead of
    replacing them (the bare `symlink` / `copy` / `ignore` keys still replace).
  - `[sync.overrides]` sets a per-entry strategy: `name = "symlink" | "copy" | "ignore"`.
    `node_modules = "copy"` physically copies the directory — the escape hatch for tools
    that break on symlinked node_modules (Vite / Vitest / pnpm monorepos).

### Changed
- **`DATABASE_URL` under `--isolated` is now derived from your existing one.** If your
  copied `.env`/`.env.local` already has a `DATABASE_URL`, workz keeps its driver, host,
  port, credentials, and query string and only swaps the database name
  (`…@rds.example.com:5432/prod` → `…@rds.example.com:5432/feat_x`). Falls back to
  `postgres://localhost/<db>` when there's no existing URL.

### Fixed
- **First filename character dropped in `modified_files`.** `git()` trimmed the porcelain
  output, stripping the leading space of the first status line so the first changed file
  showed as `hared.txt` instead of `shared.txt`. Affected `workz conflicts` and the MCP
  `workz_conflicts` tool. Now reads untrimmed stdout; covered by a regression test.
- **Global/project config merge.** Project `.workz.toml` now overrides global config
  per key. Previously, a project that customized any sync value silently discarded the
  global sync config (and vice-versa) due to an equality-to-default check.

## [0.9.0] ("The Strip")

workz is refocusing. Claude Code, Cursor, and Codex all create worktrees natively
now, and every one of them punts on the hard part: making a fresh worktree actually
*runnable* (dependencies, env files, ports, databases, compose projects). That layer
— `workz sync` and `--isolated` — is what workz is now built around. See `V2.md` for
the full rationale.

### Removed
- **`workz fleet`** — the parallel-agent orchestrator. It spawned agents without a
  prompt or a TTY and never actually drove them, and the orchestrator category has
  consolidated into native agent features (Claude Code agent teams, Cursor parallel
  agents). Use your agent's native parallel mode and put `workz sync --isolated` in
  its worktree-create hook instead.
- **`workz serve`** — the local web dashboard. Commoditized by native and free tools;
  removing it drops the `axum` + `tokio` dependency tree.
- **The 4-panel TUI dashboard** (bare `workz`). Running `workz` with no arguments now
  prints the status table (same as `workz status`). Drops `ratatui` + `crossterm`.

The binary is now ~2,200 lines lighter and builds without four heavy dependencies.

### Added
- **`workz sync` is now the hero command — the setup step other tools' worktree
  hooks call.** New flags:
  - `workz sync <path>` — sync any worktree by path (not just the current dir).
  - `--isolated` — allocate the PORT range / DB / compose project during sync, so a
    host hook can do everything in one call: `workz sync --isolated --quiet "$PATH"`.
  - `--json` — emit a single machine-readable object (`worktree`, `branch`,
    `symlinked`, `copied`, `installed`, `isolation`, `warnings`).
  - `--quiet` — suppress success output (warnings still go to stderr).
  - `--no-install` — symlink + copy only, skip dependency install.
  - Fully **idempotent**: re-running only fills what's missing.

### Fixed
- **`--isolated` no longer destroys the copied `.env.local`.** Isolation vars now
  live in a marked block and are merged in; user secrets outside the markers are
  preserved, and repeated runs are idempotent.

### Changed
- **`workz init <shell>` is now `workz shell-init <shell>`.** The old `init` name is
  kept as a hidden alias, so existing `eval "$(workz init zsh)"` lines keep working.
  `init` is reserved for the setup wizard landing in a later release.
- New tagline everywhere: *the environment engine for agent worktrees*.
- `sync_worktree` now returns a structured `SyncReport` instead of printing directly,
  so callers render human / `--json` / `--quiet` output consistently.

### Kept
- `start`, `sync`, `switch`, `list`, `status`, `done`, `clean`
- `--isolated` port / DB / compose-project allocation
- The `workz mcp` stdio server (6 tools)
- Shell integration + completions for zsh / bash / fish
