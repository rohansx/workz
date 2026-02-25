use anyhow::{Context, Result};
use std::path::Path;

use crate::config::SyncConfig;

/// Sync a worktree: symlink heavy directories and copy env files.
pub fn sync_worktree(source: &Path, target: &Path, config: &SyncConfig) -> Result<()> {
    symlink_dirs(source, target, &config.symlink, &config.ignore)?;
    copy_files(source, target, &config.copy, &config.ignore)?;
    Ok(())
}

/// Symlink heavy directories from source into target.
fn symlink_dirs(
    source: &Path,
    target: &Path,
    dirs: &[String],
    ignore: &[String],
) -> Result<()> {
    for dir_name in dirs {
        if ignore.iter().any(|i| i == dir_name) {
            continue;
        }

        let src = source.join(dir_name);
        let dst = target.join(dir_name);

        // Only symlink if the source directory actually exists
        if !src.exists() {
            continue;
        }

        // Never overwrite an existing file, dir, or symlink
        if dst.exists() || dst.symlink_metadata().is_ok() {
            continue;
        }

        if let Err(e) = create_symlink(&src, &dst) {
            eprintln!("  warning: could not symlink {}: {}", dir_name, e);
        } else {
            println!("  symlinked {}", dir_name);
        }
    }

    Ok(())
}

/// Copy files matching glob patterns from source into target.
fn copy_files(
    source: &Path,
    target: &Path,
    patterns: &[String],
    ignore: &[String],
) -> Result<()> {
    for pattern in patterns {
        let full_pattern = source.join(pattern);
        let pat_str = full_pattern.to_str().unwrap_or("");

        let entries = glob::glob(pat_str).context("invalid glob pattern")?;

        for entry in entries.flatten() {
            let file_name = match entry.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };

            if ignore.iter().any(|i| i == &file_name) {
                continue;
            }

            // Only copy regular files
            if !entry.is_file() {
                continue;
            }

            let dst = target.join(&file_name);
            if dst.exists() {
                continue;
            }

            if let Err(e) = std::fs::copy(&entry, &dst) {
                eprintln!("  warning: could not copy {}: {}", file_name, e);
            } else {
                println!("  copied {}", file_name);
            }
        }
    }

    Ok(())
}

/// Create a symbolic link (Unix) or directory junction (Windows).
fn create_symlink(src: &Path, dst: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)
            .with_context(|| format!("symlink {} -> {}", dst.display(), src.display()))?;
    }

    #[cfg(windows)]
    {
        // Use directory junction — works without admin privileges
        std::process::Command::new("cmd")
            .args([
                "/c",
                "mklink",
                "/J",
                &dst.to_string_lossy(),
                &src.to_string_lossy(),
            ])
            .output()
            .with_context(|| format!("junction {} -> {}", dst.display(), src.display()))?;
    }

    Ok(())
}
