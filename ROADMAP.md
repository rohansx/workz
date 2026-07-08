# workz roadmap

> **This roadmap has moved.** As of v0.9.0, workz is refocused on one thing: being
> **the environment engine for agent worktrees** — the setup command that every
> tool's worktree-create hook calls.
>
> - **Direction & rationale** (what to build, what was cut, and why): [`V2.md`](V2.md)
> - **Shipped changes**: [`CHANGELOG.md`](CHANGELOG.md)
> - **Competitive gap analysis** (what to build next vs. worktrunk + others): [`launch/worktrunk-gap-plan.md`](launch/worktrunk-gap-plan.md)

The earlier plan to grow workz into a parallel-agent orchestrator ("swarm", a
"Conductor for Linux") has been **cancelled**. That category consolidated into native
features in Claude Code, Cursor, and Codex; workz's durable, differentiated layer is the
environment engine (`workz sync` + `--isolated`) that all of those tools punt on. See
`V2.md §1` for the full analysis.

## Post-relaunch feature plan (v0.12 → v0.14)

The "Teardown" (v0.12) → "Warm deps" (v0.13) → "Services" (v0.14) plan in
[`launch/worktrunk-gap-plan.md`](launch/worktrunk-gap-plan.md) is the post-relaunch
direction. Each release owns a defensible layer that no other worktree tool ships:

- **v0.12 "Teardown"** — guaranteed teardown you can trust (`workz reap` +
  extended `done` + extended `doctor`). The stateful port registry makes safe
  cleanup possible where stateless tools can't do it.
- **v0.13 "Warm deps"** — CoW reflink sync strategy (`cp --reflink=auto` on
  btrfs/XFS, `clonefile` on APFS). Kills three pains: Vite/pnpm symlink
  breakage, agents mutating deps and poisoning the main tree, and multi-minute
  installs.
- **v0.14 "Services"** — named service ports (PORT_WEB/PORT_API per worktree,
  the monorepo answer worktrunk users hand-salt `hash_port` for), a docker-run
  Postgres fallback for `--create-db`, `--carry-from` (snapshot uncommitted
  changes read-only via `git stash create`), and `workz env diff` for secrets drift.
