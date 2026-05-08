//! `henk init` — first-run setup wizard.
//!
//! With `--dry-run`, runs detection only and prints the report.
//! With `--yes`, accepts every consent prompt automatically (CI / scripted runs).
//! Otherwise, an `inquire`-driven wizard walks the user through:
//!
//! 1. Detection report (read-only probe of the host).
//! 2. Plan summary — every action grouped, with privileged steps flagged.
//! 3. Per-package install consent for missing Homebrew packages.
//! 4. Pre-flight sudo (one password, primes credentials for the rest).
//! 5. Execute (no more prompts past this line).
//! 6. Smoke-test summary so the user can immediately tell whether it worked.
//!
//! Idempotency: re-running `henk init` on an already-initialised host is
//! safe and short-circuits when nothing has drifted (config + cert +
//! resolver + dnsmasq + traefik all good). The wizard surfaces the state
//! before deciding to do work.

use anyhow::{Context, Result, bail};
use std::path::Path;

use crate::config::Config;
use crate::detect::{self, DetectionReport, Status, TldReason};
use crate::runner::SystemRunner;
use crate::stack::lifecycle;
use crate::stack::paths;

pub async fn run(dry_run: bool, tld: Option<String>, yes: bool) -> Result<()> {
    let runner = SystemRunner::new();
    let report: DetectionReport = detect::run_all(&runner, tld.as_deref()).await?;

    if dry_run {
        report.print();
        return Ok(());
    }

    print_wizard_header();
    report.print();

    if report.has_blockers() {
        bail!("Detection found blockers (see above). Resolve them and re-run `henk init`.");
    }

    let cfg = Config::load_or_init(report.tld.value())?;

    // Already-initialised shortcut. If everything is in place, all we
    // need is to make sure the stack is up.
    if !yes && let Some(()) = maybe_already_initialized(&runner, &cfg).await {
        return Ok(());
    }

    print_plan(&report, &cfg);
    if !yes && !confirm("Proceed with these changes?", true)? {
        println!("Aborted. No changes made.");
        return Ok(());
    }

    install_missing_brew_packages(&runner, &report, yes).await?;
    print_section("Privileged steps");
    explain_sudo_usage(&cfg);
    prime_sudo(&runner).await?;

    print_section("Executing");
    lifecycle::init_full(&runner, &cfg).await?;

    print_completion_summary(&cfg);
    Ok(())
}

fn print_wizard_header() {
    use owo_colors::OwoColorize;
    println!();
    println!(
        "{}",
        "╭─ henk · first-run setup ─────────────────────────────╮".bright_blue()
    );
    println!(
        "{}",
        "│ Local-dev URLs over HTTPS for Docker on macOS.       │".bright_blue()
    );
    println!(
        "{}",
        "╰──────────────────────────────────────────────────────╯".bright_blue()
    );
}

fn print_section(title: &str) {
    use owo_colors::OwoColorize;
    println!();
    println!("{}", format!("── {title} ──").bold().bright_blue());
}

fn print_plan(report: &DetectionReport, cfg: &Config) {
    use owo_colors::OwoColorize;

    print_section("Plan");
    println!();

    let missing = missing_brew_pkgs(report);
    if !missing.is_empty() {
        println!(
            "  {} install Homebrew packages: {}",
            "○".bright_black(),
            missing.join(", ")
        );
    }
    println!(
        "  {} install mkcert local CA in your system keychain",
        "○".bright_black()
    );
    println!(
        "  {} issue wildcard cert for *.{tld} (and {tld})",
        "○".bright_black(),
        tld = cfg.tld
    );
    println!(
        "  {} write Homebrew dnsmasq drop-in for .{tld}",
        "○".bright_black(),
        tld = cfg.tld
    );
    println!(
        "  {} {} restart dnsmasq via launchd (binds privileged :53)",
        "○".bright_black(),
        "[sudo]".yellow()
    );
    println!(
        "  {} {} write /etc/resolver/{tld} so *.{tld} resolves locally",
        "○".bright_black(),
        "[sudo]".yellow(),
        tld = cfg.tld
    );
    println!(
        "  {} render the global Traefik stack to ~/.config/henk/traefik/",
        "○".bright_black()
    );
    println!(
        "  {} `docker compose up -d` for the global stack",
        "○".bright_black()
    );
    println!();
    println!(
        "  TLD: .{}  ({})",
        cfg.tld,
        match report.tld.reason() {
            TldReason::Default => "default — RFC 6761 reserved",
            TldReason::ValetHerdFallback => "Valet/Herd already owns `.test`",
            TldReason::UserOverride => "--tld override",
        }
    );
    println!();
    println!(
        "  {} every step is reversible via `henk uninstall` (M7).",
        "i".bright_black()
    );
    println!();
}

fn explain_sudo_usage(cfg: &Config) {
    use owo_colors::OwoColorize;
    println!();
    println!("  henk needs sudo for two things:");
    println!(
        "    1. {}: dnsmasq launchd plist binds :53 (privileged port).",
        "brew services".italic()
    );
    println!(
        "    2. {}: write /etc/resolver/{tld} so macOS routes `*.{tld}` queries.",
        "/etc/resolver".italic(),
        tld = cfg.tld
    );
    println!("  No other privileged operations happen during init.");
    println!();
}

