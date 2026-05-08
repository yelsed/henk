//! `henk dashboard` — live read-only TUI showing stack health, linked
//! projects, certificate state. Refreshes every 2 seconds, and on
//! demand via `r`.
//!
//! Render-only — the dashboard never writes anywhere on the system. To
//! restart Traefik or rotate certs, exit and use the regular CLI.

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::stack::paths;

/// Run the dashboard until the user quits with `q` / `Esc` / `Ctrl-C`.
pub fn run() -> Result<()> {
    let mut terminal = setup_terminal()?;
    let res = run_loop(&mut terminal);
    restore_terminal(&mut terminal)?;
    res
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("entering alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend).context("creating ratatui terminal")?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().context("disabling raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leaving alternate screen")?;
    terminal.show_cursor().ok();
    Ok(())
}

const REFRESH_EVERY: Duration = Duration::from_secs(2);
const POLL_TICK: Duration = Duration::from_millis(100);

fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let mut state = DashboardState::collect()?;
    let mut list_state = ListState::default();
    if !state.projects.is_empty() {
        list_state.select(Some(0));
    }

    loop {
        terminal.draw(|f| render(f, &state, &mut list_state))?;

        if state.last_refresh.elapsed() >= REFRESH_EVERY {
            state.refresh()?;
        }

        if !event::poll(POLL_TICK)? {
            continue;
        }
        let evt = event::read()?;
        let Event::Key(key) = evt else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(());
            }
            KeyCode::Char('r') => {
                state.refresh()?;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                move_selection(&mut list_state, &state, -1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                move_selection(&mut list_state, &state, 1);
            }
            _ => {}
        }
    }
}

fn move_selection(list_state: &mut ListState, state: &DashboardState, delta: i32) {
    if state.projects.is_empty() {
        return;
    }
    let len = state.projects.len() as i32;
    let cur = list_state.selected().unwrap_or(0) as i32;
    let next = (cur + delta).rem_euclid(len);
    list_state.select(Some(next as usize));
}

#[derive(Debug, Clone)]
struct DashboardState {
    tld: String,
    http_port: u16,
    https_port: u16,
    dashboard_port: u16,
    traefik_running: bool,
    dnsmasq_answering: bool,
    cert_sans: Vec<String>,
    cert_expires: Option<String>,
    projects: Vec<ProjectRow>,
    last_refresh: Instant,
}

#[derive(Debug, Clone)]
struct ProjectRow {
    slug: String,
    hosts: Vec<String>,
    /// Best-effort mode from the file-provider entry (`docker` if backend
    /// URL points at a service hostname; `host` if it points at
    /// `host.docker.internal`). Picked from the YAML body.
    mode: String,
    /// Backend URL(s) Traefik forwards to.
    backends: Vec<String>,
}

impl DashboardState {
    fn collect() -> Result<Self> {
        let cfg = Config::load()?;
        let (tld, http, https, dash) = match cfg {
            Some(c) => (c.tld, c.ports.http, c.ports.https, c.ports.dashboard),
            None => ("test".into(), 80, 443, 19080),
        };
        let mut state = Self {
            tld,
            http_port: http,
            https_port: https,
            dashboard_port: dash,
            traefik_running: false,
            dnsmasq_answering: false,
            cert_sans: Vec::new(),
            cert_expires: None,
            projects: Vec::new(),
            last_refresh: Instant::now(),
        };
        state.refresh()?;
        Ok(state)
    }

    fn refresh(&mut self) -> Result<()> {
        self.traefik_running = container_running("henk-traefik");
        self.dnsmasq_answering = dnsmasq_answers(&self.tld);
        let cert_path = cert_path_for(&self.tld);
        if let Some(p) = cert_path.as_deref()
            && p.exists()
        {
            self.cert_sans = read_cert_sans(p).unwrap_or_default();
            self.cert_expires = read_cert_expiry(p);
        } else {
            self.cert_sans.clear();
            self.cert_expires = None;
        }
        self.projects = collect_projects()?;
        self.last_refresh = Instant::now();
        Ok(())
    }
}

fn cert_path_for(tld: &str) -> Option<PathBuf> {
    paths::traefik_dir().ok().map(|p| {
        p.join("certs").join(format!("_wildcard.{tld}.pem"))
    })
}

