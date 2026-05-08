//! `henk uninstall` — tiered reversal of every change henk has made.
//!
//! Three modes:
//!   - default: stop stack, delete only henk's own files (`~/.config/henk`,
//!     `/etc/resolver/<tld>`, dnsmasq drop-in). Foreign mkcert + nss +
//!     dnsmasq stay installed.
//!   - `--keep-config`: stop the stack but keep `~/.config/henk/`. Useful
//!     when the user wants a clean re-init later without losing their
//!     `state.json` audit log.
//!   - `--deep`: default + `brew uninstall` every Homebrew package
//!     `state.json` says we ourselves installed (`installed_by = Henk`).
//!     Pre-existing packages are NEVER touched.
//!
//! The `# managed by henk` header is the per-file safety net. Even if
//! `state.json` is missing or corrupt, we refuse to delete a file that
//! doesn't carry our marker.

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use std::path::Path;

use crate::consts::HENK_FILE_HEADER;
use crate::manifest::StateManifest;
use crate::runner::SystemRunner;
use crate::stack::lifecycle;
use crate::stack::paths;

pub async fn run(deep: bool, keep_config: bool, auto_yes: bool) -> Result<()> {
    use owo_colors::OwoColorize;

    let runner = SystemRunner::new();

    println!();
    println!("{}", "henk — uninstall".bold());
    println!();

    let state = StateManifest::load()?;
    print_plan(&state, deep, keep_config);

    if !auto_yes && !confirm("Proceed with uninstall?", false)? {
        println!("Aborted. No changes made.");
        return Ok(());
    }
    if auto_yes {
        println!("  --yes given; proceeding without confirmation.");
    }

    println!();
    println!("{}", "── Stopping stack ──".bold().bright_blue());
    let _ = lifecycle::down(&runner).await; // best-effort

    if !keep_config {
        println!();
        println!("{}", "── Removing files ──".bold().bright_blue());
        remove_resolver_file(&runner, &state).await?;
        remove_dnsmasq_dropin(&runner, &state).await?;
        remove_config_dir()?;
        StateManifest::delete()?;
    }

    if deep {
        println!();
        println!("{}", "── Homebrew (deep) ──".bold().bright_blue());
        if let Some(state) = state.as_ref() {
            uninstall_henk_brew_pkgs(&runner, state).await?;
        } else {
            println!(
                "  state.json missing — can't tell which brew packages were installed by henk;"
            );
            println!("  skipping `brew uninstall` (refusing to guess).");
        }
    }

    println!();
    println!("{}  henk has been uninstalled.", "✓".green().bold());
    if !deep {
        println!("  Homebrew packages (mkcert, nss, dnsmasq) left in place. Re-run with `--deep`");
        println!("  to remove the ones henk itself installed.");
    }
    Ok(())
}

fn print_plan(state: &Option<StateManifest>, deep: bool, keep_config: bool) {
    use owo_colors::OwoColorize;
    println!("Will:");
    println!("  · stop the global Traefik stack");
    if !keep_config {
        println!(
            "  · delete {} (henk-authored files only)",
            "~/.config/henk/".italic()
        );
        if let Some(s) = state {
            if let Some(step) = s.steps.get(crate::manifest::steps::RESOLVER_FILE)
                && let Some(p) = &step.path
            {
                println!("  · delete {}", p.display());
            }
            if let Some(step) = s.steps.get(crate::manifest::steps::DNSMASQ_DROPIN)
                && let Some(p) = &step.path
            {
                println!("  · delete {}", p.display());
            }
        }
        println!("  · delete state.json");
    } else {
        println!(
            "  · {} files (--keep-config)",
            "preserve config + state".italic()
        );
    }
    if deep {
        println!();
        println!(
            "  Plus {} (state.json says we installed):",
            "brew uninstall".bold()
        );
        let pkgs = state
            .as_ref()
            .map(|s| s.brew_packages_we_installed())
            .unwrap_or_default();
        if pkgs.is_empty() {
            println!("    (none — every brew package was already on the box)");
        } else {
            for pkg in &pkgs {
                println!("    · {pkg}");
            }
        }
    }
    println!();
    println!(
        "  Foreign files (resolvers, configs without our header) are {} touched.",
        "never".bold()
    );
    println!();
}

/// `inquire::Confirm` wrapper that defaults to `false` for irreversible
/// operations and falls back to the default on non-TTY runs.
fn confirm(prompt: &str, default_yes: bool) -> Result<bool> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Ok(default_yes);
    }
    let res = inquire::Confirm::new(prompt)
        .with_default(default_yes)
        .with_help_message("y/n, Enter for default")
        .prompt();
    match res {
        Ok(b) => Ok(b),
        Err(inquire::InquireError::OperationInterrupted)
        | Err(inquire::InquireError::OperationCanceled) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Decision for whether a file is safe for `henk uninstall` to delete.
/// Encapsulates the "header check" rule so we can unit-test it without
/// touching the filesystem or shelling to sudo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteDecision {
    /// File doesn't exist — nothing to do.
    Absent,
    /// File is ours (header present) — safe to delete.
    DeleteOurs,
    /// File exists but doesn't carry our header — must be left alone.
    RefuseForeign,
}

