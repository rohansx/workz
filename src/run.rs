//! `workz run` / `workz preview` — the runtime layer.
//!
//! workz allocates a port range, a database and a compose project, then stops:
//! you get a configured-but-dead worktree. `run` starts the worktree's dev
//! server on the port workz already assigned it, and `preview` shows which
//! worktrees are actually live and at which URL — the "click a link and see the
//! feature running" half of reviewing parallel agent work (see V3.md §4.1–4.2).
//!
//! Liveness is **observed**, never tracked: `preview` asks which processes are
//! listening on the ports workz allocated, so there is no PID file to go stale.
//! `--stop` reuses the reap path, which by construction only touches ports workz
//! owns — it can never kill a dev server it didn't start.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{config, git, isolation};

/// Where a worktree's `run` log lives. Alongside the port registry so all
/// workz state sits in one place.
pub fn log_path(repo: &str, branch: &str) -> Option<PathBuf> {
    let slug = isolation::branch_to_slug(branch);
    dirs::config_dir().map(|d| {
        d.join("workz")
            .join("logs")
            .join(format!("{}--{}.log", isolation::branch_to_slug(repo), slug))
    })
}

/// Auto-detect the dev-server command for a project. Mirrors `sync.rs`'s
/// package-manager detection so the command matches the lockfile that's there.
/// Returns `None` when nothing is recognizable — the user sets `[run] cmd`.
pub fn detect_dev_cmd(wt_path: &Path) -> Option<String> {
    // Node: use the package manager implied by the lockfile, and only if the
    // project actually defines a `dev` script.
    let pkg_json = wt_path.join("package.json");
    if pkg_json.exists() {
        let has_dev = std::fs::read_to_string(&pkg_json)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| v.get("scripts").and_then(|s| s.get("dev")).is_some())
            .unwrap_or(false);
        if has_dev {
            let pm = if wt_path.join("bun.lockb").exists() || wt_path.join("bun.lock").exists() {
                "bun"
            } else if wt_path.join("pnpm-lock.yaml").exists() {
                "pnpm"
            } else if wt_path.join("yarn.lock").exists() {
                "yarn"
            } else {
                "npm"
            };
            // npm needs `run`; bun/pnpm/yarn accept the bare script name but
            // `run` is valid for all of them, so keep it uniform.
            return Some(format!("{pm} run dev"));
        }
    }

    // Rails
    if wt_path.join("bin/rails").exists() {
        return Some("bin/rails server".to_string());
    }
    // Django
    if wt_path.join("manage.py").exists() {
        return Some("python manage.py runserver".to_string());
    }
    // Rust
    if wt_path.join("Cargo.toml").exists() {
        return Some("cargo run".to_string());
    }
    // Go
    if wt_path.join("go.mod").exists() {
        return Some("go run .".to_string());
    }

    None
}

/// Resolve the command to run: explicit `[run] cmd` wins, else auto-detection.
fn resolve_cmd(cfg: &config::RunConfig, wt_path: &Path) -> Result<String> {
    if let Some(cmd) = cfg.cmd.as_ref().filter(|c| !c.trim().is_empty()) {
        return Ok(cmd.clone());
    }
    detect_dev_cmd(wt_path).ok_or_else(|| {
        anyhow::anyhow!(
            "couldn't detect a dev command for {} — set it in .workz.toml:\n\n  [run]\n  cmd = \"npm run dev\"",
            wt_path.display()
        )
    })
}

/// The ports workz allocated to a branch, if any.
fn allocated_ports(repo: &str, branch: &str) -> Vec<u16> {
    match isolation::get_allocation(repo, branch) {
        Some(a) => (a.port..a.port.saturating_add(a.port_count)).collect(),
        None => Vec::new(),
    }
}

/// Processes listening on any port workz allocated to this branch.
fn live_listeners(repo: &str, branch: &str) -> Vec<(u16, isolation::ParsedListener)> {
    let mut out = Vec::new();
    for port in allocated_ports(repo, branch) {
        for l in isolation::listeners_on_port(port) {
            out.push((port, l));
        }
    }
    out
}

