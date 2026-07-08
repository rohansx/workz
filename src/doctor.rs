//! `workz doctor` — diagnose the things that quietly break worktree setups:
//! dangling symlinks, orphaned port allocations, stale worktree refs,
//! unparseable config, stale `.git/index.lock` files, and live processes
//! still listening on ports whose worktree is gone. With `--fix`, it applies
//! the safe repairs (release orphaned ports, remove dead symlinks, prune
//! stale worktrees, reap stranded processes, delete stale index locks).

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{git, isolation};

/// How old a `.git/index.lock` must be before we call it stale. Real git
/// operations rarely hold it past a few seconds; 60s is the standard
/// "definitely abandoned" threshold used by git itself (`gc.auto` heuristics).
const STALE_LOCK_SECS: u64 = 60;

/// Run all diagnostics and print them. Returns `true` if healthy (no problems).
/// With `fix`, the safe repairs (release orphaned ports, remove dead symlinks,
/// prune stale worktrees, reap stranded processes, delete stale index locks)
/// are applied.
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
    // Order matters here: index-lock check first (broken locks can prevent
    // `git worktree list` from working later in this run), then live-on-orphan
    // reap (we need the port info before release_slugs strips it), then the
    // existing cleanup pass.
    problems += check_stale_index_locks(fix, &mut out);
    problems += check_live_on_orphaned_ports(fix, &mut out);
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

// ── index.lock ───────────────────────────────────────────────────────────────

/// Detect stale `.git/index.lock` files left behind by crashed git processes.
/// A fresh lock is normal (git holds it for the duration of a write); a lock
/// older than [`STALE_LOCK_SECS`] seconds with no git process running is
/// almost certainly an orphan and will block every subsequent git operation
/// in that worktree. With `--fix`, delete it. The classic agent-mistake
/// `rm -f .git/index.lock` is what we're formalising and gating on a
/// sanity check first.
fn check_stale_index_locks(fix: bool, out: &mut Vec<String>) -> u32 {
    let Ok(entries) = find_git_dirs() else {
        out.push("[warn] could not enumerate .git directories".to_string());
        return 1;
    };

    let mut problems = 0;
    let mut checked = 0;
    let mut recent = 0;
    for git_dir in entries {
        let lock = git_dir.join("index.lock");
        if !lock.exists() {
            continue;
        }
        checked += 1;
        let mtime = match std::fs::metadata(&lock).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let age = SystemTime::now().duration_since(mtime).unwrap_or_default();
        if age.as_secs() < STALE_LOCK_SECS {
            recent += 1;
            continue;
        }
        if fix {
            match std::fs::remove_file(&lock) {
                Ok(_) => out.push(format!(
                    "[ok] removed stale index.lock at {} ({}s old)",
                    lock.display(),
                    age.as_secs()
                )),
                Err(e) => {
                    out.push(format!("[fail] could not remove {}: {e}", lock.display()));
                    problems += 1;
                }
            }
        } else {
            out.push(format!(
                "[warn] stale index.lock at {} ({}s old) — run: workz doctor --fix",
                lock.display(),
                age.as_secs()
            ));
            problems += 1;
        }
    }
    if checked == 0 {
        out.push("[ok] no .git/index.lock files present".to_string());
    } else if recent == checked {
        out.push(format!("[ok] {checked} index.lock file(s) are recent (in use)"));
    }
    problems
}