fn render(
    f: &mut ratatui::Frame,
    state: &DashboardState,
    list_state: &mut ListState,
) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Min(8),     // Body
            Constraint::Length(3),  // Footer
        ])
        .split(area);

    render_title(f, state, chunks[0]);
    render_body(f, state, list_state, chunks[1]);
    render_footer(f, chunks[2]);
}

fn render_title(f: &mut ratatui::Frame, state: &DashboardState, area: Rect) {
    let stack_status = if state.traefik_running {
        Span::styled("running", Style::default().fg(Color::Green))
    } else {
        Span::styled("stopped", Style::default().fg(Color::Red))
    };
    let dnsmasq_status = if state.dnsmasq_answering {
        Span::styled(":53 answering", Style::default().fg(Color::Green))
    } else {
        Span::styled(":53 silent", Style::default().fg(Color::Red))
    };
    let line = Line::from(vec![
        Span::styled(
            "henk dashboard",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  ·  .{}", state.tld)),
        Span::raw("  ·  stack: "),
        stack_status,
        Span::raw("  ·  dnsmasq: "),
        dnsmasq_status,
        Span::raw(format!(
            "  ·  ports {}/{}/{}",
            state.http_port, state.https_port, state.dashboard_port
        )),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    let p = Paragraph::new(line).block(block);
    f.render_widget(p, area);
}

fn render_body(
    f: &mut ratatui::Frame,
    state: &DashboardState,
    list_state: &mut ListState,
    area: Rect,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(5)])
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Min(1)])
        .split(chunks[0]);

    render_project_list(f, state, list_state, cols[0]);
    render_project_detail(f, state, list_state.selected(), cols[1]);
    render_cert_panel(f, state, chunks[1]);
}