/// Start the dev server for `branch` (detached), logging to the state dir.
pub fn start(repo: &str, branch: &str, wt_path: &Path, cfg: &config::RunConfig) -> Result<()> {
    if !wt_path.exists() {
        bail!("worktree not found at {}", wt_path.display());
    }

    // Already running? Don't stack a second server on the same port.
    let live = live_listeners(repo, branch);
    if let Some((port, l)) = live.first() {
        println!(
            "already running: {} (pid {}) on port {port} — `workz run --stop {branch}` first",
            l.command, l.pid
        );
        return Ok(());
    }

    let cmd = resolve_cmd(cfg, wt_path)?;
    let log = log_path(repo, branch)
        .ok_or_else(|| anyhow::anyhow!("could not resolve a log directory"))?;
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(&log)?;
    let errfile = file.try_clone()?;

    // `setsid` puts the child in its own session so it survives the terminal
    // going away (a dev server that dies with your shell is useless to an agent
    // workflow). Fall back to a plain spawn where setsid isn't available.
    let use_setsid = Command::new("sh")
        .args(["-c", "command -v setsid"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let mut command = if use_setsid {
        let mut c = Command::new("setsid");
        c.args(["sh", "-c", &cmd]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", &cmd]);
        c
    };

    // Inject the worktree's managed vars (PORT, DB_NAME, DATABASE_URL,
    // COMPOSE_PROJECT_NAME, PORT_<SERVICE>…) into the child. Without this the
    // dev server would bind its framework default and every worktree would
    // collide on 3000 — the isolation workz allocated would be decorative.
    // Frameworks that read .env.local themselves (Next, Vite) just see the same
    // values twice; plain processes get them for the first time.
    let managed = isolation::read_managed_env(wt_path, branch);
    for (k, v) in &managed.vars {
        command.env(k, v);
    }

    let child = command
        .current_dir(wt_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(file))
        .stderr(std::process::Stdio::from(errfile))
        .spawn()?;

    let ports = allocated_ports(repo, branch);
    println!("started '{cmd}' in {}", wt_path.display());
    println!("  pid {} · log {}", child.id(), log.display());
    match ports.first() {
        Some(p) => println!("  once it binds: http://localhost:{p}  (workz preview)"),
        None => println!(
            "  note: no port allocation for this worktree — start it with `--isolated` for a dedicated port"
        ),
    }
    Ok(())
}

/// Stop whatever is listening on the branch's allocated ports. Reuses the reap
/// path, so only ports workz allocated are ever touched.
pub fn stop(repo: &str, branch: &str, force: bool) -> Result<()> {
    let live = live_listeners(repo, branch);
    if live.is_empty() {
        println!("nothing running for '{branch}'");
        return Ok(());
    }
    let report = isolation::reap_branch(repo, branch, force)?;
    if report.killed.is_empty() {
        println!("nothing to stop for '{branch}'");
    } else {
        for k in &report.killed {
            println!("  stopped {} (pid {}) on port {}", k.command, k.pid, k.port);
        }
    }
    Ok(())
}

/// Print the tail of a worktree's run log.
pub fn logs(repo: &str, branch: &str, lines: usize) -> Result<()> {
    let path = log_path(repo, branch)
        .ok_or_else(|| anyhow::anyhow!("could not resolve a log directory"))?;
    let content = std::fs::read_to_string(&path)
        .map_err(|_| anyhow::anyhow!("no log for '{branch}' yet — start it with `workz run {branch}`"))?;
    let all: Vec<&str> = content.lines().collect();
    let start = all.len().saturating_sub(lines);
    for line in &all[start..] {
        println!("{line}");
    }
    Ok(())
}

/// One row of `workz preview`.
pub struct PreviewRow {
    pub branch: String,
    pub path: PathBuf,
    pub port: Option<u16>,
    pub port_end: Option<u16>,
    pub live: bool,
    pub pid: Option<u32>,
    pub command: Option<String>,
}

impl PreviewRow {
    pub fn url(&self) -> Option<String> {
        self.port.map(|p| format!("http://localhost:{p}"))
    }
}

/// Build the preview table: every non-bare worktree, its allocated port range,
/// and whether something is actually listening on it.
pub fn collect(repo: &str) -> Result<Vec<PreviewRow>> {
    let worktrees = git::worktree_list()?;
    let mut rows = Vec::new();
    for wt in worktrees.into_iter().filter(|w| !w.is_bare) {
        let alloc = isolation::get_allocation(repo, &wt.branch);
        let (port, port_end) = match &alloc {
            Some(a) => (
                Some(a.port),
                Some(a.port + a.port_count.saturating_sub(1)),
            ),
            None => (None, None),
        };
        let listener = live_listeners(repo, &wt.branch).into_iter().next();
        rows.push(PreviewRow {
            branch: wt.branch,
            path: wt.path,
            port,
            port_end,
            live: listener.is_some(),
            pid: listener.as_ref().map(|(_, l)| l.pid),
            command: listener.as_ref().map(|(_, l)| l.command.clone()),
        });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("workz_run_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn detects_node_dev_script_with_the_lockfile_package_manager() {
        let d = tmp("node");
        std::fs::write(d.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        // No lockfile → npm.
        assert_eq!(detect_dev_cmd(&d).as_deref(), Some("npm run dev"));
        // The lockfile picks the package manager.
        std::fs::write(d.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect_dev_cmd(&d).as_deref(), Some("pnpm run dev"));
        std::fs::write(d.join("bun.lockb"), "").unwrap();
        assert_eq!(detect_dev_cmd(&d).as_deref(), Some("bun run dev"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn node_without_a_dev_script_is_not_detected() {
        // A package.json alone isn't a dev server — don't invent one.
        let d = tmp("nodev");
        std::fs::write(d.join("package.json"), r#"{"name":"x"}"#).unwrap();
        assert!(detect_dev_cmd(&d).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn detects_other_stacks_and_gives_up_cleanly() {
        let d = tmp("rust");
        std::fs::write(d.join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(detect_dev_cmd(&d).as_deref(), Some("cargo run"));
        let _ = std::fs::remove_dir_all(&d);

        let d = tmp("django");
        std::fs::write(d.join("manage.py"), "").unwrap();
        assert_eq!(detect_dev_cmd(&d).as_deref(), Some("python manage.py runserver"));
        let _ = std::fs::remove_dir_all(&d);

        // Nothing recognizable → None, so the caller can tell the user to set
        // `[run] cmd` instead of silently running the wrong thing.
        let d = tmp("empty");
        assert!(detect_dev_cmd(&d).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn explicit_cmd_beats_detection_and_blank_falls_back() {
        let d = tmp("override");
        std::fs::write(d.join("Cargo.toml"), "[package]").unwrap();

        let explicit = config::RunConfig { cmd: Some("just serve".into()) };
        assert_eq!(resolve_cmd(&explicit, &d).unwrap(), "just serve");

        // Whitespace-only is treated as unset, not as a command.
        let blank = config::RunConfig { cmd: Some("   ".into()) };
        assert_eq!(resolve_cmd(&blank, &d).unwrap(), "cargo run");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn log_path_is_repo_qualified() {
        // Two repos with the same branch name must not share a log file.
        let a = log_path("repo-a", "feat/x").unwrap();
        let b = log_path("repo-b", "feat/x").unwrap();
        assert_ne!(a, b);
        assert!(a.to_string_lossy().ends_with("repo_a--feat_x.log"));
    }
}
