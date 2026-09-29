//! Linux probes: pacman packages and systemd-resolved. The macOS equivalents
//! are Homebrew (`brew.rs`) and `/etc/resolver/` (`resolver.rs`).

use crate::detect::{DetectionItem, Status};
use crate::runner::SystemRunner;

/// Arch package names for what `mkcert -install` needs. `nss` ships
/// `certutil`, which mkcert uses to trust its CA in Chromium's and Firefox's
/// NSS databases.
pub const PACKAGES: &[&str] = &["mkcert", "nss"];

/// What other distributions call these, for the "install them yourself" hint.
pub const MANUAL_INSTALL_HINT: &str = "install mkcert and certutil with your package manager \
     (Debian/Ubuntu: `apt install mkcert libnss3-tools`, Fedora: `dnf install mkcert nss-tools`)";

pub async fn probe_pacman(runner: &SystemRunner) -> DetectionItem {
    if !runner.which("pacman").await {
        return DetectionItem {
            name: "pacman",
            status: Status::Info,
            detail: format!(
                "not found — henk can't install packages for you; {MANUAL_INSTALL_HINT}"
            ),
        };
    }
    let version = runner
        .run("pacman", ["--version"])
        .await
        .ok()
        .and_then(|out| {
            out.stdout.lines().find_map(|line| {
                line.split_once("Pacman v")
                    .map(|(_, rest)| rest.to_string())
            })
        })
        .map(|rest| format!("pacman v{}", rest.split_whitespace().next().unwrap_or("")))
        .unwrap_or_else(|| "installed".into());
    DetectionItem {
        name: "pacman",
        status: Status::Ok,
        detail: version,
    }
}

pub async fn probe_mkcert(runner: &SystemRunner) -> DetectionItem {
    probe_package(runner, "mkcert", "mkcert").await
}

/// `nss` is a library package; `certutil` is the binary mkcert needs from it.
pub async fn probe_nss(runner: &SystemRunner) -> DetectionItem {
    probe_package(runner, "nss", "certutil").await
}

async fn probe_package(
    runner: &SystemRunner,
    package: &'static str,
    binary: &str,
) -> DetectionItem {
    if runner.which(binary).await {
        return DetectionItem {
            name: package,
            status: Status::Ok,
            detail: "installed".into(),
        };
    }
    if runner.which("pacman").await {
        return DetectionItem {
            name: package,
            status: Status::Warn,
            detail: format!("missing — `henk init` will offer `sudo pacman -S {package}`"),
        };
    }
    DetectionItem {
        name: package,
        status: Status::Block,
        detail: format!("`{binary}` missing — {MANUAL_INSTALL_HINT}"),
    }
}

pub async fn probe_resolved(runner: &SystemRunner) -> DetectionItem {
    let active = runner
        .ok("systemctl", ["is-active", "--quiet", "systemd-resolved"])
        .await;
    let resolv_conf = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    resolved_item(active, &resolv_conf)
}

/// henk routes `*.<tld>` through a systemd-resolved drop-in, so without
/// resolved there is nothing to route through. Resolved running but bypassed
/// (`/etc/resolv.conf` not pointing at its stub) still works for programs that
/// resolve through nss-resolve, so that only warns.
fn resolved_item(active: bool, resolv_conf: &str) -> DetectionItem {
    const LINK_STUB: &str = "sudo ln -sf /run/systemd/resolve/stub-resolv.conf /etc/resolv.conf";
    if !active {
        return DetectionItem {
            name: "systemd-resolved",
            status: Status::Block,
            detail: format!(
                "not running — henk resolves *.<tld> through it on Linux. Enable it with \
                 `sudo systemctl enable --now systemd-resolved` and `{LINK_STUB}`"
            ),
        };
    }
    let uses_stub = resolv_conf.lines().any(|line| {
        let mut words = line.split_whitespace();
        words.next() == Some("nameserver") && words.next() == Some("127.0.0.53")
    });
    if !uses_stub {
        return DetectionItem {
            name: "systemd-resolved",
            status: Status::Warn,
            detail: format!(
                "running, but /etc/resolv.conf bypasses it — programs reading that file \
                 won't see *.<tld>. Fix: `{LINK_STUB}`"
            ),
        };
    }
    DetectionItem {
        name: "systemd-resolved",
        status: Status::Ok,
        detail: "running; /etc/resolv.conf uses its stub".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STUB: &str = "# This is /run/systemd/resolve/stub-resolv.conf\n\
                        nameserver 127.0.0.53\noptions edns0 trust-ad\nsearch .\n";

    #[test]
    fn resolved_running_with_stub_is_ok() {
        assert_eq!(resolved_item(true, STUB).status, Status::Ok);
    }

    #[test]
    fn resolved_not_running_blocks_with_the_fix() {
        let item = resolved_item(false, STUB);
        assert_eq!(item.status, Status::Block);
        assert!(
            item.detail
                .contains("systemctl enable --now systemd-resolved")
        );
    }

    #[test]
    fn resolv_conf_bypassing_the_stub_warns() {
        let item = resolved_item(true, "nameserver 192.168.1.1\n");
        assert_eq!(item.status, Status::Warn);
        assert!(item.detail.contains("stub-resolv.conf"));
    }

    #[test]
    fn a_commented_stub_line_does_not_count() {
        let item = resolved_item(true, "# nameserver 127.0.0.53\nnameserver 1.1.1.1\n");
        assert_eq!(item.status, Status::Warn);
    }
}
