#![allow(clippy::unwrap_used)]

use so_core::{Config, Paths};
use so_supervisor::services::decider::{self, DeciderError, DeciderPatch, KeyPatch, KeySource};

#[test]
fn validation_accepts_and_rejects_each_field_without_writes_on_error() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(dir.path().to_path_buf());
    let key_path = paths.config_dir.join("decider.env");
    let config = b"# keep config bytes\n";
    let key = b"existing secret bytes\n";
    let cases = [
        (
            "endpoint",
            serde_json::json!("https://example.com/decisions"),
            true,
        ),
        (
            "endpoint",
            serde_json::json!("http://127.0.0.1:1234/"),
            true,
        ),
        ("endpoint", serde_json::json!("http://example.com"), false),
        ("model", serde_json::json!("example/model"), true),
        ("model", serde_json::json!(""), false),
        ("model", serde_json::json!("a b"), false),
        ("fallback_model", serde_json::json!(""), true),
        ("fallback_model", serde_json::json!("none"), true),
        ("fallback_model", serde_json::json!("example/model"), true),
        ("fallback_model", serde_json::json!("a b"), false),
        ("on_unreachable", serde_json::json!("error"), true),
        ("on_unreachable", serde_json::json!("heuristic"), true),
        ("on_unreachable", serde_json::json!("bad"), false),
        ("timeout", serde_json::json!("1s"), true),
        ("timeout", serde_json::json!("120s"), true),
        ("timeout", serde_json::json!("0s"), false),
        ("timeout", serde_json::json!("500ms"), false),
        ("timeout", serde_json::json!("121s"), false),
        ("timeout", serde_json::json!("garbage"), false),
        ("key", serde_json::json!({"env":"TEST_KEY_1"}), true),
        ("key", serde_json::json!({"env":"invalid"}), false),
        ("key", serde_json::json!({"file":"/tmp/key.env"}), true),
        ("key", serde_json::json!({"file":""}), false),
        ("key", serde_json::json!({"paste":"sk-valid"}), true),
        ("key", serde_json::json!({"paste":""}), false),
        ("key", serde_json::json!({"paste":"bad key"}), false),
        ("key", serde_json::json!({"paste":"\"quoted\""}), false),
        ("key", serde_json::json!({"paste":"'quoted'"}), false),
    ];
    for (field, value, accepted) in cases {
        std::fs::write(paths.config_toml(), config).unwrap();
        std::fs::write(&key_path, key).unwrap();
        let result = DeciderPatch::from_value(serde_json::json!({(field):value}))
            .and_then(|patch| decider::set(&paths, &Config::default(), patch));
        assert_eq!(result.is_ok(), accepted, "{field}: {value}: {result:?}");
        if !accepted {
            assert!(result.unwrap_err().to_string().contains(field));
            assert_eq!(std::fs::read(paths.config_toml()).unwrap(), config);
            assert_eq!(std::fs::read(&key_path).unwrap(), key);
        }
    }
    for field in ["min_confidence", "min_margin"] {
        for (value, accepted) in [
            (0.0, true),
            (1.0, true),
            (-0.1, false),
            (1.1, false),
            (f64::NAN, false),
            (f64::INFINITY, false),
        ] {
            std::fs::write(paths.config_toml(), config).unwrap();
            std::fs::write(&key_path, key).unwrap();
            let mut patch = DeciderPatch::default();
            if field == "min_confidence" {
                patch.min_confidence = Some(value);
            } else {
                patch.min_margin = Some(value);
            }
            let result = decider::set(&paths, &Config::default(), patch);
            assert_eq!(result.is_ok(), accepted, "{field}: {value}: {result:?}");
            if !accepted {
                assert!(result.unwrap_err().to_string().contains(field));
                assert_eq!(std::fs::read(paths.config_toml()).unwrap(), config);
                assert_eq!(std::fs::read(&key_path).unwrap(), key);
            }
        }
    }
}

