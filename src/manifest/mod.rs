//! State manifest at `~/.config/henk/state.json`. The single source of
//! truth for everything henk has done on the user's machine. Drives
//! idempotent `init`, `doctor --repair`, `update` migrations, and
//! `uninstall`.
//!
//! Two design rules:
//!   1. Append-only audit. We never rewrite history; failed steps stay
//!      recorded so `doctor` can see them.
//!   2. `installed_by` matters. `henk uninstall --deep` may only uninstall
//!      Homebrew packages we ourselves installed; pre-existing ones are
//!      sacred. The state file is the only thing that tells us which is
//!      which.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::consts::{STACK_VERSION, STATE_SCHEMA_VERSION};

pub const FILENAME: &str = "state.json";

/// Path to `~/.config/henk/state.json`.
pub fn path() -> Result<PathBuf> {
    Ok(crate::stack::paths::config_dir()?.join(FILENAME))
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct StateManifest {
    pub schema_version: u32,
    pub stack_version: u32,
    /// TLD without leading dot, mirroring Config.
    pub tld: String,
    #[serde(default)]
    pub init_runs: Vec<InitRun>,
    /// Step name → step record. Names are stable identifiers like
    /// `brew_mkcert`, `mkcert_ca`, `wildcard_cert`, `resolver_file`.
    #[serde(default)]
    pub steps: BTreeMap<String, Step>,
    /// Per-project records mirroring `.henk.toml`s. Lets `henk uninstall`
    /// walk every linked project even when the dir was deleted under us.
    #[serde(default)]
    pub linked_projects: Vec<LinkedProject>,
    /// Append-only log of sudo / privileged operations.
    #[serde(default)]
    pub audit: Vec<AuditEntry>,
}

impl Default for StateManifest {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            stack_version: STACK_VERSION,
            tld: String::new(),
            init_runs: Vec::new(),
            steps: BTreeMap::new(),
            linked_projects: Vec::new(),
            audit: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct InitRun {
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    /// `"success" | "failed" | "aborted"`.
    pub result: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Step {
    pub state: StepState,
    /// Filesystem path the step produced (cert file, resolver file, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// For `brew_*` steps: did henk install the package, or was it
    /// already on the box? Drives `uninstall --deep`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_by: Option<InstalledBy>,
    /// Last error message, when `state == Failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Wall-clock when this step last transitioned to its current state.
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StepState {
    Pending,
    Complete,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstalledBy {
    /// We ran `brew install` for it during this session.
    Henk,
    /// Already on the box when we ran detection.
    Preexisting,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct LinkedProject {
    pub slug: String,
    pub path: PathBuf,
    pub mode: String,
    pub hosts: Vec<String>,
    /// Paths to every file henk created inside the project.
    pub files_written: Vec<PathBuf>,
    /// `.env` keys we appended (e.g. `APP_PORT`).
    pub env_appended: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AuditEntry {
    pub ts: DateTime<Utc>,
    pub op: String,
    pub exit: i32,
}

impl StateManifest {
    /// Load `state.json` if it exists, otherwise return `None` so the
    /// caller can decide whether to create a fresh one.
    pub fn load() -> Result<Option<Self>> {
        let p = path()?;
        if !p.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&p)
            .with_context(|| format!("reading {}", p.display()))?;
        let parsed: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parsing {} as JSON", p.display()))?;
        Ok(Some(parsed))
    }

    /// Load existing state or build a fresh skeleton seeded with `tld`.
    pub fn load_or_init(tld: &str) -> Result<Self> {
        if let Some(existing) = Self::load()? {
            return Ok(existing);
        }
        let now = Utc::now();
        Ok(Self {
            schema_version: STATE_SCHEMA_VERSION,
            stack_version: STACK_VERSION,
            tld: tld.to_string(),
            init_runs: Vec::new(),
            steps: BTreeMap::new(),
            linked_projects: Vec::new(),
            audit: vec![AuditEntry {
                ts: now,
                op: "state.json created".into(),
                exit: 0,
            }],
        })
    }

    /// Write atomically (temp + rename) under `~/.config/henk/state.json`.
    pub fn save(&self) -> Result<()> {
        let p = path()?;
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self).context("serialising state.json")?;
        let tmp = p.with_extension("json.tmp");
        fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &p)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), p.display()))?;
        Ok(())
    }

    /// Mark a step complete, optionally recording the path it produced
    /// or who installed (for brew steps).
    pub fn mark_step_complete(
        &mut self,
        name: &str,
        path: Option<PathBuf>,
        installed_by: Option<InstalledBy>,
    ) {
        let now = Utc::now();
        self.steps.insert(
            name.to_string(),
            Step {
                state: StepState::Complete,
                path,
                installed_by,
                error: None,
                updated_at: now,
            },
        );
    }

    pub fn mark_step_failed(&mut self, name: &str, error: impl Into<String>) {
        let now = Utc::now();
        self.steps.insert(
            name.to_string(),
            Step {
                state: StepState::Failed,
                path: None,
                installed_by: None,
                error: Some(error.into()),
                updated_at: now,
            },
        );
    }

    pub fn audit(&mut self, op: impl Into<String>, exit: i32) {
        self.audit.push(AuditEntry {
            ts: Utc::now(),
            op: op.into(),
            exit,
        });
    }

    pub fn open_init_run(&mut self) {
        self.init_runs.push(InitRun {
            started_at: Utc::now(),
            completed_at: None,
            result: "in_progress".into(),
        });
    }

    pub fn close_init_run(&mut self, result: &str) {
        if let Some(last) = self.init_runs.last_mut() {
            last.completed_at = Some(Utc::now());
            last.result = result.to_string();
        }
    }

    /// Brew packages we ourselves installed. Used by `uninstall --deep`
    /// to know which to `brew uninstall`.
    pub fn brew_packages_we_installed(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for name in ["brew_mkcert", "brew_nss", "brew_dnsmasq"] {
            if let Some(step) = self.steps.get(name) {
                if matches!(step.installed_by, Some(InstalledBy::Henk))
                    && step.state == StepState::Complete
                {
                    // Map step name → package name.
                    let pkg = match name {
                        "brew_mkcert" => "mkcert",
                        "brew_nss" => "nss",
                        "brew_dnsmasq" => "dnsmasq",
                        _ => continue,
                    };
                    out.push(pkg);
                }
            }
        }
        out
    }

    /// True if `state.json` exists on disk (i.e. henk has been at least
    /// partially initialised on this machine). Used by `uninstall` to
    /// decide whether there's anything to undo.
    pub fn is_present() -> bool {
        path().map(|p| p.exists()).unwrap_or(false)
    }

    /// Delete the state file. Used by `uninstall` after every other
    /// reversal step has succeeded.
    pub fn delete() -> Result<()> {
        let p = path()?;
        if p.exists() {
            fs::remove_file(&p)
                .with_context(|| format!("removing {}", p.display()))?;
        }
        Ok(())
    }
}

/// Convenience for `mark_path` calls when the step doesn't track an
/// install attribution.
pub fn no_install_attribution() -> Option<InstalledBy> {
    None
}

/// Per-step name constants so callers can't typo a key.
pub mod steps {
    pub const BREW_MKCERT: &str = "brew_mkcert";
    pub const BREW_NSS: &str = "brew_nss";
    pub const BREW_DNSMASQ: &str = "brew_dnsmasq";
    pub const MKCERT_CA: &str = "mkcert_ca";
    pub const WILDCARD_CERT: &str = "wildcard_cert";
    pub const DNSMASQ_DROPIN: &str = "dnsmasq_dropin";
    pub const RESOLVER_FILE: &str = "resolver_file";
    pub const STACK_RENDERED: &str = "stack_rendered";
    pub const STACK_UP: &str = "stack_up";
}

/// Guard helper: load and return state under a borrow that auto-saves
/// when dropped. Used by short critical sections that mutate state and
/// want crash-resilience even if the caller forgets to call `.save()`.
pub struct StateGuard {
    inner: StateManifest,
    dirty: bool,
}

impl StateGuard {
    pub fn open(tld: &str) -> Result<Self> {
        Ok(Self {
            inner: StateManifest::load_or_init(tld)?,
            dirty: false,
        })
    }

    pub fn state(&self) -> &StateManifest {
        &self.inner
    }

    pub fn state_mut(&mut self) -> &mut StateManifest {
        self.dirty = true;
        &mut self.inner
    }
}

impl Drop for StateGuard {
    fn drop(&mut self) {
        if self.dirty {
            // Best-effort save — we can't propagate Result from Drop.
            let _ = self.inner.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn with_temp_home(f: impl FnOnce()) {
        // paths::config_dir uses dirs::home_dir() which reads $HOME.
        // Tests run in parallel by default; serialise mutations via a
        // process-wide mutex so they don't trample each other's
        // temp dirs.
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();

        let dir = TempDir::new().unwrap();
        let prev = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        f();
        match prev {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn roundtrip_empty_state() {
        with_temp_home(|| {
            let s = StateManifest::load_or_init("test").unwrap();
            s.save().unwrap();
            let loaded = StateManifest::load().unwrap().unwrap();
            assert_eq!(loaded.tld, "test");
            assert_eq!(loaded.schema_version, STATE_SCHEMA_VERSION);
            assert!(loaded.steps.is_empty());
        });
    }

    #[test]
    fn mark_step_complete_records_metadata() {
        with_temp_home(|| {
            let mut s = StateManifest::load_or_init("test").unwrap();
            s.mark_step_complete(
                steps::BREW_MKCERT,
                None,
                Some(InstalledBy::Henk),
            );
            s.save().unwrap();
            let loaded = StateManifest::load().unwrap().unwrap();
            let step = loaded.steps.get(steps::BREW_MKCERT).unwrap();
            assert_eq!(step.state, StepState::Complete);
            assert_eq!(step.installed_by, Some(InstalledBy::Henk));
        });
    }

    #[test]
    fn brew_packages_we_installed_excludes_preexisting() {
        with_temp_home(|| {
            let mut s = StateManifest::load_or_init("test").unwrap();
            s.mark_step_complete(steps::BREW_MKCERT, None, Some(InstalledBy::Henk));
            s.mark_step_complete(steps::BREW_NSS, None, Some(InstalledBy::Preexisting));
            s.mark_step_complete(steps::BREW_DNSMASQ, None, Some(InstalledBy::Henk));
            assert_eq!(s.brew_packages_we_installed(), vec!["mkcert", "dnsmasq"]);
        });
    }

    #[test]
    fn audit_appends_with_timestamp() {
        with_temp_home(|| {
            let mut s = StateManifest::load_or_init("test").unwrap();
            s.audit("sudo install /etc/resolver/test", 0);
            s.audit("brew install mkcert", 0);
            assert_eq!(s.audit.len(), 3); // 1 from creation + 2 added
        });
    }

    #[test]
    fn init_run_open_close_pairs_up() {
        with_temp_home(|| {
            let mut s = StateManifest::load_or_init("test").unwrap();
            s.open_init_run();
            assert_eq!(s.init_runs.len(), 1);
            assert!(s.init_runs[0].completed_at.is_none());
            s.close_init_run("success");
            assert!(s.init_runs[0].completed_at.is_some());
            assert_eq!(s.init_runs[0].result, "success");
        });
    }
}
