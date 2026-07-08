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
direction. **All three releases shipped.**

- **v0.12 "Teardown"** — ✅ shipped. `workz reap`, auto-reap on `done`,
  extended `doctor` (stale `.git/index.lock` + live processes on orphaned
  ports). Stateful registry makes safe cleanup possible.
- **v0.13 "Warm deps"** — ✅ shipped. `clone` strategy with CoW reflink,
  auto-detected by `workz init`, fallback to full copy on FS without
  reflink support.
- **v0.14 "Services"** — ✅ shipped. Named service ports, docker postgres
  fallback, `--carry-from`, `workz env-diff`.

Next post-relaunch: distribution (cla-squad #260 reply, worktrunk docs PR,
Show HN draft, runpane coverage fix). See the plan in
[`launch/worktrunk-gap-plan.md`](launch/worktrunk-gap-plan.md) § Distribution.
