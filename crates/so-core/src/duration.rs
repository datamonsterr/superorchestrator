//! Human-readable durations such as `20s`, `5m`, `5h` and `7d`.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::CoreError;

/// A duration serialised as a string like `30m`. A bare integer is rejected: a unit is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HumanDuration(pub Duration);

impl HumanDuration {
    pub const fn from_secs(s: u64) -> Self {
        Self(Duration::from_secs(s))
    }
    pub const fn from_millis(ms: u64) -> Self {
        Self(Duration::from_millis(ms))
    }
    pub fn as_duration(&self) -> Duration {
        self.0
    }
}

impl From<HumanDuration> for Duration {
    fn from(d: HumanDuration) -> Self {
        d.0
    }
}

impl FromStr for HumanDuration {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        let bad = || CoreError::InvalidDuration(s.to_string());
        let split = t.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
        let (num, unit) = t.split_at(split);
        let n: u64 = num.parse().map_err(|_| bad())?;
        let ms = match unit {
            "ms" => Some(n),
            "s" => n.checked_mul(1_000),
            "m" => n.checked_mul(60_000),
            "h" => n.checked_mul(3_600_000),
            "d" => n.checked_mul(86_400_000),
            _ => None,
        }
        .ok_or_else(bad)?;
        Ok(Self::from_millis(ms))
    }
}

impl fmt::Display for HumanDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = self.0.as_millis();
        if ms == 0 {
            return write!(f, "0s");
        }
        for (unit, size) in [
            ("d", 86_400_000u128),
            ("h", 3_600_000),
            ("m", 60_000),
            ("s", 1_000),
        ] {
            if ms.is_multiple_of(size) {
                return write!(f, "{}{unit}", ms / size);
            }
        }
        write!(f, "{ms}ms")
    }
}

impl Serialize for HumanDuration {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for HumanDuration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = HumanDuration;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a duration string with a unit, like \"5m\"")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                v.parse().map_err(E::custom)
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Err(E::custom(format!(
                    "duration `{v}` needs a unit (ms, s, m, h or d), for example \"{v}s\""
                )))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Err(E::custom(format!(
                    "duration `{v}` needs a unit (ms, s, m, h or d), for example \"{v}s\""
                )))
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units() {
        assert_eq!(
            "20s".parse::<HumanDuration>().unwrap(),
            HumanDuration::from_secs(20)
        );
        assert_eq!(
            "5m".parse::<HumanDuration>().unwrap(),
            HumanDuration::from_secs(300)
        );
        assert_eq!(
            "30m".parse::<HumanDuration>().unwrap(),
            HumanDuration::from_secs(1800)
        );
        assert_eq!(
            "5h".parse::<HumanDuration>().unwrap(),
            HumanDuration::from_secs(18000)
        );
        assert_eq!(
            "7d".parse::<HumanDuration>().unwrap(),
            HumanDuration::from_secs(604800)
        );
        assert_eq!(
            "250ms".parse::<HumanDuration>().unwrap(),
            HumanDuration::from_millis(250)
        );
    }

    #[test]
    fn rejects_garbage() {
        for s in ["", "5", "m", "5x", "-5s", "1.5h", "5 m x"] {
            assert!(s.parse::<HumanDuration>().is_err(), "{s}");
        }
    }

    #[test]
    fn display_round_trips() {
        for s in ["20s", "5m", "30m", "5h", "7d", "250ms", "0s"] {
            let d: HumanDuration = s.parse().unwrap();
            assert_eq!(d.to_string(), s);
        }
        assert_eq!(HumanDuration::from_secs(3600).to_string(), "1h");
    }

    #[test]
    fn serde_string_only_integer_rejected() {
        #[derive(Serialize, Deserialize)]
        struct W {
            d: HumanDuration,
        }
        let w: W = toml::from_str("d = \"5m\"").unwrap();
        assert_eq!(w.d, HumanDuration::from_secs(300));
        let err = toml::from_str::<W>("d = 1500").err().unwrap().to_string();
        assert!(err.contains("needs a unit"), "{err}");
        assert!(serde_json::from_str::<HumanDuration>("1500").is_err());
        assert_eq!(
            serde_json::to_string(&HumanDuration::from_secs(20)).unwrap(),
            "\"20s\""
        );
    }
}
