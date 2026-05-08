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
use crate::manifest::{InstalledBy, StateManifest, steps};
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

    let mut state = StateManifest::load_or_init(&cfg.tld)?;
    state.open_init_run();
    seed_preexisting_brew_attribution(&mut state, &report);
    state.save()?;

    let exec_result = execute_init(&runner, &cfg, &report, yes, &mut state).await;
    match &exec_result {
        Ok(()) => state.close_init_run("success"),
        Err(_) => state.close_init_run("failed"),
    }
    state.save()?;
    exec_result?;

    print_completion_summary(&cfg);
    Ok(())
}

/// All the privileged + filesystem-mutating work, factored out so the
/// state manifest's `init_runs` entry can be closed regardless of which
/// step failed. Each successful step writes itself into `state.steps`
/// before continuing — so a partial run produces an actionable manifest
/// that `henk doctor --repair` (and `henk uninstall`) can read.
async fn execute_init(
    runner: &SystemRunner,
    cfg: &Config,
    report: &DetectionReport,
    auto_yes: bool,
    state: &mut StateManifest,
) -> Result<()> {
    install_missing_brew_packages(runner, report, auto_yes, state).await?;

    print_section("Privileged steps");
    explain_sudo_usage(cfg);
    prime_sudo(runner, state).await?;

    print_section("Executing");
    lifecycle::init_full(runner, cfg).await?;

    // init_full bundles all the inner work; record the steps it
    // performed in one go. Per-step granularity inside lifecycle is a
    // future refit when `doctor --repair` needs to retry individual
    // failures rather than the whole bundle.
    let cert_path = paths::traefik_dir()
        .ok()
        .map(|p| p.join("certs").join(format!("_wildcard.{}.pem", cfg.tld)));
    let resolver_path = std::path::PathBuf::from(format!("/etc/resolver/{}", cfg.tld));
    let dropin_path = brew_dnsmasq_dropin_path(runner, &cfg.tld).await;

    state.mark_step_complete(steps::MKCERT_CA, None, None);
    state.mark_step_complete(steps::WILDCARD_CERT, cert_path, None);
    state.mark_step_complete(steps::DNSMASQ_DROPIN, dropin_path, None);
    state.mark_step_complete(steps::RESOLVER_FILE, Some(resolver_path), None);
    state.mark_step_complete(steps::STACK_RENDERED, None, None);
    state.mark_step_complete(steps::STACK_UP, None, None);
    state.save()?;

    Ok(())
}

/// For each brew package detection saw as already-installed, record
/// `installed_by = Preexisting` so `henk uninstall --deep` knows it
/// must NOT touch them.
fn seed_preexisting_brew_attribution(state: &mut StateManifest, report: &DetectionReport) {
    for item in &report.items {
        let key = match item.name {
            "mkcert" => steps::BREW_MKCERT,
            "nss" => steps::BREW_NSS,
            "dnsmasq" => steps::BREW_DNSMASQ,
            _ => continue,
        };
        if item.status == Status::Ok && !state.steps.contains_key(key) {
            state.mark_step_complete(key, None, Some(InstalledBy::Preexisting));
        }
    }
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
    state: &mut StateManifest,
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
        state.audit(format!("brew install {pkg}"), exit);
        if exit != 0 {
            let key = brew_step_key(pkg);
            state.mark_step_failed(key, format!("brew install {pkg} exit {exit}"));
            state.save().ok();
            bail!("`brew install {pkg}` failed with exit code {exit}");
        }
        let key = brew_step_key(pkg);
        state.mark_step_complete(key, None, Some(InstalledBy::Henk));
        state.save().ok();
    }
    Ok(())
}

fn brew_step_key(pkg: &str) -> &'static str {
    match pkg {
        "mkcert" => steps::BREW_MKCERT,
        "nss" => steps::BREW_NSS,
        "dnsmasq" => steps::BREW_DNSMASQ,
        _ => steps::BREW_MKCERT, // unreachable: missing list is fixed
    }
}

/// Run `sudo -v` to prime the credential cache so subsequent non-interactive
/// `sudo` calls within the cache window don't re-prompt.
async fn prime_sudo(runner: &SystemRunner, state: &mut StateManifest) -> Result<()> {
    println!("  ⤷ priming sudo credentials (one password prompt) ...");
    let exit = runner
        .run_inherit("sudo", ["-v"])
        .await
        .context("running `sudo -v`")?;
    state.audit("sudo -v", exit);
    state.save().ok();
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
    println!(
        "  {}  dnsmasq      {}",
        "✓".green(),
        dnsmasq_drop_in.display()
    );
    println!();

    // Backfill state.json on first re-run after M7 lands. Pre-M7 installs
    // didn't track state, so `doctor` and `uninstall` had nothing to
    // work with on already-up boxes. We mark filesystem-level steps
    // complete and brew packages `Preexisting` — the safe default
    // means `uninstall --deep` will skip them, never accidentally
    // removing a tool that was on the box before henk arrived.
    if let Err(e) = backfill_state(cfg, &cert, &resolver, &dnsmasq_drop_in) {
        eprintln!("  ! could not backfill state.json: {e}");
    }
    println!("  Re-running init would only re-render templates and bring the stack up.");
    println!("  Tip: `henk up` brings the stack up; `henk doctor --repair` (M7) fixes drift.");

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

/// Build a `state.json` for an already-up host that predates state
/// tracking. Idempotent: re-running on a host with state.json is a
/// no-op.
fn backfill_state(cfg: &Config, cert: &Path, resolver: &Path, dnsmasq_dropin: &Path) -> Result<()> {
    if StateManifest::is_present() {
        return Ok(());
    }
    let mut state = StateManifest::load_or_init(&cfg.tld)?;
    state.audit("state.json backfilled from already-initialised host", 0);

    state.mark_step_complete(steps::BREW_MKCERT, None, Some(InstalledBy::Preexisting));
    state.mark_step_complete(steps::BREW_NSS, None, Some(InstalledBy::Preexisting));
    state.mark_step_complete(steps::BREW_DNSMASQ, None, Some(InstalledBy::Preexisting));
    state.mark_step_complete(steps::MKCERT_CA, None, None);
    state.mark_step_complete(steps::WILDCARD_CERT, Some(cert.to_path_buf()), None);
    state.mark_step_complete(
        steps::DNSMASQ_DROPIN,
        Some(dnsmasq_dropin.to_path_buf()),
        None,
    );
    state.mark_step_complete(steps::RESOLVER_FILE, Some(resolver.to_path_buf()), None);
    state.mark_step_complete(steps::STACK_RENDERED, None, None);
    state.mark_step_complete(steps::STACK_UP, None, None);
    state.save()
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
    println!("  Dashboard:  https://traefik.{tld}", tld = cfg.tld);
    println!(
        "              http://localhost:{port}",
        port = cfg.ports.dashboard
    );
    println!();
    println!("  Next:  cd into a project and run `henk link`.");
    println!();
}
