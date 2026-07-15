use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Run a git command and return stdout as a trimmed string.
fn git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .context("failed to execute git — is it installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run a git command within a specific directory.
fn git_in(dir: &Path, args: &[&str]) -> Result<String> {
    let mut full_args = vec!["-C", dir.to_str().unwrap_or(".")];
    full_args.extend_from_slice(args);
    git(&full_args)
}

/// Find the root of the main git repository (not a worktree).
/// Uses --git-common-dir to always resolve to the main repo, even when
/// called from inside a worktree.
pub fn repo_root() -> Result<PathBuf> {
    let toplevel = git(&["rev-parse", "--show-toplevel"])
        .context("not inside a git repository")?;
    let common_dir = git(&["rev-parse", "--git-common-dir"])?;

    let common = PathBuf::from(&common_dir);
    // If common_dir is ".git", we're in the main repo — use toplevel
    // If common_dir is an absolute path (e.g. /repo/.git), parent is the main repo
    // If common_dir is a relative path (e.g. ../../repo/.git), resolve from toplevel
    if common_dir == ".git" {
        Ok(PathBuf::from(toplevel))
    } else {
        let abs = if common.is_absolute() {
            common
        } else {
            PathBuf::from(&toplevel).join(&common)
        };
        // common_dir points to the .git dir — parent is the repo root
        abs.parent()
            .map(|p| p.to_path_buf())
            .and_then(|p| p.canonicalize().ok())
            .ok_or_else(|| anyhow::anyhow!("could not resolve main repo root"))
    }
}

/// Get the repository name from the root path.
pub fn repo_name(root: &Path) -> String {
    root.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

/// Compute the worktree directory path for `branch`.
///
/// Default (no `[worktree] dir` configured): `../<repo>--<safe-branch>`, placed
/// next to the main checkout. When a directory is configured, worktrees go under
/// `<dir>/<safe-branch>` — a relative `dir` resolves against the repo root (so
/// `.worktrees` nests them inside the project), an absolute `dir` is used as-is.
/// `safe-branch` is the branch name with `/` and `\` replaced by `-`.
pub fn worktree_path(root: &Path, branch: &str, dir: Option<&str>) -> PathBuf {
    let safe = branch.replace(['/', '\\'], "-");
    match dir {
        Some(d) if !d.is_empty() => {
            let base = if Path::new(d).is_absolute() {
                PathBuf::from(d)
            } else {
                root.join(d)
            };
            base.join(safe)
        }
        _ => {
            let base = root.parent().unwrap_or(root);
            base.join(format!("{}--{}", repo_name(root), safe))
        }
    }
}

/// Check whether a local branch exists.
pub fn branch_exists(name: &str) -> Result<bool> {
    let result = git(&["rev-parse", "--verify", &format!("refs/heads/{name}")]);
    Ok(result.is_ok())
}

/// Create a new worktree. Creates the branch if it doesn't exist.
pub fn worktree_add(path: &Path, branch: &str, base: Option<&str>) -> Result<()> {
    let path_str = path.to_str().unwrap_or(".");

    if branch_exists(branch)? {
        git(&["worktree", "add", path_str, branch])?;
    } else {
        // Create a new branch from base (or HEAD)
        let mut args = vec!["worktree", "add", "-b", branch, path_str];
        if let Some(b) = base {
            args.push(b);
        }
        git(&args)?;
    }

    Ok(())
}

/// Remove a worktree.
pub fn worktree_remove(path: &Path, force: bool) -> Result<()> {
    let path_str = path.to_str().unwrap_or(".");
    if force {
        git(&["worktree", "remove", "--force", path_str])?;
    } else {
        git(&["worktree", "remove", path_str])?;
    }
    Ok(())
}

/// Delete a local branch.
pub fn branch_delete(name: &str, force: bool) -> Result<()> {
    let flag = if force { "-D" } else { "-d" };
    git(&["branch", flag, name])?;
    Ok(())
}

/// Prune stale worktree entries.
pub fn worktree_prune() -> Result<String> {
    git(&["worktree", "prune", "-v"])
}

/// A parsed worktree entry.
#[derive(Debug)]
#[allow(dead_code)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub is_bare: bool,
    pub is_detached: bool,
}

/// List all worktrees (parsed from porcelain output).
pub fn worktree_list() -> Result<Vec<Worktree>> {
    let output = git(&["worktree", "list", "--porcelain"])?;
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch = String::new();
    let mut is_bare = false;
    let mut is_detached = false;

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            // Flush previous entry
            if let Some(prev_path) = current_path.take() {
                worktrees.push(Worktree {
                    path: prev_path,
                    branch: std::mem::take(&mut current_branch),
                    is_bare,
                    is_detached,
                });
            }
            current_path = Some(PathBuf::from(path.trim()));
            is_bare = false;
            is_detached = false;
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            current_branch = b.trim().to_string();
        } else if line.trim() == "bare" {
            is_bare = true;
        } else if line.trim() == "detached" {
            is_detached = true;
            current_branch = "(detached)".to_string();
        }
    }

    // Flush last entry
    if let Some(path) = current_path {
        worktrees.push(Worktree {
            path,
            branch: current_branch,
            is_bare,
            is_detached,
        });
    }

    Ok(worktrees)
}

