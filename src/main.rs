mod cli;
mod config;
mod doctor;
mod git;
mod hook;
mod init;
mod isolation;
mod mcp;
mod sync;

use anyhow::{bail, Result};
use clap::Parser;
use cli::{AiTool, Commands, Shell};
use skim::prelude::*;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;

/// Sentinel prefix for shell integration — the wrapper function parses this to cd.
const CD_PREFIX: &str = "__workz_cd:";

/// Tell the shell wrapper where to `cd` after workz exits.
///
/// The wrapper sets `WORKZ_CD_FILE` to a scratch file and runs workz with its
/// stdio attached straight to the terminal — no output capture. We write the
/// target there. That's what lets `--ai` hand an interactive agent the real
/// terminal by plain inheritance: the old wrapper captured stdout to scan for
/// the `__workz_cd:` line, which forced us to feed agents a `/dev/tty` fd that
/// Bun (Claude Code) can't build a `WriteStream` over on macOS — the kqueue
/// crash in issue #11.
///
/// When `WORKZ_CD_FILE` is unset (workz run without the wrapper, or an older
/// wrapper that still captures stdout), fall back to printing the sentinel so
/// the directory change keeps working.
fn emit_cd(target: impl std::fmt::Display) {
    emit_cd_to(std::env::var("WORKZ_CD_FILE").ok().as_deref(), target);
}

/// Core of [`emit_cd`], split out so the file/sentinel decision is testable
/// without touching the process environment. Writes `target` to `cd_file` when
/// one is given and non-empty; otherwise (or if the write fails) prints the
/// `__workz_cd:` sentinel.
fn emit_cd_to(cd_file: Option<&str>, target: impl std::fmt::Display) {
    let target = target.to_string();
    if let Some(cd_file) = cd_file {
        if !cd_file.is_empty() && std::fs::write(cd_file, format!("{target}\n")).is_ok() {
            return;
        }
    }
    println!("{CD_PREFIX}{target}");
}

fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    // Bare `workz` prints the status table.
    let Some(command) = cli.command else {
        return cmd_status();
    };

    match command {
        Commands::Start {
            branch,
            base,
            no_sync,
            ai,
            ai_tool,
            docker,
            isolated,
            create_db,
            from_db,
            carry_from,
        } => cmd_start(
            &branch,
            base.as_deref(),
            no_sync,
            ai,
            &ai_tool,
            docker,
            isolated,
            create_db,
            from_db.as_deref(),
            carry_from.as_deref(),
        ),
        Commands::List => cmd_list(),
        Commands::Switch { query } => cmd_switch(query.as_deref()),
        Commands::Done {
            branch,
            force,
            delete_branch,
            cleanup_db,
            no_reap,
            no_compose_down,
            compose_volumes,
        } => cmd_done(
            branch.as_deref(),
            force,
            delete_branch,
            cleanup_db,
            no_reap,
            no_compose_down,
            compose_volumes,
        ),
        Commands::Sync {
            path,
            isolated,
            json,
            quiet,
            no_install,
            create_db,
            from_db,
        } => cmd_sync(
            path.as_deref(),
            isolated,
            json,
            quiet,
            no_install,
            create_db,
            from_db.as_deref(),
        ),
        Commands::Status => cmd_status(),
        Commands::Clean { merged, base } => cmd_clean(merged, base.as_deref()),
        Commands::Reap {
            branch,
            all,
            yes,
            dry_run,
            force,
            json,
        } => cmd_reap(branch.as_deref(), all, yes, dry_run, force, json),
        Commands::Conflicts => cmd_conflicts(),
        Commands::EnvDiff { branch } => cmd_env_diff(branch.as_deref()),
        Commands::Doctor { fix } => {
            if doctor::run(fix)? {
                Ok(())
            } else {
                std::process::exit(1);
            }
        }
        Commands::Hook { host, install } => hook::run(host, install),
        Commands::Init { yes, shell } => match shell {
            // Back-compat: `workz init <shell>` used to print shell integration.
            Some(s) => match s.as_str() {
                "zsh" | "bash" | "fish" => {
                    eprintln!(
                        "note: `workz init {s}` is deprecated — use `workz shell-init {s}`"
                    );
                    let shell = match s.as_str() {
                        "zsh" => Shell::Zsh,
                        "bash" => Shell::Bash,
                        _ => Shell::Fish,
                    };
                    cmd_init(&shell)
                }
                other => bail!(
                    "unknown argument '{other}' — run `workz init` for setup, or `workz shell-init <shell>`"
                ),
            },
            None => init::run(yes),
        },
        Commands::Mcp => mcp::run(),
        Commands::ShellInit { shell } => cmd_init(&shell),
    }
}

