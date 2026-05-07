//! Minimal `.env` reader/writer.
//!
//! We deliberately don't depend on a full dotenv parser — we only need
//! to read a handful of well-known keys (`APP_URL`, `PUBLIC_URL`,
//! `NUXT_BASE_URL`, `APP_BASE_URL`, `APP_PORT`) and append append-only
//! lines with consent.
//!
//! Quoting rules covered: bare values, single-quoted, double-quoted,
//! trailing whitespace stripped, `#` comments stripped (only when not
//! inside a quoted value).

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Parse a `.env` body into a key→value map. Order is preserved by the
/// way the BTreeMap iterates lexically (we don't currently need source
/// order). Lines that don't match `KEY=VALUE` (comments, blanks) are
/// ignored.
pub fn parse(body: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        let key = line[..eq].trim().to_string();
        if key.is_empty() {
            continue;
        }
        let value = strip_value(&line[eq + 1..]);
        out.insert(key, value);
    }
    out
}

/// Read and parse `<dir>/.env`. Missing file returns an empty map.
/// Falls back to `.env.local` if `.env` is missing — common Laravel
/// setup. Both? `.env.local` wins (`.env.local` is the per-developer
/// override convention).
pub fn read(dir: &Path) -> Result<BTreeMap<String, String>> {
    let env_local = dir.join(".env.local");
    if env_local.exists() {
        let body = fs::read_to_string(&env_local)
            .with_context(|| format!("reading {}", env_local.display()))?;
        return Ok(parse(&body));
    }
    let env = dir.join(".env");
    if env.exists() {
        let body = fs::read_to_string(&env)
            .with_context(|| format!("reading {}", env.display()))?;
        return Ok(parse(&body));
    }
    Ok(BTreeMap::new())
}

/// Append a single `KEY=VALUE` line to the project's `.env`. Refuses to
/// touch any existing line — append-only is the contract.
///
/// If the key is already present in the file (even commented out), this
/// returns `Ok(false)` without writing, to keep the no-edit guarantee.
/// Caller is expected to detect this and skip the prompt.
pub fn append_if_absent(dir: &Path, key: &str, value: &str) -> Result<bool> {
    let path = dir.join(".env");
    let existing = if path.exists() {
        fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };

    if existing.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with(&format!("{key}=")) || t.starts_with(&format!("# {key}="))
    }) {
        return Ok(false);
    }

    let mut new_contents = existing;
    if !new_contents.is_empty() && !new_contents.ends_with('\n') {
        new_contents.push('\n');
    }
    new_contents.push_str(&format!("{key}={value}\n"));

    let tmp = path.with_extension("tmp");
    fs::write(&tmp, new_contents)
        .with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(true)
}

fn strip_value(raw: &str) -> String {
    let s = raw.trim_start();
    // Strip inline `#` comments — only when value isn't quoted.
    let bytes = s.as_bytes();
    if bytes.first() == Some(&b'"') {
        // double-quoted
        if let Some(end) = s[1..].find('"') {
            return s[1..1 + end].to_string();
        }
    }
    if bytes.first() == Some(&b'\'') {
        // single-quoted
        if let Some(end) = s[1..].find('\'') {
            return s[1..1 + end].to_string();
        }
    }
    // unquoted — strip trailing inline comment + whitespace
    let cut = s.find(" #").unwrap_or(s.len());
    s[..cut].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_and_unquoted_values() {
        let body = r#"
APP_URL="http://localhost"
PUBLIC_URL='http://localhost:8055'
APP_PORT=8080
DEBUG=true   # trailing comment
# whole-line comment
EMPTY=
"#;
        let env = parse(body);
        assert_eq!(env["APP_URL"], "http://localhost");
        assert_eq!(env["PUBLIC_URL"], "http://localhost:8055");
        assert_eq!(env["APP_PORT"], "8080");
        assert_eq!(env["DEBUG"], "true");
        assert_eq!(env["EMPTY"], "");
        assert!(!env.contains_key("# whole-line comment"));
    }

    #[test]
    fn append_if_absent_appends_when_key_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(".env"), "FOO=bar\n").unwrap();
        let appended = append_if_absent(dir.path(), "APP_PORT", "8080").unwrap();
        assert!(appended);
        let body = std::fs::read_to_string(dir.path().join(".env")).unwrap();
        assert!(body.contains("FOO=bar"));
        assert!(body.ends_with("APP_PORT=8080\n"));
    }

    #[test]
    fn append_if_absent_skips_when_key_already_present() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(".env"), "APP_PORT=80\n").unwrap();
        let appended = append_if_absent(dir.path(), "APP_PORT", "8080").unwrap();
        assert!(!appended);
        let body = std::fs::read_to_string(dir.path().join(".env")).unwrap();
        assert_eq!(body, "APP_PORT=80\n"); // untouched
    }
}
