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
    /// Directories successfully reflinked (v0.13). Distinct from `copied` —
    /// a `cloned` entry is CoW-shared with the source until first write.
    pub cloned: Vec<String>,
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
        if !self.cloned.is_empty() {
            lines.push(format!("  cloned (reflink) {}", self.cloned.join(", ")));
        }
        if !self.copied.is_empty() {
            lines.push(format!("  copied {}", self.copied.join(", ")));
        }
        if let Some(cmd) = &self.installed {
            lines.push(format!("  installed deps ({cmd})"));
        }
        if lines.is_empty() {
            // Don't claim success when declared work was skipped — that message
            // is what made the failure invisible in issue #32.
            if self.warnings.is_empty() {
                "  nothing to sync (already up to date)".to_string()
            } else {
                "  nothing synced — see the warnings below".to_string()
            }
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
    let plan = config.resolve();
    let mut report = SyncReport {
        framework: project.framework,
        ..Default::default()
    };
    symlink_dirs(source, target, &plan.symlink_dirs, &plan.declared, &project, &mut report);
    clone_dirs(source, target, &plan.clone_dirs, &project, &mut report);
    copy_dirs(source, target, &plan.copy_dirs, &project, &mut report);
    copy_files(source, target, &plan.copy_globs, &plan.ignore, &mut report)?;
    if !opts.no_install {
        auto_install(source, target, &project, opts.quiet, &mut report);
    }
    Ok(report)
}

/// A human-facing summary of what workz detected in a repo — used by `workz init`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProjectSummary {
    pub languages: Vec<String>,
    pub install_cmd: Option<String>,
    pub framework: Option<String>,
    pub has_docker: bool,
    pub is_monorepo: bool,
}

/// Detect the project ecosystem for the setup wizard.
pub fn detect_summary(root: &Path) -> ProjectSummary {
    let p = detect_project(root);

    let mut languages = Vec::new();
    if p.has_node {
        languages.push("Node.js".to_string());
    }
    if p.has_rust {
        languages.push("Rust".to_string());
    }
    if p.has_python {
        languages.push("Python".to_string());
    }
    if p.has_go {
        languages.push("Go".to_string());
    }
    if p.has_java {
        languages.push("Java/Kotlin".to_string());
    }

    let install_cmd = p
        .node_install_cmd
        .as_ref()
        .or(p.python_install_cmd.as_ref())
        .map(|c| c.join(" "));

    let framework = match p.framework {
        Framework::Unknown => None,
        f => Some(format!("{f:?}")),
    };

    let has_docker = ["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"]
        .iter()
        .any(|f| root.join(f).exists());

    let is_monorepo = root.join("pnpm-workspace.yaml").exists()
        || root.join("lerna.json").exists()
        || std::fs::read_to_string(root.join("Cargo.toml"))
            .map(|c| c.contains("[workspace]"))
            .unwrap_or(false);

    ProjectSummary { languages, install_cmd, framework, has_docker, is_monorepo }
}

