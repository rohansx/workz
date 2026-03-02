use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "workz",
    version,
    about = "Zoxide for Git worktrees — zero-config sync, fuzzy switching, AI-ready",
    after_help = "Add shell integration with: eval \"$(workz init zsh)\""
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
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
    },

    /// Sync symlinks, env files, and deps into the current worktree
    Sync,

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

    /// Run parallel AI agents across multiple worktrees
    Fleet {
        #[command(subcommand)]
        cmd: FleetCmd,
    },

    /// Start a local web dashboard at localhost:PORT
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value = "7777")]
        port: u16,

        /// Don't automatically open the browser
        #[arg(long)]
        no_open: bool,
    },

    /// Start an MCP server exposing workz tools to AI agents (stdio transport)
    Mcp,

    /// Print shell integration script
    Init {
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

#[derive(Subcommand)]
pub enum FleetCmd {
    /// Create worktrees and launch an AI agent for each task in parallel
    Start {
        /// Task description (repeat for multiple tasks: --task "..." --task "...")
        #[arg(long = "task", action = clap::ArgAction::Append)]
        tasks: Vec<String>,

        /// Load tasks from a file (one task per non-empty line)
        #[arg(long)]
        from: Option<std::path::PathBuf>,

        /// AI agent to launch for every task
        #[arg(long, default_value = "claude", value_enum)]
        agent: AiTool,

        /// Base branch to create all worktrees from
        #[arg(long)]
        base: Option<String>,
    },

    /// Show status of all fleet worktrees
    Status,

    /// Run a shell command in every fleet worktree in parallel
    Run {
        /// Command to execute (e.g. "cargo test")
        #[arg(required = true, trailing_var_arg = true)]
        cmd: Vec<String>,
    },

    /// Remove all fleet worktrees and clean up
    Done {
        /// Force removal even with uncommitted changes
        #[arg(short, long)]
        force: bool,
    },
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