fn render_project_list(
    f: &mut ratatui::Frame,
    state: &DashboardState,
    list_state: &mut ListState,
    area: Rect,
) {
    let items: Vec<ListItem> = if state.projects.is_empty() {
        vec![ListItem::new(Span::styled(
            "(no linked projects)",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        state
            .projects
            .iter()
            .map(|p| {
                let dot = match p.mode.as_str() {
                    "docker" => Span::styled("● ", Style::default().fg(Color::Blue)),
                    _ => Span::styled("● ", Style::default().fg(Color::Magenta)),
                };
                ListItem::new(Line::from(vec![
                    dot,
                    Span::raw(p.slug.clone()),
                    Span::styled(
                        format!("  ({})", p.hosts.len()),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Linked projects ");
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    f.render_stateful_widget(list, area, list_state);
}

fn render_project_detail(
    f: &mut ratatui::Frame,
    state: &DashboardState,
    selected: Option<usize>,
    area: Rect,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Selected ");

    let lines: Vec<Line> = match selected.and_then(|i| state.projects.get(i)) {
        None => vec![Line::from(Span::styled(
            "no project selected",
            Style::default().fg(Color::DarkGray),
        ))],
        Some(p) => {
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("slug:    ", Style::default().fg(Color::DarkGray)),
                    Span::raw(p.slug.clone()),
                ]),
                Line::from(vec![
                    Span::styled("mode:    ", Style::default().fg(Color::DarkGray)),
                    Span::raw(p.mode.clone()),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "hosts",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
            ];
            for h in &p.hosts {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("https://{h}"),
                        Style::default().fg(Color::Green),
                    ),
                ]));
            }
            if !p.backends.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "backends",
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                for b in &p.backends {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(b.clone(), Style::default().fg(Color::Cyan)),
                    ]));
                }
            }
            lines
        }
    };

    let p = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn render_cert_panel(f: &mut ratatui::Frame, state: &DashboardState, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if state.cert_sans.is_empty() {
        lines.push(Line::from(Span::styled(
            "no wildcard cert on disk yet",
            Style::default().fg(Color::Red),
        )));
    } else {
        let exp = state
            .cert_expires
            .clone()
            .unwrap_or_else(|| "unknown".into());
        lines.push(Line::from(vec![
            Span::styled("cert: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} SANs", state.cert_sans.len()),
                Style::default().fg(Color::Green),
            ),
            Span::raw(format!("  ·  expires {exp}")),
        ]));
        let preview: Vec<&str> = state
            .cert_sans
            .iter()
            .take(8)
            .map(String::as_str)
            .collect();
        let suffix = if state.cert_sans.len() > 8 {
            ", …"
        } else {
            ""
        };
        lines.push(Line::from(format!("{}{}", preview.join(", "), suffix)));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Certificate ");
    let p = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn render_footer(f: &mut ratatui::Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled("q", Style::default().fg(Color::Yellow)),
        Span::raw(" quit  ·  "),
        Span::styled("r", Style::default().fg(Color::Yellow)),
        Span::raw(" refresh  ·  "),
        Span::styled("↑/k ↓/j", Style::default().fg(Color::Yellow)),
        Span::raw(" navigate"),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    let p = Paragraph::new(line).block(block);
    f.render_widget(p, area);
}

// ── system probes ──────────────────────────────────────────────────────────

fn container_running(name: &str) -> bool {
    let out = Command::new("docker")
        .args(["ps", "--filter", &format!("name={name}"), "--format", "{{.Names}}"])
        .output();
    matches!(out, Ok(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim() == name))
}

fn dnsmasq_answers(tld: &str) -> bool {
    let probe = format!("henk-dashboard-probe.{tld}");
    let out = Command::new("dig")
        .args(["+short", "+time=1", "+tries=1", "@127.0.0.1", &probe])
        .output();
    matches!(out, Ok(o) if o.status.success())
}

fn read_cert_sans(path: &std::path::Path) -> Option<Vec<String>> {
    let out = Command::new("openssl")
        .args(["x509", "-in"])
        .arg(path)
        .args(["-noout", "-text"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut iter = text.lines();
    while let Some(line) = iter.next() {
        if line.trim_start().starts_with("X509v3 Subject Alternative Name") {
            let next = iter.next()?.trim();
            return Some(
                next.split(',')
                    .filter_map(|s| s.trim().strip_prefix("DNS:").map(str::to_string))
                    .collect(),
            );
        }
    }
    None
}

fn read_cert_expiry(path: &std::path::Path) -> Option<String> {
    let out = Command::new("openssl")
        .args(["x509", "-in"])
        .arg(path)
        .args(["-noout", "-enddate"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim().strip_prefix("notAfter=").map(String::from)
}

fn collect_projects() -> Result<Vec<ProjectRow>> {
    let dyn_dir = paths::dynamic_projects_dir()?;
    if !dyn_dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dyn_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("yml") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        if stem.starts_with('_') {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        out.push(parse_project_row(stem, &body));
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(out)
}

fn parse_project_row(slug: String, body: &str) -> ProjectRow {
    use std::sync::LazyLock;
    static HOST_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"Host\(`([^`]+)`\)").expect("static regex"));
    static URL_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r#"url:\s*"([^"]+)""#).expect("static regex"));

    let hosts: Vec<String> = HOST_RE
        .captures_iter(body)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect();
    let backends: Vec<String> = URL_RE
        .captures_iter(body)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect();
    let mode = if backends
        .iter()
        .any(|u| u.contains("host.docker.internal"))
    {
        "host".to_string()
    } else {
        "docker".to_string()
    };
    ProjectRow {
        slug,
        hosts,
        mode,
        backends,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_project_row_extracts_hosts_and_backends() {
        let body = r#"
http:
  routers:
    spatiebalk:
      rule: "Host(`spatiebalk.test`)"
      service: spatiebalk
    spatiebalk-vite:
      rule: "Host(`vite.spatiebalk.test`)"
      service: spatiebalk-vite
  services:
    spatiebalk:
      loadBalancer:
        servers:
          - url: "http://laravel.test:80"
    spatiebalk-vite:
      loadBalancer:
        servers:
          - url: "http://laravel.test:5173"
"#;
        let row = parse_project_row("spatiebalk".into(), body);
        assert_eq!(row.slug, "spatiebalk");
        assert_eq!(
            row.hosts,
            vec!["spatiebalk.test".to_string(), "vite.spatiebalk.test".to_string()]
        );
        assert_eq!(row.backends.len(), 2);
        assert_eq!(row.mode, "docker");
    }

    #[test]
    fn parse_project_row_detects_host_mode_from_backend_url() {
        let body = r#"
services:
  sparkle:
    loadBalancer:
      servers:
        - url: "http://host.docker.internal:3000"
http:
  routers:
    sparkle:
      rule: "Host(`sparkle.test`)"
"#;
        let row = parse_project_row("sparkle".into(), body);
        assert_eq!(row.mode, "host");
    }
}
