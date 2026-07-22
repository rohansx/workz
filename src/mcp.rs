/// workz MCP server — exposes workz operations as tools for AI agents.
/// Speaks JSON-RPC 2.0 over stdio (the MCP transport).
///
/// Add to Claude Code:
///   claude mcp add workz -- workz mcp
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use crate::{config, git, isolation, sync};

// ── JSON-RPC types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Request {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct Response {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

impl Response {
    fn ok(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0".into(), id, result: Some(result), error: None }
    }
    fn err(id: Value, code: i32, msg: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(RpcError { code, message: msg.into() }),
        }
    }
}

// ── Server loop ─────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response::err(Value::Null, -32700, format!("parse error: {e}"));
                writeln!(out, "{}", serde_json::to_string(&resp)?)?;
                out.flush()?;
                continue;
            }
        };

        // Notifications have no id — don't respond
        let id = match req.id.clone() {
            Some(id) => id,
            None => continue,
        };

        let resp = dispatch(&req.method, id, &req.params);
        writeln!(out, "{}", serde_json::to_string(&resp)?)?;
        out.flush()?;
    }

    Ok(())
}

// ── Dispatch ────────────────────────────────────────────────────────────

fn dispatch(method: &str, id: Value, params: &Value) -> Response {
    match method {
        "initialize" => Response::ok(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "workz",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),

        "tools/list" => Response::ok(id, json!({ "tools": tool_definitions() })),

        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let args = &params["arguments"];
            match call_tool(name, args) {
                Ok(text) => Response::ok(
                    id,
                    json!({ "content": [{ "type": "text", "text": text }] }),
                ),
                Err(e) => Response::ok(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": format!("Error: {e}") }],
                        "isError": true
                    }),
                ),
            }
        }

        _ => Response::err(id, -32601, format!("method not found: {method}")),
    }
}

// ── Tool implementations ────────────────────────────────────────────────

fn call_tool(name: &str, args: &Value) -> Result<String> {
    match name {
        "workz_start" => {
            let branch = args["branch"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("branch is required"))?;
            let base = args["base"].as_str();
            let no_sync = args["no_sync"].as_bool().unwrap_or(false);
            let isolated = args["isolated"].as_bool().unwrap_or(false);

            let root = git::repo_root()?;
            let config = config::load_config(&root)?;
            let wt_path = git::worktree_path(&root, branch, config.worktree.dir.as_deref());

            if wt_path.exists() {
                return Ok(format!(
                    "worktree already exists\nbranch: {branch}\npath: {}",
                    wt_path.display()
                ));
            }

            git::worktree_add(&wt_path, branch, base)?;

            let framework = if !no_sync {
                sync::sync_worktree(&root, &wt_path, &config.sync, sync::SyncOptions::default())?
                    .framework
            } else {
                sync::Framework::Unknown
            };

            let mut result = format!(
                "worktree created\nbranch: {branch}\npath: {}",
                wt_path.display()
            );

            if isolated {
                let iso = isolation::setup_isolation(
                    &git::repo_name(&root),
                    branch,
                    &wt_path,
                    framework,
                    &config.isolation,
                )?;
                result.push_str(&format!(
                    "\nisolated: PORT={}-{} DB_NAME={} COMPOSE_PROJECT_NAME={}",
                    iso.port, iso.port_end, iso.db_name, iso.compose_project
                ));
            }

            Ok(result)
        }

        "workz_list" => {
            let worktrees = git::worktree_list()?;
            let list: Vec<Value> = worktrees
                .iter()
                .map(|wt| {
                    let dirty = git::is_dirty(&wt.path).unwrap_or(false);
                    let last = git::last_commit_relative(&wt.path);
                    json!({
                        "branch": wt.branch,
                        "path": wt.path.to_string_lossy(),
                        "is_bare": wt.is_bare,
                        "modified": dirty,
                        "last_commit": last,
                    })
                })
                .collect();
            Ok(serde_json::to_string_pretty(&list)?)
        }

        "workz_status" => {
            let worktrees = git::worktree_list()?;
            let mut lines = Vec::new();
            for wt in &worktrees {
                if wt.is_bare {
                    lines.push(format!("{} {} (bare)", wt.branch, wt.path.display()));
                    continue;
                }
                let dirty =
                    if git::is_dirty(&wt.path).unwrap_or(false) { " [modified]" } else { "" };
                let last = git::last_commit_relative(&wt.path)
                    .map(|t| format!("  {t}"))
                    .unwrap_or_default();
                lines.push(format!(
                    "{}  {}{}{}",
                    wt.branch,
                    wt.path.display(),
                    dirty,
                    last
                ));
            }
            Ok(lines.join("\n"))
        }

        "workz_sync" => {
            let path = args["path"]
                .as_str()
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let isolated = args["isolated"].as_bool().unwrap_or(false);

            let root = git::repo_root()?;
            if path == root {
                anyhow::bail!("cannot sync the main worktree");
            }
            let config = config::load_config(&root)?;
            let report =
                sync::sync_worktree(&root, &path, &config.sync, sync::SyncOptions::default())?;
            let branch = git::current_branch(&path).unwrap_or_default();

            let iso = if isolated {
                Some(isolation::setup_isolation(
                    &git::repo_name(&root),
                    &branch,
                    &path,
                    report.framework,
                    &config.isolation,
                )?)
            } else {
                None
            };

            let iso_json = match &iso {
                Some(i) => {
                    let services_obj: serde_json::Map<String, serde_json::Value> = i
                        .services
                        .iter()
                        .map(|(n, p)| (n.clone(), json!(p)))
                        .collect();
                    json!({
                        "port": i.port,
                        "port_end": i.port_end,
                        "port_count": i.port_count,
                        "db_name": i.db_name,
                        "compose_project": i.compose_project,
                        "services": services_obj,
                        "env_file": ".env.local",
                    })
                }
                None => Value::Null,
            };

            let out = json!({
                "worktree": path.to_string_lossy(),
                "branch": branch,
                "symlinked": report.symlinked,
                "copied": report.copied,
                "installed": report.installed,
                "isolation": iso_json,
                "warnings": report.warnings,
            });
            Ok(serde_json::to_string_pretty(&out)?)
        }

        "workz_done" => {
            let root = git::repo_root()?;
            let force = args["force"].as_bool().unwrap_or(false);
            let wt_path = if let Some(branch) = args["branch"].as_str() {
                let config = config::load_config(&root)?;
                git::worktree_path(&root, branch, config.worktree.dir.as_deref())
            } else {
                std::env::current_dir()?
            };

            if !force && git::is_dirty(&wt_path).unwrap_or(false) {
                anyhow::bail!(
                    "worktree has uncommitted changes — pass force:true to override"
                );
            }

            git::worktree_remove(&wt_path, force)?;
            Ok(format!("removed worktree at {}", wt_path.display()))
        }

        "workz_doctor" => crate::doctor::report(),

        "workz_reap" => {
            let all = args["all"].as_bool().unwrap_or(false);
            let force = args["force"].as_bool().unwrap_or(false);
            let branch = args["branch"].as_str();
            let repo = git::repo_root().map(|r| git::repo_name(&r)).unwrap_or_default();

            let report = if all {
                isolation::reap_all(force)?
            } else if let Some(b) = branch {
                isolation::reap_branch(&repo, b, force)?
            } else {
                // Default: current worktree's branch.
                let cwd = std::env::current_dir()?;
                let root = git::repo_root()?;
                if cwd == root {
                    anyhow::bail!(
                        "you're in the main worktree — pass a branch name or all:true"
                    );
                }
                let b = git::current_branch(&cwd)?;
                isolation::reap_branch(&repo, &b, force)?
            };

            Ok(serde_json::to_string_pretty(&report)?)
        }

        "workz_conflicts" => {
            let conflicts = git::find_conflicts()?;
            if conflicts.is_empty() {
                Ok("no conflicts detected between worktrees".to_string())
            } else {
                let mut out = String::from("conflicting files:\n");
                for (file, branches) in &conflicts {
                    out.push_str(&format!("  {} — modified in: {}\n", file, branches.join(", ")));
                }
                Ok(out)
            }
        }

        _ => anyhow::bail!("unknown tool: {name}"),
    }
}

