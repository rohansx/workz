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
    ["node_modules", "target", ".venv", "venv", ".direnv", "vendor"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn default_copy_patterns() -> Vec<String> {
    [".env", ".env.*", ".env*", ".envrc", ".tool-versions"]
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


/// Load config from `.workz.toml` in the repo root, falling back to defaults.
pub fn load_config(repo_root: &Path) -> Result<Config> {
    let config_path = repo_root.join(CONFIG_FILE);
    if config_path.exists() {
        let contents = std::fs::read_to_string(&config_path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    } else {
        Ok(Config::default())
    }
}
