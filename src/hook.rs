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
            snippet: format!("[hooks]\ncreate = \"{CMD}\"\n"),
            target_file: "your worktrunk config (e.g. ~/.config/worktrunk/config.toml)".to_string(),
            installable: None,
            note: None,
        },
        HookHost::Claude => Recipe {
            snippet: format!(
                "{{\n  \"hooks\": {{\n    \"WorktreeCreate\": [\n      {{ \"hooks\": [ {{ \"type\": \"command\", \"command\": \"{CMD}\" }} ] }}\n    ]\n  }}\n}}\n"
            ),
            target_file: ".claude/settings.json".to_string(),
            installable: None,
            note: Some(
                "Claude Code runs WorktreeCreate hooks in the new worktree. Merge this into your existing settings.json rather than replacing it."
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
            // Cursor uses a JSON array ["workz","sync",…], so check the tokens
            // rather than the joined string.
            assert!(
                r.snippet.contains("workz") && r.snippet.contains("sync"),
                "host snippet missing command: {}",
                r.snippet
            );
            assert!(!r.target_file.is_empty());
        }
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
    fn claude_snippet_is_valid_json() {
        let r = recipe(HookHost::Claude);
        let _: serde_json::Value = serde_json::from_str(&r.snippet).unwrap();
    }
}
