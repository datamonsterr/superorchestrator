#![allow(clippy::unwrap_used)]

use so_core::{Config, Paths};
use so_supervisor::services::decider::{self, DeciderPatch, KeyPatch};

#[test]
fn invalid_patches_do_not_write() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(dir.path().to_path_buf());
    std::fs::write(paths.config_toml(), "# keep\n").unwrap();
    for (field, value) in [
        ("endpoint", serde_json::json!("http://example.com")),
        ("model", serde_json::json!("a b")),
        ("fallback_model", serde_json::json!("a b")),
        ("on_unreachable", serde_json::json!("bad")),
        ("timeout", serde_json::json!("121s")),
        ("min_confidence", serde_json::json!(1.1)),
        ("min_margin", serde_json::json!(-0.1)),
        ("key", serde_json::json!({"env":"invalid"})),
        ("key", serde_json::json!({"file":""})),
        ("key", serde_json::json!({"paste":"bad key"})),
        ("key", serde_json::json!({"paste":"\"quoted\""})),
    ] {
        let patch: DeciderPatch =
            serde_json::from_value(serde_json::json!({(field):value})).unwrap();
        let error = decider::set(&paths, &Config::default(), patch).unwrap_err();
        assert!(error.contains(field), "{field}: {error}");
        assert_eq!(std::fs::read(paths.config_toml()).unwrap(), b"# keep\n");
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
    assert_eq!(view.key.source, "env");
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
    assert_eq!(missing.key.source, "unset");
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
    assert_eq!(view.key.source, "file");
    assert_eq!(view.key.masked.as_deref(), Some("…5678"));
    let view = decider::view_with_env(&paths, &running, |n| {
        (n == "OPENROUTER_API_KEY").then(|| "env-secret-1234".into())
    })
    .unwrap();
    assert_eq!(view.key.source, "env");
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
    decider::set(
        &paths,
        &running,
        DeciderPatch {
            key: Some(KeyPatch::Env("SUPERORCHESTRATOR_ROUTER_MODEL".into())),
            ..Default::default()
        },
    )
    .unwrap();
    let secret = "sensitive-secret-1234";
    let view = decider::view_with_env(&paths, &running, |n| {
        (n == "SUPERORCHESTRATOR_ROUTER_MODEL").then(|| secret.into())
    })
    .unwrap();
    assert!(!serde_json::to_string(&view).unwrap().contains(secret));
}
