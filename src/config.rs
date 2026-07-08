use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

const CONFIG_FILE: &str = ".workz.toml";

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
    #[serde(default)]
    pub isolation: IsolationConfig,
}

/// How a single directory/entry should be handled, overriding the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    /// Symlink the directory (default for heavy dirs — saves disk).
    Symlink,
    /// Physically copy the directory (escape hatch for tools that break on
    /// symlinked node_modules, e.g. Vite / Vitest / pnpm monorepos).
    Copy,
    /// Copy-on-write reflink of the directory (v0.13). Auto-selected on
    /// btrfs/XFS/APFS — gives each worktree its own fully-isolated copy
    /// of `node_modules` etc. without the multi-minute install cost and
    /// without sharing writes with the main tree (the agent-mutation
    /// poisoning problem). Falls back to a full `Copy` with a warning on
    /// filesystems that don't support reflink.
    Clone,
    /// Never touch this entry.
    Ignore,
}

#[derive(Debug, Default, Deserialize)]
pub struct SyncConfig {
    /// Base directories to symlink. `None` → built-in defaults ([`default_symlink_dirs`]).
    /// Setting this replaces the defaults; use `symlink_add` to extend them instead.
    pub symlink: Option<Vec<String>>,

    /// Base file globs to copy. `None` → built-in defaults ([`default_copy_patterns`]).
    /// Setting this replaces the defaults; use `copy_add` to extend them instead.
    pub copy: Option<Vec<String>>,

    /// Base entries to never touch. `None` → empty; use `ignore_add` to extend.
    pub ignore: Option<Vec<String>>,

    /// Additive: extra directories to symlink on top of the base list.
    #[serde(default)]
    pub symlink_add: Vec<String>,

    /// Additive: extra file globs to copy on top of the base list.
    #[serde(default)]
    pub copy_add: Vec<String>,

    /// Additive: extra entries to ignore on top of the base list.
    #[serde(default)]
    pub ignore_add: Vec<String>,

    /// Per-entry strategy override: `name = "symlink" | "copy" | "ignore"`.
    #[serde(default)]
    pub overrides: HashMap<String, Strategy>,
}

/// A resolved sync plan — the concrete set of operations after applying defaults,
/// additive keys, and per-entry overrides.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncPlan {
    /// Directories to symlink.
    pub symlink_dirs: Vec<String>,
    /// Directories to recursively copy (override = `copy`).
    pub copy_dirs: Vec<String>,
    /// Directories to CoW reflink (override = `clone`, v0.13). On filesystems
    /// without reflink support these silently fall back to a full copy.
    pub clone_dirs: Vec<String>,
    /// File globs to copy.
    pub copy_globs: Vec<String>,
    /// Entries to skip (used for glob-level filtering during copy).
    pub ignore: Vec<String>,
}

impl SyncConfig {
    /// Resolve the raw config into a concrete [`SyncPlan`].
    pub fn resolve(&self) -> SyncPlan {
        let base_symlink = self.symlink.clone().unwrap_or_else(default_symlink_dirs);
        let base_copy = self.copy.clone().unwrap_or_else(default_copy_patterns);

        // Assemble the ignore set: base ignore + ignore_add + overrides marked ignore.
        let mut ignore: Vec<String> = self
            .ignore
            .clone()
            .unwrap_or_default()
            .into_iter()
            .chain(self.ignore_add.iter().cloned())
            .collect();
        for (name, strat) in &self.overrides {
            if *strat == Strategy::Ignore {
                ignore.push(name.clone());
            }
        }
        dedup(&mut ignore);
        let is_ignored = |name: &str| ignore.iter().any(|i| i == name);

        // Split the symlink candidates by override strategy.
        let mut symlink_dirs = Vec::new();
        let mut copy_dirs = Vec::new();
        let mut clone_dirs = Vec::new();
        for dir in base_symlink.iter().chain(self.symlink_add.iter()) {
            if is_ignored(dir) {
                continue;
            }
            match self.overrides.get(dir) {
                Some(Strategy::Copy) => copy_dirs.push(dir.clone()),
                Some(Strategy::Clone) => clone_dirs.push(dir.clone()),
                Some(Strategy::Ignore) => {} // already in ignore
                _ => symlink_dirs.push(dir.clone()),
            }
        }

        // Also honor overrides for entries NOT in the symlink list — lets users
        // turn an arbitrary directory into a clone/copy without first adding
        // it to `symlink_add`. Common case: `[sync.overrides] cache = "clone"`
        // for a project whose defaults don't list `cache`.
        for (dir, strat) in &self.overrides {
            if base_symlink.contains(dir) || self.symlink_add.contains(dir) || is_ignored(dir) {
                continue;
            }
            match strat {
                Strategy::Copy => copy_dirs.push(dir.clone()),
                Strategy::Clone => clone_dirs.push(dir.clone()),
                Strategy::Ignore | Strategy::Symlink => {} // symlink is the default; ignore already handled
            }
        }

        let copy_globs: Vec<String> = base_copy
            .iter()
            .chain(self.copy_add.iter())
            .filter(|g| !is_ignored(g))
            .cloned()
            .collect();

        dedup(&mut symlink_dirs);
        dedup(&mut copy_dirs);
        dedup(&mut clone_dirs);
        let mut copy_globs = copy_globs;
        dedup(&mut copy_globs);

        SyncPlan { symlink_dirs, copy_dirs, clone_dirs, copy_globs, ignore }
    }
}