// ── start ──────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn cmd_start(
    branch: &str,
    base: Option<&str>,
    no_sync: bool,
    ai: bool,
    ai_tool: &AiTool,
    docker: bool,
    isolated: bool,
    create_db: bool,
    from_db: Option<&str>,
    carry_from: Option<&str>,
) -> Result<()> {
    let root = git::repo_root()?;
    let config = config::load_config(&root)?;
    let wt_path = git::worktree_path(&root, branch, config.worktree.dir.as_deref());

    if wt_path.exists() {
        println!("worktree already exists at {}", wt_path.display());
        emit_cd(wt_path.display());
        return Ok(());
    }

    println!("creating worktree for branch '{}'", branch);

    git::worktree_add(&wt_path, branch, base)?;
    println!("  worktree created at {}", wt_path.display());

    // --carry-from: snapshot the source's uncommitted state and apply it here.
    // Runs *before* sync/isolation so any new files the user brings along get
    // their normal sync treatment (env files get copied, deps get installed).
    if let Some(source_label) = carry_from {
        let source_path = resolve_carry_source(&root, source_label)?;
        match git::snapshot_uncommitted(&source_path)? {
            Some(snap) => {
                let tracked_msg = match &snap.tracked {
                    Some(h) => format!("tracked={}", &h[..8.min(h.len())]),
                    None => "no-tracked-changes".to_string(),
                };
                println!(
                    "  carrying uncommitted changes from {} ({} + {} untracked file(s))",
                    source_label,
                    tracked_msg,
                    snap.untracked.len()
                );
                match git::apply_carry(&wt_path, &snap) {
                    Ok(n) if n > 0 => println!("  applied carry-from snapshot ({n} untracked file(s))"),
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("  warning: could not apply carry-from snapshot: {e}");
                        eprintln!("  (source worktree is unchanged; the snapshot is still in the object store)");
                    }
                }
                if let Some(h) = &snap.tracked {
                    git::drop_stash(&wt_path, h);
                }
            }
            None => println!("  (source '{}' has nothing to carry)", source_label),
        }
    }

    let framework = if !no_sync {
        let report =
            sync::sync_worktree(&root, &wt_path, &config.sync, sync::SyncOptions::default())?;
        println!("{}", report.human_summary());
        for w in &report.warnings {
            eprintln!("  warning: {w}");
        }
        report.framework
    } else {
        sync::Framework::Unknown
    };

    let iso = if isolated {
        let iso = isolation::setup_isolation(
            branch,
            &wt_path,
            config.isolation.port_range_size,
            framework,
            &config.isolation.services,
        )?;
        println!("  isolated environment:");
        if !iso.services.is_empty() {
            // Named service ports (v0.14): each gets PORT_<NAME>=N
            for (name, port) in &iso.services {
                println!("    PORT_{}={}", name.to_uppercase(), port);
            }
            // The first service doubles as the top-level PORT for compat.
            if iso.port_count > 1 {
                println!("    PORT={}..{}            → .env.local", iso.port, iso.port_end);
            } else {
                println!("    PORT={}                 → .env.local", iso.port);
            }
        } else if iso.port_count > 1 {
            println!("    PORT={}..{}            → .env.local", iso.port, iso.port_end);
        } else {
            println!("    PORT={}                 → .env.local", iso.port);
        }
        println!("    DB_NAME={}", iso.db_name);
        println!("    COMPOSE_PROJECT_NAME={}", iso.compose_project);
        if framework != sync::Framework::Unknown {
            println!("    framework={:?}", framework);
        }
        if create_db || from_db.is_some() {
            isolation::create_database(&iso.db_name, from_db);
        }
        Some(iso)
    } else {
        if create_db || from_db.is_some() {
            eprintln!("  note: --create-db requires --isolated (skipping)");
        }
        None
    };

    // Run the post_start hook last, once the worktree is fully provisioned:
    // deps synced AND (when --isolated) .env.local written. This lets hooks read
    // the managed vars — and the WORKZ_* vars we export below give them the
    // worktree slug/branch/paths without re-deriving them by hand (issue #12).
    if let Some(hook) = &config.hooks.post_start {
        run_post_start_hook(hook, &root, &wt_path, branch, framework, iso.as_ref())?;
    }

    if docker {
        launch_docker(&wt_path)?;
    }

    if ai {
        launch_ai_tool(ai_tool, &wt_path)?;
    }

    println!("ready!");
    emit_cd(wt_path.display());
    Ok(())
}

/// Export the worktree context onto a hook `command` as environment variables.
///
/// The hook always sees `WORKZ_BRANCH`, `WORKZ_SLUG`, `WORKZ_WORKTREE`,
/// `WORKZ_REPO`, and `WORKZ_ROOT`. `WORKZ_FRAMEWORK` is set only when a
/// `framework` is known (it isn't at teardown, where nothing is synced). When
/// the worktree was `--isolated`, the allocated `WORKZ_PORT`, `WORKZ_PORT_END`,
/// `WORKZ_DB_NAME`, and `WORKZ_COMPOSE_PROJECT` are added too. This is what lets
/// a hook name a per-worktree database without re-deriving the slug from
/// `git branch` (issue #12).
fn export_hook_env(
    command: &mut Command,
    root: &Path,
    wt_path: &Path,
    branch: &str,
    framework: Option<sync::Framework>,
    iso: Option<&isolation::IsolationConfig>,
) {
    command
        .env("WORKZ_BRANCH", branch)
        .env("WORKZ_SLUG", isolation::branch_to_slug(branch))
        .env("WORKZ_WORKTREE", wt_path)
        .env("WORKZ_REPO", git::repo_name(root))
        .env("WORKZ_ROOT", root);

    if let Some(framework) = framework {
        command.env("WORKZ_FRAMEWORK", format!("{framework:?}").to_lowercase());
    }

    if let Some(iso) = iso {
        command
            .env("WORKZ_PORT", iso.port.to_string())
            .env("WORKZ_PORT_END", iso.port_end.to_string())
            .env("WORKZ_DB_NAME", &iso.db_name)
            .env("WORKZ_COMPOSE_PROJECT", &iso.compose_project);
    }
}

