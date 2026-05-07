//! Decide which top-level domain henk should use.
//!
//! Default: `.test` (RFC 6761 reserved).
//! Fallback: `.henk` when Valet/Herd is detected (they own `.test`).
//! Override: whatever the user passes via `--tld`.

use crate::consts::{DEFAULT_TLD, FALLBACK_TLD, RESERVED_TLDS};

#[derive(Debug, Clone)]
pub struct TldChoice {
    value: String,
    reason: TldReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TldReason {
    /// Default `.test`.
    Default,
    /// `.henk` because Valet/Herd is on the box.
    ValetHerdFallback,
    /// User passed `--tld <foo>`.
    UserOverride,
}

impl TldChoice {
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn reason(&self) -> TldReason {
        self.reason
    }

    pub fn summary(&self) -> String {
        let prefix = match self.reason {
            TldReason::Default => "TLD: .{}  (default — RFC 6761 reserved for testing)",
            TldReason::ValetHerdFallback => {
                "TLD: .{}  (Valet/Herd already owns `.test`, falling back)"
            }
            TldReason::UserOverride => "TLD: .{}  (--tld override)",
        };
        prefix.replace("{}", &self.value)
    }
}

pub fn decide(
    user_override: Option<&str>,
    valet_present: bool,
    herd_present: bool,
) -> TldChoice {
    if let Some(raw) = user_override {
        let cleaned = raw.trim_start_matches('.').to_ascii_lowercase();
        return TldChoice {
            value: cleaned,
            reason: TldReason::UserOverride,
        };
    }
    if valet_present || herd_present {
        return TldChoice {
            value: FALLBACK_TLD.to_string(),
            reason: TldReason::ValetHerdFallback,
        };
    }
    TldChoice {
        value: DEFAULT_TLD.to_string(),
        reason: TldReason::Default,
    }
}

/// True if the given TLD is on the reserved-don't-use list.
#[allow(dead_code)] // Used by full init in M5.
pub fn is_reserved(tld: &str) -> bool {
    let lower = tld.trim_start_matches('.').to_ascii_lowercase();
    RESERVED_TLDS.iter().any(|r| *r == lower)
}
