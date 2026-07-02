use anyhow::{Context, Result};
use std::path::Path;

use crate::config::SyncConfig;

/// Detected web framework — used by isolation to write framework-specific env vars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Framework {
    #[default]
    Unknown,
    // Node.js
    NextJs,
    Vite,
    Express,
    NestJs,
    Nuxt,
    SvelteKit,
    // Python
    Django,
    Flask,
    FastApi,
    // Java/Kotlin
    SpringBoot,
    // Ruby
    Rails,
    // Elixir
    Phoenix,
    // Go (generic web)
    GoGeneric,
}

/// Options controlling a sync run.
#[derive(Debug, Default, Clone, Copy)]
pub struct SyncOptions {
    /// Skip the auto-install step (symlink + copy only).
    pub no_install: bool,
    /// Suppress the human-facing "installing..." notice (subprocess output still streams).
    pub quiet: bool,
}

/// What a sync actually did — returned so the caller decides how to render it
/// (human summary, `--json`, or `--quiet`).
#[derive(Debug, Default)]
pub struct SyncReport {
    pub symlinked: Vec<String>,
    pub copied: Vec<String>,
    /// The package-manager command that ran, e.g. "npm ci" — None if nothing installed.
    pub installed: Option<String>,
    pub warnings: Vec<String>,
    pub framework: Framework,
}

impl SyncReport {
    /// Multi-line indented summary for human output. Empty string if nothing happened.
    pub fn human_summary(&self) -> String {
        let mut lines = Vec::new();
        if !self.symlinked.is_empty() {
            lines.push(format!("  symlinked {}", self.symlinked.join(", ")));
        }
        if !self.copied.is_empty() {
            lines.push(format!("  copied {}", self.copied.join(", ")));
        }
        if let Some(cmd) = &self.installed {
            lines.push(format!("  installed deps ({cmd})"));
        }
        if lines.is_empty() {
            "  nothing to sync (already up to date)".to_string()
        } else {
            lines.join("\n")
        }
    }
}

/// Sync a worktree: symlink heavy directories, copy env files, and auto-install deps.
/// Collects everything it did into a [`SyncReport`]. Idempotent — already-correct
/// symlinks/copies are left untouched.
pub fn sync_worktree(
    source: &Path,
    target: &Path,
    config: &SyncConfig,
    opts: SyncOptions,
) -> Result<SyncReport> {
    let project = detect_project(source);
    let mut report = SyncReport {
        framework: project.framework,
        ..Default::default()
    };
    symlink_dirs(source, target, &config.symlink, &config.ignore, &project, &mut report);
    copy_files(source, target, &config.copy, &config.ignore, &mut report)?;
    if !opts.no_install {
        auto_install(source, target, &project, opts.quiet, &mut report);
    }
    Ok(report)
}

/// Detected project types (a repo can be multiple, e.g. Node + Python monorepo).
#[derive(Default)]
struct ProjectInfo {
    has_node: bool,
    has_rust: bool,
    has_python: bool,
    has_go: bool,
    has_java: bool,
    framework: Framework,
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

    // Framework detection (best-effort, file reads only)
    info.framework = detect_framework(root, &info);

    info
}

fn detect_framework(root: &Path, info: &ProjectInfo) -> Framework {
    if info.has_node {
        if let Some(fw) = detect_node_framework(root) {
            return fw;
        }
    }
    if info.has_python {
        if let Some(fw) = detect_python_framework(root) {
            return fw;
        }
    }
    if info.has_java {
        if let Some(fw) = detect_java_framework(root) {
            return fw;
        }
    }
    // Ruby
    if root.join("Gemfile").exists() {
        if let Ok(content) = std::fs::read_to_string(root.join("Gemfile")) {
            if content.contains("'rails'") || content.contains("\"rails\"") {
                return Framework::Rails;
            }
        }
    }
    // Elixir
    if root.join("mix.exs").exists() {
        if let Ok(content) = std::fs::read_to_string(root.join("mix.exs")) {
            if content.contains(":phoenix") {
                return Framework::Phoenix;
            }
        }
    }
    if info.has_go {
        return Framework::GoGeneric;
    }
    Framework::Unknown
}

