use discord_context_mcp::config::{self, Settings};

#[test]
fn test_settings_serde_defaults() {
    let s: Settings = serde_json::from_str(r#"{"gateway":true}"#).unwrap();
    assert!(!s.autostart);
    assert!(s.gateway);
    assert!(!s.allow_writes);

    let s2: Settings = serde_json::from_str(r#"{"autostart":true}"#).unwrap();
    assert!(s2.autostart);
    assert!(!s2.gateway);
    assert!(!s2.allow_writes);

    let s3: Settings = serde_json::from_str(r#"{"allow_writes":true}"#).unwrap();
    assert!(!s3.autostart);
    assert!(!s3.gateway);
    assert!(s3.allow_writes);
}

#[test]
fn test_config_loaders_with_partial_settings() {
    let temp_dir1 = tempfile::tempdir().unwrap();
    let dir1 = temp_dir1.path();
    std::fs::write(dir1.join("settings.json"), r#"{"gateway":true}"#).unwrap();
    assert!(config::load_gateway(dir1));
    assert!(!config::load_allow_writes(dir1));

    let temp_dir2 = tempfile::tempdir().unwrap();
    let dir2 = temp_dir2.path();
    std::fs::write(dir2.join("settings.json"), r#"{"autostart":true}"#).unwrap();
    assert!(config::load_autostart(dir2));
    assert!(!config::load_allow_writes(dir2));
}

#[test]
fn test_save_gateway_and_autostart_persist_together() {
    let temp_dir = tempfile::tempdir().unwrap();
    let dir = temp_dir.path();

    // Saving each field separately must preserve the other field.
    config::save_gateway(dir, true).unwrap();
    config::save_autostart(dir, true).unwrap();
    config::save_allow_writes(dir, true).unwrap();

    assert!(config::load_gateway(dir));
    assert!(config::load_autostart(dir));
    assert!(config::load_allow_writes(dir));

    let content = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    let persisted: Settings = serde_json::from_str(&content).unwrap();
    assert!(persisted.gateway);
    assert!(persisted.autostart);
    assert!(persisted.allow_writes);

    // Toggling one field must not clobber the other.
    config::save_gateway(dir, false).unwrap();
    assert!(!config::load_gateway(dir));
    assert!(config::load_autostart(dir));
    assert!(config::load_allow_writes(dir));

    config::save_allow_writes(dir, false).unwrap();
    assert!(!config::load_gateway(dir));
    assert!(config::load_autostart(dir));
    assert!(!config::load_allow_writes(dir));

    // The atomic write must not leave any temporary files behind.
    let leftover_tmp: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp"))
        .collect();
    assert!(
        leftover_tmp.is_empty(),
        "unexpected temporary files left behind: {:?}",
        leftover_tmp
    );
}

#[test]
fn test_update_settings_returns_err_on_corrupt_json() {
    let temp_dir = tempfile::tempdir().unwrap();
    let dir = temp_dir.path();
    std::fs::write(dir.join("settings.json"), "invalid json {[[[").unwrap();
    let res = config::save_gateway(dir, true);
    assert!(res.is_err(), "expected error on corrupted settings.json");
}
