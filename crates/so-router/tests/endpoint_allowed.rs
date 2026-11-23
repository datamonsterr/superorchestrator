use so_router::jev::endpoint_allowed;

#[test]
fn endpoint_policy_is_available_to_config_callers() {
    assert!(endpoint_allowed(
        "https://openrouter.ai/api/alpha/decisions"
    ));
    assert!(endpoint_allowed("http://127.0.0.1:1234/decisions"));
    assert!(!endpoint_allowed("http://example.com/decisions"));
    assert!(!endpoint_allowed("not a url"));
}
