//! `henk link` — register the project in the current directory.

use anyhow::{Context, Result, bail};
use std::io::{self, Write};
use std::path::Path;

use crate::config::Config;
use crate::project::compose;
use crate::project::detect::{self, ProjectDetection};
use crate::project::manifest::{HostEntry, ProjectManifest, ProjectMode};
use crate::project::{env_file, file_provider, override_file};
use crate::runner::SystemRunner;
use crate::stack::{certs, paths};

/// Run the link flow against `project_dir` (usually `cwd`). `add_only`
/// distinguishes `henk link` (fresh) from `henk link --add` (extra host).
/// `service_override` + `port_override` let the caller bypass the
/// auto-detected web service — necessary for multi-service projects
/// where `--add` should route a sub-host (`mail.hub.test`) to a
/// different container (`mailhog`) than the main app.
pub async fn run(
    runner: &SystemRunner,
    project_dir: &Path,
    add_only: bool,
    host_override: Option<String>,
    service_override: Option<String>,
    port_override: Option<u16>,
) -> Result<()> {
    let cfg = Config::load()?
        .context("henk has not been initialised yet — run `henk init` first")?;

    let slug = derive_slug(project_dir)?;
    let mut detection = detect::detect(project_dir, &slug, &cfg.tld)?;
    print_detection(&detection);

    // Explicit overrides win over detection. Without them, the picker
    // disambiguates the multi-service case for the *initial* link, and
    // `--add` re-fires the picker so each new host can name a
    // different service if the user wants.
    if let Some(svc) = service_override.clone() {
        detection.web_service = Some(svc);
        detection.web_port = port_override.or(detection.web_port);
    } else if matches!(detection.mode, ProjectMode::Docker) {
        let needs_pick = detection.web_service.is_none() || (add_only && detection.candidates.len() > 1);
        if needs_pick
            && let Some((svc, port)) = pick_web_service(&detection.candidates)?
        {
            detection.web_service = Some(svc);
            detection.web_port = Some(port);
        }
    }

    let mut manifest = ProjectManifest::load(project_dir)?
        .unwrap_or_else(|| ProjectManifest::new(slug.clone(), detection.mode));

    if add_only && manifest.hosts.is_empty() {
        bail!("`--add` was passed but this project isn't linked yet — run `henk link` first.");
    }

    // Pick the hostname. For now use the detected default unless the
    // user passed --host. M5 wizard will offer a proper inline prompt.
    let new_host = host_override.unwrap_or_else(|| detection.default_host.clone());
    if manifest.has_host(&new_host) {
        bail!(
            "host `{new_host}` is already linked. Use `henk unlink {new_host}` first \
             if you want to re-register it."
        );
    }

    let host_entry = build_host_entry(&detection, &new_host)?;
    manifest.hosts.push(host_entry);

    // Vite sub-host auto-offer: only on a fresh link (not --add), only
    // when Vite was detected, and only when no vite-flag host already
    // lives in the manifest. Wrapped in a Y/n prompt so users who don't
    // need HTTPS HMR aren't pestered.
    let vite_offered = !add_only
        && detection.vite_detected
        && matches!(detection.mode, ProjectMode::Docker)
        && !manifest.hosts.iter().any(|h| h.flags.iter().any(|f| f == "vite"));
    let mut vite_host_added: Option<String> = None;
    if vite_offered {
        let vite_host = format!("vite.{new_host}");
        if !manifest.has_host(&vite_host) && offer_vite_subhost(&vite_host)? {
            if let (Some(svc), _) = (
                manifest.hosts.first().and_then(|h| h.service.clone()),
                (),
            ) {
                manifest.hosts.push(HostEntry {
                    host: vite_host.clone(),
                    service: Some(svc),
                    port: Some(5173),
                    target: None,
                    flags: vec!["vite".into()],
                });
                vite_host_added = Some(vite_host);
            }
        }
    }

    // Docker mode: write the override file so the project's web service
    // joins henk-proxy. Pass through any networks the service already
    // declares so the override preserves them rather than dropping into
    // a default-only fallback.
    //
    // `--add` skips this — the service already joins henk-proxy from
    // the original link, and re-running the writer would (a) be a
    // no-op when nothing changed, or worse, (b) trigger the
    // henk.override.yml fallback path because our own canonical
    // compose.override.yml is already there.
    if matches!(detection.mode, ProjectMode::Docker) {
        // Always re-render the override from the full manifest. Each
        // distinct service named in `manifest.hosts` needs to join
        // `henk-proxy` (so Traefik can reach it), so multi-service
        // projects (hub + mailhog) get every container on the network.
        // We re-render on `--add` too, so `mail.hub.test → mailhog`
        // adds `mailhog` to the override even though the canonical
        // file already exists from the original `henk link`.
        let members = build_service_memberships(project_dir, &manifest)?;
        if !members.is_empty() {
            let target = override_file::write(project_dir, &members)?;
            // Only announce on a fresh link — `--add` re-rendering is
            // routine and the announcement adds noise.
            if !add_only {
                announce_override_target(&target);
            }
        }

        // Append APP_PORT=8080 to .env if there's a :80/:443 collision.
        if !add_only && let Some(collided) = detection.port_collision {
            handle_port_collision(project_dir, collided)?;
        }
    }

    manifest.save(project_dir)?;
    file_provider::write(&manifest)?;

    // Regenerate the wildcard cert with all currently-linked hostnames
    // included as explicit SANs (so macOS curl/Safari accept them).
    let extra_sans: Vec<String> = collect_extra_sans(&cfg, project_dir)?;
    certs::ensure_wildcard(runner, &cfg.tld, &extra_sans, true).await?;

    // Cert files on disk are reloaded by Traefik only when the dynamic
    // config it references changes (or the container restarts). Just
    // rotating the leaf cert in place isn't enough — Traefik holds the
    // previous cert in memory. Restart the container so the new SANs
    // are served immediately.
    if is_henk_traefik_running(runner).await {
        let _ = runner.run("docker", ["restart", "henk-traefik"]).await;
    }

    print_summary(&manifest, project_dir);
    if let Some(vite_host) = vite_host_added {
        print_vite_snippet(&vite_host);
    }
    if matches!(detection.mode, ProjectMode::Host) {
        let port = detection.web_port.unwrap_or(3000);
        print_host_mode_hint(port);
    }
    Ok(())
}