/// Whether a heavy dir is relevant to this project (exposed for `workz init`).
pub fn node_project(root: &Path) -> bool {
    detect_project(root).has_node
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
/// `dirs` is already ignore-filtered by [`SyncConfig::resolve`].
/// Symlink one entry, reporting *why* nothing happened when it doesn't.
///
/// `declared` marks entries the user asked for explicitly (`symlink_add`).
/// Those get a warning on every skip path — a declared path that silently
/// doesn't get linked leaves the worktree running on private state while
/// workz reports success (issue #32). Built-in defaults stay quiet, so a
/// Python project doesn't get told about `node_modules` on every sync.
fn symlink_one(
    source: &Path,
    target: &Path,
    rel: &str,
    declared: bool,
    report: &mut SyncReport,
) {
    let src = source.join(rel);
    let dst = target.join(rel);

    if !src.exists() {
        if declared {
            report.warnings.push(format!(
                "skipped {rel}: not found in the main worktree — nothing was linked \
                 (check the path, or bootstrap it before creating worktrees)"
            ));
        }
        return;
    }

    // Never overwrite an existing file, dir, or symlink (idempotent). An
    // already-correct symlink is a no-op, not a problem — only report the case
    // where something real is standing in the way.
    if let Ok(meta) = dst.symlink_metadata() {
        if declared && !meta.file_type().is_symlink() {
            report.warnings.push(format!(
                "skipped {rel}: path already exists in the worktree and is not a symlink \
                 (a tracked file inside an ignored directory will do this) — it was NOT linked"
            ));
        }
        return;
    }

    // Parent must exist for nested entries like `website/node_modules`.
    if let Some(parent) = dst.parent() {
        if !parent.exists() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                report
                    .warnings
                    .push(format!("skipped {rel}: could not create parent directory: {e}"));
                return;
            }
        }
    }

    match create_symlink(&src, &dst) {
        Err(e) => report.warnings.push(format!("could not symlink {rel}: {e}")),
        Ok(()) => {
            // A synced symlink that git doesn't ignore will show as untracked in
            // every worktree, and `git add -A` then commits a symlink containing
            // one machine's absolute path — broken for everyone else and for CI.
            // The usual cause is a trailing-slash pattern: `.cache/` matches a
            // directory but NOT a symlink named `.cache` (issue #34).
            if !is_git_ignored(target, rel) {
                report.warnings.push(format!(
                    "{rel} is symlinked but NOT gitignored — `git add -A` would commit an \
                     absolute path. If .gitignore has `{rel}/`, drop the trailing slash so it \
                     matches the symlink too"
                ));
            }
            report.symlinked.push(rel.to_string());
        }
    }
}

/// Whether git ignores `rel` inside `dir`. Uses `git check-ignore`, so it
/// honours every ignore source git does (repo, global, excludes). Returns
/// `true` when git can't be consulted — we only warn on a definite "not
/// ignored", never on uncertainty.
fn is_git_ignored(dir: &Path, rel: &str) -> bool {
    std::process::Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "check-ignore", "-q", rel])
        .status()
        .map(|s| s.success())
        .unwrap_or(true)
}

fn symlink_dirs(
    source: &Path,
    target: &Path,
    dirs: &[String],
    declared: &[String],
    project: &ProjectInfo,
    report: &mut SyncReport,
) {
    for dir_name in dirs {
        let is_declared = declared.iter().any(|d| d == dir_name);

        // Relevance filtering applies to built-in defaults only: an explicit
        // `symlink_add` entry means the user knows their layout better than our
        // project sniffing does (which is why `website/node_modules` in a
        // monorepo used to vanish — issue #36).
        if !is_declared && !is_relevant(dir_name, project) {
            continue;
        }

        // Glob entries expand to their matches (issue #33). Previously the
        // pattern was joined as a literal path, never existed, and was dropped
        // in silence.
        if dir_name.contains('*') {
            let pattern = source.join(dir_name);
            let matches: Vec<std::path::PathBuf> = match glob::glob(&pattern.to_string_lossy()) {
                Ok(paths) => paths.filter_map(Result::ok).collect(),
                Err(e) => {
                    report
                        .warnings
                        .push(format!("skipped {dir_name}: invalid glob pattern ({e})"));
                    continue;
                }
            };
            if matches.is_empty() {
                if is_declared {
                    report.warnings.push(format!(
                        "skipped {dir_name}: glob matched nothing in the main worktree"
                    ));
                }
                continue;
            }
            for m in matches {
                if let Ok(rel) = m.strip_prefix(source) {
                    symlink_one(source, target, &rel.to_string_lossy(), is_declared, report);
                }
            }
            continue;
        }

        symlink_one(source, target, dir_name, is_declared, report);
    }
}