#[test]
fn paste_rolls_back_if_config_edit_fails() {
    // This valid TOML inline table can be loaded, but cannot be edited as a table.
    for existing in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(dir.path().to_path_buf());
        let config = b"router = {}\n";
        std::fs::write(paths.config_toml(), config).unwrap();
        let key_path = paths.config_dir.join("decider.env");
        if existing {
            std::fs::write(&key_path, b"previous key\n").unwrap();
        }
        let result = decider::set(
            &paths,
            &Config::default(),
            DeciderPatch {
                key: Some(KeyPatch::Paste("new-secret".into())),
                ..Default::default()
            },
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read(paths.config_toml()).unwrap(), config);
        if existing {
            assert_eq!(std::fs::read(&key_path).unwrap(), b"previous key\n");
        } else {
            assert!(!key_path.exists());
        }
    }
}

#[test]
fn paste_is_private_and_masked() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(dir.path().to_path_buf());
    let secret = "sk-or-secret1234";
    let patch = DeciderPatch {
        key: Some(KeyPatch::Paste(secret.into())),
        ..Default::default()
    };
    let view = decider::set(&paths, &Config::default(), patch).unwrap();
    let file = paths.config_dir.join("decider.env");
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("OPENROUTER_API_KEY={secret}\n")
    );
    assert_eq!(
        Config::load(&paths).unwrap().router.jev.key_file.as_deref(),
        file.to_str()
    );
    assert!(!serde_json::to_string(&view).unwrap().contains(secret));
}

#[test]
fn accepted_fields_and_environment_override() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(dir.path().to_path_buf());
    let patch: DeciderPatch = serde_json::from_value(serde_json::json!({
        "endpoint": "http://127.0.0.1:1234/decisions", "model": "example/model",
        "fallback_model": "none", "on_unreachable": "heuristic", "timeout": "120s",
        "min_confidence": 0.0, "min_margin": 1.0, "key": {"env": "TEST_KEY"}
    }))
    .unwrap();
    decider::set(&paths, &Config::default(), patch).unwrap();
    let view = decider::view_with_env(&paths, &Config::default(), |name| match name {
        "SUPERORCHESTRATOR_ROUTER_MODEL" => Some("override/model".into()),
        "TEST_KEY" => Some("secret-value-1234".into()),
        _ => None,
    })
    .unwrap();
    assert_eq!(view.effective.primary, "override/model");
    assert!(view.env_overrides.iter().any(|o| o.field == "model"));
    assert_eq!(view.key.source, KeySource::Env);
    assert!(
        !serde_json::to_string(&view)
            .unwrap()
            .contains("secret-value-1234")
    );
}

async fn decisions_server(confidence: f64) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0; 8192];
        let _ = stream.read(&mut buf).await.unwrap();
        let body = serde_json::json!({"model":"mock/model", "answers":{"complexity":{"type":"score","score":2.0,"confidence":confidence,"probabilities":{"0":0.0,"1":0.0,"2":1.0,"3":0.0,"4":0.0},"legend":{}}}}).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    format!("http://{addr}/decisions")
}

