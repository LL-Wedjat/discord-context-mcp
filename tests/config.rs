use discord_context_mcp::config::{self, Settings};

#[test]
fn test_settings_serde_defaults() {
    let s: Settings = serde_json::from_str(r#"{"gateway":true}"#).unwrap();
    assert_eq!(s.autostart, false);
    assert_eq!(s.gateway, true);

    let s2: Settings = serde_json::from_str(r#"{"autostart":true}"#).unwrap();
    assert_eq!(s2.autostart, true);
    assert_eq!(s2.gateway, false);
}

#[test]
fn test_config_loaders_with_partial_settings() {
    let temp_dir1 = tempfile::tempdir().unwrap();
    let dir1 = temp_dir1.path();
    std::fs::write(dir1.join("settings.json"), r#"{"gateway":true}"#).unwrap();
    assert_eq!(config::load_gateway(dir1), true);

    let temp_dir2 = tempfile::tempdir().unwrap();
    let dir2 = temp_dir2.path();
    std::fs::write(dir2.join("settings.json"), r#"{"autostart":true}"#).unwrap();
    assert_eq!(config::load_autostart(dir2), true);
}
