use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::sync::Framework;

// ── Registry types ───────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct PortRegistry {
    #[serde(default = "default_base_port")]
    pub base_port: u16,
    #[serde(default)]
    pub allocations: HashMap<String, PortAllocation>,
}

fn default_base_port() -> u16 {
    3000
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PortAllocation {
    pub port: u16,
    /// Number of ports in the allocated range (backward compat: defaults to 1).
    #[serde(default = "default_port_count")]
    pub port_count: u16,
    pub branch: String,
    pub db_name: String,
    pub compose_project: String,
    pub worktree_path: String,
    pub allocated_at: String,
}

fn default_port_count() -> u16 {
    1
}

pub struct IsolationConfig {
    pub port: u16,
    pub port_end: u16,
    pub port_count: u16,
    pub db_name: String,
    pub compose_project: String,
}

// ── Registry path ────────────────────────────────────────────────────────────

pub fn registry_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("workz").join("ports.json"))
}

// ── Load / Save ──────────────────────────────────────────────────────────────

pub fn load_registry() -> PortRegistry {
    let Some(path) = registry_path() else {
        return PortRegistry::default();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return PortRegistry::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

pub fn save_registry(registry: &PortRegistry) -> Result<()> {
    let Some(path) = registry_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(registry)?)?;
    Ok(())
}

// ── Branch slug ──────────────────────────────────────────────────────────────

/// "feature/add-auth" → "feature_add_auth"
pub fn branch_to_slug(branch: &str) -> String {
    branch
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

// ── Port allocation ──────────────────────────────────────────────────────────

/// Allocate a contiguous block of `range_size` ports that doesn't overlap any
/// existing allocation. Aligns to `range_size` boundaries for clean ranges.
fn next_available_port_range(registry: &PortRegistry, range_size: u16) -> u16 {
    let occupied: Vec<(u16, u16)> = registry
        .allocations
        .values()
        .map(|a| (a.port, a.port + a.port_count))
        .collect();

    let base = if registry.base_port == 0 { 3000 } else { registry.base_port };
    let mut candidate = base;

    // Align to range_size boundaries
    if range_size > 1 && candidate % range_size != 0 {
        candidate = candidate + range_size - (candidate % range_size);
    }

    loop {
        let candidate_end = candidate + range_size;
        let overlaps = occupied
            .iter()
            .any(|&(start, end)| candidate < end && candidate_end > start);
        if !overlaps {
            return candidate;
        }
        candidate += range_size;
        if candidate > 60000 {
            return candidate;
        }
    }
}

// ── Main API ─────────────────────────────────────────────────────────────────

/// Allocate a port range, compute derived names, update registry, write .env.local.
pub fn setup_isolation(
    branch: &str,
    wt_path: &Path,
    range_size: u16,
    framework: Framework,
) -> Result<IsolationConfig> {
    let mut registry = load_registry();
    let slug = branch_to_slug(branch);

    let alloc = if let Some(existing) = registry.allocations.get(&slug) {
        existing.clone()
    } else {
        let port = next_available_port_range(&registry, range_size);
        let alloc = PortAllocation {
            port,
            port_count: range_size,
            branch: branch.to_string(),
            db_name: slug.clone(),
            compose_project: slug.clone(),
            worktree_path: wt_path.to_string_lossy().to_string(),
            allocated_at: rfc3339_now(),
        };
        registry.allocations.insert(slug.clone(), alloc.clone());
        save_registry(&registry)?;
        alloc
    };

    write_env_local(wt_path, &alloc, framework)?;

    Ok(IsolationConfig {
        port: alloc.port,
        port_end: alloc.port + alloc.port_count - 1,
        port_count: alloc.port_count,
        db_name: alloc.db_name.clone(),
        compose_project: alloc.compose_project.clone(),
    })
}

/// Release a port allocation. Called by cmd_done.
pub fn release_isolation(branch: &str) -> Result<()> {
    let slug = branch_to_slug(branch);
    let mut registry = load_registry();
    if registry.allocations.remove(&slug).is_some() {
        save_registry(&registry)?;
    }
    Ok(())
}

/// Best-effort: drop the PostgreSQL database for a branch.
pub fn drop_database(branch: &str) {
    let slug = branch_to_slug(branch);
    let db_name = load_registry()
        .allocations
        .get(&slug)
        .map(|a| a.db_name.clone())
        .unwrap_or(slug);

    match Command::new("dropdb").arg("--if-exists").arg(&db_name).status() {
        Ok(s) if s.success() => println!("  dropped database '{}'", db_name),
        Ok(s) => eprintln!("  warning: dropdb exited with {}", s),
        Err(_) => eprintln!("  warning: dropdb not found, skipping DB cleanup"),
    }
}

/// Look up the allocation for a branch (for status display).
pub fn get_allocation(branch: &str) -> Option<PortAllocation> {
    let slug = branch_to_slug(branch);
    load_registry().allocations.get(&slug).cloned()
}

// ── .env.local writer ────────────────────────────────────────────────────────

/// Markers delimiting the workz-managed block inside `.env.local`. Everything
/// outside these markers is user-owned and never touched.
const MANAGED_BEGIN: &str = "# >>> workz managed — do not edit between these markers >>>";
const MANAGED_END: &str = "# <<< workz managed <<<";

/// Write the isolation env vars into the worktree's `.env.local`, **merging**
/// rather than overwriting. Any pre-existing user content (e.g. a copied
/// `.env.local` full of secrets) is preserved; only the workz-managed block is
/// rewritten. Idempotent: running twice with the same allocation is a no-op.
fn write_env_local(wt_path: &Path, alloc: &PortAllocation, framework: Framework) -> Result<()> {
    let managed = build_managed_block(alloc, framework);
    let path = wt_path.join(".env.local");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let merged = merge_managed_block(&existing, &managed);
    std::fs::write(path, merged)?;
    Ok(())
}

/// The lines that live *inside* the managed block (no markers, no trailing newline).
fn build_managed_block(alloc: &PortAllocation, framework: Framework) -> Vec<String> {
    let port = alloc.port;
    let port_end = alloc.port + alloc.port_count - 1;

    let mut lines = vec![format!("PORT={}", port)];

    // Only write PORT_END when we have a range (not a single port)
    if alloc.port_count > 1 {
        lines.push(format!("PORT_END={}", port_end));
    }

    // Framework-specific port vars
    match framework {
        Framework::SpringBoot => lines.push(format!("SERVER_PORT={}", port)),
        Framework::Flask => lines.push(format!("FLASK_RUN_PORT={}", port)),
        Framework::FastApi => lines.push(format!("UVICORN_PORT={}", port)),
        Framework::Vite => lines.push(format!("VITE_PORT={}", port)),
        _ => {}
    }

    lines.push(format!("DB_NAME={}", alloc.db_name));
    lines.push(format!("DATABASE_URL=postgres://localhost/{}", alloc.db_name));
    lines.push(format!("COMPOSE_PROJECT_NAME={}", alloc.compose_project));

    // Redis on port+1 (within the allocated range, not port+1000)
    let redis_port = if alloc.port_count > 1 { port + 1 } else { port + 1000 };
    lines.push(format!("REDIS_URL=redis://localhost:{}", redis_port));

    lines
}

/// Merge a fresh managed block into existing `.env.local` content. User lines
/// (everything outside a previous managed block) are preserved verbatim; the
/// managed block is placed at the end so its values win under dotenv semantics.
fn merge_managed_block(existing: &str, managed: &[String]) -> String {
    let mut user_lines: Vec<&str> = Vec::new();
    let mut in_block = false;
    for line in existing.lines() {
        match line.trim() {
            MANAGED_BEGIN => in_block = true,
            MANAGED_END => in_block = false,
            _ if !in_block => user_lines.push(line),
            _ => {}
        }
    }

    // Trim trailing blank user lines so re-runs don't accumulate whitespace.
    while matches!(user_lines.last(), Some(l) if l.trim().is_empty()) {
        user_lines.pop();
    }

    let mut out = String::new();
    if !user_lines.is_empty() {
        out.push_str(&user_lines.join("\n"));
        out.push_str("\n\n");
    }
    out.push_str(MANAGED_BEGIN);
    out.push('\n');
    out.push_str(&managed.join("\n"));
    out.push('\n');
    out.push_str(MANAGED_END);
    out.push('\n');
    out
}

// ── Timestamp helpers ────────────────────────────────────────────────────────

fn rfc3339_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    unix_secs_to_rfc3339(secs)
}

/// Convert Unix seconds to an RFC 3339 UTC timestamp string.
/// Uses Howard Hinnant's civil_from_days algorithm — no external crate needed.
fn unix_secs_to_rfc3339(secs: u64) -> String {
    let time_of_day = secs % 86400;
    let hour = (time_of_day / 3600) as u32;
    let min  = ((time_of_day % 3600) / 60) as u32;
    let sec  = (time_of_day % 60) as u32;

    let z: i64   = (secs / 86400) as i64 + 719468;
    let era: i64 = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe: i64 = z - era * 146097;
    let yoe: i64 = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y: i64   = yoe + era * 400;
    let doy: i64 = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp: i64  = (5 * doy + 2) / 153;
    let day: i64 = doy - (153 * mp + 2) / 5 + 1;
    let month: i64 = if mp < 10 { mp + 3 } else { mp - 9 };
    let year: i64  = if month <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month, day, hour, min, sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_basic() {
        assert_eq!(branch_to_slug("feature/add-auth"), "feature_add_auth");
        assert_eq!(branch_to_slug("fix/some-bug"), "fix_some_bug");
        assert_eq!(branch_to_slug("main"), "main");
    }

    #[test]
    fn timestamp_epoch() {
        assert_eq!(unix_secs_to_rfc3339(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn timestamp_known() {
        // 2024-03-04T12:00:00Z = 1709553600
        assert_eq!(unix_secs_to_rfc3339(1709553600), "2024-03-04T12:00:00Z");
    }

    #[test]
    fn range_allocation_no_overlap() {
        let mut registry = PortRegistry { base_port: 3000, allocations: HashMap::new() };

        let port1 = next_available_port_range(&registry, 10);
        assert_eq!(port1, 3000);

        registry.allocations.insert("first".into(), PortAllocation {
            port: 3000, port_count: 10,
            branch: "a".into(), db_name: "a".into(),
            compose_project: "a".into(), worktree_path: "/tmp/a".into(),
            allocated_at: "2024-01-01T00:00:00Z".into(),
        });

        let port2 = next_available_port_range(&registry, 10);
        assert_eq!(port2, 3010);
    }

    #[test]
    fn range_allocation_backward_compat() {
        let mut registry = PortRegistry { base_port: 3000, allocations: HashMap::new() };
        registry.allocations.insert("old".into(), PortAllocation {
            port: 3000, port_count: 1,
            branch: "old".into(), db_name: "old".into(),
            compose_project: "old".into(), worktree_path: "/tmp/old".into(),
            allocated_at: "2024-01-01T00:00:00Z".into(),
        });

        let port = next_available_port_range(&registry, 10);
        assert_eq!(port, 3010);
    }

    #[test]
    fn range_allocation_gap_filling() {
        let mut registry = PortRegistry { base_port: 3000, allocations: HashMap::new() };

        registry.allocations.insert("first".into(), PortAllocation {
            port: 3000, port_count: 10,
            branch: "a".into(), db_name: "a".into(),
            compose_project: "a".into(), worktree_path: "/tmp/a".into(),
            allocated_at: "2024-01-01T00:00:00Z".into(),
        });
        registry.allocations.insert("third".into(), PortAllocation {
            port: 3020, port_count: 10,
            branch: "c".into(), db_name: "c".into(),
            compose_project: "c".into(), worktree_path: "/tmp/c".into(),
            allocated_at: "2024-01-01T00:00:00Z".into(),
        });

        let port = next_available_port_range(&registry, 10);
        assert_eq!(port, 3010);
    }

    fn sample_alloc() -> PortAllocation {
        PortAllocation {
            port: 3010,
            port_count: 10,
            branch: "feat/x".into(),
            db_name: "feat_x".into(),
            compose_project: "feat_x".into(),
            worktree_path: "/tmp/x".into(),
            allocated_at: "2024-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn merge_into_empty_writes_only_managed_block() {
        let managed = build_managed_block(&sample_alloc(), Framework::Unknown);
        let out = merge_managed_block("", &managed);
        assert!(out.starts_with(MANAGED_BEGIN));
        assert!(out.trim_end().ends_with(MANAGED_END));
        assert!(out.contains("PORT=3010"));
        assert!(out.contains("DB_NAME=feat_x"));
    }

    #[test]
    fn merge_preserves_user_lines() {
        let existing = "API_KEY=secret123\nSTRIPE_KEY=sk_live_abc\n";
        let managed = build_managed_block(&sample_alloc(), Framework::Unknown);
        let out = merge_managed_block(existing, &managed);
        // User secrets survive
        assert!(out.contains("API_KEY=secret123"));
        assert!(out.contains("STRIPE_KEY=sk_live_abc"));
        // Managed block is appended after user content
        let user_pos = out.find("API_KEY").unwrap();
        let managed_pos = out.find(MANAGED_BEGIN).unwrap();
        assert!(user_pos < managed_pos);
    }

    #[test]
    fn merge_is_idempotent() {
        let existing = "API_KEY=secret123\n";
        let managed = build_managed_block(&sample_alloc(), Framework::Unknown);
        let once = merge_managed_block(existing, &managed);
        let twice = merge_managed_block(&once, &managed);
        assert_eq!(once, twice);
        // Only one managed block after repeated runs
        assert_eq!(twice.matches(MANAGED_BEGIN).count(), 1);
        assert!(twice.contains("API_KEY=secret123"));
    }

    #[test]
    fn merge_replaces_stale_managed_block() {
        // Simulate an old allocation, then re-run with a new port.
        let old_alloc = PortAllocation { port: 3000, ..sample_alloc() };
        let old = merge_managed_block("API_KEY=k\n", &build_managed_block(&old_alloc, Framework::Unknown));
        assert!(old.contains("PORT=3000"));

        let new = merge_managed_block(&old, &build_managed_block(&sample_alloc(), Framework::Unknown));
        assert!(new.contains("PORT=3010"));
        assert!(!new.contains("PORT=3000"));
        assert_eq!(new.matches(MANAGED_BEGIN).count(), 1);
        assert!(new.contains("API_KEY=k"));
    }

    #[test]
    fn merge_writes_framework_var() {
        let managed = build_managed_block(&sample_alloc(), Framework::Vite);
        let out = merge_managed_block("", &managed);
        assert!(out.contains("VITE_PORT=3010"));
    }
}