/// CoW reflink heavy directories from source into target (project-aware, v0.13).
/// This is the magic: the destination is a real, fully-independent copy of the
/// source as far as the kernel is concerned, but the bytes aren't duplicated
/// until either side writes — so a 2 GB `node_modules` "clones" in milliseconds
/// and the two trees share storage until the worktree dirties it (the agent
/// can't poison the main tree's deps, and the main tree's `--frozen-lockfile`
/// stays clean for commits).
///
/// Auto-selects the right tool per platform:
/// - Linux:  `cp --reflink=auto` (auto-falls-back to a regular copy)
/// - macOS:  `cp -c` (clonefile)
/// - Other:  falls back to a recursive copy with a warning
///
/// If reflink is not supported on the source filesystem (some btrfs setups
/// disable it at the FS level), we fall back to a full copy and emit a
/// `warnings` entry — the user can then switch to `copy` or `symlink` to
/// silence the warning.
fn clone_dirs(
    source: &Path,
    target: &Path,
    dirs: &[String],
    project: &ProjectInfo,
    report: &mut SyncReport,
) {
    for dir_name in dirs {
        if !is_relevant(dir_name, project) {
            continue;
        }
        let src = source.join(dir_name);
        let dst = target.join(dir_name);
        if !src.exists() {
            continue;
        }
        // Idempotent.
        if dst.exists() || dst.symlink_metadata().is_ok() {
            continue;
        }
        match reflink_copy_dir(&src, &dst) {
            Ok(ReflinkOutcome::Reflinked) => {
                report.cloned.push(dir_name.clone());
            }
            Ok(ReflinkOutcome::CopiedFallback) => {
                report.warnings.push(format!(
                    "{dir_name}: filesystem doesn't support reflink, fell back to a full copy"
                ));
                report.copied.push(format!("{dir_name}/"));
            }
            Err(e) => {
                report.warnings.push(format!("could not clone {dir_name}: {e}"));
            }
        }
    }
}

/// Outcome of a reflink attempt — distinguishes "did the CoW magic" from
/// "degraded to a full copy" so the caller can report on it.
#[derive(Debug, PartialEq, Eq)]
pub enum ReflinkOutcome {
    /// The destination is a CoW clone sharing storage with the source.
    Reflinked,
    /// Reflink was unsupported; we did a regular recursive copy instead.
    CopiedFallback,
}

/// Copy `src` directory to `dst` using CoW reflink when the filesystem
/// supports it; otherwise do a regular recursive copy.
fn reflink_copy_dir(src: &Path, dst: &Path) -> Result<ReflinkOutcome> {
    // Trust the tool instead of inspecting the result (#40). A CoW clone is
    // *not* a hardlink — `clonefile(2)` on APFS and `cp --reflink` on
    // btrfs/XFS both allocate a new inode that shares extents with the source
    // — so the old `src.ino() == dst.ino()` probe tested for a hardlink and
    // could never be true, making `Reflinked` unreachable and every clone
    // report a false "filesystem doesn't support reflink" warning.
    //
    // Both platforms give us an honest exit code if we ask for it:
    //   - Linux: `--reflink=always` fails when the FS can't clone (`=auto`
    //     deliberately degrades in silence, which is why it can't be used here).
    //   - macOS: `cp -c` errors out rather than silently degrading.
    // So a successful run *is* the confirmation, and a failure is the signal to
    // fall back to a plain recursive copy.
    #[cfg(target_os = "linux")]
    let clone_args: Option<&[&str]> = Some(&["-R", "--reflink=always"]);
    #[cfg(target_os = "macos")]
    let clone_args: Option<&[&str]> = Some(&["-R", "-c"]);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let clone_args: Option<&[&str]> = None;

    if let Some(args) = clone_args {
        let status = std::process::Command::new("cp")
            .args(args)
            .arg(src)
            .arg(dst)
            .status()
            .context("failed to spawn cp")?;
        if status.success() {
            return Ok(ReflinkOutcome::Reflinked);
        }
        // Clone unsupported (or partially written) — clear any partial result
        // before the fallback so the copy starts from a clean destination.
        let _ = std::fs::remove_dir_all(dst);
    }

    copy_dir_recursive_simple(src, dst);
    Ok(ReflinkOutcome::CopiedFallback)
}