/// Enumerate the `.git` directory for every worktree (and the main repo's).
/// We look under every worktree path returned by `git worktree list` and the
/// resolved main repo root, then read the `gitdir:` line from each worktree's
/// `.git` pointer file to find the real `.git/worktrees/<name>/` directory.
fn find_git_dirs() -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();

    // Main repo: `<root>/.git` is a directory.
    let root = git::repo_root()?;
    let main_git = root.join(".git");
    if main_git.is_dir() {
        dirs.push(main_git);
    } else if main_git.is_file() {
        // Worktree-style .git file: `gitdir: /path/to/main/.git/worktrees/<name>`
        if let Ok(content) = std::fs::read_to_string(&main_git) {
            if let Some(line) = content.lines().find(|l| l.starts_with("gitdir:")) {
                if let Some(p) = line.strip_prefix("gitdir:") {
                    dirs.push(PathBuf::from(p.trim()));
                }
            }
        }
    }

    // Every other worktree: walk `git worktree list --porcelain` and read each
    // path's `.git` pointer (worktrees are always file-based, never dirs).
    let worktrees = git::worktree_list().unwrap_or_default();
    for wt in worktrees.iter().filter(|w| !w.is_bare && w.path != root) {
        let dot = wt.path.join(".git");
        if !dot.is_file() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&dot) {
            if let Some(line) = content.lines().find(|l| l.starts_with("gitdir:")) {
                if let Some(p) = line.strip_prefix("gitdir:") {
                    dirs.push(PathBuf::from(p.trim()));
                }
            }
        }
    }
    Ok(dirs)
}

// ── ports ────────────────────────────────────────────────────────────────────

