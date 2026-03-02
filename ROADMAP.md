# workz roadmap

> Vision: evolve from "worktree manager" → "AI development runtime"
> The layer between Git and AI agents that handles isolation, orchestration, and environment management.

---

## shipped

### v0.2 (current)
- [x] Skim fuzzy TUI (`workz switch`)
- [x] Global config (`~/.config/workz/config.toml`)
- [x] Smart project detection (Node/Rust/Python/Go/Java)
- [x] Auto-symlink 22 heavy dirs + auto-install from 8 lockfile types
- [x] Docker/Podman compose lifecycle (`--docker`)
- [x] AI agent launch (`--ai` for Claude/Cursor/VS Code)
- [x] Shell tab completions (zsh/bash/fish)
- [x] `workz sync` for existing worktrees
- [x] Lifecycle hooks (`post_start`, `pre_done`)

---

## upcoming

### v0.3 — quick wins (1-2 weeks)
Low effort, high signal. Ship fast to generate content for launch posts.

- [ ] IDE config sync — auto-copy `.vscode/`, `.idea/`, `.cursor/`, `.claude/` into new worktrees
- [ ] `workz status` — rich per-worktree dashboard (branch, dirty, last commit age, disk, Docker state)
- [ ] `workz clean --merged` — auto-prune worktrees whose branches are merged to base
- [ ] More AI launchers — add Aider, Codex CLI, Gemini CLI, Windsurf to `--ai-tool`
- [ ] Submodule support — auto `git submodule update --init --recursive` on start
- [ ] `workz config` — print resolved config for debugging

### v0.4 — MCP server (2-3 weeks) ← strategic priority #1
Makes workz discoverable and usable by AI agents themselves. First mover in the ecosystem.

- [ ] `workz mcp` — stdio MCP server exposing all workz operations as tools
  - `workz_start`, `workz_list`, `workz_switch`, `workz_sync`, `workz_status`, `workz_done`, `workz_conflict_check`
- [ ] `CLAUDE.md` / `SKILL.md` — Claude Code discovers workz without any setup
- [ ] Publish to MCP registry / Claude Code integrations page

### v0.5 — fleet mode (3-4 weeks) ← HN launch moment
Turns workz into an agent orchestration platform. The killer differentiator vs all competitors.

- [ ] `workz fleet start --task "..." --task "..." --agent claude` — parallel agent launcher
- [ ] `workz fleet status` — ratatui TUI dashboard (worktree, task, agent, status)
- [ ] `workz fleet merge` — interactive merge of completed worktrees
- [ ] `workz fleet pr` — create PR per worktree
- [ ] `workz fleet done` — teardown everything
- [ ] Task file support — `workz fleet start --from tasks.md`

### v0.6 — environment isolation (3-4 weeks)
The unsolved problem. Code isolation ≠ environment isolation.

- [ ] `--isolated` flag — auto-assign unique PORT, DB_NAME, REDIS_URL per worktree
- [ ] Port registry — `~/.config/workz/ports.json` tracks allocations, avoids conflicts
- [ ] Docker project naming — auto-set `COMPOSE_PROJECT_NAME` per worktree
- [ ] Resource cleanup — `workz done` releases ports, stops containers, optionally drops DB
- [ ] DB name injection — auto-suffix `DB_NAME` with sanitized branch name in .env

### v0.7 — GitHub integration
- [ ] `workz pr` — push + create PR from current worktree
- [ ] `workz done --pr` — create PR + cleanup in one shot
- [ ] `workz review <PR#>` — fetch PR into worktree, sync deps, open editor
- [ ] `workz start --gh-issue 123` — auto-fetch title, create branch
- [ ] CI status in `workz status`

### v1.0 — advanced
- [ ] Cross-worktree conflict pre-detection (`workz conflicts`)
- [ ] `workz clone` — bare-repo + worktree-first setup in one command
- [ ] Cross-worktree search (ripgrep across uncommitted changes)
- [ ] tmux/zellij integration (`--tmux` flag)
- [ ] Monorepo + sparse worktrees (`--scope packages/auth`)
- [ ] Team registry — SQLite showing who/what agent owns which branch

---

## go-to-market

| Phase | Action | Channel |
|-------|--------|---------|
| v0.3 | Blog: *"I replaced 50 lines of bash with workz start --ai"* | Dev.to, Reddit |
| v0.4 | Blog: *"AI agents that manage their own worktrees"* | r/ClaudeAI, r/cursor, HN |
| v0.5 | **HN Launch: "Show HN: workz — run 5 AI agents in parallel"** | Hacker News |
| v0.6 | Blog: *"The git worktree tools landscape in 2026"* | Dev.to, Reddit |

## competitive position

workz is the **only** tool with: zero-config + auto-detect + auto-symlink + auto-install + Docker + AI + fuzzy TUI + shell integration.

Star gap vs competitors (claude squad 2k, worktrunk 800) is a **distribution problem**, not a product problem.
