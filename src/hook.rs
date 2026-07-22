//! `workz hook <host>` — print (or `--install`) the worktree-create hook recipe
//! for a host tool. The whole point of workz is to be the command those tools
//! call after they create a worktree; this makes wiring it up one command.

use anyhow::{bail, Result};
use std::path::PathBuf;

use crate::cli::HookHost;

/// The generated recipe for a host.
pub struct Recipe {
    /// Human-facing config snippet to print.
    pub snippet: String,
    /// Where the snippet goes.
    pub target_file: String,
    /// If the host has a dedicated file workz can safely create, the
    /// (relative path, file contents) to write on `--install`.
    pub installable: Option<(String, String)>,
    /// A short note (caveats / where to verify).
    pub note: Option<String>,
}

const CMD: &str = "workz sync --isolated --quiet";

/// Build the recipe for a host. Pure — unit-testable.
pub fn recipe(host: HookHost) -> Recipe {
    match host {
        HookHost::Cursor => {
            let content = "{\n  \"setup-worktree\": [\"workz\", \"sync\", \"--isolated\", \"--quiet\"]\n}\n";
            Recipe {
                snippet: content.to_string(),
                target_file: ".cursor/worktrees.json".to_string(),
                installable: Some((".cursor/worktrees.json".to_string(), content.to_string())),
                note: Some("Cursor runs the setup command inside the new worktree.".to_string()),
            }
        }
        HookHost::Worktrunk => Recipe {
            // worktrunk hooks are top-level keys (not a `[hooks]` table), and it
            // has no `create` hook — `pre-start` is the "set up before you start
            // working" hook (worktrunk's own template uses `pre-start = "npm ci"`).
            // It fires on `wt switch --create`, verified against worktrunk 0.68.
            snippet: format!("pre-start = \"{CMD}\"\n"),
            target_file: ".config/wt.toml".to_string(),
            installable: None,
            note: Some(
                "Add it as a top-level key in .config/wt.toml (worktrunk hooks aren't nested under [hooks]). worktrunk asks to approve a project hook once."
                    .to_string(),
            ),
        },
        HookHost::Claude => Recipe {
            // Claude Code's WorktreeCreate hook REPLACES worktree creation: it runs
            // in the main checkout, hands the hook a JSON payload on stdin, and parses
            // the hook's stdout as the new worktree's path. `workz claude-hook`
            // implements that contract natively (issue #19) — no jq, no shell script,
            // no stdout-hygiene footguns.
            snippet: r#"{
  "hooks": {
    "WorktreeCreate": [
      { "hooks": [ { "type": "command", "command": "workz claude-hook --isolated" } ] }
    ]
  }
}
"#
            .to_string(),
            target_file: ".claude/settings.json".to_string(),
            installable: None,
            note: Some(
                "Merge this into your existing .claude/settings.json. `workz claude-hook` reads \
                 the WorktreeCreate payload on stdin, creates and provisions the worktree, and \
                 prints only its path. Drop `--isolated` for no per-worktree ports/DB; add \
                 `--create-db` to also create the database."
                    .to_string(),
            ),
        },
        HookHost::Codex => Recipe {
            snippet: format!("# Codex local-environment setup script:\n{CMD}\n"),
            target_file: "your Codex environment setup script".to_string(),
            installable: None,
            note: Some("Add the command to the setup script for the worktree's local environment.".to_string()),
        },
        HookHost::Conductor => Recipe {
            snippet: format!("# .conductor/settings.local.toml — run script:\nsetup = \"{CMD}\"\n"),
            target_file: ".conductor/settings.local.toml".to_string(),
            installable: None,
            note: Some("Conductor runs the setup script when creating a workspace.".to_string()),
        },
        HookHost::Generic => Recipe {
            snippet: format!("# In your tool's post-worktree-create hook, run:\n{CMD} \"<worktree-path>\"\n"),
            target_file: "your tool's worktree-create hook".to_string(),
            installable: None,
            note: Some("Pass the new worktree path as the argument, or run it with the worktree as the working directory.".to_string()),
        },
    }
}

/// Print the recipe, or install it when the host supports a dedicated file.
pub fn run(host: HookHost, install: bool) -> Result<()> {
    let r = recipe(host);

    if install {
        let Some((rel, content)) = r.installable else {
            println!("workz can't safely auto-edit {} — add this manually:\n", r.target_file);
            print_snippet(&r);
            return Ok(());
        };

        let path = PathBuf::from(&rel);
        if path.exists() {
            bail!(
                "{} already exists — not overwriting. Merge this in manually:\n\n{}",
                rel,
                r.snippet
            );
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &content)?;
        println!("installed hook to {rel}");
        if let Some(note) = &r.note {
            println!("note: {note}");
        }
        return Ok(());
    }

    print_snippet(&r);
    Ok(())
}

fn print_snippet(r: &Recipe) {
    println!("Add to {}:\n", r.target_file);
    println!("{}", r.snippet);
    if let Some(note) = &r.note {
        println!("note: {note}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_host_has_a_snippet_mentioning_the_command() {
        for host in [
            HookHost::Claude,
            HookHost::Cursor,
            HookHost::Codex,
            HookHost::Conductor,
            HookHost::Worktrunk,
            HookHost::Generic,
        ] {
            let r = recipe(host);
            assert!(!r.snippet.is_empty());
            // Every recipe invokes workz. Most hosts run `workz sync` in an
            // already-created worktree; Claude's WorktreeCreate must instead
            // `workz start` (it creates the worktree), so accept either verb.
            assert!(
                r.snippet.contains("workz") && r.snippet.contains("--isolated"),
                "host snippet missing command: {}",
                r.snippet
            );
            assert!(!r.target_file.is_empty());
        }
    }

    #[test]
    fn worktrunk_recipe_uses_pre_start_top_level_key() {
        // Regression: worktrunk has no `create` hook and hooks are top-level keys,
        // not a `[hooks]` table. `pre-start` is the provision-before-work hook
        // (verified auto-provisioning a worktree against worktrunk 0.68).
        let r = recipe(HookHost::Worktrunk);
        assert!(r.snippet.contains("pre-start ="), "must use the pre-start hook");
        assert!(!r.snippet.contains("[hooks]"), "worktrunk hooks aren't under [hooks]");
        assert!(!r.snippet.contains("create ="), "worktrunk has no create hook");
    }

    #[test]
    fn cursor_is_installable_and_valid_json() {
        let r = recipe(HookHost::Cursor);
        let (path, content) = r.installable.expect("cursor should be installable");
        assert_eq!(path, ".cursor/worktrees.json");
        // The generated content must be valid JSON.
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(v.get("setup-worktree").is_some());
    }

    #[test]
    fn claude_recipe_uses_native_hook_not_sync() {
        // Regression for #18/#19: the WorktreeCreate recipe must be the native
        // `workz claude-hook` (creates + prints path), never `workz sync` (which
        // creates no worktree and can corrupt the main checkout).
        let r = recipe(HookHost::Claude);
        assert!(
            r.snippet.contains("workz claude-hook"),
            "Claude WorktreeCreate recipe must use the native claude-hook"
        );
        assert!(
            !r.snippet.contains("workz sync"),
            "Claude WorktreeCreate recipe must not use `workz sync`"
        );
        assert!(r.snippet.contains("WorktreeCreate"));
        // The snippet is now a plain settings.json object — must be valid JSON.
        let v: serde_json::Value = serde_json::from_str(&r.snippet).unwrap();
        assert!(v.get("hooks").is_some());
    }
}
