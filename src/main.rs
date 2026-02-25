mod cli;
mod config;
mod git;
mod sync;

use anyhow::{bail, Result};
use clap::Parser;
use cli::{AiTool, Commands, Shell};
use skim::prelude::*;
use std::io::Cursor;
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
            docker,
        } => cmd_start(&branch, base.as_deref(), no_sync, ai, &ai_tool, docker),
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
    docker: bool,
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

    if docker {
        launch_docker(&wt_path)?;
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

fn launch_docker(path: &std::path::Path) -> Result<()> {
    // Check for compose file
    let has_compose = path.join("docker-compose.yml").exists()
        || path.join("docker-compose.yaml").exists()
        || path.join("compose.yml").exists()
        || path.join("compose.yaml").exists();

    if !has_compose {
        return Ok(());
    }

    // Prefer podman-compose, fall back to docker compose
    let (cmd, args): (&str, Vec<&str>) = if which_exists("podman-compose") {
        ("podman-compose", vec!["up", "-d"])
    } else if which_exists("docker") {
        ("docker", vec!["compose", "up", "-d"])
    } else {
        eprintln!("  warning: neither docker nor podman-compose found, skipping");
        return Ok(());
    };

    println!("  starting containers ({})...", cmd);
    let status = Command::new(cmd)
        .args(&args)
        .current_dir(path)
        .status()?;

    if !status.success() {
        eprintln!("  warning: {} compose up exited with {}", cmd, status);
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
        let size = if !wt.is_bare {
            format!(" ({})", human_size(dir_size_shallow(&wt.path)))
        } else {
            String::new()
        };

        println!(
            "  {:<width$}  {}{}{}{}",
            wt.branch,
            wt.path.display(),
            label,
            dirty,
            size,
            width = max_branch,
        );
    }

    Ok(())
}

fn dir_size_shallow(path: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if e.path().symlink_metadata().ok()?.file_type().is_symlink() {
                return Some(0);
            }
            Some(meta.len())
        })
        .sum()
}

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}M", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{}K", bytes / KB)
    } else {
        format!("{}B", bytes)
    }
}

// ── switch ─────────────────────────────────────────────────────────────

fn cmd_switch(query: Option<&str>) -> Result<()> {
    let worktrees = git::worktree_list()?;

    let candidates: Vec<_> = worktrees.iter().filter(|w| !w.is_bare).collect();

    if candidates.is_empty() {
        println!("no worktrees to switch to");
        return Ok(());
    }

    if candidates.len() == 1 {
        println!("{}{}", CD_PREFIX, candidates[0].path.display());
        return Ok(());
    }

    // Build display lines: "branch  ->  /path"
    let items: Vec<String> = candidates
        .iter()
        .map(|wt| format!("{}\t{}", wt.branch, wt.path.display()))
        .collect();

    let input = items.join("\n");

    let query_string = query.map(|s| s.to_string());
    let options = SkimOptionsBuilder::default()
        .height(Some("40%"))
        .multi(false)
        .reverse(true)
        .prompt(Some("switch> "))
        .query(query_string.as_deref())
        .build()
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(Cursor::new(input));

    let output = Skim::run_with(&options, Some(items));

    let selected = match output {
        Some(out) if !out.is_abort && !out.selected_items.is_empty() => {
            out.selected_items[0].output().to_string()
        }
        _ => {
            println!("cancelled");
            return Ok(());
        }
    };

    // Parse the path from "branch\t/path"
    let path = selected
        .split('\t')
        .nth(1)
        .unwrap_or(&selected)
        .trim();

    println!("{}{}", CD_PREFIX, path);
    Ok(())
}

// ── done ───────────────────────────────────────────────────────────────

