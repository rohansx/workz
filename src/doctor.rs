//! `workz doctor` — diagnose the things that quietly break worktree setups:
//! dangling symlinks, orphaned port allocations, stale worktree refs, and
//! unparseable config. With `--fix`, it applies the safe repairs.

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::{git, isolation};

/// Run all diagnostics. Returns `true` if healthy (no problems found).
/// With `fix`, the safe repairs (release orphaned ports, remove dead symlinks,
/// prune stale worktrees) are applied.
pub fn run(fix: bool) -> Result<bool> {
    println!("workz doctor{}\n", if fix { " (--fix)" } else { "" });
    let mut problems = 0;

    let root = git::repo_root()?;

    problems += check_config(&root);
    problems += check_orphaned_ports(fix);
    problems += check_worktrees(&root, fix);
    check_tooling();

    println!();
    if problems == 0 {
        println!("[ok] all checks passed");
        Ok(true)
    } else {
        let hint = if fix { "" } else { " — re-run with --fix to repair" };
        println!("[fail] {problems} problem(s) found{hint}");
        Ok(false)
    }
}

// ── config ───────────────────────────────────────────────────────────────────

/// Strictly parse project + global config (unlike `load_config`, which ignores
/// parse errors). Returns the number of problems.
fn check_config(root: &Path) -> u32 {
    let mut problems = 0;

    let project = root.join(".workz.toml");
    if project.exists() {
        match std::fs::read_to_string(&project)
            .map_err(|e| e.to_string())
            .and_then(|c| toml::from_str::<toml::Value>(&c).map_err(|e| e.to_string()))
        {
            Ok(_) => println!("[ok] .workz.toml parses"),
            Err(e) => {
                println!("[fail] .workz.toml does not parse: {e}");
                problems += 1;
            }
        }
    } else {
        println!("[ok] no .workz.toml (using defaults)");
    }

    if let Some(global) = dirs::config_dir().map(|d| d.join("workz").join("config.toml")) {
        if global.exists() {
            match std::fs::read_to_string(&global)
                .map_err(|e| e.to_string())
                .and_then(|c| toml::from_str::<toml::Value>(&c).map_err(|e| e.to_string()))
            {
                Ok(_) => println!("[ok] global config parses"),
                Err(e) => {
                    println!("[fail] global config does not parse: {e}");
                    problems += 1;
                }
            }
        }
    }

    problems
}

// ── ports ────────────────────────────────────────────────────────────────────

fn check_orphaned_ports(fix: bool) -> u32 {
    let registry = isolation::load_registry();
    let orphans = isolation::orphaned_allocations(&registry, |p| Path::new(p).exists());

    if orphans.is_empty() {
        println!("[ok] no orphaned port allocations");
        return 0;
    }

    if fix {
        match isolation::release_slugs(&orphans) {
            Ok(n) => {
                println!("[ok] released {n} orphaned port allocation(s): {}", orphans.join(", "));
                0
            }
            Err(e) => {
                println!("[fail] could not release orphaned allocations: {e}");
                1
            }
        }
    } else {
        println!(
            "[warn] {} orphaned port allocation(s) (worktree gone): {}",
            orphans.len(),
            orphans.join(", ")
        );
        1
    }
}

// ── worktrees: broken symlinks + stale refs ──────────────────────────────────

fn check_worktrees(root: &Path, fix: bool) -> u32 {
    let mut problems = 0;

    let worktrees = match git::worktree_list() {
        Ok(w) => w,
        Err(e) => {
            println!("[fail] could not list worktrees: {e}");
            return 1;
        }
    };

    // Stale refs: registered worktree paths that no longer exist on disk.
    let stale: Vec<_> = worktrees
        .iter()
        .filter(|w| !w.is_bare && !w.path.exists())
        .collect();
    if stale.is_empty() {
        println!("[ok] no stale worktree refs");
    } else if fix {
        match git::worktree_prune() {
            Ok(_) => println!("[ok] pruned {} stale worktree ref(s)", stale.len()),
            Err(e) => {
                println!("[fail] could not prune worktrees: {e}");
                problems += 1;
            }
        }
    } else {
        println!("[warn] {} stale worktree ref(s) — run: workz clean", stale.len());
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
                    Ok(_) => println!("[ok] removed broken symlink {}", link.display()),
                    Err(e) => println!("[fail] could not remove {}: {e}", link.display()),
                }
            } else {
                println!("[warn] broken symlink {} — run: workz sync (or doctor --fix)", link.display());
            }
        }
    }
    if broken_total == 0 {
        println!("[ok] no broken symlinks in worktrees");
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

fn check_tooling() {
    // git got us this far. Report optional tools used by --isolated DB features.
    for tool in ["dropdb", "createdb"] {
        if which_exists(tool) {
            println!("[ok] {tool} available (for --isolated database cleanup)");
        } else {
            println!("[warn] {tool} not found — --cleanup-db / DB creation will be skipped");
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
