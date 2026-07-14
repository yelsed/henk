//! Render the global stack config (Traefik + dnsmasq) to
//! `~/.config/henk/traefik/`.
//!
//! Templates are embedded via `include_str!` and substituted using a tiny
//! `{{NAME}}` syntax. We deliberately avoid pulling in a templating engine
//! while substitutions are this trivial; switch to `minijinja` if templates
//! grow.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::consts::HENK_FILE_HEADER;
use crate::stack::paths;

const COMPOSE_TMPL: &str = include_str!("../../assets/traefik/compose.yml.tmpl");
const TRAEFIK_TMPL: &str = include_str!("../../assets/traefik/traefik.yml.tmpl");
const DYNAMIC_TMPL: &str = include_str!("../../assets/traefik/dynamic.yml.tmpl");
const ERRORPAGE_NGINX_TMPL: &str = include_str!("../../assets/errorpages/nginx.conf.tmpl");
const ERRORPAGE_SHELL_TMPL: &str = include_str!("../../assets/errorpages/shell.html.tmpl");

/// The error pages, each rendered twice: HTML for browsers, plain text for
/// curl and coding agents. `nginx.conf` picks between them on `Accept`, and
/// picks the page itself from the request path.
struct ErrorPage {
    /// Basename of the rendered files — `<name>.html` and `<name>.txt`.
    name: &'static str,
    title: &'static str,
    html_body: &'static str,
    text: &'static str,
}

const ERROR_PAGES: &[ErrorPage] = &[
    ErrorPage {
        name: "down",
        title: "Dev server not answering",
        html_body: include_str!("../../assets/errorpages/down.html.tmpl"),
        text: include_str!("../../assets/errorpages/down.txt.tmpl"),
    },
    ErrorPage {
        name: "app-error",
        title: "Your app returned an error",
        html_body: include_str!("../../assets/errorpages/app-error.html.tmpl"),
        text: include_str!("../../assets/errorpages/app-error.txt.tmpl"),
    },
    ErrorPage {
        name: "unlinked",
        title: "Nothing linked to this hostname",
        html_body: include_str!("../../assets/errorpages/unlinked.html.tmpl"),
        text: include_str!("../../assets/errorpages/unlinked.txt.tmpl"),
    },
];

/// Substitution variables shared across the templates.
fn vars_from(cfg: &Config) -> BTreeMap<&'static str, String> {
    let mut vars = BTreeMap::new();
    vars.insert("HENK_FILE_HEADER", HENK_FILE_HEADER.to_string());
    vars.insert("HTTP_PORT", cfg.ports.http.to_string());
    vars.insert("HTTPS_PORT", cfg.ports.https.to_string());
    vars.insert("DASHBOARD_PORT", cfg.ports.dashboard.to_string());
    vars.insert("TLD", cfg.tld.clone());
    vars
}

fn render(template: &str, vars: &BTreeMap<&'static str, String>) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
}

/// Wrap a page's body fragment in the shared HTML shell (head, title, styling).
/// The body is substituted last so its own text can never be read as a
/// template variable.
fn render_html_page(page: &ErrorPage, vars: &BTreeMap<&'static str, String>) -> String {
    let mut shell = render(ERRORPAGE_SHELL_TMPL, vars);
    shell = shell.replace("{{TITLE}}", page.title);
    shell.replace("{{BODY}}", render(page.html_body, vars).trim_end())
}

/// Write all stack-config files into `~/.config/henk/traefik/`.
/// Idempotent: if the existing file matches what we'd write, leaves it
/// untouched. Atomic per-file (temp + rename).
///
/// Returns true when a file the containers only read at boot has changed, so
/// the caller must restart them. Traefik watches the dynamic directory and nginx
/// re-reads the pages each request, but `traefik.yml` and `nginx.conf` are boot
/// config: without a restart the stack silently keeps serving the old routing.
pub fn render_all(cfg: &Config) -> Result<bool> {
    let vars = vars_from(cfg);
    let traefik_dir = paths::traefik_dir()?;
    fs::create_dir_all(&traefik_dir)
        .with_context(|| format!("creating {}", traefik_dir.display()))?;
    fs::create_dir_all(traefik_dir.join("certs"))
        .with_context(|| format!("creating {}", traefik_dir.join("certs").display()))?;

    write_if_changed(
        &paths::traefik_compose_path()?,
        &render(COMPOSE_TMPL, &vars),
    )?;
    let traefik_changed =
        write_if_changed(&paths::traefik_static_path()?, &render(TRAEFIK_TMPL, &vars))?;
    write_if_changed(
        &paths::traefik_dynamic_path()?,
        &render(DYNAMIC_TMPL, &vars),
    )?;
    for page in ERROR_PAGES {
        write_if_changed(
            &paths::errorpage_path(&format!("{}.html", page.name))?,
            &render_html_page(page, &vars),
        )?;
        write_if_changed(
            &paths::errorpage_path(&format!("{}.txt", page.name))?,
            &render(page.text, &vars),
        )?;
    }
    let nginx_changed = write_if_changed(
        &paths::errorpage_nginx_path()?,
        &render(ERRORPAGE_NGINX_TMPL, &vars),
    )?;
    // dnsmasq.conf is no longer rendered into the compose dir — dnsmasq runs
    // under Homebrew/launchd on the host (M3.5). See `stack/dnsmasq.rs`.

    Ok(traefik_changed || nginx_changed)
}

