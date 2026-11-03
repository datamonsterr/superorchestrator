//! Endpoint account configuration and provider defaults.

use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

const AUTH: &str = "ANTHROPIC_AUTH_TOKEN";
const API_KEY: &str = "ANTHROPIC_API_KEY";
const ZAI_ENV: &[(&str, &str)] = &[
    ("API_TIMEOUT_MS", "3000000"),
    ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
];

/// Supported endpoint provider presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EndpointPreset {
    /// Z.ai coding plan.
    Zai,
    /// Kimi Code plan.
    Kimi,
    /// MiniMax coding plan.
    Minimax,
    /// User-defined compatible endpoint.
    Custom,
}

impl EndpointPreset {
    /// All supported presets.
    pub const ALL: [Self; 4] = [Self::Zai, Self::Kimi, Self::Minimax, Self::Custom];

    /// Preset name used in account files.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zai => "zai",
            Self::Kimi => "kimi",
            Self::Minimax => "minimax",
            Self::Custom => "custom",
        }
    }

    fn defaults(self) -> &'static Defaults {
        &DEFAULTS[self as usize]
    }
}

impl FromStr for EndpointPreset {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| CoreError::UnknownValue {
                kind: "endpoint preset",
                value: s.into(),
            })
    }
}

struct Defaults {
    base: &'static str,
    token: &'static str,
    slots: [&'static str; 3],
    env: &'static [(&'static str, &'static str)],
}

const DEFAULTS: [Defaults; 4] = [
    Defaults {
        base: "https://api.z.ai/api/anthropic",
        token: AUTH,
        slots: ["glm-5.3", "glm-5.3", "glm-5.3-flash"],
        env: ZAI_ENV,
    },
    Defaults {
        base: "https://api.kimi.ai/coding/",
        token: API_KEY,
        slots: ["k3.1-pro"; 3],
        env: &[],
    },
    Defaults {
        base: "https://api.minimax.io/anthropic",
        token: AUTH,
        slots: ["MiniMax-M3.1-Flash"; 3],
        env: &[],
    },
    Defaults {
        base: "",
        token: AUTH,
        slots: [""; 3],
        env: &[],
    },
];

/// Claude model slot to resolve for an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// Opus model slot.
    Opus,
    /// Sonnet model slot.
    Sonnet,
    /// Haiku model slot.
    Haiku,
}

/// Optional model overrides for Claude slots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slots {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Opus model override.
    pub opus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Sonnet model override.
    pub sonnet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Haiku model override.
    pub haiku: Option<String>,
}

impl Slots {
    fn is_empty(&self) -> bool {
        self.opus.is_none() && self.sonnet.is_none() && self.haiku.is_none()
    }
}

/// Endpoint configuration attached to a Claude account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    /// Provider preset supplying defaults.
    pub preset: EndpointPreset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional API base URL override.
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Environment variable holding the token.
    pub token_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Path to a file holding the token.
    pub token_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Override for the child process token variable.
    pub token_var: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    /// Model IDs explicitly served by this endpoint.
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Slots::is_empty")]
    /// Model overrides for Claude slots.
    pub slots: Slots,
}

impl Endpoint {
    /// Resolve the API base URL.
    pub fn base_url(&self) -> &str {
        self.base_url
            .as_deref()
            .unwrap_or(self.preset.defaults().base)
    }

    /// Resolve the child process token variable.
    pub fn token_var(&self) -> &str {
        self.token_var
            .as_deref()
            .unwrap_or(self.preset.defaults().token)
    }

    /// Return the alternate Anthropic token variable.
    pub fn other_token_var(&self) -> &str {
        if self.token_var() == AUTH {
            API_KEY
        } else {
            AUTH
        }
    }

    /// Resolve a Claude slot to a model ID.
    pub fn slot(&self, slot: Slot) -> &str {
        let (value, index) = match slot {
            Slot::Opus => (&self.slots.opus, 0),
            Slot::Sonnet => (&self.slots.sonnet, 1),
            Slot::Haiku => (&self.slots.haiku, 2),
        };
        value
            .as_deref()
            .unwrap_or(self.preset.defaults().slots[index])
    }

    /// Return the preset-specific environment variables.
    pub fn extra_env(&self) -> &[(&str, &str)] {
        self.preset.defaults().env
    }

    pub(crate) fn validate(&self) -> std::result::Result<(), &'static str> {
        if self.token_env.is_some() == self.token_file.is_some() {
            return Err("endpoint requires exactly one token source");
        }
        if self
            .token_env
            .as_deref()
            .is_some_and(|s| !crate::child_env::valid_name(s))
        {
            return Err("invalid endpoint token_env");
        }
        if self
            .token_var
            .as_deref()
            .is_some_and(|s| s != AUTH && s != API_KEY)
        {
            return Err("invalid endpoint token_var");
        }
        if self.preset == EndpointPreset::Custom
            && (self.base_url.as_deref().is_none_or(str::is_empty)
                || self.models.is_empty()
                || [&self.slots.opus, &self.slots.sonnet, &self.slots.haiku]
                    .iter()
                    .any(|s| s.as_deref().is_none_or(str::is_empty)))
        {
            return Err("custom endpoint requires base_url, models and all slots");
        }
        let url = self.base_url();
        let local = url.strip_prefix("http://").is_some_and(|rest| {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
            ["127.0.0.1", "localhost"].iter().any(|host| {
                authority == *host
                    || authority
                        .strip_prefix(host)
                        .and_then(|s| s.strip_prefix(':'))
                        .is_some_and(|port| {
                            !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
                        })
            })
        });
        if !url.starts_with("https://") && !local {
            return Err("endpoint base_url must use HTTPS or loopback HTTP");
        }
        Ok(())
    }
}
