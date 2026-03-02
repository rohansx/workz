# workz MCP Integration

workz exposes all its operations as MCP tools so AI agents can manage worktrees directly.

## Setup

```bash
claude mcp add workz -- workz mcp
```

Or add manually to your Claude Code config (`~/.claude/settings.json`):

```json
{
  "mcpServers": {
    "workz": {
      "command": "workz",
      "args": ["mcp"]
    }
  }
}
```

## Available Tools

| Tool | Description |
|------|-------------|
| `workz_start` | Create a worktree with auto-synced deps and env |
| `workz_list` | List all worktrees as JSON |
| `workz_status` | Rich status: branch, dirty state, last commit |
| `workz_sync` | Sync symlinks/env into an existing worktree |
| `workz_done` | Remove a worktree (with optional force) |
| `workz_conflicts` | Detect files modified in multiple worktrees |

## Parallel Agent Workflow

To run parallel agents on independent tasks:

1. Use `workz_start` to create isolated worktrees for each task
2. Each agent gets its own branch, deps, env — no interference
3. Use `workz_conflicts` to check for overlapping file changes before merging
4. Use `workz_done` to clean up when finished
