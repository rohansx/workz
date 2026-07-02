//! `workz doctor` — diagnose the things that quietly break worktree setups:
//! dangling symlinks, orphaned port allocations, stale worktree refs, and
//! unparseable config. With `--fix`, it applies the safe repairs.

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::{git, isolation};

/// Run all diagnostics and print them. Returns `true` if healthy (no problems).
/// With `fix`, the safe repairs (release orphaned ports, remove dead symlinks,
/// prune stale worktrees) are applied.
pub fn run(fix: bool) -> Result<bool> {
    let (lines, healthy) = diagnose(fix)?;
    for line in &lines {
        println!("{line}");
    }
    Ok(healthy)
}

/// Read-only diagnostics as a text report (for the MCP `workz_doctor` tool).
/// Never applies fixes.
pub fn report() -> Result<String> {
    let (lines, _) = diagnose(false)?;
    Ok(lines.join("\n"))
}

/// Collect all diagnostics into lines + a healthy flag.
fn diagnose(fix: bool) -> Result<(Vec<String>, bool)> {
    let mut out = Vec::new();
    out.push(format!("workz doctor{}", if fix { " (--fix)" } else { "" }));
    out.push(String::new());

    let root = git::repo_root()?;

    let mut problems = 0;
    problems += check_config(&root, &mut out);
    problems += check_orphaned_ports(fix, &mut out);
    problems += check_worktrees(&root, fix, &mut out);
    check_tooling(&mut out);

    out.push(String::new());
    if problems == 0 {
        out.push("[ok] all checks passed".to_string());
        Ok((out, true))
    } else {
        let hint = if fix { "" } else { " — re-run with --fix to repair" };
        out.push(format!("[fail] {problems} problem(s) found{hint}"));
        Ok((out, false))
    }
}

// ── config ───────────────────────────────────────────────────────────────────

/// Strictly parse project + global config (unlike `load_config`, which ignores
/// parse errors). Returns the number of problems.
fn check_config(root: &Path, out: &mut Vec<String>) -> u32 {
    let mut problems = 0;

    let project = root.join(".workz.toml");
    if project.exists() {
        match std::fs::read_to_string(&project)
            .map_err(|e| e.to_string())
            .and_then(|c| toml::from_str::<toml::Value>(&c).map_err(|e| e.to_string()))
        {
            Ok(_) => out.push("[ok] .workz.toml parses".to_string()),
            Err(e) => {
                out.push(format!("[fail] .workz.toml does not parse: {e}"));
                problems += 1;
            }
        }
    } else {
        out.push("[ok] no .workz.toml (using defaults)".to_string());
    }

    if let Some(global) = dirs::config_dir().map(|d| d.join("workz").join("config.toml")) {
        if global.exists() {
            match std::fs::read_to_string(&global)
                .map_err(|e| e.to_string())
                .and_then(|c| toml::from_str::<toml::Value>(&c).map_err(|e| e.to_string()))
            {
                Ok(_) => out.push("[ok] global config parses".to_string()),
                Err(e) => {
                    out.push(format!("[fail] global config does not parse: {e}"));
                    problems += 1;
                }
            }
        }
    }

    problems
}

// ── ports ────────────────────────────────────────────────────────────────────

fn check_orphaned_ports(fix: bool, out: &mut Vec<String>) -> u32 {
    let registry = isolation::load_registry();
    let orphans = isolation::orphaned_allocations(&registry, |p| Path::new(p).exists());

    if orphans.is_empty() {
        out.push("[ok] no orphaned port allocations".to_string());
        return 0;
    }

    if fix {
        match isolation::release_slugs(&orphans) {
            Ok(n) => {
                out.push(format!("[ok] released {n} orphaned port allocation(s): {}", orphans.join(", ")));
                0
            }
            Err(e) => {
                out.push(format!("[fail] could not release orphaned allocations: {e}"));
                1
            }
        }
    } else {
        out.push(format!(
            "[warn] {} orphaned port allocation(s) (worktree gone): {}",
            orphans.len(),
            orphans.join(", ")
        ));
        1
    }
}

// ── worktrees: broken symlinks + stale refs ──────────────────────────────────

fn check_worktrees(root: &Path, fix: bool, out: &mut Vec<String>) -> u32 {
    let mut problems = 0;

    let worktrees = match git::worktree_list() {
        Ok(w) => w,
        Err(e) => {
            out.push(format!("[fail] could not list worktrees: {e}"));
            return 1;
        }
    };

    // Stale refs: registered worktree paths that no longer exist on disk.
    let stale: Vec<_> = worktrees
        .iter()
        .filter(|w| !w.is_bare && !w.path.exists())
        .collect();
    if stale.is_empty() {
        out.push("[ok] no stale worktree refs".to_string());
    } else if fix {
        match git::worktree_prune() {
            Ok(_) => out.push(format!("[ok] pruned {} stale worktree ref(s)", stale.len())),
            Err(e) => {
                out.push(format!("[fail] could not prune worktrees: {e}"));
                problems += 1;
            }
        }
    } else {
        out.push(format!("[warn] {} stale worktree ref(s) — run: workz clean", stale.len()));
        problems += 1;
    }

    // Broken symlinks inside each live worktree.
    let mut broken_total = 0;
    for wt in worktrees.iter().filter(|w| !w.is_bare && w.path.exists() && w.path != *root) {
        let broken = broken_symlinks(&wt.path);
        for link in &broken {
            broken_total += 1;
            if fix {
                match std::fs::remove_file(link) {
                    Ok(_) => out.push(format!("[ok] removed broken symlink {}", link.display())),
                    Err(e) => out.push(format!("[fail] could not remove {}: {e}", link.display())),
                }
            } else {
                out.push(format!("[warn] broken symlink {} — run: workz sync (or doctor --fix)", link.display()));
            }
        }
    }
    if broken_total == 0 {
        out.push("[ok] no broken symlinks in worktrees".to_string());
    } else if !fix {
        problems += broken_total;
    }

    problems
}

/// Top-level entries in `dir` that are symlinks pointing to a nonexistent target.
/// Pure and filesystem-testable.
pub fn broken_symlinks(dir: &Path) -> Vec<PathBuf> {
    let mut broken = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return broken;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // symlink_metadata does not follow the link; is_symlink tells us it IS one.
        let is_symlink = path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        // path.exists() follows the link — false means the target is gone.
        if is_symlink && !path.exists() {
            broken.push(path);
        }
    }
    broken.sort();
    broken
}

// ── tooling ──────────────────────────────────────────────────────────────────

fn check_tooling(out: &mut Vec<String>) {
    // git got us this far. Report optional tools used by --isolated DB features.
    for tool in ["dropdb", "createdb"] {
        if which_exists(tool) {
            out.push(format!("[ok] {tool} available (for --isolated database cleanup)"));
        } else {
            out.push(format!("[warn] {tool} not found — --cleanup-db / DB creation will be skipped"));
        }
    }
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {cmd}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn broken_symlinks_detects_dangling() {
        let base = std::env::temp_dir().join(format!("workz_doctor_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        // A good symlink (target exists) and a dangling one.
        let real = base.join("real");
        fs::write(&real, "x").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, base.join("good")).unwrap();
            std::os::unix::fs::symlink(base.join("missing"), base.join("bad")).unwrap();
        }

        let broken = broken_symlinks(&base);
        #[cfg(unix)]
        {
            assert_eq!(broken.len(), 1);
            assert!(broken[0].ends_with("bad"));
        }
        let _ = fs::remove_dir_all(&base);
    }
}