#[tokio::test]
async fn live_decider_test_uses_disk_config_and_reports_failures() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(dir.path().to_path_buf());
    let cfg = Config::default();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    decider::set(
        &paths,
        &cfg,
        DeciderPatch {
            key: Some(KeyPatch::Env("SO_E5_TEST_MISSING_KEY".into())),
            ..Default::default()
        },
    )
    .unwrap();
    let missing = decider::test_decider(&paths, &client, None).await.unwrap();
    assert!(!missing.ok && missing.error.unwrap().contains("key"));
    let keyfile = paths.config_dir.join("key.env");
    std::fs::write(&keyfile, "SO_E5_TEST_MISSING_KEY=sk-test-secret\n").unwrap();
    let endpoint = decisions_server(0.9).await;
    decider::set(
        &paths,
        &cfg,
        DeciderPatch {
            endpoint: Some(endpoint),
            fallback_model: Some("none".into()),
            key: Some(KeyPatch::File(keyfile.to_str().unwrap().into())),
            ..Default::default()
        },
    )
    .unwrap();
    let success = decider::test_decider(&paths, &client, None).await.unwrap();
    assert!(success.ok, "{:?}", success.error);
    assert_eq!(success.complexity, Some(50.0));
    assert_eq!(success.level.as_deref(), Some("mid"));
    assert_eq!(success.confidence, Some(0.9));
    assert_eq!(success.model.as_deref(), Some("mock/model"));
    assert!(serde_json::to_value(&success).unwrap()["latency_ms"].is_u64());
    let endpoint = decisions_server(0.1).await;
    decider::set(
        &paths,
        &cfg,
        DeciderPatch {
            endpoint: Some(endpoint),
            ..Default::default()
        },
    )
    .unwrap();
    let rejected = decider::test_decider(&paths, &client, Some("test".into()))
        .await
        .unwrap();
    assert!(!rejected.ok);
    assert!(rejected.error.unwrap().contains("confidence"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    drop(listener);
    decider::set(
        &paths,
        &cfg,
        DeciderPatch {
            endpoint: Some(endpoint),
            ..Default::default()
        },
    )
    .unwrap();
    let unreachable = decider::test_decider(&paths, &client, None).await.unwrap();
    assert!(!unreachable.ok && unreachable.error.is_some());
    Config::set_in_file(
        &paths,
        "router.jev.endpoint",
        "http://example.com/decisions",
    )
    .unwrap();
    let refused = decider::test_decider(&paths, &client, None).await.unwrap();
    assert!(!refused.ok && refused.error.unwrap().contains("endpoint"));
}

#[test]
fn key_sources_and_rejected_mixed_patch() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(dir.path().to_path_buf());
    let running = Config::default();
    let missing = decider::view_with_env(&paths, &running, |_| None).unwrap();
    assert_eq!(missing.key.source, KeySource::Unset);
    assert_eq!(missing.key.masked, None);
    assert_eq!(missing.key.name.as_deref(), Some("OPENROUTER_API_KEY"));
    let invalid = DeciderPatch {
        endpoint: Some("ftp://example.org".into()),
        key: Some(KeyPatch::Paste("a-secret".into())),
        ..Default::default()
    };
    assert!(decider::set(&paths, &running, invalid).is_err());
    assert!(!paths.config_dir.join("decider.env").exists());
    let file = paths.config_dir.join("test.env");
    std::fs::write(&file, "OPENROUTER_API_KEY=file-secret-5678\n").unwrap();
    decider::set(
        &paths,
        &running,
        DeciderPatch {
            key: Some(KeyPatch::File(file.to_string_lossy().into())),
            ..Default::default()
        },
    )
    .unwrap();
    let view = decider::view_with_env(&paths, &running, |_| None).unwrap();
    assert_eq!(view.key.source, KeySource::File);
    assert_eq!(view.key.masked.as_deref(), Some("…5678"));
    let view = decider::view_with_env(&paths, &running, |n| {
        (n == "OPENROUTER_API_KEY").then(|| "env-secret-1234".into())
    })
    .unwrap();
    assert_eq!(view.key.source, KeySource::Env);
    assert_eq!(view.key.masked.as_deref(), Some("…1234"));
    let shadow = view
        .env_overrides
        .iter()
        .find(|v| v.field == "key_file")
        .unwrap();
    assert_eq!(shadow.name, "OPENROUTER_API_KEY");
    assert_eq!(shadow.value, None);
}

#[test]
fn paste_debug_and_tracing_do_not_expose_secret() {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let captured = Capture(Arc::new(Mutex::new(Vec::new())));
    let copy = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || copy.clone())
        .without_time()
        .finish();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(dir.path().to_path_buf());
    let secret = "sk-sensitive-9876";
    let patch = DeciderPatch {
        key: Some(KeyPatch::Paste(secret.into())),
        ..Default::default()
    };
    assert!(!format!("{patch:?}").contains(secret));
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(?patch, "testing decider patch");
        decider::set(&paths, &Config::default(), patch).unwrap();
    });
    assert!(
        !String::from_utf8(captured.0.lock().unwrap().clone())
            .unwrap()
            .contains(secret)
    );
}

