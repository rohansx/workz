use anyhow::{Context, Result};
use std::path::Path;

use crate::config::SyncConfig;

/// Sync a worktree: symlink heavy directories, copy env files, and auto-install deps.
pub fn sync_worktree(source: &Path, target: &Path, config: &SyncConfig) -> Result<()> {
    let project = detect_project(source);
    symlink_dirs(source, target, &config.symlink, &config.ignore, &project)?;
    copy_files(source, target, &config.copy, &config.ignore)?;
    auto_install(source, target, &project)?;
    Ok(())
}

/// Detected project types (a repo can be multiple, e.g. Node + Python monorepo).
#[derive(Default)]
struct ProjectInfo {
    has_node: bool,
    has_rust: bool,
    has_python: bool,
    has_go: bool,
    has_java: bool,
    /// Detected package manager command for Node projects.
    node_install_cmd: Option<Vec<String>>,
    /// Detected package manager command for Python projects.
    python_install_cmd: Option<Vec<String>>,
}

fn detect_project(root: &Path) -> ProjectInfo {
    let mut info = ProjectInfo::default();

    // Node.js detection + package manager
    if root.join("package.json").exists() {
        info.has_node = true;
        info.node_install_cmd = if root.join("bun.lockb").exists() || root.join("bun.lock").exists()
        {
            Some(vec!["bun".into(), "install".into(), "--frozen-lockfile".into()])
        } else if root.join("pnpm-lock.yaml").exists() {
            Some(vec!["pnpm".into(), "install".into(), "--frozen-lockfile".into()])
        } else if root.join("yarn.lock").exists() {
            Some(vec!["yarn".into(), "install".into(), "--frozen-lockfile".into()])
        } else if root.join("package-lock.json").exists() {
            Some(vec!["npm".into(), "ci".into()])
        } else {
            None
        };
    }

    // Rust
    if root.join("Cargo.toml").exists() {
        info.has_rust = true;
    }

    // Python detection + package manager
    if root.join("pyproject.toml").exists()
        || root.join("requirements.txt").exists()
        || root.join("setup.py").exists()
    {
        info.has_python = true;
        info.python_install_cmd = if root.join("uv.lock").exists() {
            Some(vec!["uv".into(), "sync".into()])
        } else if root.join("Pipfile.lock").exists() {
            Some(vec!["pipenv".into(), "install".into()])
        } else if root.join("poetry.lock").exists() {
            Some(vec!["poetry".into(), "install".into()])
        } else if root.join("requirements.txt").exists() {
            Some(vec!["pip".into(), "install".into(), "-r".into(), "requirements.txt".into()])
        } else {
            None
        };
    }

    // Go
    if root.join("go.mod").exists() {
        info.has_go = true;
    }

    // Java / Kotlin
    if root.join("build.gradle").exists()
        || root.join("build.gradle.kts").exists()
        || root.join("pom.xml").exists()
    {
        info.has_java = true;
    }

    info
}

/// Directories that only matter for specific project types.
fn is_relevant(dir_name: &str, project: &ProjectInfo) -> bool {
    match dir_name {
        // Node-specific
        "node_modules" | ".next" | ".nuxt" | ".svelte-kit" | ".turbo" | ".parcel-cache"
        | ".angular" => project.has_node,
        // Rust-specific
        "target" => project.has_rust,
        // Python-specific
        ".venv" | "venv" | "__pycache__" | ".mypy_cache" | ".pytest_cache" | ".ruff_cache" => {
            project.has_python
        }
        // Go-specific
        "vendor" => project.has_go,
        // Java-specific
        ".gradle" | "build" => project.has_java,
        // General — always relevant
        _ => true,
    }
}

/// Symlink heavy directories from source into target (project-aware).
fn symlink_dirs(
    source: &Path,
    target: &Path,
    dirs: &[String],
    ignore: &[String],
    project: &ProjectInfo,
) -> Result<()> {
    for dir_name in dirs {
        if ignore.iter().any(|i| i == dir_name) {
            continue;
        }

        // Skip dirs not relevant to this project type
        if !is_relevant(dir_name, project) {
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

/// Auto-install dependencies if the deps dir doesn't exist in source or target.
fn auto_install(source: &Path, target: &Path, project: &ProjectInfo) -> Result<()> {
    // Node: if node_modules doesn't exist anywhere, offer to install
    if project.has_node && !source.join("node_modules").exists() && !target.join("node_modules").exists() {
        if let Some(cmd) = &project.node_install_cmd {
            println!("  installing node dependencies ({})...", cmd[0]);
            let status = std::process::Command::new(&cmd[0])
                .args(&cmd[1..])
                .current_dir(target)
                .status();
            match status {
                Ok(s) if s.success() => println!("  dependencies installed"),
                Ok(s) => eprintln!("  warning: {} exited with {}", cmd[0], s),
                Err(e) => eprintln!("  warning: could not run {}: {}", cmd[0], e),
            }
        }
    }

    // Python: if .venv doesn't exist anywhere, offer to install
    if project.has_python
        && !source.join(".venv").exists()
        && !target.join(".venv").exists()
        && !source.join("venv").exists()
        && !target.join("venv").exists()
    {
        if let Some(cmd) = &project.python_install_cmd {
            println!("  installing python dependencies ({})...", cmd[0]);
            let status = std::process::Command::new(&cmd[0])
                .args(&cmd[1..])
                .current_dir(target)
                .status();
            match status {
                Ok(s) if s.success() => println!("  dependencies installed"),
                Ok(s) => eprintln!("  warning: {} exited with {}", cmd[0], s),
                Err(e) => eprintln!("  warning: could not run {}: {}", cmd[0], e),
            }
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