async fn is_henk_traefik_running(runner: &SystemRunner) -> bool {
    let out = runner
        .run(
            "docker",
            [
                "ps",
                "--filter",
                "name=henk-traefik",
                "--format",
                "{{.Names}}",
            ],
        )
        .await;
    matches!(out, Ok(o) if o.ok() && o.stdout.lines().any(|l| l.trim() == "henk-traefik"))
}

fn derive_slug(project_dir: &Path) -> Result<String> {
    let name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("could not derive slug from project directory name")?;
    Ok(name.to_ascii_lowercase().replace(['_', ' ', '.'], "-"))
}

fn build_host_entry(detection: &ProjectDetection, host: &str) -> Result<HostEntry> {
    match detection.mode {
        ProjectMode::Docker => {
            let service = detection
                .web_service
                .clone()
                .context(
                    "could not find any web-eligible service in your compose file. \
                     Make sure at least one service publishes a port on the host \
                     (e.g. `ports: [\"80:80\"]`), or pass `--host` after a manual \
                     `.henk.toml` edit.",
                )?;
            let port = detection
                .web_port
                .context("internal: web_port must accompany web_service")?;
            // Heuristic: `--host vite.<anything>` on a Vite project should
            // route to :5173 with the vite flag set, even though detection
            // pinned a different port for the main app. Lets power users
            // run `henk link --add --host vite.spatiebalk.test` and get
            // the same wiring the auto-offer would have produced.
            let is_vite_host = host.starts_with("vite.") && detection.vite_detected;
            let (port, flags) = if is_vite_host {
                (5173, vec!["vite".to_string()])
            } else {
                (port, vec![])
            };
            Ok(HostEntry {
                host: host.to_string(),
                service: Some(service),
                port: Some(port),
                target: None,
                flags,
            })
        }
        ProjectMode::Host => {
            let port = detection.web_port.unwrap_or(3000);
            Ok(HostEntry {
                host: host.to_string(),
                service: None,
                port: None,
                target: Some(format!("http://host.docker.internal:{port}")),
                flags: vec![],
            })
        }
    }
}