fn missing_brew_pkgs(report: &DetectionReport) -> Vec<&str> {
    let mut missing = Vec::new();
    for item in &report.items {
        if matches!(item.status, Status::Warn)
            && (item.name == "mkcert" || item.name == "nss" || item.name == "dnsmasq")
        {
            missing.push(item.name);
        }
    }
    missing
}

async fn install_missing_brew_packages(
    runner: &SystemRunner,
    report: &DetectionReport,
    auto_yes: bool,
) -> Result<()> {
    let missing = missing_brew_pkgs(report);
    if missing.is_empty() {
        return Ok(());
    }

    print_section("Homebrew packages");
    for pkg in missing {
        let prompt = format!("Install `{pkg}` via Homebrew now?");
        if !auto_yes && !confirm(&prompt, true)? {
            bail!("`{pkg}` is required; aborting.");
        }
        println!("  ⤷ brew install {pkg} ...");
        let exit = runner
            .run_inherit("brew", ["install", pkg])
            .await
            .with_context(|| format!("running `brew install {pkg}`"))?;
        if exit != 0 {
            bail!("`brew install {pkg}` failed with exit code {exit}");
        }
    }
    Ok(())
}

/// Run `sudo -v` to prime the credential cache so subsequent non-interactive
/// `sudo` calls within the cache window don't re-prompt.
async fn prime_sudo(runner: &SystemRunner) -> Result<()> {
    println!("  ⤷ priming sudo credentials (one password prompt) ...");
    let exit = runner
        .run_inherit("sudo", ["-v"])
        .await
        .context("running `sudo -v`")?;
    if exit != 0 {
        bail!("could not prime sudo credentials (exit {exit}). Aborting.");
    }
    Ok(())
}

/// `inquire::Confirm` wrapper that falls back to a stdio prompt on
/// non-TTY runs. Inquire panics if it can't open the terminal, which is
/// not what we want in CI.
fn confirm(prompt: &str, default_yes: bool) -> Result<bool> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        // Non-interactive: honour the default.
        return Ok(default_yes);
    }
    let res = inquire::Confirm::new(prompt)
        .with_default(default_yes)
        .with_help_message("y/n, Enter for default")
        .prompt();
    match res {
        Ok(b) => Ok(b),
        Err(inquire::InquireError::OperationInterrupted)
        | Err(inquire::InquireError::OperationCanceled) => bail!("aborted by user"),
        Err(e) => Err(e.into()),
    }
}

/// Detect the "everything is already set up" case so re-running `henk
/// init` doesn't repeat all of the privileged work. Returns `Some(())`
/// when we short-circuited; `None` when the wizard should continue
/// normally.
async fn maybe_already_initialized(runner: &SystemRunner, cfg: &Config) -> Option<()> {
    let cert = paths::traefik_dir()
        .ok()?
        .join("certs")
        .join(format!("_wildcard.{}.pem", cfg.tld));
    let resolver = Path::new("/etc/resolver").join(&cfg.tld);
    let dnsmasq_drop_in = brew_dnsmasq_dropin_path(runner, &cfg.tld).await?;

    let cert_ok = cert.exists();
    let resolver_ok = resolver.exists();
    let dropin_ok = dnsmasq_drop_in.exists();

    if !(cert_ok && resolver_ok && dropin_ok) {
        return None;
    }

    use owo_colors::OwoColorize;
    print_section("Already initialised");
    println!("  {}  cert         {}", "✓".green(), cert.display());
    println!("  {}  resolver     {}", "✓".green(), resolver.display());
    println!("  {}  dnsmasq      {}", "✓".green(), dnsmasq_drop_in.display());
    println!();
    println!("  Re-running init would only re-render templates and bring the stack up.");
    println!(
        "  Tip: `henk up` brings the stack up; `henk doctor --repair` (M7) fixes drift."
    );

    let bring_up = confirm("Bring the global stack up now?", true).ok()?;
    if bring_up {
        if let Err(e) = lifecycle::up(runner, cfg).await {
            eprintln!("  ✗ {e}");
            return None;
        }
        print_completion_summary(cfg);
    }
    Some(())
}

async fn brew_dnsmasq_dropin_path(runner: &SystemRunner, tld: &str) -> Option<std::path::PathBuf> {
    let out = runner.run("brew", ["--prefix"]).await.ok()?;
    if !out.ok() {
        return None;
    }
    let prefix = out.stdout.trim();
    Some(
        Path::new(prefix)
            .join("etc")
            .join("dnsmasq.d")
            .join(format!("henk-{tld}.conf")),
    )
}

fn print_completion_summary(cfg: &Config) {
    use owo_colors::OwoColorize;
    print_section("Done");
    println!();
    println!("  {}  henk is up.", "✓".green().bold());
    println!();
    println!(
        "  Dashboard:  https://traefik.{tld}",
        tld = cfg.tld
    );
    println!(
        "              http://localhost:{port}",
        port = cfg.ports.dashboard
    );
    println!();
    println!("  Next:  cd into a project and run `henk link`.");
    println!();
}