/// Remove duplicates while preserving first-seen order.
fn dedup(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|x| seen.insert(x.clone()));
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

#[derive(Debug, Deserialize)]
pub struct IsolationConfig {
    /// Number of ports to allocate per worktree (default: 10)
    #[serde(default = "default_port_range_size")]
    pub port_range_size: u16,

    /// Base port for first worktree (default: 3000)
    #[serde(default = "default_base_port")]
    pub base_port: u16,

    /// Named services to allocate ports for (v0.14). Each name gets one
    /// port from the range, in the order listed. The first named service
    /// is also exposed as `PORT` (backward compat); the rest get
    /// `PORT_<UPPERCASE_NAME>`. Empty/missing = no named services (the
    /// single `PORT` + framework-specific vars still apply).
    #[serde(default)]
    pub services: Vec<String>,
}

fn default_port_range_size() -> u16 { 10 }
fn default_base_port() -> u16 { 3000 }

impl Default for IsolationConfig {
    fn default() -> Self {
        Self {
            port_range_size: default_port_range_size(),
            base_port: default_base_port(),
            services: Vec::new(),
        }
    }
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

/// Merge two configs. For base lists, project replaces global when set (`.or`);
/// additive `*_add` keys concatenate; `overrides` merge per key (project wins).
fn merge_configs(global: Config, project: Config) -> Config {
    let sync = SyncConfig {
        symlink: project.sync.symlink.or(global.sync.symlink),
        copy: project.sync.copy.or(global.sync.copy),
        ignore: project.sync.ignore.or(global.sync.ignore),
        symlink_add: concat(global.sync.symlink_add, project.sync.symlink_add),
        copy_add: concat(global.sync.copy_add, project.sync.copy_add),
        ignore_add: concat(global.sync.ignore_add, project.sync.ignore_add),
        overrides: merge_maps(global.sync.overrides, project.sync.overrides),
    };

    let hooks = HooksConfig {
        post_start: project.hooks.post_start.or(global.hooks.post_start),
        pre_done: project.hooks.pre_done.or(global.hooks.pre_done),
    };

    let default_iso = IsolationConfig::default();
    let isolation = if project.isolation.port_range_size != default_iso.port_range_size
        || project.isolation.base_port != default_iso.base_port
        || !project.isolation.services.is_empty()
    {
        project.isolation
    } else {
        global.isolation
    };

    Config { sync, hooks, isolation }
}

/// Concatenate two vecs (global first, then project).
fn concat(mut a: Vec<String>, b: Vec<String>) -> Vec<String> {
    a.extend(b);
    a
}

/// Merge two override maps — entries in `project` win on key collision.
fn merge_maps(mut global: HashMap<String, Strategy>, project: HashMap<String, Strategy>) -> HashMap<String, Strategy> {
    global.extend(project);
    global
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync_from(toml_str: &str) -> SyncConfig {
        toml::from_str::<Config>(toml_str).unwrap().sync
    }

    #[test]
    fn resolve_defaults() {
        let plan = SyncConfig::default().resolve();
        assert!(plan.symlink_dirs.contains(&"node_modules".to_string()));
        assert!(plan.symlink_dirs.contains(&"target".to_string()));
        assert!(plan.copy_globs.contains(&".env*".to_string()));
        assert!(plan.copy_dirs.is_empty());
    }

    #[test]
    fn additive_symlink_extends_defaults() {
        let cfg = sync_from("[sync]\nsymlink_add = [\"my-cache\"]\n");
        let plan = cfg.resolve();
        // Defaults are still there...
        assert!(plan.symlink_dirs.contains(&"node_modules".to_string()));
        // ...plus the added one.
        assert!(plan.symlink_dirs.contains(&"my-cache".to_string()));
    }

    #[test]
    fn base_symlink_replaces_defaults() {
        let cfg = sync_from("[sync]\nsymlink = [\"only-this\"]\n");
        let plan = cfg.resolve();
        assert_eq!(plan.symlink_dirs, vec!["only-this".to_string()]);
        assert!(!plan.symlink_dirs.contains(&"node_modules".to_string()));
    }

    #[test]
    fn override_copy_moves_dir_to_copy_dirs() {
        let cfg = sync_from("[sync.overrides]\nnode_modules = \"copy\"\n");
        let plan = cfg.resolve();
        assert!(plan.copy_dirs.contains(&"node_modules".to_string()));
        assert!(!plan.symlink_dirs.contains(&"node_modules".to_string()));
    }

    #[test]
    fn override_ignore_excludes_dir() {
        let cfg = sync_from("[sync.overrides]\ntarget = \"ignore\"\n");
        let plan = cfg.resolve();
        assert!(!plan.symlink_dirs.contains(&"target".to_string()));
        assert!(!plan.copy_dirs.contains(&"target".to_string()));
        assert!(plan.ignore.contains(&"target".to_string()));
    }

    #[test]
    fn override_clone_routes_to_clone_dirs() {
        let cfg = sync_from("[sync.overrides]\nnode_modules = \"clone\"\n");
        let plan = cfg.resolve();
        assert!(plan.clone_dirs.contains(&"node_modules".to_string()));
        assert!(!plan.symlink_dirs.contains(&"node_modules".to_string()));
        assert!(!plan.copy_dirs.contains(&"node_modules".to_string()));
    }

    #[test]
    fn ignore_add_filters_copy_globs() {
        let cfg = sync_from("[sync]\ncopy = [\".env\", \".env.local\"]\nignore_add = [\".env.local\"]\n");
        let plan = cfg.resolve();
        assert!(plan.copy_globs.contains(&".env".to_string()));
        assert!(!plan.copy_globs.contains(&".env.local".to_string()));
    }

    #[test]
    fn merge_project_base_replaces_global_addities_concat() {
        let global = toml::from_str::<Config>(
            "[sync]\nsymlink = [\"g1\"]\nsymlink_add = [\"gadd\"]\n",
        )
        .unwrap();
        let project = toml::from_str::<Config>(
            "[sync]\nsymlink = [\"p1\"]\nsymlink_add = [\"padd\"]\n",
        )
        .unwrap();
        let merged = merge_configs(global, project);
        // Base list: project wins.
        assert_eq!(merged.sync.symlink, Some(vec!["p1".to_string()]));
        // Additive: concatenated (global first).
        assert_eq!(merged.sync.symlink_add, vec!["gadd".to_string(), "padd".to_string()]);
    }

    #[test]
    fn merge_uses_global_when_project_unset() {
        let global = toml::from_str::<Config>("[sync]\nsymlink = [\"g1\"]\n").unwrap();
        let project = toml::from_str::<Config>("[hooks]\npost_start = \"echo hi\"\n").unwrap();
        let merged = merge_configs(global, project);
        // Project didn't set symlink → global's value survives (the old bug lost this).
        assert_eq!(merged.sync.symlink, Some(vec!["g1".to_string()]));
    }

    #[test]
    fn merge_overrides_project_wins_on_collision() {
        let global = toml::from_str::<Config>(
            "[sync.overrides]\nnode_modules = \"symlink\"\ntarget = \"ignore\"\n",
        )
        .unwrap();
        let project = toml::from_str::<Config>(
            "[sync.overrides]\nnode_modules = \"copy\"\n",
        )
        .unwrap();
        let merged = merge_configs(global, project);
        assert_eq!(merged.sync.overrides.get("node_modules"), Some(&Strategy::Copy));
        assert_eq!(merged.sync.overrides.get("target"), Some(&Strategy::Ignore));
    }
}
