//! `henk init` — first-run setup.
//!
//! With `--dry-run`, runs detection only and prints the report. Without it,
//! drives the full M3 setup: detection → consent → brew installs → mkcert →
//! resolver write → stack up.
//!
//! The full TUI wizard from Q13 lands in M5; for M3 we use plain stdio
//! prompts that still honour the same consent rules.

use anyhow::{Context, Result, bail};
use std::io::{self, Write};

use crate::config::Config;
use crate::detect::{self, DetectionReport, Status, TldReason};
use crate::runner::SystemRunner;
use crate::stack::lifecycle;

pub async fn run(dry_run: bool, tld: Option<String>, yes: bool) -> Result<()> {
    let runner = SystemRunner::new();
    let report: DetectionReport = detect::run_all(&runner, tld.as_deref()).await?;

    if dry_run {
        report.print();
        return Ok(());
    }

    report.print();

    if report.has_blockers() {
        bail!(
            "Detection found blockers (see above). Resolve them and re-run `henk init`."
        );
    }

    let cfg = Config::load_or_init(report.tld.value())?;

    print_plan(&report, &cfg);
    if !yes && !prompt_yes_no("Proceed?", true)? {
        println!("Aborted. No changes made.");
        return Ok(());
    }

    install_missing_brew_packages(&runner, &report, yes).await?;
    prime_sudo(&runner).await?;

    lifecycle::init_full(&runner, &cfg).await?;
    Ok(())
}

fn print_plan(report: &DetectionReport, cfg: &Config) {
    use owo_colors::OwoColorize;
    println!();
    println!("{}", "henk will perform the following:".bold());
    println!();
    let missing = missing_brew_pkgs(report);
    if !missing.is_empty() {
        println!(
            "  · install missing Homebrew packages (one prompt per package): {}",
            missing.join(", ")
        );
    }
    println!("  · run `mkcert -install` to add the local CA to your system keychain");
    println!(
        "  · issue a wildcard certificate for *.{tld} and {tld}",
        tld = cfg.tld
    );
    println!(
        "  · write a dnsmasq drop-in to $(brew --prefix)/etc/dnsmasq.d/henk-{tld}.conf",
        tld = cfg.tld
    );
    println!(
        "  · `sudo brew services restart dnsmasq` (binds privileged port :53)"
    );
    println!(
        "  · write /etc/resolver/{tld} (one more sudo) so *.{tld} resolves via that dnsmasq",
        tld = cfg.tld
    );
    println!("  · render the global Traefik stack to ~/.config/henk/traefik/");
    println!("  · start the stack via `docker compose up -d`");
    println!();
    println!(
        "  TLD: .{}  ({})",
        cfg.tld,
        match report.tld.reason() {
            TldReason::Default => "default",
            TldReason::ValetHerdFallback => "Valet/Herd fallback",
            TldReason::UserOverride => "--tld override",
        }
    );
    println!();
    println!("All steps are reversible via `henk uninstall` (M7).");
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
    for pkg in missing_brew_pkgs(report) {
        let prompt = format!("Install `{pkg}` via Homebrew now?");
        if !auto_yes && !prompt_yes_no(&prompt, true)? {
            bail!("`{pkg}` is required; aborting.");
        }
        println!("⤷ brew install {pkg} ...");
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

/// Run `sudo -v` interactively to prime the credential cache so subsequent
/// non-interactive `sudo` calls within the cache window don't re-prompt.
async fn prime_sudo(runner: &SystemRunner) -> Result<()> {
    println!();
    println!("⤷ priming sudo (one password prompt) ...");
    let exit = runner
        .run_inherit("sudo", ["-v"])
        .await
        .context("running `sudo -v`")?;
    if exit != 0 {
        bail!("could not prime sudo credentials (exit {exit}). Aborting.");
    }
    Ok(())
}

fn prompt_yes_no(prompt: &str, default_yes: bool) -> Result<bool> {
    let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{prompt} {suffix} ");
    io::stdout().flush().ok();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    let trimmed = buf.trim().to_ascii_lowercase();
    Ok(match trimmed.as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    })
}