/// Check if a worktree has uncommitted changes.
pub fn is_dirty(path: &Path) -> Result<bool> {
    let status = git_in(path, &["status", "--porcelain"])?;
    Ok(!status.is_empty())
}

/// Get the current branch name in a directory.
pub fn current_branch(path: &Path) -> Result<String> {
    git_in(path, &["branch", "--show-current"])
}

/// Get the last commit time as a human-readable relative string (e.g. "2 hours ago").
pub fn last_commit_relative(path: &Path) -> Option<String> {
    git_in(path, &["log", "-1", "--format=%cr"]).ok().filter(|s| !s.is_empty())
}

/// Return the default base branch (main, then master, then HEAD).
pub fn default_branch() -> String {
    for candidate in &["main", "master"] {
        if git(&["rev-parse", "--verify", &format!("refs/heads/{candidate}")]).is_ok() {
            return candidate.to_string();
        }
    }
    "HEAD".to_string()
}

/// Return a set of branch names that are fully merged into `base`.
pub fn merged_branches(base: &str) -> Result<Vec<String>> {
    let output = git(&["branch", "--merged", base])?;
    Ok(output
        .lines()
        .map(|l| l.trim().trim_start_matches("* ").to_string())
        .filter(|b| !b.is_empty() && b != base)
        .collect())
}

/// List files with uncommitted changes (staged or unstaged) in a worktree.
///
/// Reads raw (untrimmed) stdout on purpose: `git()` trims the whole output,
/// which would strip the leading space off the first porcelain line
/// (` M file` → `M file`) and drop the first character of that filename.
pub fn modified_files(path: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["-C", path.to_str().unwrap_or("."), "status", "--porcelain"])
        .output()
        .context("failed to execute git — is it installed?")?;
    if !output.status.success() {
        bail!("git status failed in {}", path.display());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Porcelain v1: `XY PATH` — the path starts at byte offset 3.
    Ok(stdout
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| l[3..].trim().to_string())
        .collect())
}

/// Files modified in more than one worktree — potential merge conflicts before
/// they happen. Returns `(file, sorted branches)` sorted by file.
pub fn find_conflicts() -> Result<Vec<(String, Vec<String>)>> {
    let worktrees = worktree_list()?;
    let mut map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for wt in worktrees.iter().filter(|w| !w.is_bare) {
        for f in modified_files(&wt.path).unwrap_or_default() {
            map.entry(f).or_default().push(wt.branch.clone());
        }
    }
    let mut conflicts: Vec<(String, Vec<String>)> =
        map.into_iter().filter(|(_, branches)| branches.len() > 1).collect();
    for (_, branches) in conflicts.iter_mut() {
        branches.sort();
    }
    conflicts.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(conflicts)
}

/// What a snapshot of an uncommitted state captured — tracked changes,
/// untracked files, or both. The consumer gets a single opaque "carry ID"
/// back and uses [`apply_carry`] to materialize the state elsewhere.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Hash of the stash commit with the tracked changes, or `None` if the
    /// working tree had no tracked changes.
    pub tracked: Option<String>,
    /// List of (relative path, source absolute path) for each untracked
    /// file we copied. The target gets the same relative paths.
    pub untracked: Vec<(String, std::path::PathBuf)>,
}

/// Snapshot the uncommitted state of a worktree (tracked + untracked) for
/// later application in another worktree. Read-only: never mutates the
/// source's working tree, never touches the stash ref, never deletes
/// anything. Safe to call while an agent is running in the source worktree.
///
/// Returns `Ok(None)` when the source has nothing to carry (clean tree).
pub fn snapshot_uncommitted(source: &Path) -> Result<Option<Snapshot>> {
    // 1. Snapshot the tracked changes via `git stash create` (no flag —
    //    `--include-untracked` produces a merge commit that `git stash apply`
    //    doesn't fully restore in a clean target worktree, so we handle the
    //    two parts separately).
    let output = Command::new("git")
        .args(["-C", source.to_str().unwrap_or("."), "stash", "create", "--quiet"])
        .output()
        .context("failed to run git stash create")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git stash create failed in {}: {}", source.display(), stderr.trim());
    }
    let tracked = {
        let h = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if h.is_empty() { None } else { Some(h) }
    };

    // 2. Snapshot untracked files by listing them and reading their bytes.
    //    We do NOT copy them into the target yet — just record the paths so
    //    [`apply_carry`] can stream them on demand.
    let untracked_output = Command::new("git")
        .args([
            "-C", source.to_str().unwrap_or("."),
            "ls-files", "--others", "--exclude-standard", "-z",
        ])
        .output()
        .context("failed to run git ls-files --others")?;
    let mut untracked: Vec<(String, std::path::PathBuf)> = Vec::new();
    if untracked_output.status.success() {
        // `-z` separates paths with NUL bytes; iterate as raw bytes to avoid
        // any string-splitting ambiguity (filenames can contain newlines).
        for path_bytes in untracked_output.stdout.split(|b| *b == 0) {
            if path_bytes.is_empty() {
                continue;
            }
            let rel = String::from_utf8_lossy(path_bytes).into_owned();
            let abs = source.join(&rel);
            untracked.push((rel, abs));
        }
    }

    if tracked.is_none() && untracked.is_empty() {
        return Ok(None);
    }

    Ok(Some(Snapshot {
        tracked,
        untracked,
    }))
}

