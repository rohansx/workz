use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

const CONFIG_FILE: &str = ".workz.toml";

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
}

#[derive(Debug, Deserialize)]
pub struct SyncConfig {
    /// Directories to symlink into worktrees (saves disk space)
    #[serde(default = "default_symlink_dirs")]
    pub symlink: Vec<String>,

    /// File patterns to copy into worktrees
    #[serde(default = "default_copy_patterns")]
    pub copy: Vec<String>,

    /// Patterns to never touch
    #[serde(default)]
    pub ignore: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct HooksConfig {
    /// Shell command to run after worktree creation
    #[serde(default)]
    pub post_start: Option<String>,

    /// Shell command to run before worktree removal
    #[serde(default)]
    pub pre_done: Option<String>,
}

fn default_symlink_dirs() -> Vec<String> {
    [
        // JavaScript / Node
        "node_modules",
        // Rust
        "target",
        // Python
        ".venv",
        "venv",
        "__pycache__",
        ".mypy_cache",
        ".pytest_cache",
        ".ruff_cache",
        // Go
        "vendor",
        // JS framework caches
        ".next",
        ".nuxt",
        ".svelte-kit",
        ".turbo",
        ".parcel-cache",
        ".angular",
        // Java / Kotlin
        ".gradle",
        "build",
        // General
        ".direnv",
        ".cache",
        // IDE configs
        ".vscode",
        ".idea",
        ".cursor",
        ".claude",
        ".zed",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_copy_patterns() -> Vec<String> {
    [
        // Environment files
        ".env",
        ".env.*",
        ".env*",
        ".envrc",
        // Tool versions
        ".tool-versions",
        ".node-version",
        ".python-version",
        ".ruby-version",
        ".nvmrc",
        // Package manager configs (may contain local registry tokens)
        ".npmrc",
        ".yarnrc.yml",
        // Docker overrides
        "docker-compose.override.yml",
        "docker-compose.override.yaml",
        // Secrets
        ".secrets",
        ".secrets.*",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            symlink: default_symlink_dirs(),
            copy: default_copy_patterns(),
            ignore: Vec::new(),
        }
    }
}


/// Load config: global (~/.config/workz/config.toml) merged with project (.workz.toml).
/// Project config takes priority over global config.
pub fn load_config(repo_root: &Path) -> Result<Config> {
    let global = load_global_config();
    let project = load_project_config(repo_root);

    match (global, project) {
        (Some(g), Some(p)) => Ok(merge_configs(g, p)),
        (None, Some(p)) => Ok(p),
        (Some(g), None) => Ok(g),
        (None, None) => Ok(Config::default()),
    }
}

fn load_global_config() -> Option<Config> {
    let config_dir = dirs::config_dir()?;
    let path = config_dir.join("workz").join("config.toml");
    if !path.exists() {
        return None;
    }
    let contents = std::fs::read_to_string(&path).ok()?;
    toml::from_str(&contents).ok()
}

fn load_project_config(repo_root: &Path) -> Option<Config> {
    let path = repo_root.join(CONFIG_FILE);
    if !path.exists() {
        return None;
    }
    let contents = std::fs::read_to_string(&path).ok()?;
    toml::from_str(&contents).ok()
}

/// Merge two configs. Project values override global values.
fn merge_configs(global: Config, project: Config) -> Config {
    let default_sync = SyncConfig::default();

    // If project specifies sync values, use them; otherwise fall back to global
    let is_project_sync_default = project.sync.symlink == default_sync.symlink
        && project.sync.copy == default_sync.copy
        && project.sync.ignore.is_empty();

    let sync = if is_project_sync_default {
        // Project didn't customize sync, use global
        global.sync
    } else {
        project.sync
    };

    let hooks = HooksConfig {
        post_start: project.hooks.post_start.or(global.hooks.post_start),
        pre_done: project.hooks.pre_done.or(global.hooks.pre_done),
    };

    Config { sync, hooks }
}
