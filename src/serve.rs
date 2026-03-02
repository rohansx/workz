use anyhow::Result;
use axum::{
    extract::Path,
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use serde::Serialize;
use std::process::Command;

use crate::{config, git, sync};

// ── API types ───────────────────────────────────────────────────────────

#[derive(Serialize)]
struct WorktreeInfo {
    branch: String,
    path: String,
    is_bare: bool,
    is_dirty: bool,
    last_commit: Option<String>,
    disk_size: String,
    has_docker: bool,
    is_fleet: bool,
    task: Option<String>,
}

// ── handlers ────────────────────────────────────────────────────────────

async fn get_worktrees() -> impl IntoResponse {
    let worktrees = match git::worktree_list() {
        Ok(w) => w,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Load fleet state if present
    let fleet_tasks: std::collections::HashMap<String, String> = git::repo_root()
        .ok()
        .and_then(|root| {
            let path = root.join(".workz").join("fleet.json");
            std::fs::read_to_string(path).ok()
        })
        .and_then(|raw| serde_json::from_str::<crate::fleet::FleetState>(&raw).ok())
        .map(|state| {
            state
                .tasks
                .into_iter()
                .map(|t| (t.branch, t.task))
                .collect()
        })
        .unwrap_or_default();

    let infos: Vec<WorktreeInfo> = worktrees
        .iter()
        .map(|wt| {
            let is_dirty = !wt.is_bare && git::is_dirty(&wt.path).unwrap_or(false);
            let last_commit = if wt.is_bare {
                None
            } else {
                git::last_commit_relative(&wt.path)
            };
            let disk_size = if wt.is_bare {
                String::new()
            } else {
                crate::human_size(crate::dir_size_shallow(&wt.path))
            };
            let has_docker = !wt.is_bare
                && (wt.path.join("docker-compose.yml").exists()
                    || wt.path.join("docker-compose.yaml").exists()
                    || wt.path.join("compose.yml").exists()
                    || wt.path.join("compose.yaml").exists());

            let task = fleet_tasks.get(&wt.branch).cloned();
            let is_fleet = task.is_some() || wt.branch.starts_with("fleet/");

            WorktreeInfo {
                branch: wt.branch.clone(),
                path: wt.path.to_string_lossy().to_string(),
                is_bare: wt.is_bare,
                is_dirty,
                last_commit,
                disk_size,
                has_docker,
                is_fleet,
                task,
            }
        })
        .collect();

    Json(infos).into_response()
}

async fn post_sync(Path(branch): Path<String>) -> impl IntoResponse {
    let root = match git::repo_root() {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let wt_path = git::worktree_path(&root, &branch);
    if !wt_path.exists() {
        return (StatusCode::NOT_FOUND, format!("worktree '{}' not found", branch));
    }
    let config = match config::load_config(&root) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    match sync::sync_worktree(&root, &wt_path, &config.sync) {
        Ok(_) => (StatusCode::OK, "synced".to_string()),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn delete_worktree(Path(branch): Path<String>) -> impl IntoResponse {
    let root = match git::repo_root() {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let wt_path = git::worktree_path(&root, &branch);
    if !wt_path.exists() {
        return (StatusCode::NOT_FOUND, format!("worktree '{}' not found", branch));
    }
    match git::worktree_remove(&wt_path, false) {
        Ok(_) => (StatusCode::OK, "removed".to_string()),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn post_open(Path((branch, editor)): Path<(String, String)>) -> impl IntoResponse {
    let root = match git::repo_root() {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let wt_path = git::worktree_path(&root, &branch);
    if !wt_path.exists() {
        return (StatusCode::NOT_FOUND, format!("worktree '{}' not found", branch));
    }
    let cmd = match editor.as_str() {
        "cursor" => "cursor",
        "windsurf" => "windsurf",
        _ => "code",
    };
    match Command::new(cmd).arg(&wt_path).spawn() {
        Ok(_) => (StatusCode::OK, "opened".to_string()),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn serve_ui() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

// ── entrypoint ──────────────────────────────────────────────────────────

pub fn run(port: u16, no_open: bool) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async_serve(port, no_open))
}

async fn async_serve(port: u16, no_open: bool) -> Result<()> {
    let app = Router::new()
        .route("/", get(serve_ui))
        .route("/api/worktrees", get(get_worktrees))
        .route("/api/sync/:branch", post(post_sync))
        .route("/api/remove/:branch", delete(delete_worktree))
        .route("/api/open/:branch/:editor", post(post_open));

    let addr = format!("127.0.0.1:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    println!("workz dashboard running at http://localhost:{}", port);
    println!("press Ctrl+C to stop");

    if !no_open {
        let url = format!("http://localhost:{}", port);
        // Open browser after a short delay so server is ready
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let _ = open_browser(&url);
        });
    }

    axum::serve(listener, app).await?;
    Ok(())
}

fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    Command::new("open").arg(url).spawn()?;
    #[cfg(target_os = "linux")]
    Command::new("xdg-open").arg(url).spawn()?;
    #[cfg(target_os = "windows")]
    Command::new("cmd").args(["/c", "start", url]).spawn()?;
    Ok(())
}

// ── embedded dashboard HTML ─────────────────────────────────────────────

const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>workz dashboard</title>
<style>
  :root {
    --bg: #0d0d0d;
    --surface: #161616;
    --border: #2a2a2a;
    --text: #e8e8e8;
    --muted: #666;
    --accent: #7c6af7;
    --accent-dim: #3d3566;
    --green: #4ade80;
    --yellow: #fbbf24;
    --red: #f87171;
    --fleet: #38bdf8;
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { background: var(--bg); color: var(--text); font-family: 'SF Mono', 'Fira Code', monospace; font-size: 13px; min-height: 100vh; }

  header { padding: 20px 32px; border-bottom: 1px solid var(--border); display: flex; align-items: center; gap: 16px; }
  .logo { font-size: 18px; font-weight: 700; color: var(--accent); letter-spacing: -0.5px; }
  .repo-name { color: var(--muted); font-size: 12px; }
  .stats { margin-left: auto; display: flex; gap: 20px; color: var(--muted); font-size: 12px; }
  .stats span { display: flex; align-items: center; gap: 6px; }
  .dot { width: 6px; height: 6px; border-radius: 50%; background: var(--accent); }
  .refresh-btn { margin-left: 16px; background: none; border: 1px solid var(--border); color: var(--muted); padding: 4px 12px; border-radius: 4px; cursor: pointer; font-size: 11px; font-family: inherit; }
  .refresh-btn:hover { border-color: var(--accent); color: var(--text); }

  main { padding: 28px 32px; }
  .section-title { font-size: 11px; text-transform: uppercase; letter-spacing: 1px; color: var(--muted); margin-bottom: 16px; }

  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); gap: 16px; margin-bottom: 32px; }

  .card { background: var(--surface); border: 1px solid var(--border); border-radius: 8px; padding: 18px; transition: border-color 0.15s; }
  .card:hover { border-color: #3a3a3a; }
  .card.bare { opacity: 0.5; }
  .card.fleet-card { border-color: var(--accent-dim); }

  .card-header { display: flex; align-items: flex-start; gap: 10px; margin-bottom: 12px; }
  .branch-name { font-size: 14px; font-weight: 600; color: var(--text); flex: 1; word-break: break-all; }
  .badge { font-size: 10px; padding: 2px 7px; border-radius: 3px; font-weight: 600; letter-spacing: 0.5px; white-space: nowrap; }
  .badge-clean { background: #1a3d2b; color: var(--green); }
  .badge-dirty { background: #3d2a00; color: var(--yellow); }
  .badge-bare { background: #222; color: var(--muted); }
  .badge-fleet { background: #0c2d3d; color: var(--fleet); }

  .card-meta { display: flex; flex-direction: column; gap: 5px; margin-bottom: 14px; }
  .meta-row { display: flex; align-items: center; gap: 8px; color: var(--muted); font-size: 11px; }
  .meta-row .label { color: #444; width: 70px; flex-shrink: 0; }
  .meta-row .value { color: #999; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .task-text { color: #aaa; font-style: italic; }

  .card-actions { display: flex; gap: 8px; flex-wrap: wrap; }
  .btn { font-size: 11px; padding: 5px 10px; border-radius: 4px; border: 1px solid var(--border); background: none; color: var(--muted); cursor: pointer; font-family: inherit; transition: all 0.1s; }
  .btn:hover { border-color: var(--accent); color: var(--text); }
  .btn-danger:hover { border-color: var(--red); color: var(--red); }
  .btn:disabled { opacity: 0.3; cursor: not-allowed; }

  .toast { position: fixed; bottom: 24px; right: 24px; background: var(--surface); border: 1px solid var(--border); padding: 10px 16px; border-radius: 6px; font-size: 12px; opacity: 0; transition: opacity 0.2s; pointer-events: none; }
  .toast.show { opacity: 1; }
  .toast.ok { border-color: var(--green); color: var(--green); }
  .toast.err { border-color: var(--red); color: var(--red); }

  .empty { color: var(--muted); text-align: center; padding: 48px; }
  .loading { color: var(--muted); padding: 48px 0; }
</style>
</head>
<body>
<header>
  <div class="logo">⬡ workz</div>
  <div class="repo-name" id="repo-name">loading...</div>
  <div class="stats">
    <span><div class="dot"></div><span id="wt-count">0</span> worktrees</span>
    <span id="dirty-count" style="display:none"><div class="dot" style="background:var(--yellow)"></div><span id="dirty-num">0</span> modified</span>
    <span id="fleet-count" style="display:none"><div class="dot" style="background:var(--fleet)"></div><span id="fleet-num">0</span> fleet</span>
  </div>
  <button class="refresh-btn" onclick="load()">↻ refresh</button>
</header>

<main>
  <div id="fleet-section" style="display:none">
    <div class="section-title">fleet</div>
    <div class="grid" id="fleet-grid"></div>
  </div>
  <div class="section-title" id="main-title">worktrees</div>
  <div class="grid" id="main-grid"><div class="loading">loading...</div></div>
</main>

<div class="toast" id="toast"></div>

<script>
let allWorktrees = [];

async function load() {
  try {
    const res = await fetch('/api/worktrees');
    allWorktrees = await res.json();
    render(allWorktrees);
  } catch(e) {
    document.getElementById('main-grid').innerHTML = '<div class="empty">could not connect to workz server</div>';
  }
}

function render(wts) {
  const fleet = wts.filter(w => w.is_fleet && !w.is_bare);
  const main = wts.filter(w => !w.is_fleet);
  const dirty = wts.filter(w => w.is_dirty).length;

  // stats
  document.getElementById('wt-count').textContent = wts.length;
  if (dirty > 0) {
    document.getElementById('dirty-count').style.display = '';
    document.getElementById('dirty-num').textContent = dirty;
  }
  if (fleet.length > 0) {
    document.getElementById('fleet-count').style.display = '';
    document.getElementById('fleet-num').textContent = fleet.length;
    document.getElementById('fleet-section').style.display = '';
    document.getElementById('fleet-grid').innerHTML = fleet.map(cardHtml).join('');
  } else {
    document.getElementById('fleet-section').style.display = 'none';
  }

  // repo name from bare worktree path
  const bare = wts.find(w => w.is_bare);
  if (bare) {
    const parts = bare.path.split('/');
    document.getElementById('repo-name').textContent = parts[parts.length - 1] || '';
  }

  const grid = document.getElementById('main-grid');
  if (main.length === 0) {
    grid.innerHTML = '<div class="empty">no worktrees found</div>';
  } else {
    grid.innerHTML = main.map(cardHtml).join('');
  }
}

function cardHtml(wt) {
  const safeBranch = encodeURIComponent(wt.branch);
  const badge = wt.is_bare
    ? '<span class="badge badge-bare">bare</span>'
    : wt.is_dirty
    ? '<span class="badge badge-dirty">modified</span>'
    : '<span class="badge badge-clean">clean</span>';

  const fleetBadge = wt.is_fleet ? '<span class="badge badge-fleet">fleet</span>' : '';

  const task = wt.task
    ? `<div class="meta-row"><span class="label">task</span><span class="value task-text">${esc(wt.task)}</span></div>` : '';

  const lastCommit = wt.last_commit
    ? `<div class="meta-row"><span class="label">commit</span><span class="value">${esc(wt.last_commit)}</span></div>` : '';

  const docker = wt.has_docker
    ? `<div class="meta-row"><span class="label">docker</span><span class="value">compose detected</span></div>` : '';

  const actions = wt.is_bare ? '' : `
    <button class="btn" onclick="syncWt('${safeBranch}', this)">sync</button>
    <button class="btn" onclick="openWt('${safeBranch}', 'code', this)">VS Code</button>
    <button class="btn" onclick="openWt('${safeBranch}', 'cursor', this)">Cursor</button>
    <button class="btn btn-danger" onclick="removeWt('${safeBranch}', this)">remove</button>
  `;

  return `<div class="card${wt.is_bare ? ' bare' : ''}${wt.is_fleet ? ' fleet-card' : ''}">
    <div class="card-header">
      <div class="branch-name">${esc(wt.branch)}</div>
      ${badge}${fleetBadge}
    </div>
    <div class="card-meta">
      <div class="meta-row"><span class="label">path</span><span class="value" title="${esc(wt.path)}">${esc(wt.path)}</span></div>
      ${task}${lastCommit}
      ${wt.disk_size ? `<div class="meta-row"><span class="label">size</span><span class="value">${esc(wt.disk_size)}</span></div>` : ''}
      ${docker}
    </div>
    <div class="card-actions">${actions}</div>
  </div>`;
}

function esc(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

async function syncWt(branch, btn) {
  btn.disabled = true; btn.textContent = 'syncing...';
  try {
    const r = await fetch('/api/sync/' + branch, { method: 'POST' });
    toast(r.ok ? 'synced ' + branch : await r.text(), r.ok ? 'ok' : 'err');
  } catch(e) { toast(e.message, 'err'); }
  btn.disabled = false; btn.textContent = 'sync';
}

async function openWt(branch, editor, btn) {
  btn.disabled = true;
  try {
    const r = await fetch('/api/open/' + branch + '/' + editor, { method: 'POST' });
    toast(r.ok ? 'opened in ' + editor : await r.text(), r.ok ? 'ok' : 'err');
  } catch(e) { toast(e.message, 'err'); }
  btn.disabled = false;
}

async function removeWt(branch, btn) {
  if (!confirm('Remove worktree ' + branch + '?')) return;
  btn.disabled = true; btn.textContent = 'removing...';
  try {
    const r = await fetch('/api/remove/' + branch, { method: 'DELETE' });
    if (r.ok) { toast('removed ' + branch, 'ok'); load(); }
    else toast(await r.text(), 'err');
  } catch(e) { toast(e.message, 'err'); }
  btn.disabled = false; btn.textContent = 'remove';
}

function toast(msg, type) {
  const t = document.getElementById('toast');
  t.textContent = msg;
  t.className = 'toast show ' + (type || '');
  clearTimeout(t._timer);
  t._timer = setTimeout(() => t.className = 'toast', 3000);
}

// Auto-refresh every 10s
load();
setInterval(load, 10000);
</script>
</body>
</html>"#;