fn detect_node_framework(root: &Path) -> Option<Framework> {
    let content = std::fs::read_to_string(root.join("package.json")).ok()?;
    let pkg: serde_json::Value = serde_json::from_str(&content).ok()?;

    let has_dep = |name: &str| -> bool {
        pkg.get("dependencies").and_then(|d| d.get(name)).is_some()
            || pkg.get("devDependencies").and_then(|d| d.get(name)).is_some()
    };

    if has_dep("next") { return Some(Framework::NextJs); }
    if has_dep("@sveltejs/kit") { return Some(Framework::SvelteKit); }
    if has_dep("nuxt") || has_dep("nuxt3") { return Some(Framework::Nuxt); }
    if has_dep("@nestjs/core") { return Some(Framework::NestJs); }
    if has_dep("vite") { return Some(Framework::Vite); }
    if has_dep("express") { return Some(Framework::Express); }
    None
}

fn detect_python_framework(root: &Path) -> Option<Framework> {
    for filename in &["pyproject.toml", "requirements.txt", "Pipfile"] {
        if let Ok(content) = std::fs::read_to_string(root.join(filename)) {
            let lower = content.to_lowercase();
            if lower.contains("django") { return Some(Framework::Django); }
            if lower.contains("fastapi") { return Some(Framework::FastApi); }
            if lower.contains("flask") { return Some(Framework::Flask); }
        }
    }
    None
}

fn detect_java_framework(root: &Path) -> Option<Framework> {
    for filename in &["build.gradle", "build.gradle.kts", "pom.xml"] {
        if let Ok(content) = std::fs::read_to_string(root.join(filename)) {
            if content.contains("spring-boot") || content.contains("org.springframework.boot") {
                return Some(Framework::SpringBoot);
            }
        }
    }
    None
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
    report: &mut SyncReport,
) {
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

        // Never overwrite an existing file, dir, or symlink (idempotent).
        if dst.exists() || dst.symlink_metadata().is_ok() {
            continue;
        }

        match create_symlink(&src, &dst) {
            Err(e) => report.warnings.push(format!("could not symlink {dir_name}: {e}")),
            Ok(()) => report.symlinked.push(dir_name.clone()),
        }
    }
}

/// Auto-install dependencies if the deps dir doesn't exist in source or target.
fn auto_install(
    source: &Path,
    target: &Path,
    project: &ProjectInfo,
    quiet: bool,
    report: &mut SyncReport,
) {
    // Node: if node_modules doesn't exist anywhere, offer to install
    if project.has_node && !source.join("node_modules").exists() && !target.join("node_modules").exists() {
        if let Some(cmd) = &project.node_install_cmd {
            run_install(cmd, target, quiet, report);
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
            run_install(cmd, target, quiet, report);
        }
    }
}

/// Run one install command, streaming its output and recording the result.
fn run_install(cmd: &[String], target: &Path, quiet: bool, report: &mut SyncReport) {
    let pretty = cmd.join(" ");
    if !quiet {
        println!("  installing dependencies ({pretty})...");
    }
    match std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .current_dir(target)
        .status()
    {
        Ok(s) if s.success() => report.installed = Some(pretty),
        Ok(s) => report.warnings.push(format!("{} exited with {}", cmd[0], s)),
        Err(e) => report.warnings.push(format!("could not run {}: {}", cmd[0], e)),
    }
}

