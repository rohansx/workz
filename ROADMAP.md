# workz roadmap

## v0.2 (shipped)
- [x] Skim fuzzy TUI for `workz switch`
- [x] Global config (`~/.config/workz/config.toml`)
- [x] Smart project detection (Node/Rust/Python/Go/Java)
- [x] Auto-install deps from 8 lockfile types
- [x] Docker/Podman compose lifecycle (`--docker`)
- [x] AI agent launch (`--ai` for Claude/Cursor/VS Code)
- [x] Shell tab completions (zsh/bash/fish)
- [x] `workz sync` for existing worktrees
- [x] Lifecycle hooks (`post_start`, `pre_done`)

## v0.3 — status & cleanup
- [ ] `workz status` — rich dashboard (branch, dirty/clean, last commit age, disk size, color-coded)
- [ ] `workz gc` — find worktrees whose branches are already merged, offer removal
- [ ] IDE config syncing — auto-copy `.vscode/`, `.idea/`, `.zed/` settings
- [ ] `workz config` — print resolved config (global + project merged) for debugging

## v0.4 — GitHub integration
- [ ] `workz pr` — auto-push + create PR with template from current worktree
- [ ] `workz done --pr` — create PR + cleanup in one shot
- [ ] `workz review <PR#>` — fetch PR into a worktree, sync deps, open editor
- [ ] CI status in `workz list` / `workz status` (green/red/pending via `gh` CLI)

## v0.5 — multi-agent orchestration
- [ ] `workz fan-out tasks.toml` — create N worktrees + launch an AI agent per task
- [ ] `workz agents` — dashboard of running agents, their worktrees, and status
- [ ] `workz agents wait` — block until all agents finish
- [ ] Prompt/context file injection (auto-generate `.workz-context.md` per worktree)
- [ ] Conflict pre-flight — detect overlapping file changes across agent worktrees

## v0.6 — advanced workflows
- [ ] `workz clone` — bare-repo setup in one command (clone + main worktree)
- [ ] Docker port isolation — auto-offset ports across worktrees
- [ ] Submodule worktree fix — auto-correct `.git` file paths
- [ ] `workz template` — reusable worktree profiles (`--template backend`)

## v0.7 — monorepo & ecosystem
- [ ] Sparse worktrees — `workz start feature/auth --scope packages/auth`
- [ ] Workspace-aware dep installation (pnpm/turborepo/nx)
- [ ] LSP cache sharing (rust-analyzer, tsserver)
- [ ] VS Code extension — sidebar with worktree list, one-click switching
