//! `henk doctor` — re-run all detection probes against the running
//! system, then cross-check against `state.json` to surface drift.
//!
//! `--repair` re-renders the global stack templates and brings them up.
//! Surgical per-step retry waits until init steps land at finer
//! granularity than the current init_full bundle.

use anyhow::Result;

use crate::config::Config;
use crate::detect;
use crate::manifest::{StateManifest, StepState, steps};
use crate::runner::SystemRunner;
use crate::stack::lifecycle;
use crate::stack::paths;

pub async fn run(repair: bool) -> Result<()> {
    use owo_colors::OwoColorize;

    let runner = SystemRunner::new();

    println!();
    println!("{}", "henk — doctor".bold());
    println!();

    let cfg = Config::load()?;
    let Some(cfg) = cfg else {
        println!("  henk has not been initialised yet. Run `henk init`.");
        return Ok(());
    };

    let report = detect::run_all(&runner, None).await?;
    report.print();

    let state = StateManifest::load()?;
    let Some(state) = state else {
        println!(
            "  {}  state.json is missing — `henk init` predates state tracking,",
            "·".bright_black()
        );
        println!("  or you initialised on a clone before M7. Re-run `henk init` to backfill");
        println!("  the manifest; doctor stops here without it.");
        println!();
        return Ok(());
    };
    print_state_summary(&state);

    let drift = collect_drift(&state, &cfg);
    if drift.is_empty() {
        println!(
            "  {}  no drift between state.json and the running system.",
            "✓".green()
        );
    } else {
        println!("  {}  drift detected:", "!".yellow());
        for d in &drift {
            println!("    · {d}");
        }
    }
    println!();

    if !repair {
        if !drift.is_empty() {
            println!("  Run `henk doctor --repair` to fix these.");
        }
        return Ok(());
    }

    println!("{}", "── Repair ──".bold().bright_blue());
    if let Err(e) = lifecycle::up(&runner, &cfg).await {
        eprintln!("  ✗ {e}");
        return Err(e);
    }
    println!("  {}  stack rebuilt and brought up.", "✓".green());
    Ok(())
}

fn print_state_summary(state: &StateManifest) {
    use owo_colors::OwoColorize;
    println!("{}", "State manifest".bold());
    println!("  schema_version:  {}", state.schema_version);
    println!("  stack_version:   {}", state.stack_version);
    println!("  init_runs:       {}", state.init_runs.len());
    let henk_pkgs = state.brew_packages_we_installed();
    let pkg_str = if henk_pkgs.is_empty() {
        "(none)".to_string()
    } else {
        henk_pkgs.join(", ")
    };
    println!("  brew (henk):     {pkg_str}");

    let step_status = |name: &str| -> String {
        match state.steps.get(name) {
            Some(s) => match s.state {
                StepState::Complete => "✓ complete".green().to_string(),
                StepState::Pending => "· pending".bright_black().to_string(),
                StepState::Failed => {
                    format!("{} {}", "✗ failed".red(), s.error.as_deref().unwrap_or(""))
                }
                StepState::Skipped => "○ skipped".bright_black().to_string(),
            },
            None => "· unrecorded".bright_black().to_string(),
        }
    };
    println!();
    println!("  steps:");
    for name in [
        steps::MKCERT_CA,
        steps::WILDCARD_CERT,
        steps::DNSMASQ_DROPIN,
        steps::RESOLVER_FILE,
        steps::STACK_RENDERED,
        steps::STACK_UP,
    ] {
        println!("    {name:18}  {}", step_status(name));
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{InstalledBy, StateManifest};
    use tempfile::TempDir;

    fn cfg() -> Config {
        Config {
            schema_version: 1,
            tld: "test".into(),
            ports: crate::config::Ports {
                http: 80,
                https: 443,
                dnsmasq: 35353,
                dashboard: 19080,
            },
            update_check: true,
        }
    }

    #[test]
    fn drift_flags_missing_cert_path() {
        let mut state = StateManifest::default();
        state.tld = "test".into();
        state.mark_step_complete(
            steps::WILDCARD_CERT,
            Some(std::path::PathBuf::from(
                "/tmp/henk-cert-that-doesnt-exist.pem",
            )),
            None,
        );
        let drift = collect_drift(&state, &cfg());
        assert!(
            drift.iter().any(|d| d.contains("wildcard cert")),
            "expected drift entry for missing cert; got {drift:?}"
        );
    }

    #[test]
    fn drift_flags_missing_resolver_file() {
        let mut state = StateManifest::default();
        state.tld = "test".into();
        state.mark_step_complete(
            steps::RESOLVER_FILE,
            Some(std::path::PathBuf::from(
                "/tmp/henk-resolver-that-doesnt-exist",
            )),
            None,
        );
        let drift = collect_drift(&state, &cfg());
        assert!(
            drift.iter().any(|d| d.contains("resolver file")),
            "expected drift entry for resolver; got {drift:?}"
        );
    }

    #[test]
    fn drift_empty_when_files_exist() {
        let dir = TempDir::new().unwrap();
        let cert = dir.path().join("cert.pem");
        std::fs::write(&cert, "fake").unwrap();
        let mut state = StateManifest::default();
        state.tld = "test".into();
        state.mark_step_complete(steps::WILDCARD_CERT, Some(cert), None);
        // no resolver step → not checked
        let drift = collect_drift(&state, &cfg());
        // Drift may still flag a missing compose file (we didn't render
        // templates here), so allow that one.
        for d in &drift {
            assert!(d.contains("compose file"), "unexpected drift entry: {d}");
        }
    }

    #[test]
    fn print_state_summary_doesnt_panic() {
        let mut state = StateManifest::default();
        state.tld = "test".into();
        state.mark_step_complete(steps::BREW_MKCERT, None, Some(InstalledBy::Henk));
        state.mark_step_complete(steps::WILDCARD_CERT, None, None);
        // Just make sure it doesn't blow up — we don't capture stdout
        // here, the test exists to catch fmt-panics on edge cases.
        print_state_summary(&state);
    }
}

fn collect_drift(state: &StateManifest, cfg: &Config) -> Vec<String> {
    let mut out = Vec::new();

    if let Some(step) = state.steps.get(steps::WILDCARD_CERT)
        && let Some(p) = &step.path
        && !p.exists()
    {
        out.push(format!(
            "wildcard cert recorded at {} but file is missing",
            p.display()
        ));
    }
    if let Some(step) = state.steps.get(steps::RESOLVER_FILE)
        && let Some(p) = &step.path
        && !p.exists()
    {
        out.push(format!(
            "resolver file recorded at {} but missing — `*.{}` won't resolve",
            p.display(),
            cfg.tld
        ));
    }
    if let Some(step) = state.steps.get(steps::DNSMASQ_DROPIN)
        && let Some(p) = &step.path
        && !p.exists()
    {
        out.push(format!(
            "dnsmasq drop-in recorded at {} but missing",
            p.display()
        ));
    }
    if let Ok(compose) = paths::traefik_compose_path()
        && !compose.exists()
    {
        out.push(format!(
            "traefik compose file missing at {} — templates haven't been rendered",
            compose.display()
        ));
    }

    out
}
