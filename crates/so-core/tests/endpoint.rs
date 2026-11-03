#![allow(clippy::unwrap_used)]

use so_core::{AccountFile, Endpoint, EndpointPreset, Harness, Paths, Slot, Slots};
use std::os::unix::fs::PermissionsExt;

fn endpoint(preset: EndpointPreset) -> Endpoint {
    Endpoint {
        preset,
        base_url: None,
        token_env: Some("MY_TOKEN".into()),
        token_file: None,
        token_var: None,
        models: vec![],
        slots: Slots::default(),
    }
}
fn account(ep: Endpoint) -> AccountFile {
    let mut a = AccountFile::new("claude-test", Harness::Claude, "h", "claude").unwrap();
    a.endpoint = Some(ep);
    a
}
#[test]
fn round_trip_and_legacy() {
    let legacy = AccountFile::new("claude-test", Harness::Claude, "h", "claude").unwrap();
    let text = toml::to_string_pretty(&legacy).unwrap();
    assert_eq!(
        text,
        "name = \"claude-test\"\nharness = \"claude\"\nhome = \"h\"\nbinary = \"claude\"\npriority = 0\ncap = 2\nenabled = true\npools = [\"default\"]\npass_env = []\n"
    );
    assert_eq!(
        toml::to_string_pretty(&toml::from_str::<AccountFile>(&text).unwrap()).unwrap(),
        text
    );
    let mut custom = endpoint(EndpointPreset::Custom);
    custom.token_env = None;
    custom.token_file = Some("/keys/test".into());
    custom.base_url = Some("https://example.org".into());
    custom.models = vec!["m".into()];
    custom.slots = Slots {
        opus: Some("o".into()),
        sonnet: Some("s".into()),
        haiku: Some("h".into()),
    };
    for ep in [endpoint(EndpointPreset::Zai), custom] {
        let a = account(ep);
        a.validate().unwrap();
        let text = toml::to_string_pretty(&a).unwrap();
        assert_eq!(toml::from_str::<AccountFile>(&text).unwrap(), a);
    }
    assert!(
        toml::from_str::<AccountFile>(&format!(
            "{text}\n[endpoint]\npreset = 'zai'\ntoken_env = 'X'\nunknown = 1\n"
        ))
        .is_err()
    );
}
#[test]
fn defaults_and_overrides() {
    for (preset, base, token, opus, haiku) in [
        (
            EndpointPreset::Zai,
            "https://api.z.ai/api/anthropic",
            "ANTHROPIC_AUTH_TOKEN",
            "glm-5.3",
            "glm-5.3-flash",
        ),
        (
            EndpointPreset::Kimi,
            "https://api.kimi.ai/coding/",
            "ANTHROPIC_API_KEY",
            "k3.1-pro",
            "k3.1-pro",
        ),
        (
            EndpointPreset::Minimax,
            "https://api.minimax.io/anthropic",
            "ANTHROPIC_AUTH_TOKEN",
            "MiniMax-M3.1-Flash",
            "MiniMax-M3.1-Flash",
        ),
    ] {
        let mut ep = endpoint(preset);
        assert_eq!(ep.base_url(), base);
        assert_eq!(ep.token_var(), token);
        assert_eq!(ep.slot(Slot::Opus), opus);
        assert_eq!(ep.slot(Slot::Sonnet), opus);
        assert_eq!(ep.slot(Slot::Haiku), haiku);
        assert_ne!(ep.other_token_var(), token);
        assert_eq!(preset.as_str().parse::<EndpointPreset>().unwrap(), preset);
        ep.base_url = Some("https://override".into());
        ep.token_var = Some("ANTHROPIC_API_KEY".into());
        ep.slots.opus = Some("override".into());
        assert_eq!(ep.base_url(), "https://override");
        assert_eq!(ep.token_var(), "ANTHROPIC_API_KEY");
        assert_eq!(ep.other_token_var(), "ANTHROPIC_AUTH_TOKEN");
        assert_eq!(ep.slot(Slot::Opus), "override");
    }
    assert_eq!(
        endpoint(EndpointPreset::Zai).extra_env(),
        &[
            ("API_TIMEOUT_MS", "3000000"),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        ]
    );
    assert!(endpoint(EndpointPreset::Kimi).extra_env().is_empty());
    assert_eq!(
        endpoint(EndpointPreset::Custom).token_var(),
        "ANTHROPIC_AUTH_TOKEN"
    );
    assert_eq!(EndpointPreset::ALL.len(), 4);
    assert!("unknown".parse::<EndpointPreset>().is_err());
}
#[test]
fn validation_rules() {
    let good = account(endpoint(EndpointPreset::Zai));
    assert!(good.validate().is_ok());
    let mut a = good.clone();
    a.harness = Harness::Codex;
    assert!(a.validate().is_err());
    for (env, file) in [
        (None, None),
        (Some("A"), Some("f")),
        (Some(""), None),
        (Some("BAD=NAME"), None),
        (Some("BAD\0NAME"), None),
    ] {
        let mut a = good.clone();
        let ep = a.endpoint.as_mut().unwrap();
        ep.token_env = env.map(str::to_owned);
        ep.token_file = file.map(str::to_owned);
        assert!(a.validate().is_err());
    }
    let mut a = good.clone();
    a.endpoint.as_mut().unwrap().token_env = None;
    a.endpoint.as_mut().unwrap().token_file = Some("file".into());
    assert!(a.validate().is_ok());
    for value in ["bad", "", "ANTHROPIC_BASE_URL"] {
        let mut a = good.clone();
        a.endpoint.as_mut().unwrap().token_var = Some(value.into());
        assert!(a.validate().is_err());
    }
    for value in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"] {
        let mut a = good.clone();
        a.endpoint.as_mut().unwrap().token_var = Some(value.into());
        assert!(a.validate().is_ok());
    }
    for value in [
        "http://example.org",
        "ftp://localhost",
        "http://localhost.evil",
        "http://localhost:1234@evil.example",
        "http://127.0.0.1.evil",
        "",
    ] {
        let mut a = good.clone();
        a.endpoint.as_mut().unwrap().base_url = Some(value.into());
        assert!(a.validate().is_err(), "{value}");
    }
    for value in [
        "https://example.org",
        "http://localhost:1234/path",
        "http://127.0.0.1/path",
    ] {
        let mut a = good.clone();
        a.endpoint.as_mut().unwrap().base_url = Some(value.into());
        assert!(a.validate().is_ok(), "{value}");
    }
    let mut a = account(endpoint(EndpointPreset::Custom));
    assert!(a.validate().is_err());
    let ep = a.endpoint.as_mut().unwrap();
    ep.base_url = Some("https://example.org".into());
    assert!(a.validate().is_err());
    a.endpoint.as_mut().unwrap().models.push("m".into());
    assert!(a.validate().is_err());
    a.endpoint.as_mut().unwrap().slots = Slots {
        opus: Some("o".into()),
        sonnet: Some("s".into()),
        haiku: Some("h".into()),
    };
    assert!(a.validate().is_ok());
}
#[test]
fn key_file_permissions() {
    let d = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(d.path().to_path_buf());
    let p = paths.key_file("abc");
    assert_eq!(p, paths.keys_dir().join("abc.key"));
    so_core::fsutil::write_key_file(&p, "secret").unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "secret\n");
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(paths.keys_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}
#[test]
fn redaction_patterns() {
    for name in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"] {
        assert_eq!(
            so_core::redact::redact(&format!("{name}=secret-value next")),
            format!("{name}=[REDACTED] next")
        );
        assert_eq!(
            so_core::redact::redact(&format!("\"{name}\": \"secret-value\"")),
            format!("\"{name}\": \"[REDACTED]\"")
        );
    }
    let key = "0123456789abcdef0123456789abcdef.AbCd123456789012";
    assert_eq!(so_core::redact::redact(key), "[REDACTED]");
    let mut value = serde_json::json!({"ANTHROPIC_API_KEY": "secret", "nested": {"ANTHROPIC_AUTH_TOKEN": "secret"}});
    so_core::redact::redact_json(&mut value);
    assert_eq!(value["ANTHROPIC_API_KEY"], "[REDACTED]");
    assert_eq!(value["nested"]["ANTHROPIC_AUTH_TOKEN"], "[REDACTED]");
    let near = "0123456789abcdef0123456789abcdeZ.AbCd123456789012";
    assert_eq!(so_core::redact::redact(near), near);
}