/// Probe whether the filesystem containing `dir` supports CoW reflink. Writes
/// a tiny temp file, attempts to reflink it, and checks whether the two
/// files share an inode. Returns `None` if the probe can't be run (no temp
/// dir, no write permission, etc.) — callers should treat that as "unknown"
/// and not recommend `clone`.
///
/// Cached per directory by the caller when used in a hot path; here we
/// always re-probe because the cost is one stat and one tiny file write.
pub fn probe_reflink_support(dir: &Path) -> Option<bool> {
    let probe_dir = dir.join(".workz-probe");
    std::fs::create_dir_all(&probe_dir).ok()?;
    let src = probe_dir.join("src");
    let dst = probe_dir.join("dst");

    // 4 KiB is the smallest block on every common FS — enough to get past any
    // "empty file" optimizations cp might apply.
    let data = b"workz-reflink-probe\n".repeat(256);
    std::fs::write(&src, &data).ok()?;

    // `--reflink=always` / `-c` fail when the filesystem can't clone, so the
    // exit status is the answer. (`--reflink=auto` must NOT be used here: it
    // degrades to a plain copy silently and would always report success.)
    // The old inode comparison was a hardlink test and always said "no" (#40).
    #[cfg(target_os = "linux")]
    let status = std::process::Command::new("cp")
        .args(["--reflink=always", src.to_str()?, dst.to_str()?])
        .status();
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("cp")
        .args(["-c", src.to_str()?, dst.to_str()?])
        .status();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let status: std::io::Result<std::process::ExitStatus> =
        Err(std::io::Error::new(std::io::ErrorKind::Other, "unsupported"));

    let supported = status.ok()?.success();

    // Clean up regardless of outcome.
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
    let _ = std::fs::remove_dir(&probe_dir);

    Some(supported)
}

/// Plain recursive copy as a fallback when reflink is unavailable. Mirrors
/// the existing `copy_dir_recursive` but isolated so reflink failures don't
/// double-report as "symlinked" or "cloned" in the report.
fn copy_dir_recursive_simple(src: &Path, dst: &Path) {
    if std::fs::create_dir_all(dst).is_err() {
        return;
    }
    let entries = match std::fs::read_dir(src) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if let Ok(ft) = entry.file_type() {
            if ft.is_dir() {
                copy_dir_recursive_simple(&from, &to);
            } else if !to.exists() {
                let _ = std::fs::copy(&from, &to);
            }
        }
    }
}

/// Physically copy directories (the `copy` strategy override) — the escape hatch
/// for tools that break on symlinked node_modules (Vite/Vitest/pnpm monorepos).
fn copy_dirs(
    source: &Path,
    target: &Path,
    dirs: &[String],
    project: &ProjectInfo,
    report: &mut SyncReport,
) {
    for dir_name in dirs {
        if !is_relevant(dir_name, project) {
            continue;
        }
        let src = source.join(dir_name);
        let dst = target.join(dir_name);
        if !src.exists() {
            continue;
        }
        // Idempotent: don't re-copy if the target dir already exists.
        if dst.exists() || dst.symlink_metadata().is_ok() {
            continue;
        }
        copy_dir_recursive(&src, &dst, report);
        report.copied.push(format!("{dir_name}/"));
    }
}

