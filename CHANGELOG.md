# Changelog

All notable changes to workz are documented here. This project adheres to
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **worktrunk subcommand (`wt workz`).** A `wt-workz` extension (`examples/wt-workz`)
  that worktrunk dispatches as `wt workz`: bare it provisions the current worktree
  (`workz sync --isolated`), otherwise it forwards to `workz`. Drop it on your PATH.

- **Configurable worktree placement** (`[worktree] dir` in `.workz.toml`).
  A relative path resolves against the repo root (`dir = ".worktrees"` nests
  worktrees inside the project, Claude Code–style); an absolute path is used
  verbatim. Default behavior is unchanged: `../<repo>--<branch>` next to the
  main checkout. (#13)

- **Hook environment.** The `post_start` and `pre_done` hooks now receive the
  worktree context as environment variables: `WORKZ_BRANCH`, `WORKZ_SLUG`,
  `WORKZ_WORKTREE`, `WORKZ_REPO`, `WORKZ_ROOT`, plus
  `WORKZ_PORT`/`WORKZ_PORT_END`/`WORKZ_DB_NAME`/`WORKZ_COMPOSE_PROJECT` when the
  worktree is isolated. `post_start` additionally gets `WORKZ_FRAMEWORK` and now
  runs after the worktree is fully provisioned (deps synced and, with
  `--isolated`, `.env.local` written). A `post_start` hook that spins up a
  per-worktree database can now be mirrored by a `pre_done` hook that tears it
  down by the same name — neither has to re-derive the slug from `git branch`.
  (#12)

### Fixed

- **Same-named worktrees in different repos no longer collide** (data-loss-adjacent).
  The port registry was keyed by the bare branch slug, so `workz start feature-x
  --isolated` in two repos double-booked the same port range and database, and
  `workz done --cleanup-db` in one repo tore down the other's environment. The
  registry is now keyed **per repo** (`<repo>/<slug>`), so each repo gets its own
  allocation. New `[isolation] db_name` / `compose_project` name templates
  (placeholders `{slug}`, `{repo}`, `{branch}`; default `{slug}`) let you set
  `db_name = "{repo}_{slug}"` so the database/compose names are distinct across
  repos too. (#28)

- **The worktrunk hook recipe was wrong.** `workz hook worktrunk` (and the README)
  emitted `[hooks] create = "workz sync --isolated --quiet"`, but worktrunk has no
  `create` hook and hooks aren't nested under a `[hooks]` table. The correct recipe
  is a top-level `pre-start = "workz sync --isolated --quiet"` in `.config/wt.toml` —
  verified end-to-end against worktrunk 0.68 (it auto-provisions the new worktree on
  `wt switch --create`).

- **`workz clean --merged` matches worktree branches again.** `git branch --merged`
  prefixes a branch checked out in a linked worktree with `+ ` (git ≥ 2.23), which
  the parser never stripped — so the command, whose targets are *always* worktree
  branches, matched nothing and was a silent no-op. It now lists merged branches
  with `--format=%(refname:short)` (no prefix, no detached-HEAD line). (#22)

- **`workz done` completes cleanup when the worktree directory is already gone.**
  It used to bail with "worktree not found", skipping every teardown step — but
  agent hosts (e.g. Claude Code) delete worktree directories themselves at session
  end, orphaning the database, port allocation, branch, and git metadata. `done`
  now finishes from the registry: reaps/releases ports, drops the database (with
  `--cleanup-db`), prunes the stale git metadata, and honors `--delete-branch`.
  The database is now dropped *before* the allocation is released, since the
  db name lives in the registry entry. (#23)

- **A single invalid value in `.workz.toml` no longer silently discards the whole
  file.** `load_config` swallowed deserialization errors, so one typo'd strategy
  or wrong-typed value dropped the entire config — hooks stopped, overrides were
  ignored, with no warning. Parse errors are now propagated with the offending
  key, and `workz doctor` deserializes into the real `Config` (not just TOML
  syntax), so it reports the same failures `load_config` does. (#21)

- **`[isolation] base_port` in `.workz.toml` is now honored.** The value was
  parsed (and `workz init` even wrote it) but never reached the allocator, which
  only read `ports.json`. A project that sets `base_port = 4000` now actually
  allocates from 4000; the per-repo config value takes precedence over the
  machine-global registry default. (#24)

- **`workz hook claude` now emits a recipe that actually works.** Claude Code's
  `WorktreeCreate` hook *replaces* worktree creation — it runs in the main
  checkout, receives `{name}` on stdin, and must create the worktree and print
  its path as the only stdout. The old recipe ran `workz sync` (which provisions
  an *existing* directory, so no worktree was ever created) and the README
  variant passed `"$WORKTREE_PATH"`, which expands empty and made
  `workz sync --isolated ""` write a managed `.env.local` into the main checkout.
  The recipe is now a `workz start`-shaped create-and-print-path bridge, and
  `workz sync` rejects an empty-string path instead of falling through to the
  cwd. A native `workz claude-hook` subcommand is tracked in #19. (#18)

- **`workz start <branch> --ai` no longer corrupts the terminal.** Two stacked
  bugs: workz passed `--worktree` to Claude Code (making it create a *nested*
  worktree), and the shell wrapper captured workz's stdout, so an interactive
  agent's TUI escape sequences got replayed as shell commands. The `--worktree`
  flag is gone, and the shell wrapper now runs workz attached straight to the
  terminal — it reads the directory to `cd` into from a scratch file
  (`WORKZ_CD_FILE`) instead of scraping stdout — so interactive agents (Claude,
  Aider, Codex, Gemini) inherit the real tty by plain fd inheritance. This
  replaces an interim `/dev/tty` approach that crashed Claude Code on macOS,
  where Bun could not build a `WriteStream` over that fd. **Re-run
  `workz shell-init <shell>` (or re-source it) after upgrading.** (#11)

## [0.14.0] ("Services")

The runtime story for monorepos and per-worktree databases. Named service
ports, a docker fallback for the database, safe cross-worktree carries of
uncommitted work, and env-block drift detection.

### Added

- **Named service ports** (`[isolation.services]` in `.workz.toml`):
  ```toml
  [isolation]
  services = ["web", "api", "worker"]
  ```
  Each name gets one port from the allocated range, in order. The first
  named service doubles as the top-level `PORT` for backward compat; the
  rest get `PORT_<UPPERCASE_NAME>` (e.g. `PORT_API=3011`). The Redis
  default-port is suppressed when a named service has already claimed
  that slot. `workz status` shows the service map alongside the range.
  This is the monorepo answer worktrunk users currently hand-salt
  `hash_port` for — now declarative.

- **`workz start --carry-from <branch|main>`** — snapshot a source
  worktree's uncommitted state (tracked + untracked) and apply it in the
  new worktree. Uses `git stash create` (read-only — never mutates the
  source) plus a manual untracked-file copy, then drops the temp stash
  commit. Safe to use while an agent is running in the source worktree.
  Worktrunk users have been asking for this for months (issues #938 and
  #3276) — workz ships it.

- **Docker postgres fallback for `--create-db`.** When `createdb` isn't
  available (the common case on dev machines without a local Postgres
  install), `--create-db` now spins up a per-worktree
  `postgres:16-alpine` container named `workz-pg-<slug>` instead of
  failing. Container is started via `docker` (or `podman` if `docker`
  isn't installed), bound to `localhost:5432`, and torn down on
  `workz done --cleanup-db`. The DATABASE_URL derivation in
  `.env.local` works against it transparently.

- **`workz env-diff`** — show drift between `.env.local` managed blocks
  across worktrees. For each env var, prints "all aligned" when every
  worktree has the same value, or a per-worktree breakdown when they
  disagree. Catches the "is my `.env` even current?" question that's
  loud in HN commentary and has no tool answer.

## [0.13.0] ("Warm Deps")

The dep-sync story for any project whose `node_modules` is large enough that
"reinstall in every worktree" or "symlink and pray" is the actual cost. A new
`clone` strategy reflinks the dep directory into each worktree, so it behaves
like a full copy (isolated, agent-mutation-safe) but takes milliseconds and
shares storage with the main tree until first write.

### Added

- **New sync strategy: `clone` (CoW reflink).** In `.workz.toml`:
  ```toml
  [sync.overrides]
  node_modules = "clone"
  ```
  Auto-selects the right tool per platform (`cp --reflink=auto` on Linux,
  `cp -c` on macOS). On filesystems that don't support reflink (some btrfs
  setups disable it at the FS level, plain tmpfs, etc.) it falls back to a
  full `copy` and emits a warning — the user can then switch to `copy` or
  `symlink` to silence it.
- **`workz init` detects reflink support** and recommends `clone` when it's
  available. The probe (`probe_reflink_support`) writes a tiny temp file and
  checks whether the result shares an inode with the source.
- **`workz doctor` reports reflink support** alongside the other `[info]`
  lines. Useful for the "is clone actually going to work here?" question.
- **Overrides now apply to non-default entries.** Writing
  `cache = "clone"` in `[sync.overrides]` no longer requires also adding
  `cache` to `symlink_add` — common case for caches you didn't pre-list.
- **New `cloned` field in the sync report.** The CLI summary now prints
  `cloned (reflink) node_modules` distinctly from `copied`, the JSON output
  includes it, and the machine-readable report makes it possible to monitor
  "did the reflink actually happen or did we fall back?" in CI.

### Changed

- **`workz init`'s Node-detected prompt is now clone-aware.** When the FS
  supports reflink the default flips to `clone` (with `copy` offered as a
  secondary fallback if the user declines); when reflink is unsupported it
  falls back to the original copy/symlink choice.

## [0.12.0] ("The Teardown")

worktrees managed by workz are now navigable *and* removable — `workz reap`
plus the extended `workz done` and `workz doctor` give you a complete,
reliable teardown story that no other worktree tool ships.

### Added

- **`workz reap [branch]` — kill processes bound to ports workz allocated.**
  Uses the stateful port registry to know *exactly* which ports a worktree
  owns, so it never touches a process on a port workz doesn't track
  (the false-positive risk worktrunk cited when refusing the equivalent —
  issue #3365). Flags: `--all` (global cleanup of every allocation), `--yes`
  (skip the kill confirmation, for hooks), `--dry-run` (list what would be
  killed, no signal sent), `--force` (SIGKILL directly), `--json` (machine
  output). Backed by `lsof` — doctor now reports on lsof availability and
  degrades gracefully if it's missing.
- **`workz done` is now a one-flag teardown.** It always kills the
  worktree's allocated-port processes (skippable with `--no-reap`), then
  runs `docker compose down` (skippable with `--no-compose-down`, with
  `--compose-volumes` to also drop volumes), then drops the optional DB
  (`--cleanup-db`), then removes the worktree. One command = ports released
  + DB dropped + compose down + processes dead.
- **`workz doctor` flags two more failure modes.**
  - **Stale `.git/index.lock`** — left behind by crashed git processes or
    `rm -f`-happy agents. Anything older than 60s is reported; `--fix`
    removes it (after a sanity check that another git isn't actively using
    it). Detected across every worktree, not just the current one.
  - **Live processes on orphaned port ranges** — a worktree was deleted but
    its dev server is still listening. `--fix` reaps them, which is
    (deliberately) the same code path `workz reap` uses.
- **`workz doctor` now reports on `lsof` availability** so users know
  whether the reap path is actually armed.

### Changed

- **Default `workz done` is now destructive on the worktree's running
  processes.** Previously, `done` removed the worktree directory but left
  any dev server bound to the allocated port dangling (the process would
  keep running and the port would stay held until restart). Now reap runs
  first. If you actually want the old behavior, pass `--no-reap`.

## [0.11.0] ("The Wizard")

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

### Fixed
- **`workz switch` no longer panics without a terminal.** With a query it now jumps
  directly on an exact or unique match (zoxide-style — works in scripts and non-tty
  contexts); it only opens the fuzzy picker when selection is ambiguous, and prints a
  clear error instead of a skim backtrace when there's no terminal.

### Changed
- **`init` is now the setup wizard.** `workz init <shell>` (the old way to print shell
  integration) still works but prints a deprecation note pointing at
  `workz shell-init <shell>`.
- **MCP `workz_sync` retargeted.** It now accepts `isolated` and returns the same
  structured JSON as the CLI (`worktree`, `branch`, `symlinked`, `copied`, `installed`,
  `isolation`, `warnings`) so agents can allocate an isolated environment in one call.
- **New MCP tool `workz_doctor`** — agents can self-diagnose a broken worktree environment
  (dangling symlinks, orphaned ports, stale refs, unparseable config); read-only.

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
- **Port allocation now skips ports that are actually in use.** In addition to avoiding
  ranges tracked in `ports.json`, workz bind-checks a candidate's base port and moves to
  the next range if some other (non-workz) process already holds it.
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
