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

        /// AI tool to launch (claude, cursor, code)
        #[arg(long, default_value = "claude", value_enum)]
        ai_tool: AiTool,
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

    /// Prune orphaned worktrees
    Clean,

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
}

impl std::fmt::Display for AiTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiTool::Claude => write!(f, "claude"),
            AiTool::Cursor => write!(f, "cursor"),
            AiTool::Code => write!(f, "code"),
        }
    }
}