pub(crate) fn classify_file_for_delete(path: &Path, body: Option<&str>) -> DeleteDecision {
    if !path.exists() {
        return DeleteDecision::Absent;
    }
    let body = body.unwrap_or("");
    if body.contains(HENK_FILE_HEADER) {
        DeleteDecision::DeleteOurs
    } else {
        DeleteDecision::RefuseForeign
    }
}

async fn remove_resolver_file(runner: &SystemRunner, state: &Option<StateManifest>) -> Result<()> {
    let path = state
        .as_ref()
        .and_then(|s| {
            s.steps
                .get(crate::manifest::steps::RESOLVER_FILE)
                .and_then(|step| step.path.clone())
        })
        .unwrap_or_else(|| {
            // Best-effort fallback when state.json is missing: try the
            // default `.test` location. We still header-check before
            // deleting, so this can't clobber a foreign resolver.
            Path::new("/etc/resolver/test").to_path_buf()
        });
    let body = std::fs::read_to_string(&path).ok();
    match classify_file_for_delete(&path, body.as_deref()) {
        DeleteDecision::Absent => return Ok(()),
        DeleteDecision::RefuseForeign => {
            println!(
                "  {}  {} — header check failed; leaving alone.",
                "○".bright_black(),
                path.display()
            );
            return Ok(());
        }
        DeleteDecision::DeleteOurs => {}
    }
    println!("  ⤷ sudo rm {}", path.display());
    let path_str = path.to_str().context("resolver path must be UTF-8")?;
    let exit = runner
        .run_inherit("sudo", ["rm", "-f", path_str])
        .await
        .context("running `sudo rm` on resolver file")?;
    if exit != 0 {
        anyhow::bail!("`sudo rm {}` failed (exit {exit})", path.display());
    }
    println!("    ✓ removed.");
    Ok(())
}

async fn remove_dnsmasq_dropin(runner: &SystemRunner, state: &Option<StateManifest>) -> Result<()> {
    let Some(state) = state else { return Ok(()) };
    let Some(step) = state.steps.get(crate::manifest::steps::DNSMASQ_DROPIN) else {
        return Ok(());
    };
    let Some(path) = &step.path else {
        return Ok(());
    };
    let body = std::fs::read_to_string(path).ok();
    match classify_file_for_delete(path, body.as_deref()) {
        DeleteDecision::Absent => return Ok(()),
        DeleteDecision::RefuseForeign => {
            println!(
                "  {}  {} — header check failed; leaving alone.",
                "○".bright_black(),
                path.display()
            );
            return Ok(());
        }
        DeleteDecision::DeleteOurs => {}
    }
    std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    println!("  ✓ removed {}", path.display());

    // Best-effort restart so the running dnsmasq drops the henk-tld
    // mapping. Won't fail uninstall if the brew binary is absent (e.g.
    // the user already brew-uninstalled dnsmasq manually).
    let _ = runner
        .run_inherit("brew", ["services", "restart", "dnsmasq"])
        .await;
    Ok(())
}

fn remove_config_dir() -> Result<()> {
    let dir = paths::config_dir()?;
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    println!("  ✓ removed {}", dir.display());
    Ok(())
}

async fn uninstall_henk_brew_pkgs(runner: &SystemRunner, state: &StateManifest) -> Result<()> {
    let pkgs = state.brew_packages_we_installed();
    if pkgs.is_empty() {
        println!("  no packages to remove (state.json says nothing was henk-installed).");
        return Ok(());
    }
    for pkg in pkgs {
        println!("  ⤷ brew uninstall {pkg}");
        let exit = runner
            .run_inherit("brew", ["uninstall", pkg])
            .await
            .with_context(|| format!("running `brew uninstall {pkg}`"))?;
        if exit != 0 {
            // Don't bail — uninstall is best-effort. The user can finish
            // up by hand without losing the rest of the cleanup.
            println!("    ! `brew uninstall {pkg}` exited {exit}; skipping.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn classify_returns_absent_for_missing_file() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("nope");
        let res = classify_file_for_delete(&p, None);
        assert_eq!(res, DeleteDecision::Absent);
    }

    #[test]
    fn classify_returns_delete_ours_for_henk_authored() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("ours");
        std::fs::write(&p, format!("{HENK_FILE_HEADER}\nnameserver 127.0.0.1\n")).unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        let res = classify_file_for_delete(&p, Some(&body));
        assert_eq!(res, DeleteDecision::DeleteOurs);
    }

    #[test]
    fn classify_refuses_foreign_resolver_file() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("foreign");
        std::fs::write(
            &p,
            "# resolver written by Valet\nnameserver 127.0.0.1\nport 35353\n",
        )
        .unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        let res = classify_file_for_delete(&p, Some(&body));
        assert_eq!(res, DeleteDecision::RefuseForeign);
    }

    #[test]
    fn classify_refuses_when_body_unreadable_but_file_present() {
        // Edge case: file exists but read failed (perms, etc.). We
        // treat the body as empty, which lacks our header → refuse.
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("opaque");
        std::fs::write(&p, "").unwrap();
        let res = classify_file_for_delete(&p, None);
        assert_eq!(res, DeleteDecision::RefuseForeign);
    }
}