#[test]
fn model_override_that_is_also_the_key_is_not_exposed() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(dir.path().to_path_buf());
    let running = Config::default();
    // The API refuses this key_env, but a hand-edited file can still set it.
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::write(
        paths.config_toml(),
        "[router.jev]\nkey_env = \"SUPERORCHESTRATOR_ROUTER_MODEL\"\n",
    )
    .unwrap();
    let secret = "sensitive-secret-1234";
    let view = decider::view_with_env(&paths, &running, |n| {
        (n == "SUPERORCHESTRATOR_ROUTER_MODEL").then(|| secret.into())
    })
    .unwrap();
    assert!(!serde_json::to_string(&view).unwrap().contains(secret));
}

#[test]
fn from_value_errors_name_the_field_and_never_echo_values() {
    let secret = "SECRETVALUE";
    for body in [
        serde_json::json!({"key": secret}),
        serde_json::json!({"key": {"paste": 123}}),
        serde_json::json!({"key": {"paste": secret, "env": secret}}),
        serde_json::json!({"model": 7}),
        serde_json::json!({"on_unreachable": secret}),
        serde_json::json!({ secret: 1 }),
        serde_json::json!(secret),
    ] {
        let error = DeciderPatch::from_value(body).unwrap_err();
        assert!(matches!(error, DeciderError::Invalid(_)));
        assert!(!error.to_string().contains(secret), "{error}");
    }
    let error = DeciderPatch::from_value(serde_json::json!({"key": {"paste": 1}})).unwrap_err();
    assert!(error.to_string().starts_with("key:"), "{error}");
    let ok = DeciderPatch::from_value(
        serde_json::json!({"model": "a/b", "on_unreachable": "heuristic"}),
    )
    .unwrap();
    assert_eq!(
        ok.on_unreachable,
        Some(so_core::config::OnUnreachable::Heuristic)
    );
}

#[test]
fn errors_are_classified_invalid_or_internal_without_the_secret() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(dir.path().to_path_buf());
    let secret = "sk-classify-1234";
    let invalid = decider::set(
        &paths,
        &Config::default(),
        DeciderPatch {
            model: Some("a b".into()),
            key: Some(KeyPatch::Paste(secret.into())),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(invalid, DeciderError::Invalid(_)), "{invalid:?}");
    // A config the editor cannot modify is a server-side failure.
    std::fs::write(paths.config_toml(), b"router = {}\n").unwrap();
    let internal = decider::set(
        &paths,
        &Config::default(),
        DeciderPatch {
            key: Some(KeyPatch::Paste(secret.into())),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        matches!(internal, DeciderError::Internal(_)),
        "{internal:?}"
    );
    assert!(!format!("{internal:?}{internal}").contains(secret));
    std::fs::write(paths.config_toml(), b"not [valid toml").unwrap();
    assert!(matches!(
        decider::view(&paths, &Config::default()).unwrap_err(),
        DeciderError::Internal(_)
    ));
}

#[test]
fn pasted_key_shadowed_by_environment_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(dir.path().to_path_buf());
    let view = decider::set_with_env(
        &paths,
        &Config::default(),
        DeciderPatch {
            key: Some(KeyPatch::Paste("pasted-secret-0000".into())),
            ..Default::default()
        },
        |n| (n == "OPENROUTER_API_KEY").then(|| "env-secret-9999".into()),
    )
    .unwrap();
    let shadow = view
        .env_overrides
        .iter()
        .find(|o| o.field == "key_file")
        .expect("env override for key_file");
    assert_eq!(shadow.name, "OPENROUTER_API_KEY");
    assert_eq!(shadow.value, None);
    assert_eq!(view.key.source, KeySource::Env);
    assert!(!serde_json::to_string(&view).unwrap().contains("secret-"));
}

#[test]
fn model_override_variables_are_rejected_as_key_env() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(dir.path().to_path_buf());
    for name in [
        "SUPERORCHESTRATOR_ROUTER_MODEL",
        "SUPERORCHESTRATOR_ROUTER_FALLBACK_MODEL",
    ] {
        let error = decider::set(
            &paths,
            &Config::default(),
            DeciderPatch {
                key: Some(KeyPatch::Env(name.into())),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(error, DeciderError::Invalid(_)));
        let message = error.to_string();
        assert!(message.starts_with("key.env:") && message.contains("model override"));
    }
}
