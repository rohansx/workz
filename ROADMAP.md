# workz roadmap

> Vision: evolve from "worktree manager" → **open-source AI development runtime for Linux (and Mac)**
> The layer between Git and AI agents that handles isolation, orchestration, and environment management.
> The open-source, cross-platform alternative to Conductor — with a zero-config environment engine nobody else has.

---

## the moat

Two features that no other tool (including Conductor) ships:

1. **Auto-symlink + zero-config deps** — `node_modules`, `target`, `.venv`, 27 dirs auto-detected and symlinked. No setup script. No `conductor.json`. Works out of the box for Node, Rust, Python, Go, Java.
2. **`--isolated` environment engine** — auto-assigns unique PORT, DB_NAME, COMPOSE_PROJECT_NAME, REDIS_URL per worktree. Every Conductor user writes 50–100 lines of bash for this. workz does it in one flag.

These two features together mean: spin up 5 isolated AI agents in parallel with a single command, each with their own port, database, and dependencies — zero configuration required.

---

## why this matters (the Conductor gap)

[Conductor.build](https://conductor.build) is gaining real traction as the GUI for parallel Claude Code agents. But:

- **Mac-only** (Apple Silicon required; Linux/Windows: "hopefully soon-ish, but not sure")
- **Closed source** — no Linux, no self-hosting, no customization
- **No environment engine** — users must write their own setup bash scripts per worktree
- **No dependency management** — no auto-symlink, no auto-install

Every Linux developer who wants to run parallel AI agents has no Conductor. workz is the answer:

| | Conductor | workz |
|---|---|---|
| Platform | Mac only | Linux, Mac (Windows planned) |
| Source | Closed | Open source (MIT) |
| Interface | GUI (Electron-style) | TUI (ratatui) + CLI |
| Dep management | Manual bash | Zero-config auto-detect |
| Port isolation | Manual bash | `--isolated` built-in |
| Docker isolation | Manual bash | `--isolated` built-in |
| DB isolation | Manual bash | `--isolated` built-in |
| Install | Download .dmg | `cargo install workz` |
| Binary size | ~200MB+ | ~5MB |
| Agent support | Claude Code | Claude, Aider, Codex, Gemini, Windsurf, any CLI |

---

## shipped

### v0.2
- [x] Skim fuzzy TUI (`workz switch`)
- [x] Global config (`~/.config/workz/config.toml`)
- [x] Smart project detection (Node/Rust/Python/Go/Java)
- [x] Auto-symlink 27 heavy dirs + auto-install from 8 lockfile types
- [x] Docker/Podman compose lifecycle (`--docker`)
- [x] AI agent launch (`--ai` for Claude/Cursor/VS Code)
- [x] Shell tab completions (zsh/bash/fish)
- [x] `workz sync` for existing worktrees
- [x] Lifecycle hooks (`post_start`, `pre_done`)

### v0.3
- [x] IDE config sync — auto-symlink `.vscode/`, `.idea/`, `.cursor/`, `.claude/`, `.zed/`
- [x] `workz status` — rich per-worktree dashboard (branch, dirty, last commit age)
- [x] `workz clean --merged` — auto-prune worktrees whose branches are merged
- [x] More AI launchers — Aider, Codex CLI, Gemini CLI, Windsurf

### v0.4
- [x] `workz mcp` — stdio MCP server exposing 6 tools for AI agents
- [x] `CLAUDE.md` — Claude Code auto-discovers workz tools
- [x] Published to crates.io, Homebrew, awesome-mcp-servers

### v0.5 — fleet mode
- [x] `workz fleet start --task "..." --task "..." --agent claude` — parallel agent launcher
- [x] `workz fleet status` — ratatui TUI live dashboard (worktree, task, agent PID, status)
- [x] `workz fleet run "cmd"` — run command across all fleet worktrees in parallel
- [x] `workz fleet merge` — interactive merge of completed worktrees
- [x] `workz fleet pr` — create PR per worktree via `gh`
- [x] `workz fleet done` — teardown everything
- [x] Task file support — `workz fleet start --from tasks.md`

### v0.6 — web dashboard
- [x] `workz serve` — local web dashboard at `localhost:7777`
- [x] Worktree cards — branch, dirty/clean, last commit, disk usage, Docker state
- [x] One-click actions — open in VS Code / Cursor, sync, remove

### v0.7 — environment isolation ← **THE MOAT** (shipped)
- [x] `--isolated` flag — auto-assign unique PORT, DB_NAME, REDIS_URL per worktree
- [x] Port registry — `~/.config/workz/ports.json` tracks allocations, avoids conflicts
- [x] Docker project naming — auto-set `COMPOSE_PROJECT_NAME` per worktree
- [x] DB name injection — auto-suffix `DB_NAME` with sanitized branch name in `.env.local`
- [x] Resource cleanup — `workz done` releases ports, optionally drops DB (`--cleanup-db`)
- [x] `workz status` shows PORT column for isolated worktrees

---

## upcoming

### v0.8 — open-source build tool (Conductor alternative for Linux)
This is the positioning shift: workz becomes the open-source, cross-platform answer to Conductor.
The narrative: *"Conductor for Linux. Zero-config. Single binary. Open source."*

- [ ] `workz swarm` — full orchestration CLI (replacement for `workz fleet`, cleaner UX)
  - `workz swarm start --task "..." --task "..."` — creates isolated worktrees + launches agents
  - `workz swarm status` — live TUI dashboard (enhanced fleet status)
  - `workz swarm diff` — inline diff viewer across all worktrees
  - `workz swarm merge` / `workz swarm pr` — review + ship completed work
  - `workz swarm done` — teardown + cleanup
- [ ] **Interactive TUI dashboard** (ratatui) — Conductor-style interface, terminal-native
  - Keyboard-driven: `[N]ew` `[D]iff` `[M]erge` `[P]R` `[A]rchive` `[Q]uit`
  - Agent log tail per worktree (live output)
  - PORT, DISK, STATUS columns for isolated worktrees
  - Single-key launch: highlight a task row → press `Enter` → spawns isolated agent
- [ ] `workz pr` — push + create GitHub PR from current worktree (standalone, no fleet required)
- [ ] Windows support — test on Windows WSL2, document setup

### v0.9 — GitHub integration
- [ ] `workz done --pr` — create PR + cleanup in one shot
- [ ] `workz review <PR#>` — fetch PR into worktree, sync deps, open editor
- [ ] `workz start --gh-issue 123` — auto-fetch title, create branch
- [ ] CI status in `workz status`
- [ ] Agent status — detect running AI agents by process, show in dashboard

### v1.0 — advanced
- [ ] Cross-worktree conflict pre-detection (`workz conflicts`)
- [ ] `workz clone` — bare-repo + worktree-first setup in one command
- [ ] Cross-worktree search (ripgrep across uncommitted changes)
- [ ] tmux/zellij integration (`--tmux` flag)
- [ ] Monorepo + sparse worktrees (`--scope packages/auth`)
- [ ] Team registry — SQLite showing who/what agent owns which branch
- [ ] workz.dev — landing page + full docs site

---

## go-to-market

| Phase | Action | Channel |
|-------|--------|---------|
| ✅ v0.3 | Blog: *"I replaced 50 lines of bash with `workz start --ai`"* | Dev.to, Reddit |
| ✅ v0.4 | Blog: *"AI agents that manage their own worktrees"* | r/ClaudeAI, r/cursor, HN |
| ✅ v0.5 | **HN Launch: "Show HN: workz — run 5 AI agents in parallel"** | Hacker News |
| ✅ v0.7 | Blog: *"How I replaced 100 lines of Conductor setup bash with one flag"* | Dev.to, r/commandline |
| v0.8 | **HN Launch: "Show HN: workz — open-source Conductor for Linux"** | Hacker News |
| v0.8 | Message Charlie (Conductor founder) — offer workz as setup engine | Direct outreach |
| v1.0 | Launch workz.dev — full landing page + Product Hunt | Product Hunt |

## competitive position

workz is the **only** tool with: zero-config + auto-detect + auto-symlink + auto-install + `--isolated` + Docker + AI + fuzzy TUI + shell integration + open source + Linux support.

**Nobody else has the environment engine.** Not Conductor (Mac only, no dep management). Not claude squad (tmux, no isolation). Not Crystal (desktop, no worktree automation). Not Floki (no environment layer).

Star gap vs competitors (claude squad 2k, worktrunk 800) is a **distribution problem**, not a product problem. The v0.8 launch ("open-source Conductor for Linux") is the unlock.
