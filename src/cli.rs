use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "workz",
    version,
    about = "The environment engine for agent worktrees — zero-config dep sync + port/DB isolation",
    after_help = "Add shell integration with: eval \"$(workz shell-init zsh)\""
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Create a new worktree with automatic dependency syncing
    Start {
        /// Branch name (created if it doesn't exist)
        branch: String,

        /// Base branch to create from (defaults to current HEAD)
        #[arg(short, long)]
        base: Option<String>,

        /// Skip symlink and copy operations
        #[arg(long)]
        no_sync: bool,

        /// Launch an AI coding tool in the new worktree
        #[arg(short, long)]
        ai: bool,

        /// AI tool to launch
        #[arg(long, default_value = "claude", value_enum)]
        ai_tool: AiTool,

        /// Run docker/podman compose up in the new worktree
        #[arg(long)]
        docker: bool,

        /// Auto-assign PORT, DB_NAME, COMPOSE_PROJECT_NAME and write .env.local
        #[arg(long)]
        isolated: bool,

        /// With --isolated, actually create the Postgres database (runs createdb)
        #[arg(long)]
        create_db: bool,

        /// Clone the created database from this template (implies --create-db)
        #[arg(long, value_name = "DB")]
        from_db: Option<String>,

        /// Snapshot uncommitted changes from <source> and apply them in the
        /// new worktree. <source> is a branch name (its worktree's uncommitted
        /// state), or the literal "main" for the main worktree. Safe to use
        /// while an agent is running in the source — uses `git stash create`
        /// (read-only) and never mutates the source. (v0.14)
        #[arg(long, value_name = "SOURCE")]
        carry_from: Option<String>,
    },

    /// List all worktrees with status
    #[command(alias = "ls")]
    List,

    /// Fuzzy-switch to a worktree (zoxide-style)
    #[command(alias = "s")]
    Switch {
        /// Fuzzy search query
        query: Option<String>,
    },

    /// Remove a worktree and clean up
    Done {
        /// Branch name of worktree to remove (defaults to current)
        branch: Option<String>,

        /// Force removal even with uncommitted changes
        #[arg(short, long)]
        force: bool,

        /// Also delete the branch after removal
        #[arg(short, long)]
        delete_branch: bool,

        /// Drop the database created by --isolated
        #[arg(long)]
        cleanup_db: bool,

        /// Skip killing processes bound to the worktree's allocated ports
        #[arg(long)]
        no_reap: bool,

        /// Skip `docker compose down` even if a compose file is present
        #[arg(long)]
        no_compose_down: bool,

        /// Also drop compose volumes (`docker compose down -v`)
        #[arg(long)]
        compose_volumes: bool,
    },

    /// Sync symlinks, env files, and deps into a worktree (the hook other tools call)
    Sync {
        /// Worktree path to sync (defaults to the current directory)
        path: Option<std::path::PathBuf>,

        /// Also allocate an isolated PORT range, DB_NAME, COMPOSE_PROJECT_NAME
        #[arg(long)]
        isolated: bool,

        /// Emit a single JSON object describing what was synced
        #[arg(long)]
        json: bool,

        /// Suppress success output (warnings still go to stderr)
        #[arg(long)]
        quiet: bool,

        /// Skip dependency auto-install (symlink + copy only)
        #[arg(long)]
        no_install: bool,

        /// With --isolated, actually create the Postgres database (runs createdb)
        #[arg(long)]
        create_db: bool,

        /// Clone the created database from this template (implies --create-db)
        #[arg(long, value_name = "DB")]
        from_db: Option<String>,
    },

    /// Show rich status of all worktrees
    Status,

    /// Prune orphaned worktrees
    Clean {
        /// Also remove worktrees whose branches are already merged into base
        #[arg(long)]
        merged: bool,

        /// Base branch to check merged status against (defaults to main or master)
        #[arg(long)]
        base: Option<String>,
    },

    /// Kill processes bound to ports workz allocated to a worktree
    Reap {
        /// Branch to reap (defaults to the current worktree's branch)
        branch: Option<String>,

        /// Reap every port workz has ever allocated (global cleanup)
        #[arg(long)]
        all: bool,

        /// Skip confirmation prompt (for hooks / scripts)
        #[arg(short = 'y', long)]
        yes: bool,

        /// Show what would be killed without actually killing anything
        #[arg(long)]
        dry_run: bool,

        /// Skip SIGTERM, send SIGKILL immediately
        #[arg(short, long)]
        force: bool,

        /// Emit a JSON object describing what was killed
        #[arg(long)]
        json: bool,
    },

    /// Show files modified in more than one worktree (conflicts before merge)
    Conflicts,

    /// Show drift between .env.local managed blocks across worktrees
    EnvDiff {
        /// Only show worktrees whose branch matches this query
        #[arg(long, value_name = "BRANCH")]
        branch: Option<String>,
    },

    /// Diagnose broken symlinks, orphaned ports, and stale config
    Doctor {
        /// Apply safe repairs (release orphaned ports, remove dead symlinks, prune)
        #[arg(long)]
        fix: bool,
    },

    /// Print (or --install) the worktree-hook recipe for a host tool
    Hook {
        /// Host tool to wire workz into
        #[arg(value_enum)]
        host: HookHost,

        /// Write the config file for hosts that support it (never overwrites)
        #[arg(long)]
        install: bool,
    },

    /// Start an MCP server exposing workz tools to AI agents (stdio transport)
    Mcp,

    /// Claude Code `WorktreeCreate` hook: read the JSON payload on stdin, create
    /// (and provision) the worktree, and print ONLY its path to stdout. Wire it
    /// up with `workz hook claude`. Progress goes to stderr so stdout stays clean.
    ClaudeHook {
        /// Auto-assign PORT/DB_NAME/COMPOSE_PROJECT_NAME and write .env.local
        #[arg(long)]
        isolated: bool,

        /// With --isolated, actually create the Postgres database (runs createdb)
        #[arg(long)]
        create_db: bool,

        /// Clone the created database from this template (implies --create-db)
        #[arg(long, value_name = "DB")]
        from_db: Option<String>,

        /// Skip symlink/copy/install
        #[arg(long)]
        no_sync: bool,

        /// Base branch to create the worktree from (defaults to current HEAD)
        #[arg(short, long)]
        base: Option<String>,
    },

    /// Set up workz for this project (interactive wizard; -y for defaults)
    Init {
        /// Run non-interactively with detected defaults
        #[arg(short = 'y', long)]
        yes: bool,

        /// Deprecated: `workz init <shell>` — use `workz shell-init <shell>`
        #[arg(hide = true)]
        shell: Option<String>,
    },

    /// Print shell integration script (add: eval "$(workz shell-init zsh)")
    #[command(name = "shell-init")]
    ShellInit {
        /// Shell to generate integration for
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Clone, ValueEnum)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
}

/// Host tools workz can generate a worktree-create hook recipe for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HookHost {
    Claude,
    Cursor,
    Codex,
    Conductor,
    Worktrunk,
    Generic,
}

#[derive(Clone, ValueEnum)]
pub enum AiTool {
    Claude,
    Cursor,
    Code,
    Aider,
    Codex,
    Gemini,
    Windsurf,
}

impl std::fmt::Display for AiTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiTool::Claude => write!(f, "claude"),
            AiTool::Cursor => write!(f, "cursor"),
            AiTool::Code => write!(f, "code"),
            AiTool::Aider => write!(f, "aider"),
            AiTool::Codex => write!(f, "codex"),
            AiTool::Gemini => write!(f, "gemini"),
            AiTool::Windsurf => write!(f, "windsurf"),
        }
    }
}