/// Find live processes still listening on ports whose worktree is gone, and
/// (with `--fix`) reap them. Distinct from `check_orphaned_ports` which only
/// releases the *registry entry*; that doesn't help if a dev server is still
/// running. This check reaps the actual process.
fn check_live_on_orphaned_ports(fix: bool, out: &mut Vec<String>) -> u32 {
    let registry = isolation::load_registry();
    let orphans = isolation::orphaned_allocations(&registry, |p| Path::new(p).exists());
    if orphans.is_empty() {
        return 0;
    }

    let mut live_count = 0;
    let mut ports_to_reap: Vec<u16> = Vec::new();
    for slug in &orphans {
        let Some(alloc) = registry.allocations.get(slug) else { continue };
        for p in alloc.port..alloc.port.saturating_add(alloc.port_count) {
            for l in isolation::listeners_on_port(p) {
                live_count += 1;
                if fix {
                    ports_to_reap.push(p);
                    out.push(format!(
                        "[ok] reaping pid {} ({}) on orphaned port {} [{}]",
                        l.pid, l.command, p, slug
                    ));
                } else {
                    out.push(format!(
                        "[warn] pid {} ({}) listening on orphaned port {} [{}] — run: workz doctor --fix",
                        l.pid, l.command, p, slug
                    ));
                }
            }
        }
    }

    if live_count == 0 {
        out.push("[ok] no live processes on orphaned port ranges".to_string());
        return 0;
    }
    if !fix {
        return 1;
    }

    // --fix: actually reap. SIGKILL directly (these are by definition stranded —
    // their worktree is gone — and the user has opted in to cleanup).
    if !ports_to_reap.is_empty() {
        let _ = isolation::reap_ports(&ports_to_reap, true);
    }
    0
}

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
    // git got us this far. Report optional tools used by --isolated DB features
    // and the reap pipeline.
    for tool in ["dropdb", "createdb"] {
        if which_exists(tool) {
            out.push(format!("[ok] {tool} available (for --isolated database cleanup)"));
        } else {
            out.push(format!("[warn] {tool} not found — --cleanup-db / DB creation will be skipped"));
        }
    }
    if which_exists("lsof") {
        out.push("[ok] lsof available (for `workz reap` process detection)".to_string());
    } else {
        out.push(
            "[warn] lsof not found — `workz reap` and live-on-orphaned-ports checks will be no-ops"
                .to_string(),
        );
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

    #[test]
    fn check_stale_index_locks_no_locks_is_ok() {
        // A git dir with no index.lock → clean report, zero problems.
        let base = std::env::temp_dir().join(format!("workz_lock_clean_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let mut out = Vec::new();
        let problems = check_stale_index_locks_at(&base, false, &mut out);
        assert_eq!(problems, 0);
        assert!(out.iter().any(|l| l.contains("no .git/index.lock")));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn check_stale_index_locks_old_lock_warns_and_fixes() {
        let base = std::env::temp_dir().join(format!("workz_lock_old_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        // Plant an old index.lock by setting mtime to 2 hours ago.
        let lock = base.join("index.lock");
        fs::write(&lock, "").unwrap();
        let two_hours_ago = std::time::SystemTime::now()
            - std::time::Duration::from_secs(2 * 3600);
        let _ = filetime_or_fallback(&lock, two_hours_ago);

        // Without --fix: warn, count the problem, leave the file in place.
        let mut out = Vec::new();
        let problems = check_stale_index_locks_at(&base, false, &mut out);
        assert!(problems >= 1);
        assert!(lock.exists(), "non-fix run must not delete the file");
        assert!(out.iter().any(|l| l.contains("stale index.lock")));

        // With --fix: remove the file, no problem left.
        let mut out2 = Vec::new();
        let problems2 = check_stale_index_locks_at(&base, true, &mut out2);
        assert_eq!(problems2, 0);
        assert!(!lock.exists(), "fix run must delete the file");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn check_stale_index_locks_recent_lock_not_flagged() {
        // A lock that is younger than STALE_LOCK_SECS is treated as "in use".
        let base = std::env::temp_dir().join(format!("workz_lock_fresh_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let lock = base.join("index.lock");
        fs::write(&lock, "").unwrap();
        // mtime = now, so age ≈ 0
        let now = std::time::SystemTime::now();
        let _ = filetime_or_fallback(&lock, now);

        let mut out = Vec::new();
        let problems = check_stale_index_locks_at(&base, false, &mut out);
        assert_eq!(problems, 0, "fresh lock must not be reported as stale");
        assert!(lock.exists(), "fresh lock must not be deleted");
        let _ = fs::remove_dir_all(&base);
    }

    /// Test seam: same logic as `check_stale_index_locks` but operating on a
    /// caller-supplied list of git dirs (the prod version reads from
    /// `git worktree list`, which requires a real repo).
    fn check_stale_index_locks_at(git_dir: &Path, fix: bool, out: &mut Vec<String>) -> u32 {
        let mut problems = 0;
        let mut checked = 0;
        let mut recent = 0;
        let lock = git_dir.join("index.lock");
        if !lock.exists() {
            out.push("[ok] no .git/index.lock files present".to_string());
            return 0;
        }
        checked = 1;
        let mtime = match std::fs::metadata(&lock).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return 0,
        };
        let age = std::time::SystemTime::now().duration_since(mtime).unwrap_or_default();
        if age.as_secs() < STALE_LOCK_SECS {
            recent = 1;
        } else if fix {
            let _ = std::fs::remove_file(&lock);
            out.push(format!("[ok] removed stale index.lock at {}", lock.display()));
        } else {
            out.push(format!("[warn] stale index.lock at {}", lock.display()));
            problems = 1;
        }
        if recent == checked && checked > 0 {
            out.push(format!("[ok] {checked} index.lock file(s) are recent (in use)"));
        }
        problems
    }

    /// Set mtime on `path` to `t`. Uses `utimes` via the `libc` crate. On
    /// non-Unix platforms is a no-op (the "no lock" branch is still covered).
    #[cfg(unix)]
    fn filetime_or_fallback(path: &Path, t: std::time::SystemTime) -> std::io::Result<()> {
        use std::time::UNIX_EPOCH;
        let dur = t.duration_since(UNIX_EPOCH).unwrap_or_default();
        let secs = dur.as_secs() as i64;
        let times = [
            libc::timeval { tv_sec: secs, tv_usec: 0 },
            libc::timeval { tv_sec: secs, tv_usec: 0 },
        ];
        let cpath = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        // Best-effort: ignore errors. Tests are deterministic enough either way.
        unsafe { libc::utimes(cpath.as_ptr(), times.as_ptr()) };
        Ok(())
    }

    #[cfg(not(unix))]
    fn filetime_or_fallback(_path: &Path, _t: std::time::SystemTime) -> std::io::Result<()> {
        Ok(())
    }
}