/// Returns true if the file was written (i.e. its contents changed).
fn write_if_changed(path: &Path, contents: &str) -> Result<bool> {
    if let Ok(existing) = fs::read_to_string(path)
        && existing == contents
    {
        return Ok(false);
    }
    let parent = path
        .parent()
        .context("template path must have a parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    // Append rather than replace the extension: `down.html` and `down.txt` would
    // otherwise both stage through `down.tmp`.
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn compose_template_substitutes_ports_and_header() {
        let rendered = render(COMPOSE_TMPL, &vars_from(&cfg()));
        assert!(rendered.contains("# managed by henk"));
        assert!(rendered.contains("\"80:80\""));
        assert!(rendered.contains("\"443:443\""));
        assert!(rendered.contains("127.0.0.1:19080:8080"));
        assert!(rendered.contains("name: henk-proxy"));
        assert!(
            rendered.contains("error-pages"),
            "ships the branded error-pages service"
        );
        assert!(
            rendered.contains("../errorpages:/usr/share/nginx/html"),
            "the whole errorpages dir is the web root, so a new page needs no compose change"
        );
        assert!(
            !rendered.contains("{{"),
            "no template residue: \n{rendered}"
        );
    }

    fn page(name: &str) -> &'static ErrorPage {
        ERROR_PAGES
            .iter()
            .find(|page| page.name == name)
            .expect("page exists")
    }

    #[test]
    fn every_page_renders_as_html_and_as_text() {
        let vars = vars_from(&cfg());
        for page in ERROR_PAGES {
            let html = render_html_page(page, &vars);
            assert!(
                html.contains(page.title),
                "{}: title in the shell",
                page.name
            );
            assert!(html.contains("<main>"), "{}: body in the shell", page.name);
            assert!(html.contains("<style>"), "{}: shared chrome", page.name);
            assert!(!html.contains("{{"), "{}: html residue", page.name);

            let text = render(page.text, &vars);
            // `<sub>` / `<port>` are CLI placeholders, so only a closing tag
            // proves markup leaked in.
            assert!(!text.contains("</"), "{}: text carries markup", page.name);
            assert!(!text.contains("{{"), "{}: text residue", page.name);
        }
    }

    #[test]
    fn down_page_names_the_three_causes_and_hands_off_to_doctor() {
        // Both formats say the same thing — the text one is what a coding agent
        // reads, so it can't be a stub.
        let vars = vars_from(&cfg());
        let down = page("down");
        for rendered in [render_html_page(down, &vars), render(down.text, &vars)] {
            assert!(rendered.contains("0.0.0.0"), "the loopback-bind fix");
            assert!(rendered.contains("allowedHosts"), "the rejected-host fix");
            assert!(rendered.contains("henk doctor"), "hands off to the probe");
            assert!(rendered.contains(".test"), "TLD substituted");
        }
    }

    #[test]
    fn app_error_page_does_not_blame_the_dev_server() {
        // The whole point of splitting this page out: the app answered, so the
        // down page's three fixes are all wrong here.
        let vars = vars_from(&cfg());
        let app_error = page("app-error");
        for rendered in [
            render_html_page(app_error, &vars),
            render(app_error.text, &vars),
        ] {
            assert!(!rendered.contains("0.0.0.0"));
            assert!(!rendered.contains("allowedHosts"));
            assert!(!rendered.contains("isn't running"));
        }
    }

    #[test]
    fn unlinked_page_points_at_status_and_link() {
        let vars = vars_from(&cfg());
        let unlinked = page("unlinked");
        for rendered in [
            render_html_page(unlinked, &vars),
            render(unlinked.text, &vars),
        ] {
            assert!(rendered.contains("henk status"));
            assert!(rendered.contains("henk link"));
        }
    }

    #[test]
    fn error_page_nginx_answers_an_error_status_not_200() {
        // The failover fallback forwards the visitor's original path here, and
        // the catchall router sends unlinked hosts to /unlinked. A 200 would
        // tell the browser (and any script) a dead app is fine. No `=` on
        // error_page, so nginx keeps the original status.
        let rendered = render(ERRORPAGE_NGINX_TMPL, &vars_from(&cfg()));
        assert!(rendered.contains("return 503;"));
        assert!(rendered.contains("return 404;"));
        assert!(rendered.contains("error_page 404 500 501 502 503 504 505 /henk-page;"));
        assert!(
            rendered.contains("listen 8080;"),
            "matches the Traefik service URL"
        );
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn error_page_nginx_negotiates_format_and_page() {
        let rendered = render(ERRORPAGE_NGINX_TMPL, &vars_from(&cfg()));
        assert!(
            rendered.contains("map $http_accept $henk_ext"),
            "html for browsers, text for curl and coding agents"
        );
        assert!(rendered.contains("map $request_uri $henk_body"));
        assert!(rendered.contains("try_files /$henk_body.$henk_ext"));
        assert!(
            rendered.contains("Vary Accept"),
            "the two variants are cacheable"
        );
        for page in ERROR_PAGES {
            assert!(
                rendered.contains(page.name),
                "nginx can reach the {} page",
                page.name
            );
        }
    }

    #[test]
    fn traefik_template_uses_file_provider_only() {
        // M3 architecture: docker provider intentionally absent (Docker 29.x
        // rejects Traefik's hardcoded /v1.24 API calls). henk maintains
        // dynamic.yml directly. See traefik.yml.tmpl comment for context.
        let rendered = render(TRAEFIK_TMPL, &vars_from(&cfg()));
        assert!(rendered.contains("file:"), "needs file provider");
        assert!(
            !rendered.contains("docker:"),
            "must NOT enable docker provider"
        );
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn entrypoints_default_to_the_error_middleware() {
        // Both entrypoints carry `henk-errors@file` so a down container's 502 is
        // rewritten to a branded page (STACK_VERSION 2).
        let rendered = render(TRAEFIK_TMPL, &vars_from(&cfg()));
        assert_eq!(
            rendered.matches("henk-errors@file").count(),
            2,
            "errors middleware default on web + websecure"
        );
    }

    #[test]
    fn dynamic_template_carries_wildcard_cert_paths() {
        let rendered = render(DYNAMIC_TMPL, &vars_from(&cfg()));
        assert!(rendered.contains("certFile: /certs/_wildcard.test.pem"));
        assert!(rendered.contains("keyFile: /certs/_wildcard.test-key.pem"));
        assert!(rendered.contains("# managed by henk"));
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn web_entrypoint_does_not_redirect_wholesale() {
        // An entrypoint-wide redirect would also grab unlinked hostnames and
        // bounce them to https, where the cert can't cover them — a typo would
        // die in the TLS handshake instead of reaching the unlinked page. The
        // redirect is per-host instead (see file_provider).
        let rendered = render(TRAEFIK_TMPL, &vars_from(&cfg()));
        assert!(!rendered.contains("redirections:"));
    }

    #[test]
    fn catchall_answers_unlinked_hosts_on_both_entrypoints() {
        // Priority 1 is the floor; a project router's default priority is its
        // rule length, which is always greater. The http twin exists because an
        // unknown host can't survive the TLS handshake.
        let rendered = render(DYNAMIC_TMPL, &vars_from(&cfg()));
        assert!(rendered.contains("henk-catchall:"));
        assert!(rendered.contains("henk-catchall-http:"));
        assert!(rendered.contains("priority: 1"));
        assert!(
            rendered.contains("path: /unlinked"),
            "replacePath gives nginx a path to key the unlinked page off"
        );
        assert!(
            rendered.contains("henk-https-redirect:"),
            "the per-host routers reference it by name"
        );
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn dynamic_template_defines_error_middleware_and_service() {
        let rendered = render(DYNAMIC_TMPL, &vars_from(&cfg()));
        assert!(
            rendered.contains("henk-errors:"),
            "errors middleware defined"
        );
        assert!(
            rendered.contains("henk-error-pages:"),
            "error service defined"
        );
        assert!(
            rendered.contains("http://error-pages:8080"),
            "points at the error-pages container"
        );
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn fallback_tld_substitutes_throughout() {
        let cfg = Config {
            tld: "henk".into(),
            ..Config::default()
        };
        let v = vars_from(&cfg);
        // Dashboard router rule lives in dynamic.yml (M3 file-provider-only
        // architecture). Cert paths follow the chosen TLD.
        assert!(render(DYNAMIC_TMPL, &v).contains("Host(`traefik.henk`)"));
        assert!(render(DYNAMIC_TMPL, &v).contains("certFile: /certs/_wildcard.henk.pem"));
    }
}