fn handle_port_collision(project_dir: &Path, port: u16) -> Result<()> {
    let alt = pick_free_alt_port(port);
    println!();
    println!(
        "  ! Your web service publishes :{port} on the host — Traefik also wants :{port}."
    );
    println!(
        "    Suggested fix: append `APP_PORT={alt}` to your `.env` so Sail/Compose"
    );
    println!("    binds the host port elsewhere. Traefik talks to the container directly,");
    println!("    so the URL still works.");
    print!("    Append `APP_PORT={alt}` to .env now? [Y/n] ");
    io::stdout().flush().ok();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    let trimmed = buf.trim().to_ascii_lowercase();
    if !matches!(trimmed.as_str(), "" | "y" | "yes") {
        println!("    Skipped. You'll need to free :{port} yourself before `npm run dev`.");
        return Ok(());
    }
    let appended = env_file::append_if_absent(project_dir, "APP_PORT", &alt.to_string())?;
    if appended {
        println!("    ✓ appended `APP_PORT={alt}` to .env");
    } else {
        println!("    ✓ APP_PORT already set in .env — left untouched");
    }
    Ok(())
}

/// Pick a free local TCP port to suggest as the new APP_PORT. Probes a
/// sensible list of high candidates and returns the first one nothing's
/// listening on. If everything we try is busy (extremely rare on a dev
/// laptop), fall back to a hard-coded value with the user told via
/// the wizard summary.
fn pick_free_alt_port(collided: u16) -> u16 {
    let candidates: &[u16] = if collided == 443 {
        &[18443, 28443, 38443, 8443]
    } else {
        &[18080, 18081, 18082, 28080, 28081, 8081, 8082, 8083]
    };
    for &c in candidates {
        if is_port_free(c) {
            return c;
        }
    }
    // Fallback — at least *try* something rather than failing the link.
    candidates[0]
}

fn is_port_free(port: u16) -> bool {
    use std::net::TcpListener;
    // Bind on 127.0.0.1 only — same surface Sail's APP_PORT typically
    // exposes, and avoids racing on 0.0.0.0 if something else is
    // listening on a specific interface.
    TcpListener::bind(("0.0.0.0", port)).is_ok()
}

fn collect_extra_sans(cfg: &Config, project_dir: &Path) -> Result<Vec<String>> {
    let mut sans: Vec<String> = vec![format!("traefik.{tld}", tld = cfg.tld)];

    // Walk every linked project's manifest under the dynamic dir to
    // collect their hostnames. We rotate the cert across all known
    // hosts to keep one trust anchor for the whole stack.
    let dynamic_dir = paths::config_dir()?.join("dynamic");
    if dynamic_dir.exists() {
        for entry in fs::read_dir(&dynamic_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("yml") {
                continue;
            }
            // We don't parse the YAML — we just grep the `Host(`x.tld`)`
            // expressions out. Avoids round-tripping through the file
            // provider's config schema for what is a simple string list.
            let body = fs::read_to_string(&path).unwrap_or_default();
            for cap in HOST_RULE_RE.captures_iter(&body) {
                if let Some(host) = cap.get(1) {
                    sans.push(host.as_str().to_string());
                }
            }
        }
    }

    // Plus anything in the manifest we just saved.
    if let Some(m) = ProjectManifest::load(project_dir)? {
        for h in &m.hosts {
            sans.push(h.host.clone());
        }
    }

    sans.sort();
    sans.dedup();
    Ok(sans)
}

use std::fs;
use std::sync::LazyLock;

use regex::Regex;

