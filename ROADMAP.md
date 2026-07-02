# workz roadmap

> **This roadmap has moved.** As of v0.9.0, workz is refocused on one thing: being
> **the environment engine for agent worktrees** — the setup command that every
> tool's worktree-create hook calls.
>
> - **Direction & rationale** (what to build, what was cut, and why): [`V2.md`](V2.md)
> - **Shipped changes**: [`CHANGELOG.md`](CHANGELOG.md)

The earlier plan to grow workz into a parallel-agent orchestrator ("swarm", a
"Conductor for Linux") has been **cancelled**. That category consolidated into native
features in Claude Code, Cursor, and Codex; workz's durable, differentiated layer is the
environment engine (`workz sync` + `--isolated`) that all of those tools punt on. See
`V2.md §1` for the full analysis.