/// Recursively copy `src` into `dst`, recording any failures as warnings.
fn copy_dir_recursive(src: &Path, dst: &Path, report: &mut SyncReport) {
    if let Err(e) = std::fs::create_dir_all(dst) {
        report.warnings.push(format!("could not create {}: {e}", dst.display()));
        return;
    }
    let entries = match std::fs::read_dir(src) {
        Ok(e) => e,
        Err(e) => {
            report.warnings.push(format!("could not read {}: {e}", src.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => copy_dir_recursive(&from, &to, report),
            Ok(_) => {
                if !to.exists() {
                    if let Err(e) = std::fs::copy(&from, &to) {
                        report.warnings.push(format!("could not copy {}: {e}", from.display()));
                    }
                }
            }
            Err(e) => report.warnings.push(format!("stat failed {}: {e}", from.display())),
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
    fn declared_entry_missing_at_source_is_reported() {
        // Regression for #32: the highest-impact case — a declared path that
        // doesn't exist in the main checkout was skipped in total silence, so
        // every worktree quietly ran on private state.
        let (source, target) = setup_dirs();
        let mut report = SyncReport::default();
        let declared = vec!["data/ledger.jsonl".to_string()];
        symlink_dirs(
            &source,
            &target,
            &declared,
            &declared,
            &ProjectInfo::default(),
            &mut report,
        );
        assert!(report.symlinked.is_empty());
        assert_eq!(report.warnings.len(), 1, "must warn, got {:?}", report.warnings);
        assert!(report.warnings[0].contains("not found in the main worktree"));
    }

    #[test]
    fn declared_entry_blocked_by_a_real_path_is_reported() {
        // #32(b): a gitignored dir holding one tracked file — git materialises
        // the directory, so workz can never symlink it. Must say so.
        let (source, target) = setup_dirs();
        fs::create_dir_all(source.join("models")).unwrap();
        fs::create_dir_all(target.join("models")).unwrap();
        fs::write(target.join("models/README.md"), "index").unwrap();

        let mut report = SyncReport::default();
        let declared = vec!["models".to_string()];
        symlink_dirs(&source, &target, &declared, &declared, &ProjectInfo::default(), &mut report);
        assert!(report.symlinked.is_empty());
        assert!(
            report.warnings.iter().any(|w| w.contains("not a symlink")),
            "got {:?}",
            report.warnings
        );
    }

    #[test]
    fn built_in_defaults_stay_quiet_when_they_dont_apply() {
        // The other half of #32: warning on every irrelevant built-in would
        // bury the real signal. Only user-declared entries are noisy.
        let (source, target) = setup_dirs();
        let mut report = SyncReport::default();
        let builtins = vec!["node_modules".to_string(), "target".to_string()];
        symlink_dirs(
            &source,
            &target,
            &builtins,
            &[], // nothing declared by the user
            &ProjectInfo::default(),
            &mut report,
        );
        assert!(report.warnings.is_empty(), "built-ins must be quiet: {:?}", report.warnings);
    }

    #[test]
    fn glob_in_symlink_add_expands_and_links_matches() {
        // #33: globs were joined as a literal path, never matched, and vanished.
        let (source, target) = setup_dirs();
        fs::create_dir_all(source.join("models/sub")).unwrap();
        fs::write(source.join("models/a.gguf"), "w").unwrap();

        let mut report = SyncReport::default();
        let declared = vec!["models/*".to_string()];
        symlink_dirs(&source, &target, &declared, &declared, &ProjectInfo::default(), &mut report);

        assert_eq!(report.symlinked.len(), 2, "both matches link: {:?}", report.symlinked);
        assert!(target.join("models/a.gguf").symlink_metadata().unwrap().file_type().is_symlink());
        assert!(target.join("models/sub").symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[test]
    fn declared_entry_bypasses_the_project_relevance_filter() {
        // #36: `website/node_modules` in a monorepo with no root package.json was
        // dropped by is_relevant(). An explicit declaration beats our sniffing,
        // and the nested parent directory gets created.
        let (source, target) = setup_dirs();
        fs::create_dir_all(source.join("website/node_modules")).unwrap();

        let mut report = SyncReport::default();
        let declared = vec!["website/node_modules".to_string()];
        symlink_dirs(&source, &target, &declared, &declared, &ProjectInfo::default(), &mut report);
        assert_eq!(report.symlinked, vec!["website/node_modules".to_string()]);
    }

    #[test]
    fn summary_does_not_claim_success_when_work_was_skipped() {
        // The message that made #32 invisible: "nothing to sync (already up to
        // date)" printed while declared entries had been dropped.
        let mut report = SyncReport::default();
        assert!(report.human_summary().contains("already up to date"));
        report.warnings.push("skipped x: not found".to_string());
        assert!(report.human_summary().contains("see the warnings"));
        assert!(!report.human_summary().contains("already up to date"));
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

    // ── reflink / clone tests ──────────────────────────────────────────────

    #[test]
    fn reflink_copy_dir_reports_reflinked_or_fallback() {
        // The outcome depends on the host FS — we assert the contract: the
        // returned outcome is one of the two valid variants, and the dst dir
        // exists either way.
        let (source, target) = setup_dirs();
        let cache = source.join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("a.txt"), "hello").unwrap();
        fs::write(cache.join("b.txt"), "world").unwrap();

        let outcome = reflink_copy_dir(&cache, &target.join("cache")).unwrap();
        assert!(
            outcome == ReflinkOutcome::Reflinked || outcome == ReflinkOutcome::CopiedFallback,
            "outcome must be one of the two"
        );
        // Either way, the destination must exist with the same files.
        assert!(target.join("cache/a.txt").exists());
        assert!(target.join("cache/b.txt").exists());
        assert_eq!(fs::read_to_string(target.join("cache/a.txt")).unwrap(), "hello");
    }

    #[test]
    fn reflink_probe_agrees_with_the_filesystem() {
        // Regression for #40: the probe used to compare inodes, which is a
        // *hardlink* test — a CoW clone always allocates a new inode, so it
        // reported "unsupported" even on APFS/btrfs where cloning works.
        // Ground-truth the filesystem independently and assert the probe agrees;
        // this passes on a non-reflink FS (both false) and on a reflink FS (both
        // true), where the old inode logic would have failed.
        let (source, _target) = setup_dirs();
        let a = source.join("probe_src");
        fs::write(&a, b"x".repeat(4096)).unwrap();
        let b = source.join("probe_dst");

        #[cfg(target_os = "linux")]
        let args: &[&str] = &["--reflink=always"];
        #[cfg(target_os = "macos")]
        let args: &[&str] = &["-c"];
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let args: &[&str] = &[];

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let truth = std::process::Command::new("cp")
                .args(args)
                .arg(&a)
                .arg(&b)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            let _ = fs::remove_file(&b);
            assert_eq!(
                probe_reflink_support(&source),
                Some(truth),
                "probe must match what the filesystem actually does"
            );
        }
    }

    #[test]
    fn clone_dirs_routes_to_cloned_or_copied() {
        // The project info doesn't matter for cache_dir / a generic "cache" entry
        // because `is_relevant` returns true for unknown dirs.
        let (source, target) = setup_dirs();
        let cache = source.join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("file.txt"), "data").unwrap();

        let project = ProjectInfo::default();
        let mut report = SyncReport::default();
        clone_dirs(&source, &target, &["cache".to_string()], &project, &mut report);

        // Exactly one of cloned/copied is populated; the other is empty.
        assert!(
            !report.cloned.is_empty() || !report.copied.is_empty(),
            "sync should have either cloned or copied the dir"
        );
        assert_eq!(
            report.cloned.len() + report.copied.len(),
            1,
            "exactly one outcome must be recorded, got cloned={:?} copied={:?}",
            report.cloned,
            report.copied
        );
        // And dst must exist.
        assert!(target.join("cache/file.txt").exists());
    }

    #[test]
    fn clone_dirs_skips_irrelevant_for_project() {
        // A Node-only project should not be cloning a "target" (Rust) dir
        // even if it's listed in clone_dirs.
        let (source, target) = setup_dirs();
        fs::create_dir_all(source.join("target")).unwrap();
        fs::write(source.join("target/x"), "x").unwrap();

        let project = ProjectInfo {
            has_node: true,
            ..Default::default()
        };
        let mut report = SyncReport::default();
        clone_dirs(&source, &target, &["target".to_string()], &project, &mut report);
        assert!(report.cloned.is_empty());
        assert!(report.copied.is_empty());
        // src untouched.
        assert!(!target.join("target").exists());
    }

    #[test]
    fn clone_dirs_idempotent_no_overwrite() {
        // Pre-existing destination → skip entirely.
        let (source, target) = setup_dirs();
        let cache = source.join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("file.txt"), "new").unwrap();
        fs::create_dir_all(target.join("cache")).unwrap();
        fs::write(target.join("cache/file.txt"), "old").unwrap();

        let project = ProjectInfo::default();
        let mut report = SyncReport::default();
        clone_dirs(&source, &target, &["cache".to_string()], &project, &mut report);
        // The "old" content must survive — we never overwrite.
        assert_eq!(fs::read_to_string(target.join("cache/file.txt")).unwrap(), "old");
        assert!(report.cloned.is_empty());
        assert!(report.copied.is_empty());
    }

    #[test]
    fn sync_worktree_with_clone_strategy_in_config() {
        // End-to-end: a SyncConfig with a `clone` override actually drives
        // the reflink path.
        let (source, target) = setup_dirs();
        let cache = source.join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("file.txt"), "data").unwrap();

        // Parse as the full Config (overrides live under [sync.overrides]).
        let toml = "[sync.overrides]\ncache = \"clone\"\n";
        let cfg: crate::config::Config = toml::from_str(toml).unwrap();
        let opts = SyncOptions { no_install: true, quiet: true };
        let report = sync_worktree(&source, &target, &cfg.sync, opts).unwrap();

        // Resulting state: clone_dirs had "cache" in it (via resolve()).
        let plan = cfg.sync.resolve();
        assert!(plan.clone_dirs.contains(&"cache".to_string()));
        // sync reported either cloned or copied the cache.
        assert!(
            !report.cloned.is_empty() || !report.copied.is_empty(),
            "sync_worktree should have processed the clone strategy"
        );
        assert!(target.join("cache/file.txt").exists());
    }
}
