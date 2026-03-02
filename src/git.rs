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

/// Compute the worktree directory path: `../<repo>--<safe-branch>`.
pub fn worktree_path(root: &Path, branch: &str) -> PathBuf {
    let safe = branch.replace(['/', '\\'], "-");
    let base = root.parent().unwrap_or(root);
    base.join(format!("{}--{}", repo_name(root), safe))
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