/// Run the configured `post_start` hook once the worktree is fully provisioned,
/// with the workz context exported via [`export_hook_env`].
fn run_post_start_hook(
    hook: &str,
    root: &Path,
    wt_path: &Path,
    branch: &str,
    framework: sync::Framework,
    iso: Option<&isolation::IsolationConfig>,
) -> Result<()> {
    println!("  running post_start hook...");

    let mut command = Command::new("sh");
    command.args(["-c", hook]).current_dir(wt_path);
    export_hook_env(&mut command, root, wt_path, branch, Some(framework), iso);

    let status = command.status()?;
    if !status.success() {
        eprintln!("  warning: post_start hook exited with {}", status);
    }
    Ok(())
}

fn launch_ai_tool(tool: &AiTool, path: &std::path::Path) -> Result<()> {
    let path_str = path.to_str().unwrap_or(".");

    // `interactive` marks terminal UIs (as opposed to GUI editors that fork and
    // return). workz already created and cd'd into the worktree, so the CLI
    // agents must NOT be told to make their own worktree — passing `--worktree`
    // to Claude Code made it spin up a nested worktree and corrupted the shell
    // (see issue #11). They just run in `path`.
    let (cmd, args, interactive): (&str, Vec<&str>, bool) = match tool {
        AiTool::Claude => ("claude", vec![], true),
        AiTool::Cursor => ("cursor", vec![path_str], false),
        AiTool::Code => ("code", vec![path_str], false),
        AiTool::Windsurf => ("windsurf", vec![path_str], false),
        AiTool::Aider => ("aider", vec![], true),
        AiTool::Codex => ("codex", vec![], true),
        AiTool::Gemini => ("gemini", vec![], true),
    };

    if !which_exists(cmd) {
        eprintln!("  warning: '{}' not found in PATH, skipping", cmd);
        return Ok(());
    }

    println!("  launching {}...", tool);
    let mut command = Command::new(cmd);
    command.args(&args).current_dir(path);

    if interactive {
        // Interactive agents inherit workz's own stdio, which is the terminal:
        // the shell wrapper runs workz *without* capturing its output (it reads
        // the target directory from `WORKZ_CD_FILE` instead), so fds 0/1/2 are
        // the real tty. We wait for the agent to exit before returning so the
        // `cd` is applied only after the session ends.
        //
        // The earlier fix handed the child a freshly-opened `/dev/tty` instead;
        // that broke Claude Code on macOS, where Bun can't build a `WriteStream`
        // over that fd (`EINVAL: kqueue`, `process.stdout.isTTY` undefined —
        // issue #11). Plain inheritance avoids the whole problem.
        command.status()?;
    } else {
        // GUI editors detach on their own; a background spawn is correct.
        command.spawn()?;
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

/// Resolve a `--carry-from` source label to a worktree path. The label is
/// either a branch name (we look up the worktree holding it) or the
/// literal "main" / "master" / "root" / "." for the main worktree.
fn resolve_carry_source(root: &Path, source: &str) -> Result<std::path::PathBuf> {
    match source {
        "main" | "master" | "root" | "." => {
            // The main worktree is the canonical root.
            Ok(root.to_path_buf())
        }
        _ => {
            // Look up the worktree holding this branch. We can't use
            // `git::worktree_path` here because that's the convention
            // `<root_parent>/<repo>--<branch>` — which is what workz uses
            // for *its* worktrees, but the source might be a worktree
            // created some other way.
            let worktrees = git::worktree_list()?;
            let found = worktrees
                .iter()
                .find(|w| w.branch == source && !w.is_bare)
                .map(|w| w.path.clone());
            match found {
                Some(p) => Ok(p),
                None => bail!(
                    "no worktree found for branch '{}' — pass a branch name with an active worktree, or 'main' for the main worktree",
                    source
                ),
            }
        }
    }
}

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

pub fn dir_size_shallow(path: &std::path::Path) -> u64 {
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

pub fn human_size(bytes: u64) -> String {
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
    use std::io::IsTerminal;

    let worktrees = git::worktree_list()?;

    let candidates: Vec<_> = worktrees.iter().filter(|w| !w.is_bare).collect();

    if candidates.is_empty() {
        println!("no worktrees to switch to");
        return Ok(());
    }

    if candidates.len() == 1 {
        emit_cd(candidates[0].path.display());
        return Ok(());
    }

    // With a query, try a direct jump first (zoxide-style — works in scripts,
    // no terminal required). Exact branch match wins; otherwise a unique
    // substring match.
    if let Some(q) = query {
        if let Some(w) = candidates.iter().find(|w| w.branch == q) {
            emit_cd(w.path.display());
            return Ok(());
        }
        let subs: Vec<_> = candidates
            .iter()
            .filter(|w| w.branch.contains(q) || w.path.to_string_lossy().contains(q))
            .collect();
        if subs.len() == 1 {
            emit_cd(subs[0].path.display());
            return Ok(());
        }
    }

    // Multiple candidates → interactive fuzzy select, which needs a terminal.
    if !std::io::stdin().is_terminal() {
        bail!(
            "`workz switch` needs a terminal for fuzzy selection — pass a query that uniquely matches a worktree (e.g. `workz switch {}`)",
            candidates[0].branch
        );
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

    emit_cd(path);
    Ok(())
}

// ── done ───────────────────────────────────────────────────────────────

fn cmd_done(
    branch: Option<&str>,
    force: bool,
    delete_branch: bool,
    cleanup_db: bool,
    no_reap: bool,
    no_compose_down: bool,
    compose_volumes: bool,
) -> Result<()> {
    let root = git::repo_root()?;
    let config = config::load_config(&root)?;

    let (wt_path, branch_name) = if let Some(b) = branch {
        (
            git::worktree_path(&root, b, config.worktree.dir.as_deref()),
            b.to_string(),
        )
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

    // Stop containers if docker-compose exists (skippable via --no-compose-down)
    if !no_compose_down {
        stop_docker(&wt_path, compose_volumes);
    }

    // Reap processes bound to the worktree's allocated ports (skippable via --no-reap).
    // The stateful registry makes this precise — we only touch ports workz allocated,
    // so a dev server on a port we don't own is never touched.
    if !no_reap {
        let report = isolation::reap_branch(&branch_name, force)?;
        print_reap_summary(&report);
    }

    // Capture the isolation allocation *before* releasing it, so the pre_done
    // hook can still read WORKZ_PORT/WORKZ_DB_NAME/etc. for the worktree it's
    // tearing down (issue #12).
    let iso = isolation::lookup_isolation(&branch_name);

    // Release isolated port allocation (no-op if not isolated)
    let _ = isolation::release_isolation(&branch_name);

    if cleanup_db {
        isolation::drop_database(&branch_name);
    }

    // Run pre_done hook if configured. It gets the same WORKZ_* context as
    // post_start (minus WORKZ_FRAMEWORK — nothing is synced at teardown) so a
    // hook that created a per-worktree database in post_start can drop it here
    // by the same name.
    if let Some(hook) = &config.hooks.pre_done {
        println!("  running pre_done hook...");
        let mut command = Command::new("sh");
        command.args(["-c", hook]).current_dir(&wt_path);
        export_hook_env(&mut command, &root, &wt_path, &branch_name, None, iso.as_ref());
        let status = command.status()?;
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

fn stop_docker(path: &std::path::Path, volumes: bool) {
    let has_compose = path.join("docker-compose.yml").exists()
        || path.join("docker-compose.yaml").exists()
        || path.join("compose.yml").exists()
        || path.join("compose.yaml").exists();

    if !has_compose {
        return;
    }

    let (cmd, base_args): (&str, Vec<&str>) = if which_exists("podman-compose") {
        ("podman-compose", vec!["down"])
    } else if which_exists("docker") {
        ("docker", vec!["compose", "down"])
    } else {
        return;
    };

    let mut args: Vec<String> = base_args.into_iter().map(String::from).collect();
    if volumes {
        args.push("-v".to_string());
    }

    println!("  stopping containers ({}{})...", cmd, if volumes { " -v" } else { "" });
    let _ = Command::new(cmd)
        .args(&args)
        .current_dir(path)
        .status();
}

// ── reap ───────────────────────────────────────────────────────────────

/// Build the sorted, de-duplicated list of port numbers we'd reap for the
/// given target (single branch or every allocation). Returns Ok(vec![]) when
/// no allocations exist — callers handle that as a no-op.
fn compute_target_ports(branch: Option<&str>, all: bool) -> Result<Vec<u16>> {
    let registry = isolation::load_registry();
    let mut out: Vec<u16> = Vec::new();
    if all {
        for a in registry.allocations.values() {
            for p in a.port..a.port.saturating_add(a.port_count) {
                out.push(p);
            }
        }
    } else if let Some(b) = branch {
        let slug = isolation::branch_to_slug(b);
        if let Some(a) = registry.allocations.get(&slug) {
            for p in a.port..a.port.saturating_add(a.port_count) {
                out.push(p);
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Render a ReapReport for humans (CLI). JSON path is handled by the caller.
fn print_reap_summary(report: &isolation::ReapReport) {
    if report.killed.is_empty() && report.errors.is_empty() {
        return; // silent: nothing to do, no need to spam the user
    }
    if !report.killed.is_empty() {
        println!("  reaped {} process(es):", report.killed.len());
        for k in &report.killed {
            println!("    pid {} ({}) on port {}", k.pid, k.command, k.port);
        }
    }
    for e in &report.errors {
        eprintln!("  warning: {e}");
    }
}

fn cmd_reap(
    branch: Option<&str>,
    all: bool,
    yes: bool,
    dry_run: bool,
    force: bool,
    json: bool,
) -> Result<()> {
    // Resolve the target branch. `--all` reaps every port workz has allocated.
    let target_branch: Option<String> = if all {
        None
    } else if let Some(b) = branch {
        Some(b.to_string())
    } else {
        // Default: reap the current worktree's branch (only if we're inside one).
        let cwd = std::env::current_dir()?;
        let root = git::repo_root()?;
        if cwd == root {
            bail!(
                "you're in the main worktree — pass a branch name or --all (or run from inside a worktree)"
            );
        }
        Some(git::current_branch(&cwd)?)
    };

    // Dry-run: list listeners without killing. Handed off to a side path that
    // uses the no-side-effect `listeners_on_port` scanner.
    if dry_run {
        return cmd_reap_dry_run(target_branch.as_deref(), all, json);
    }

    // Compute the target port list once so we can both preview it (for the
    // confirmation prompt) and pass it to the reap call without double-running.
    let target_ports = compute_target_ports(target_branch.as_deref(), all)?;

    // Interactive confirmation when there's something to kill, unless --yes.
    if !yes && !json && !target_ports.is_empty() {
        let preview_count: usize = target_ports
            .iter()
            .map(|p| isolation::listeners_on_port(*p).len())
            .sum();
        if preview_count == 0 {
            // Nothing actually listening — silent early return.
            return Ok(());
        }
        eprintln!(
            "about to kill up to {preview_count} process(es) on {} port(s) — re-run with --yes to skip this prompt",
            target_ports.len()
        );
    }

    let report = if all {
        isolation::reap_all(force)?
    } else if let Some(b) = &target_branch {
        isolation::reap_branch(b, force)?
    } else {
        isolation::ReapReport::default()
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    print_reap_summary(&report);
    Ok(())
}

/// --dry-run path: scan the target ports for live listeners and report, never
/// send a signal. Uses the public `listeners_on_port` scanner so there's zero
/// side effects — unlike `reap_*` which always terminates.
fn cmd_reap_dry_run(branch: Option<&str>, all: bool, json: bool) -> Result<()> {
    use serde::Serialize;

    #[derive(Serialize)]
    struct DryLine {
        branch: String,
        port: u16,
        pid: u32,
        command: String,
    }

    let registry = isolation::load_registry();

    let entries: Vec<(String, isolation::PortAllocation)> = if all {
        registry.allocations.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    } else if let Some(b) = branch {
        let slug = isolation::branch_to_slug(b);
        match registry.allocations.get(&slug).cloned() {
            Some(a) => vec![(slug, a)],
            None => {
                if !json {
                    println!("no port allocation for branch '{b}'");
                }
                return Ok(());
            }
        }
    } else {
        Vec::new()
    };

    let mut lines: Vec<DryLine> = Vec::new();
    for (slug, alloc) in entries {
        for port in alloc.port..alloc.port.saturating_add(alloc.port_count) {
            for l in isolation::listeners_on_port(port) {
                lines.push(DryLine {
                    branch: slug.clone(),
                    port,
                    pid: l.pid,
                    command: l.command,
                });
            }
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&lines)?);
        return Ok(());
    }
    if lines.is_empty() {
        println!("no live listeners on allocated ports");
        return Ok(());
    }
    println!("would kill (dry run):");
    for l in &lines {
        println!("  pid {} ({}) on port {} [{}]", l.pid, l.command, l.port, l.branch);
    }
    Ok(())
}

// ── sync ───────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn cmd_sync(
    path: Option<&std::path::Path>,
    isolated: bool,
    json: bool,
    quiet: bool,
    no_install: bool,
    create_db: bool,
    from_db: Option<&str>,
) -> Result<()> {
    let root = git::repo_root()?;
    let target = match path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let target = target.canonicalize().unwrap_or(target);

    if target == root {
        bail!("that's the main worktree — pass a worktree path or run from inside one");
    }

    let config = config::load_config(&root)?;
    let opts = sync::SyncOptions { no_install, quiet };
    let report = sync::sync_worktree(&root, &target, &config.sync, opts)?;

    // Resolve the branch (used for isolation + JSON output).
    let branch = git::current_branch(&target).unwrap_or_default();

    // Optionally allocate an isolated environment.
    let iso = if isolated {
        Some(isolation::setup_isolation(
            &branch,
            &target,
            config.isolation.port_range_size,
            report.framework,
            &config.isolation.services,
        )?)
    } else {
        None
    };

    // Optionally create the Postgres database (status → stderr, JSON-safe).
    match &iso {
        Some(i) if create_db || from_db.is_some() => {
            isolation::create_database(&i.db_name, from_db);
        }
        None if create_db || from_db.is_some() => {
            eprintln!("note: --create-db requires --isolated (skipping)");
        }
        _ => {}
    }

    if json {
        let iso_json = match &iso {
            Some(i) => {
                let services_obj: serde_json::Map<String, serde_json::Value> = i
                    .services
                    .iter()
                    .map(|(n, p)| (n.clone(), serde_json::json!(p)))
                    .collect();
                serde_json::json!({
                    "port": i.port,
                    "port_end": i.port_end,
                    "port_count": i.port_count,
                    "db_name": i.db_name,
                    "compose_project": i.compose_project,
                    "services": services_obj,
                    "env_file": ".env.local",
                })
            }
            None => serde_json::Value::Null,
        };
        let out = serde_json::json!({
            "worktree": target.to_string_lossy(),
            "branch": branch,
            "symlinked": report.symlinked,
            "copied": report.copied,
            "installed": report.installed,
            "isolation": iso_json,
            "warnings": report.warnings,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    if !quiet {
        println!("synced {}", target.display());
        println!("{}", report.human_summary());
        if let Some(i) = &iso {
            let range = if i.port_count > 1 {
                format!("{}-{}", i.port, i.port_end)
            } else {
                i.port.to_string()
            };
            let services_str = if i.services.is_empty() {
                String::new()
            } else {
                let pairs: Vec<String> = i
                    .services
                    .iter()
                    .map(|(n, p)| format!("PORT_{}={}", n.to_uppercase(), p))
                    .collect();
                format!(" [{}]", pairs.join(", "))
            };
            println!(
                "  isolated: PORT={}{} DB_NAME={} COMPOSE_PROJECT_NAME={} → .env.local",
                range, services_str, i.db_name, i.compose_project
            );
        }
    }

    // Warnings always surface on stderr, even under --quiet.
    for w in &report.warnings {
        eprintln!("  warning: {w}");
    }

    Ok(())
}

// ── status ─────────────────────────────────────────────────────────────

fn cmd_status() -> Result<()> {
    let worktrees = git::worktree_list()?;

    if worktrees.is_empty() {
        println!("no worktrees found");
        return Ok(());
    }

    let max_branch = worktrees.iter().map(|w| w.branch.len()).max().unwrap_or(0);

    for wt in &worktrees {
        if wt.is_bare {
            println!("  {:<width$}  {} (bare)", wt.branch, wt.path.display(), width = max_branch);
            continue;
        }

        let dirty = if git::is_dirty(&wt.path).unwrap_or(false) { " [modified]" } else { "" };
        let size = human_size(dir_size_shallow(&wt.path));
        let last = git::last_commit_relative(&wt.path)
            .map(|t| format!("  {}", t))
            .unwrap_or_default();

        let has_compose = wt.path.join("docker-compose.yml").exists()
            || wt.path.join("docker-compose.yaml").exists()
            || wt.path.join("compose.yml").exists()
            || wt.path.join("compose.yaml").exists();
        let docker = if has_compose { "  [docker]" } else { "" };

        let port_info = isolation::get_allocation(&wt.branch)
            .map(|a| {
                if a.port_count > 1 {
                    format!("  PORT:{}-{}", a.port, a.port + a.port_count - 1)
                } else {
                    format!("  PORT:{}", a.port)
                }
            })
            .unwrap_or_default();

        // Named services are shown alongside the range when the .workz.toml
        // in the main repo configured any. We look it up best-effort — if the
        // main repo's config can't be loaded we just skip the extra detail.
        let services_info = match git::repo_root().ok().and_then(|r| config::load_config(&r).ok()) {
            Some(cfg) if !cfg.isolation.services.is_empty() => {
                if let Some(alloc) = isolation::get_allocation(&wt.branch) {
                    let pairs: Vec<String> = cfg
                        .isolation
                        .services
                        .iter()
                        .enumerate()
                        .map(|(i, name)| format!("{}={}", name, alloc.port.saturating_add(i as u16)))
                        .collect();
                    if pairs.is_empty() {
                        String::new()
                    } else {
                        format!("  [{}]", pairs.join(", "))
                    }
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        };

        println!(
            "  {:<width$}  {}{}  {}{}{}{}{}",
            wt.branch,
            wt.path.display(),
            dirty,
            size,
            last,
            port_info,
            services_info,
            docker,
            width = max_branch,
        );
    }

    Ok(())
}

// ── conflicts ──────────────────────────────────────────────────────────

fn cmd_conflicts() -> Result<()> {
    let conflicts = git::find_conflicts()?;
    if conflicts.is_empty() {
        println!("no files modified in more than one worktree");
        return Ok(());
    }
    println!("files modified in multiple worktrees:");
    for (file, branches) in &conflicts {
        println!("  {}  —  {}", file, branches.join(", "));
    }
    Ok(())
}

// ── env diff ──────────────────────────────────────────────────────────

fn cmd_env_diff(branch_filter: Option<&str>) -> Result<()> {
    let worktrees = git::worktree_list()?;
    let envs: Vec<isolation::ManagedEnv> = worktrees
        .iter()
        .filter(|w| !w.is_bare)
        .filter(|w| branch_filter.is_none_or(|b| w.branch == b))
        .map(|w| isolation::read_managed_env(&w.path, &w.branch))
        .collect();

    let report = isolation::env_drift_report(&envs);
    for line in &report {
        println!("{line}");
    }
    Ok(())
}

// ── clean ──────────────────────────────────────────────────────────────

fn cmd_clean(merged: bool, base: Option<&str>) -> Result<()> {
    println!("pruning stale worktrees...");
    let output = git::worktree_prune()?;
    if output.is_empty() {
        println!("  nothing stale to prune");
    } else {
        println!("{}", output);
    }

    if merged {
        let base_branch = base
            .map(|s| s.to_string())
            .unwrap_or_else(git::default_branch);

        let merged_branches = git::merged_branches(&base_branch)?;
        let worktrees = git::worktree_list()?;

        let to_remove: Vec<_> = worktrees
            .iter()
            .filter(|wt| !wt.is_bare && merged_branches.contains(&wt.branch))
            .collect();

        if to_remove.is_empty() {
            println!("  no worktrees with merged branches found");
        } else {
            for wt in to_remove {
                println!("  removing merged worktree: {} ({})", wt.branch, wt.path.display());
                if let Err(e) = git::worktree_remove(&wt.path, false) {
                    eprintln!("  warning: could not remove {}: {}", wt.branch, e);
                }
            }
        }
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
#   eval "$(workz shell-init zsh)"

workz() {
    # workz writes the directory to cd into (if any) to this scratch file, and
    # runs attached straight to the terminal — no output capture. That's what
    # lets `workz start --ai` hand an interactive agent the real tty. (Capturing
    # stdout, as this wrapper used to, forced workz to feed agents a /dev/tty fd
    # that broke Claude Code on macOS.)
    local cd_file exit_code cd_target
    cd_file=$(mktemp "${TMPDIR:-/tmp}/workz_cd.XXXXXX" 2>/dev/null)

    WORKZ_CD_FILE="$cd_file" command workz "$@"
    exit_code=$?

    if [[ -n "$cd_file" ]]; then
        [[ -s "$cd_file" ]] && IFS= read -r cd_target < "$cd_file"
        rm -f "$cd_file"
    fi

    if [[ -n "$cd_target" ]]; then
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
            'sync:Sync symlinks, env files, and deps'
            'status:Show rich status of all worktrees'
            'done:Remove a worktree (kills allocated-port processes, compose down, optional DB drop)'
            'reap:Kill processes bound to ports workz allocated'
            'clean:Prune orphaned worktrees'
            'conflicts:Show files modified in multiple worktrees'
            'doctor:Diagnose broken symlinks, orphaned ports, stale locks, config'
            'hook:Print/install the worktree-hook recipe for a host'
            'init:Set up workz for this project'
            'mcp:Start the MCP server for AI agents'
            'shell-init:Print shell integration script'
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
            reap)
                local -a branches
                branches=(${(f)"$(_workz_branches)"})
                _arguments \
                    '1:branch:->branch' \
                    '--all[Reap every allocated port]' \
                    '-y[Skip confirmation]' \
                    '--yes[Skip confirmation]' \
                    '--dry-run[Show what would be killed]' \
                    '-f[SIGKILL directly]' \
                    '--force[SIGKILL directly]' \
                    '--json[Machine output]'
                if [[ ${state} == branch ]]; then
                    compadd -- "${branches[@]}"
                fi
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
                    '--docker[Run docker compose up]' \
                    '--isolated[Auto-assign PORT, DB_NAME, COMPOSE_PROJECT_NAME]'
                ;;
            clean)
                _arguments \
                    '--merged[Remove worktrees with merged branches]' \
                    '--base[Base branch]:branch:'
                ;;
            shell-init|init)
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
        COMPREPLY=($(compgen -W "start list ls switch s sync status done reap clean conflicts doctor hook init mcp shell-init" -- "$cur"))
                return
            fi

            case "${COMP_WORDS[1]}" in
                switch|s)
                    COMPREPLY=($(compgen -W "$(_workz_branches)" -- "$cur"))
                    ;;
                done)
                    COMPREPLY=($(compgen -W "$(_workz_branches) --no-reap --no-compose-down --compose-volumes --cleanup-db --force --delete-branch" -- "$cur"))
                    ;;
                reap)
                    COMPREPLY=($(compgen -W "$(_workz_branches) --all --yes --dry-run --force --json" -- "$cur"))
                    ;;
            start)
                COMPREPLY=($(compgen -W "--base --no-sync --ai --ai-tool --docker --isolated --create-db --from-db" -- "$cur"))
                if [[ "$prev" == "--ai-tool" ]]; then
                    COMPREPLY=($(compgen -W "claude cursor code aider codex gemini windsurf" -- "$cur"))
                fi
                ;;
            clean)
                COMPREPLY=($(compgen -W "--merged --base" -- "$cur"))
                ;;
            shell-init|init)
                COMPREPLY=($(compgen -W "zsh bash fish" -- "$cur"))
                ;;
        esac
    }
    complete -F _workz_completion workz
fi
"#;

const SHELL_INIT_FISH: &str = r#"# workz shell integration
# Add to your config.fish:
#   workz shell-init fish | source

function workz
    # workz writes the directory to cd into (if any) to this scratch file, and
    # runs attached straight to the terminal — no output capture. That's what
    # lets `workz start --ai` hand an interactive agent the real tty. (Capturing
    # stdout, as this wrapper used to, forced workz to feed agents a /dev/tty fd
    # that broke Claude Code on macOS.)
    set -l cd_file (mktemp (test -n "$TMPDIR"; and echo $TMPDIR; or echo /tmp)/workz_cd.XXXXXX)

    WORKZ_CD_FILE=$cd_file command workz $argv
    set -l exit_code $status

    if test -n "$cd_file"
        if test -s "$cd_file"
            builtin cd (cat $cd_file)
        end
        rm -f $cd_file
    end

    return $exit_code
end

# Tab completions
complete -c workz -e
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done reap clean conflicts doctor hook mcp shell-init init" -a start -d "Create a new worktree"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done reap clean conflicts doctor hook mcp shell-init init" -a list -d "List all worktrees"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done reap clean conflicts doctor hook mcp shell-init init" -a switch -d "Fuzzy-switch to a worktree"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done reap clean conflicts doctor hook mcp shell-init init" -a sync -d "Sync symlinks, env files, and deps"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done reap clean conflicts doctor hook mcp shell-init init" -a status -d "Show rich status of all worktrees"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done reap clean conflicts doctor hook mcp shell-init init" -a done -d "Remove a worktree (kills allocated-port processes, compose down, optional DB drop)"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done reap clean conflicts doctor hook mcp shell-init init" -a reap -d "Kill processes bound to ports workz allocated"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done reap clean conflicts doctor hook mcp shell-init init" -a clean -d "Prune orphaned worktrees"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done reap clean conflicts doctor hook mcp shell-init init" -a conflicts -d "Show files modified in multiple worktrees"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done reap clean conflicts doctor hook mcp shell-init init" -a doctor -d "Diagnose broken symlinks, orphaned ports, stale locks, config"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done reap clean conflicts doctor hook mcp shell-init init" -a hook -d "Print/install the worktree-hook recipe for a host"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done reap clean conflicts doctor hook mcp shell-init init" -a init -d "Set up workz for this project"
complete -c workz -n "__fish_seen_subcommand_from init" -s y -d "Run non-interactively with detected defaults"
complete -c workz -n "__fish_seen_subcommand_from hook" -a "claude cursor codex conductor worktrunk generic"
complete -c workz -n "__fish_seen_subcommand_from hook" -l install -d "Write the config file (never overwrites)"
complete -c workz -n "__fish_seen_subcommand_from doctor" -l fix -d "Apply safe repairs"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done clean conflicts doctor hook mcp shell-init init" -a mcp -d "Start the MCP server for AI agents"
complete -c workz -n "not __fish_seen_subcommand_from start list ls switch s sync status done clean conflicts doctor hook mcp shell-init init" -a shell-init -d "Print shell integration script"
complete -c workz -n "__fish_seen_subcommand_from switch s" -a "(git worktree list --porcelain 2>/dev/null | string match -r '^branch refs/heads/(.+)' | string replace 'branch refs/heads/' '')"
complete -c workz -n "__fish_seen_subcommand_from done" -a "(git worktree list --porcelain 2>/dev/null | string match -r '^branch refs/heads/(.+)' | string replace 'branch refs/heads/' '')"
complete -c workz -n "__fish_seen_subcommand_from start" -l base -d "Base branch"
complete -c workz -n "__fish_seen_subcommand_from start" -l no-sync -d "Skip sync operations"
complete -c workz -n "__fish_seen_subcommand_from start" -s a -l ai -d "Launch AI coding tool"
complete -c workz -n "__fish_seen_subcommand_from start" -l ai-tool -a "claude cursor code aider codex gemini windsurf" -d "AI tool to launch"
complete -c workz -n "__fish_seen_subcommand_from start" -l docker -d "Run docker compose up"
complete -c workz -n "__fish_seen_subcommand_from start" -l isolated -d "Auto-assign PORT, DB_NAME, COMPOSE_PROJECT_NAME"
complete -c workz -n "__fish_seen_subcommand_from start" -l create-db -d "Create the Postgres database (with --isolated)"
complete -c workz -n "__fish_seen_subcommand_from start" -l from-db -d "Clone the created database from this template"
complete -c workz -n "__fish_seen_subcommand_from done" -l cleanup-db -d "Drop the database created by --isolated"
complete -c workz -n "__fish_seen_subcommand_from done" -l no-reap -d "Skip killing processes bound to allocated ports"
complete -c workz -n "__fish_seen_subcommand_from done" -l no-compose-down -d "Skip `docker compose down`"
complete -c workz -n "__fish_seen_subcommand_from done" -l compose-volumes -d "Also drop compose volumes"
complete -c workz -n "__fish_seen_subcommand_from reap" -l all -d "Reap every port workz has ever allocated"
complete -c workz -n "__fish_seen_subcommand_from reap" -s y -l yes -d "Skip confirmation prompt"
complete -c workz -n "__fish_seen_subcommand_from reap" -l dry-run -d "Show what would be killed, no signal sent"
complete -c workz -n "__fish_seen_subcommand_from reap" -s f -l force -d "SIGKILL directly"
complete -c workz -n "__fish_seen_subcommand_from reap" -l json -d "Emit JSON"
complete -c workz -n "__fish_seen_subcommand_from clean" -l merged -d "Remove worktrees with merged branches"
complete -c workz -n "__fish_seen_subcommand_from clean" -l base -d "Base branch to check merged against"
complete -c workz -n "__fish_seen_subcommand_from shell-init init" -a "zsh bash fish"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_cd_writes_target_to_cd_file() {
        // When WORKZ_CD_FILE points somewhere, the target lands there (with a
        // trailing newline) and nothing is printed for the shell to scrape.
        let path = std::env::temp_dir().join("workz_emit_cd_test_file");
        let _ = std::fs::remove_file(&path);
        emit_cd_to(Some(path.to_str().unwrap()), "/some/worktree");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "/some/worktree\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn emit_cd_empty_cd_file_does_not_write() {
        // An empty WORKZ_CD_FILE means "no file" — fall back to the sentinel
        // (printed to stdout) rather than writing to a path named "".
        emit_cd_to(Some(""), "/x");
        assert!(!Path::new("").exists());
    }

    #[test]
    fn shell_wrappers_use_cd_file_not_stdout_capture() {
        // Regression guard for issue #11: the wrappers must run workz attached
        // to the terminal (via WORKZ_CD_FILE), not capture its stdout — capture
        // is what forced the /dev/tty fd that broke Claude Code on macOS.
        for wrapper in [SHELL_INIT_BASH, SHELL_INIT_FISH] {
            assert!(wrapper.contains("WORKZ_CD_FILE"));
            assert!(!wrapper.contains("$(command workz"));
            assert!(!wrapper.contains("(command workz $argv 2>&1)"));
        }
    }
}
