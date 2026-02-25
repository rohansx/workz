mod cli;
mod config;
mod git;
mod sync;

use anyhow::{bail, Result};
use clap::Parser;
use cli::{AiTool, Commands, Shell};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use std::process::Command;

/// Sentinel prefix for shell integration — the wrapper function parses this to cd.
const CD_PREFIX: &str = "__workz_cd:";

fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    match cli.command {
        Commands::Start {
            branch,
            base,
            no_sync,
            ai,
            ai_tool,
        } => cmd_start(&branch, base.as_deref(), no_sync, ai, &ai_tool),
        Commands::List => cmd_list(),
        Commands::Switch { query } => cmd_switch(query.as_deref()),
        Commands::Done {
            branch,
            force,
            delete_branch,
        } => cmd_done(branch.as_deref(), force, delete_branch),
        Commands::Clean => cmd_clean(),
        Commands::Init { shell } => cmd_init(&shell),
    }
}

// ── start ──────────────────────────────────────────────────────────────

fn cmd_start(
    branch: &str,
    base: Option<&str>,
    no_sync: bool,
    ai: bool,
    ai_tool: &AiTool,
) -> Result<()> {
    let root = git::repo_root()?;
    let wt_path = git::worktree_path(&root, branch);

    if wt_path.exists() {
        println!("worktree already exists at {}", wt_path.display());
        println!("{}{}", CD_PREFIX, wt_path.display());
        return Ok(());
    }

    println!("creating worktree for branch '{}'", branch);

    git::worktree_add(&wt_path, branch, base)?;
    println!("  worktree created at {}", wt_path.display());

    if !no_sync {
        let config = config::load_config(&root)?;
        sync::sync_worktree(&root, &wt_path, &config.sync)?;

        // Run post_start hook if configured
        if let Some(hook) = &config.hooks.post_start {
            println!("  running post_start hook...");
            let status = Command::new("sh")
                .args(["-c", hook])
                .current_dir(&wt_path)
                .status()?;
            if !status.success() {
                eprintln!("  warning: post_start hook exited with {}", status);
            }
        }
    }

    if ai {
        launch_ai_tool(ai_tool, &wt_path)?;
    }

    println!("ready!");
    println!("{}{}", CD_PREFIX, wt_path.display());
    Ok(())
}

fn launch_ai_tool(tool: &AiTool, path: &std::path::Path) -> Result<()> {
    let path_str = path.to_str().unwrap_or(".");

    let (cmd, args): (&str, Vec<&str>) = match tool {
        AiTool::Claude => ("claude", vec!["--worktree"]),
        AiTool::Cursor => ("cursor", vec![path_str]),
        AiTool::Code => ("code", vec![path_str]),
    };

    // Check if the tool exists
    if which_exists(cmd) {
        println!("  launching {}...", tool);
        Command::new(cmd)
            .args(&args)
            .current_dir(path)
            .spawn()?;
    } else {
        eprintln!("  warning: '{}' not found in PATH, skipping", cmd);
    }

    Ok(())
}

fn which_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ── list ───────────────────────────────────────────────────────────────

fn cmd_list() -> Result<()> {
    let worktrees = git::worktree_list()?;

    if worktrees.is_empty() {
        println!("no worktrees found");
        return Ok(());
    }

    // Find the longest branch name for alignment
    let max_branch = worktrees
        .iter()
        .map(|w| w.branch.len())
        .max()
        .unwrap_or(0);

    for wt in &worktrees {
        let dirty = if !wt.is_bare && git::is_dirty(&wt.path).unwrap_or(false) {
            " [modified]"
        } else {
            ""
        };

        let label = if wt.is_bare { " (bare)" } else { "" };

        println!(
            "  {:<width$}  {}{}{}",
            wt.branch,
            wt.path.display(),
            label,
            dirty,
            width = max_branch,
        );
    }

    Ok(())
}

// ── switch ─────────────────────────────────────────────────────────────