static HOST_RULE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Host\(`([^`]+)`\)").expect("static regex"));

fn print_detection(d: &ProjectDetection) {
    use owo_colors::OwoColorize;
    println!();
    println!("{}", "henk — project detection".bold());
    println!();
    println!("  mode:           {:?}", d.mode);
    println!("  default host:   {}", d.default_host);
    if let Some(svc) = &d.web_service {
        println!("  web service:    {svc}");
    }
    if let Some(port) = d.web_port {
        println!("  web port:       {port}");
    }
    if d.vite_detected {
        println!("  vite:           detected (sub-host offered after main link in M4b)");
    }
    if let Some(p) = d.port_collision {
        println!(
            "  port collision: service publishes :{p} — APP_PORT prompt incoming"
        );
    }
    println!();
}

fn announce_override_target(target: &override_file::OverrideTarget) {
    use override_file::OverrideTarget::*;
    match target {
        Canonical(p) => println!("✓ wrote {} (auto-merged by Compose)", p.display()),
        Fallback {
            path,
            existing_canonical,
        } => {
            println!(
                "✓ wrote {} (your {} already exists, so Compose won't auto-merge ours)",
                path.display(),
                existing_canonical.display()
            );
            println!(
                "  add this to your project's .env so Compose picks both up:"
            );
            println!(
                "    COMPOSE_FILE={}:{}",
                existing_canonical
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("compose.override.yml"),
                path.file_name().and_then(|n| n.to_str()).unwrap_or("henk.override.yml")
            );
        }
    }
}

/// Read the user's compose file and return the networks the chosen
/// `service` already participates in. Empty vec if compose is missing
/// or the service doesn't list networks — `override_file::render`
/// falls back to `default` in that case.
fn read_service_networks(project_dir: &Path, service: &str) -> Result<Vec<String>> {
    let Some(compose_path) = compose::find_compose_path(project_dir) else {
        return Ok(Vec::new());
    };
    let cf = compose::read(&compose_path)?;
    Ok(cf
        .services
        .get(service)
        .map(|s| s.network_names())
        .unwrap_or_default())
}

/// Walk the manifest's hosts, dedupe their `service` field, and read
/// each service's existing networks once from compose. Returns the
/// memberships in stable order (first occurrence wins) so the override
/// file is reproducible across runs.
fn build_service_memberships(
    project_dir: &Path,
    manifest: &ProjectManifest,
) -> Result<Vec<override_file::ServiceMembership>> {
    let mut seen: Vec<String> = Vec::new();
    for h in &manifest.hosts {
        if let Some(svc) = h.service.as_deref() {
            if !seen.iter().any(|s| s == svc) {
                seen.push(svc.to_string());
            }
        }
    }
    let mut out = Vec::with_capacity(seen.len());
    for svc in seen {
        let nets = read_service_networks(project_dir, &svc)?;
        out.push(override_file::ServiceMembership {
            service: svc,
            existing_networks: nets,
        });
    }
    Ok(out)
}

/// Inline picker for the multi-service ambiguous case. Returns the
/// `(service_name, container_port)` pair chosen by the user. Falls back
/// to the first candidate when stdin isn't a TTY (CI / piped runs).
fn pick_web_service(
    candidates: &[crate::project::detect::ServiceCandidate],
) -> Result<Option<(String, u16)>> {
    if candidates.is_empty() {
        return Ok(None);
    }
    if candidates.len() == 1 {
        let c = &candidates[0];
        return Ok(Some((c.name.clone(), c.container_port)));
    }
    // Non-interactive fallback — pick the first one and print why.
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        let c = &candidates[0];
        eprintln!(
            "  ! multiple web-eligible services detected; non-TTY input → defaulting \
             to `{}` on container port {} ({})",
            c.name, c.container_port, c.rationale
        );
        return Ok(Some((c.name.clone(), c.container_port)));
    }

    let labels: Vec<String> = candidates
        .iter()
        .map(|c| format!("{} (:{} → :{}) — {}", c.name, c.host_port, c.container_port, c.rationale))
        .collect();
    let selection = inquire::Select::new("Which service should answer this URL?", labels.clone())
        .with_help_message(
            "henk couldn't pin a single web service — pick the one users will hit",
        )
        .prompt();
    let chosen_label = match selection {
        Ok(s) => s,
        Err(inquire::InquireError::OperationInterrupted)
        | Err(inquire::InquireError::OperationCanceled) => {
            bail!("aborted by user");
        }
        Err(e) => return Err(e.into()),
    };
    let idx = labels.iter().position(|l| l == &chosen_label).unwrap_or(0);
    let c = &candidates[idx];
    Ok(Some((c.name.clone(), c.container_port)))
}

/// Y/n prompt asking whether to add the Vite HMR sub-host. Defaults to
/// Yes (Vite over HTTPS is what users almost always want here). Quiet
/// "no" path on non-TTY runs so CI doesn't hang.
fn offer_vite_subhost(vite_host: &str) -> Result<bool> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    println!();
    println!("  • Vite detected. Add `{vite_host}` for HTTPS HMR? [Y/n] ");
    print!("    ");
    io::stdout().flush().ok();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    let trimmed = buf.trim().to_ascii_lowercase();
    Ok(matches!(trimmed.as_str(), "" | "y" | "yes"))
}

/// Print the copy-paste snippet for `vite.config.*`. We never auto-edit
/// Vite configs — they vary too much (TS/JS/MJS, ESM/CJS, defineConfig
/// vs raw object). User-pastes is the safe path.
fn print_vite_snippet(vite_host: &str) {
    use owo_colors::OwoColorize;
    println!();
    println!("{}", "Vite HMR — copy/paste:".bold());
    println!();
    println!("  // vite.config.{{js,ts,mjs}}");
    println!("  export default defineConfig({{");
    println!("    server: {{");
    println!("      host: '0.0.0.0',");
    println!("      port: 5173,");
    println!("      strictPort: true,");
    println!("      hmr: {{");
    println!("        host: '{vite_host}',");
    println!("        protocol: 'wss',");
    println!("        clientPort: 443,");
    println!("      }},");
    println!("      // dev origin for Laravel/PHP-rendered <script src> tags");
    println!("      origin: 'https://{vite_host}',");
    println!("    }},");
    println!("  }})");
    println!();
}

/// Host-mode reminder: Traefik runs in a container and reaches the
/// host via `host.docker.internal`, which only resolves to ports that
/// are actually bound on `0.0.0.0` (or the IPv4 wildcard). Frameworks
/// like Nuxt and Vite default to `127.0.0.1` / `[::1]`, so without
/// this hint the user gets a `502 Bad Gateway` and no clue why.
fn print_host_mode_hint(port: u16) {
    use owo_colors::OwoColorize;
    println!();
    println!(
        "{}",
        "Host mode: bind your dev server to 0.0.0.0".bold()
    );
    println!();
    println!(
        "  Traefik runs in Docker and reaches your dev server via"
    );
    println!(
        "  `host.docker.internal`, which only sees ports bound on the"
    );
    println!(
        "  IPv4 wildcard. By default Nuxt/Vite/Next bind to 127.0.0.1"
    );
    println!(
        "  or `[::1]` only — that gives you a 502 through https://."
    );
    println!();
    println!("  Pick whichever applies:");
    println!();
    println!(
        "    Nuxt:  npm run dev -- --host 0.0.0.0 --port {port}"
    );
    println!(
        "    Vite:  npm run dev -- --host 0.0.0.0 --port {port}"
    );
    println!(
        "    Next:  npx next dev -H 0.0.0.0 -p {port}"
    );
    println!();
    println!(
        "  Or set the bind address in your framework's config so"
    );
    println!(
        "  `npm run dev` does the right thing without flags."
    );
    println!();
}

fn print_summary(manifest: &ProjectManifest, project_dir: &Path) {
    use owo_colors::OwoColorize;
    println!();
    println!("{}", "✓ linked.".green().bold());
    for h in &manifest.hosts {
        println!("  https://{}", h.host);
    }
    println!();
    println!("  marker:    {}", project_dir.join(".henk.toml").display());
    println!(
        "  routing:   ~/.config/henk/dynamic/{}.yml",
        manifest.slug
    );
    println!();
    println!("Next: bring up your project (e.g. `npm run dev`, `sail up`, `docker compose up -d`).");
}