// ── Tool definitions (JSON Schema) ──────────────────────────────────────

fn tool_definitions() -> Value {
    json!([
        {
            "name": "workz_start",
            "description": "Create a new git worktree with auto-synced dependencies, env files, and IDE configs. Returns the worktree path so you can work in it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "branch": { "type": "string", "description": "Branch name to create or checkout (created from base if it doesn't exist)" },
                    "base":   { "type": "string", "description": "Base branch to create from (defaults to HEAD)" },
                    "no_sync":{ "type": "boolean","description": "Skip symlink/env sync (faster, for bare clones)" },
                    "isolated":{ "type": "boolean","description": "Auto-assign PORT range, DB_NAME, COMPOSE_PROJECT_NAME and write .env.local" }
                },
                "required": ["branch"]
            }
        },
        {
            "name": "workz_list",
            "description": "List all git worktrees with branch name, path, modified status, and last commit time.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "workz_status",
            "description": "Show rich status of all worktrees: branch, path, uncommitted changes, last commit age.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "workz_sync",
            "description": "Make a worktree runnable: symlink deps, copy env files, install, and optionally allocate an isolated PORT range / DB / compose project. Returns a JSON object (worktree, branch, symlinked, copied, installed, isolation, warnings). Idempotent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path to the worktree (defaults to current directory)" },
                    "isolated": { "type": "boolean", "description": "Also allocate PORT range, DB_NAME, COMPOSE_PROJECT_NAME and write .env.local" }
                }
            }
        },
        {
            "name": "workz_done",
            "description": "Remove a worktree and clean up. Fails if uncommitted changes exist unless force is true.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "branch": { "type": "string", "description": "Branch name of worktree to remove (defaults to current directory)" },
                    "force":  { "type": "boolean","description": "Force removal even with uncommitted changes" }
                }
            }
        },
        {
            "name": "workz_conflicts",
            "description": "Detect files modified in multiple worktrees simultaneously — potential merge conflicts before they happen.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "workz_doctor",
            "description": "Diagnose a broken worktree environment: dangling symlinks, orphaned port allocations, stale worktree refs, stale .git/index.lock files, live processes on orphaned ports, and unparseable config. Read-only (does not apply fixes).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "workz_reap",
            "description": "Kill processes bound to ports workz allocated to a worktree. Uses the stateful port registry, so it only touches ports workz actually owns (no false-positive kills). Pass `all:true` to reap every allocation, or a `branch` to target one. Pass `force:true` to SIGKILL directly. Returns a JSON object (ports_checked, killed, already_free, errors, escalated).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "branch": { "type": "string", "description": "Branch whose allocated ports to reap (defaults to the current worktree's branch)" },
                    "all":    { "type": "boolean", "description": "Reap every port workz has ever allocated (global cleanup)" },
                    "force":  { "type": "boolean", "description": "Skip SIGTERM, send SIGKILL immediately" }
                }
            }
        }
    ])
}