/// Copy files matching glob patterns from source into target.
fn copy_files(
    source: &Path,
    target: &Path,
    patterns: &[String],
    ignore: &[String],
    report: &mut SyncReport,
) -> Result<()> {
    for pattern in patterns {
        let full_pattern = source.join(pattern);
        let pat_str = full_pattern.to_str().unwrap_or("");

        let entries = glob::glob(pat_str).context("invalid glob pattern")?;

        for entry in entries.flatten() {
            let rel_path = match entry.strip_prefix(source) {
                Ok(p) => p.to_path_buf(),
                Err(_) => continue,
            };

            let file_name = match entry.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };

            if ignore.iter().any(|i| i == &file_name) {
                continue;
            }

            if !entry.is_file() {
                continue;
            }

            let dst = target.join(&rel_path);
            // Never overwrite an existing file (idempotent).
            if dst.exists() {
                continue;
            }

            if let Some(parent) = dst.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent)?;
                }
            }

            let display_path = rel_path.display().to_string();
            if let Err(e) = std::fs::copy(&entry, &dst) {
                report.warnings.push(format!("could not copy {display_path}: {e}"));
            } else {
                report.copied.push(display_path);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn setup_dirs() -> (std::path::PathBuf, std::path::PathBuf) {
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("workz_test_{}_{}", std::process::id(), id));
        let source = base.join("source");
        let target = base.join("target");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        (source, target)
    }

    #[test]
    fn test_copy_files_flat() {
        let (source, target) = setup_dirs();
        fs::write(source.join(".env"), "SECRET=abc").unwrap();

        let mut report = SyncReport::default();
        copy_files(&source, &target, &[".env".into()], &[], &mut report).unwrap();

        assert!(target.join(".env").exists());
        assert_eq!(fs::read_to_string(target.join(".env")).unwrap(), "SECRET=abc");
        assert_eq!(report.copied, vec![".env".to_string()]);
    }

    #[test]
    fn test_copy_files_nested() {
        let (source, target) = setup_dirs();
        fs::create_dir_all(source.join(".claude")).unwrap();
        fs::write(source.join(".claude/settings.local.json"), r#"{"key":1}"#).unwrap();

        let mut report = SyncReport::default();
        copy_files(&source, &target, &[".claude/settings.local.json".into()], &[], &mut report).unwrap();

        assert!(target.join(".claude/settings.local.json").exists());
        assert_eq!(
            fs::read_to_string(target.join(".claude/settings.local.json")).unwrap(),
            r#"{"key":1}"#
        );
        assert!(!target.join("settings.local.json").exists());
    }

    #[test]
    fn test_copy_files_ignore() {
        let (source, target) = setup_dirs();
        fs::write(source.join(".env"), "SECRET=abc").unwrap();
        fs::write(source.join(".env.local"), "LOCAL=1").unwrap();

        let mut report = SyncReport::default();
        copy_files(&source, &target, &[".env*".into()], &[".env.local".into()], &mut report).unwrap();

        assert!(target.join(".env").exists());
        assert!(!target.join(".env.local").exists());
    }

    #[test]
    fn test_copy_files_no_overwrite() {
        let (source, target) = setup_dirs();
        fs::write(source.join(".env"), "NEW").unwrap();
        fs::write(target.join(".env"), "EXISTING").unwrap();

        let mut report = SyncReport::default();
        copy_files(&source, &target, &[".env".into()], &[], &mut report).unwrap();

        assert_eq!(fs::read_to_string(target.join(".env")).unwrap(), "EXISTING");
    }

    #[test]
    fn test_copy_files_idempotent() {
        let (source, target) = setup_dirs();
        fs::write(source.join(".env"), "SECRET=abc").unwrap();

        let mut first = SyncReport::default();
        copy_files(&source, &target, &[".env".into()], &[], &mut first).unwrap();
        assert_eq!(first.copied, vec![".env".to_string()]);

        // Second run: file already present → nothing copied, no warnings.
        let mut second = SyncReport::default();
        copy_files(&source, &target, &[".env".into()], &[], &mut second).unwrap();
        assert!(second.copied.is_empty());
        assert!(second.warnings.is_empty());
    }
}