/// Apply a [`Snapshot`] to a target worktree. The source worktree is
/// untouched. Tracked changes are applied via `git stash apply`; untracked
/// files are copied byte-for-byte. Returns the number of untracked files
/// successfully copied (so the caller can warn about partial failures).
pub fn apply_carry(target: &Path, snap: &Snapshot) -> Result<usize> {
    if let Some(hash) = &snap.tracked {
        let status = Command::new("git")
            .args(["-C", target.to_str().unwrap_or("."), "stash", "apply", "--index", hash])
            .status()
            .context("failed to run git stash apply")?;
        if !status.success() {
            bail!("git stash apply failed in {}", target.display());
        }
    }

    let mut copied = 0;
    for (rel, src_abs) in &snap.untracked {
        let dst = target.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Overwrite only if the target file is identical to the source —
        // we don't want to clobber files that were already carried or that
        // were created in the new worktree by another step. Cheap content
        // check via size + first chunk.
        if dst.exists() {
            let src_meta = std::fs::metadata(src_abs).ok();
            let dst_meta = std::fs::metadata(&dst).ok();
            if let (Some(s), Some(d)) = (src_meta, dst_meta) {
                if s.len() == d.len() {
                    // Same size — assume same content; skip.
                    copied += 1;
                    continue;
                }
            }
        }
        match std::fs::copy(src_abs, &dst) {
            Ok(_) => copied += 1,
            Err(e) => eprintln!(
                "  warning: could not carry untracked file {}: {e}",
                rel
            ),
        }
    }
    Ok(copied)
}

/// Drop a tracked-changes stash commit from the object store. Called after
/// the consumer has successfully applied the snapshot — the commit would
/// otherwise sit in the reflog forever. Best-effort; ignored on failure.
pub fn drop_stash(target: &Path, stash_commit: &str) {
    let _ = Command::new("git")
        .args(["-C", target.to_str().unwrap_or("."), "stash", "drop", "--quiet", stash_commit])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_path_default_uses_parent_repo_prefix() {
        let root = Path::new("/home/me/projects/myapp");
        let p = worktree_path(root, "feature/add-auth", None);
        assert_eq!(p, Path::new("/home/me/projects/myapp--feature-add-auth"));
    }

    #[test]
    fn worktree_path_relative_dir_nests_in_repo() {
        let root = Path::new("/home/me/projects/myapp");
        let p = worktree_path(root, "feature/add-auth", Some(".worktrees"));
        assert_eq!(p, Path::new("/home/me/projects/myapp/.worktrees/feature-add-auth"));
    }

    #[test]
    fn worktree_path_absolute_dir_used_verbatim() {
        let root = Path::new("/home/me/projects/myapp");
        let p = worktree_path(root, "bugfix", Some("/tmp/wt"));
        assert_eq!(p, Path::new("/tmp/wt/bugfix"));
    }

    #[test]
    fn worktree_path_empty_dir_falls_back_to_default() {
        let root = Path::new("/home/me/projects/myapp");
        let p = worktree_path(root, "x", Some(""));
        assert_eq!(p, Path::new("/home/me/projects/myapp--x"));
    }

    #[test]
    fn modified_files_keeps_first_char() {
        // Regression: git() trims output, which dropped the leading space of the
        // first porcelain line and ate the first filename character.
        let dir = std::env::temp_dir().join(format!("workz_git_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .args(["-C", dir.to_str().unwrap()])
                .args(args)
                .output()
                .unwrap();
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@t.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.join("shared.txt"), "base").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);
        std::fs::write(dir.join("shared.txt"), "base\nmore").unwrap();

        let files = modified_files(&dir).unwrap();
        assert_eq!(files, vec!["shared.txt".to_string()], "first char must survive");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