fn cmd_switch(query: Option<&str>) -> Result<()> {
    let worktrees = git::worktree_list()?;

    // Filter out bare repos — you don't cd into those
    let candidates: Vec<_> = worktrees.iter().filter(|w| !w.is_bare).collect();

    if candidates.is_empty() {
        println!("no worktrees to switch to");
        return Ok(());
    }

    let selected_path = if let Some(q) = query {
        // Fuzzy match
        let matcher = SkimMatcherV2::default();
        let mut scored: Vec<_> = candidates
            .iter()
            .filter_map(|wt| {
                let text = format!("{} {}", wt.branch, wt.path.display());
                matcher.fuzzy_match(&text, q).map(|score| (score, wt))
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));

        match scored.first() {
            Some((_, wt)) => wt.path.clone(),
            None => bail!("no worktree matching '{}'", q),
        }
    } else if candidates.len() == 1 {
        candidates[0].path.clone()
    } else {
        // Interactive selection
        let items: Vec<String> = candidates
            .iter()
            .map(|wt| format!("{} -> {}", wt.branch, wt.path.display()))
            .collect();

        let selection =
            dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
                .with_prompt("switch to worktree")
                .items(&items)
                .default(0)
                .interact_opt()?;

        match selection {
            Some(i) => candidates[i].path.clone(),
            None => {
                println!("cancelled");
                return Ok(());
            }
        }
    };

    println!("{}{}", CD_PREFIX, selected_path.display());
    Ok(())
}

// ── done ───────────────────────────────────────────────────────────────

fn cmd_done(branch: Option<&str>, force: bool, delete_branch: bool) -> Result<()> {
    let root = git::repo_root()?;

    let (wt_path, branch_name) = if let Some(b) = branch {
        (git::worktree_path(&root, b), b.to_string())
    } else {
        // Use current directory as the worktree
        let cwd = std::env::current_dir()?;
        let branch_name = git::current_branch(&cwd)?;

        // Make sure we're not in the main worktree
        if cwd == root {
            bail!("you're in the main worktree — specify a branch name instead");
        }

        (cwd, branch_name)
    };

    if !wt_path.exists() {
        bail!("worktree not found at {}", wt_path.display());
    }

    // Warn about dirty state
    if !force && git::is_dirty(&wt_path).unwrap_or(false) {
        bail!("worktree has uncommitted changes — use --force to remove anyway");
    }

    // Run pre_done hook if configured
    let config = config::load_config(&root)?;
    if let Some(hook) = &config.hooks.pre_done {
        println!("  running pre_done hook...");
        let status = Command::new("sh")
            .args(["-c", hook])
            .current_dir(&wt_path)
            .status()?;
        if !status.success() {
            eprintln!("  warning: pre_done hook exited with {}", status);
        }
    }

    println!("removing worktree at {}", wt_path.display());
    git::worktree_remove(&wt_path, force)?;

    if delete_branch {
        println!("deleting branch '{}'", branch_name);
        git::branch_delete(&branch_name, force)?;
    }

    println!("done!");
    Ok(())
}

// ── clean ──────────────────────────────────────────────────────────────

fn cmd_clean() -> Result<()> {
    println!("pruning stale worktrees...");
    let output = git::worktree_prune()?;
    if output.is_empty() {
        println!("nothing to prune");
    } else {
        println!("{}", output);
    }
    println!("done!");
    Ok(())
}

// ── init ───────────────────────────────────────────────────────────────

fn cmd_init(shell: &Shell) -> Result<()> {
    let script = match shell {
        Shell::Zsh | Shell::Bash => SHELL_INIT_BASH,
        Shell::Fish => SHELL_INIT_FISH,
    };
    print!("{}", script);
    Ok(())
}

const SHELL_INIT_BASH: &str = r#"# workz shell integration
# Add to your .bashrc or .zshrc:
#   eval "$(workz init zsh)"

workz() {
    local result
    result=$(command workz "$@" 2>&1)
    local exit_code=$?

    local has_cd=false
    local cd_target=""
    while IFS= read -r line; do
        if [[ "$line" == __workz_cd:* ]]; then
            has_cd=true
            cd_target="${line#__workz_cd:}"
        else
            printf '%s\n' "$line"
        fi
    done <<< "$result"

    if [[ "$has_cd" == true ]]; then
        builtin cd "$cd_target" || return
    fi

    return $exit_code
}
"#;

const SHELL_INIT_FISH: &str = r#"# workz shell integration
# Add to your config.fish:
#   workz init fish | source

function workz
    set -l result (command workz $argv 2>&1)
    set -l exit_code $status

    for line in $result
        if string match -q '__workz_cd:*' $line
            set -l target (string replace '__workz_cd:' '' $line)
            builtin cd $target
        else
            echo $line
        end
    end

    return $exit_code
end
"#;