fn cmd_done(branch: Option<&str>, force: bool, delete_branch: bool) -> Result<()> {
    let root = git::repo_root()?;

    let (wt_path, branch_name) = if let Some(b) = branch {
        (git::worktree_path(&root, b), b.to_string())
    } else {
        let cwd = std::env::current_dir()?;
        let branch_name = git::current_branch(&cwd)?;

        if cwd == root {
            bail!("you're in the main worktree — specify a branch name instead");
        }

        (cwd, branch_name)
    };

    if !wt_path.exists() {
        bail!("worktree not found at {}", wt_path.display());
    }

    if !force && git::is_dirty(&wt_path).unwrap_or(false) {
        bail!("worktree has uncommitted changes — use --force to remove anyway");
    }

    // Stop containers if docker-compose exists
    stop_docker(&wt_path);

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

fn stop_docker(path: &std::path::Path) {
    let has_compose = path.join("docker-compose.yml").exists()
        || path.join("docker-compose.yaml").exists()
        || path.join("compose.yml").exists()
        || path.join("compose.yaml").exists();

    if !has_compose {
        return;
    }

    let (cmd, args): (&str, Vec<&str>) = if which_exists("podman-compose") {
        ("podman-compose", vec!["down"])
    } else if which_exists("docker") {
        ("docker", vec!["compose", "down"])
    } else {
        return;
    };

    println!("  stopping containers...");
    let _ = Command::new(cmd)
        .args(&args)
        .current_dir(path)
        .status();
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

# Tab completions
_workz_branches() {
    git worktree list --porcelain 2>/dev/null | grep '^branch ' | sed 's|^branch refs/heads/||'
}

if [ -n "$ZSH_VERSION" ]; then
    _workz_completion() {
        local -a commands
        commands=(
            'start:Create a new worktree'
            'list:List all worktrees'
            'ls:List all worktrees'
            'switch:Fuzzy-switch to a worktree'
            's:Fuzzy-switch to a worktree'
            'done:Remove a worktree'
            'clean:Prune orphaned worktrees'
            'init:Print shell integration script'
        )

        if (( CURRENT == 2 )); then
            _describe 'command' commands
            return
        fi

        case "${words[2]}" in
            switch|s)
                local -a branches
                branches=(${(f)"$(_workz_branches)"})
                _describe 'worktree' branches
                ;;
            done)
                local -a branches
                branches=(${(f)"$(_workz_branches)"})
                compadd -- "${branches[@]}"
                ;;
            start)
                _arguments \
                    '1:branch:' \
                    '--base[Base branch]:branch:' \
                    '-b[Base branch]:branch:' \
                    '--no-sync[Skip sync operations]' \
                    '--ai[Launch AI coding tool]' \
                    '-a[Launch AI coding tool]' \
                    '--ai-tool[AI tool]:tool:(claude cursor code)' \
                    '--docker[Run docker compose up]'
                ;;
            init)
                compadd -- zsh bash fish
                ;;
        esac
    }
    compdef _workz_completion workz
else
    _workz_completion() {
        local cur prev
        cur="${COMP_WORDS[COMP_CWORD]}"
        prev="${COMP_WORDS[COMP_CWORD-1]}"

        if [[ ${COMP_CWORD} -eq 1 ]]; then
            COMPREPLY=($(compgen -W "start list ls switch s done clean init" -- "$cur"))
            return
        fi

        case "${COMP_WORDS[1]}" in
            switch|s)
                COMPREPLY=($(compgen -W "$(_workz_branches)" -- "$cur"))
                ;;
            done)
                COMPREPLY=($(compgen -W "$(_workz_branches)" -- "$cur"))
                ;;
            start)
                COMPREPLY=($(compgen -W "--base --no-sync --ai --ai-tool --docker" -- "$cur"))
                ;;
            init)
                COMPREPLY=($(compgen -W "zsh bash fish" -- "$cur"))
                ;;
        esac
    }
    complete -F _workz_completion workz
fi
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

# Tab completions
complete -c workz -e
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s done clean init" -a start -d "Create a new worktree"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s done clean init" -a list -d "List all worktrees"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s done clean init" -a switch -d "Fuzzy-switch to a worktree"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s done clean init" -a done -d "Remove a worktree"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s done clean init" -a clean -d "Prune orphaned worktrees"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s done clean init" -a init -d "Print shell integration script"
complete -c workz -n "__fish_seen_subcommand_from switch s" -a "(git worktree list --porcelain 2>/dev/null | string match -r '^branch refs/heads/(.+)' | string replace 'branch refs/heads/' '')"
complete -c workz -n "__fish_seen_subcommand_from done" -a "(git worktree list --porcelain 2>/dev/null | string match -r '^branch refs/heads/(.+)' | string replace 'branch refs/heads/' '')"
complete -c workz -n "__fish_seen_subcommand_from start" -l base -d "Base branch"
complete -c workz -n "__fish_seen_subcommand_from start" -l no-sync -d "Skip sync operations"
complete -c workz -n "__fish_seen_subcommand_from start" -s a -l ai -d "Launch AI coding tool"
complete -c workz -n "__fish_seen_subcommand_from start" -l ai-tool -a "claude cursor code" -d "AI tool to launch"
complete -c workz -n "__fish_seen_subcommand_from start" -l docker -d "Run docker compose up"
complete -c workz -n "__fish_seen_subcommand_from init" -a "zsh bash fish"
"#;
