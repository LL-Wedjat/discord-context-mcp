use std::fs;
use std::path::Path;

use discord_context_mcp::mcp_register::{
    atomic_write, register_all_clients, register_antigravity, register_antigravity_at,
    register_claude_code, register_codex, register_opencode, register_t3_antigravity_providers,
    ClientKind, RegistrationStatus,
};

fn find_tmp_files(dir: &Path) -> Vec<String> {
    let mut tmps = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                tmps.extend(find_tmp_files(&path));
            } else if path.file_name().unwrap_or_default().to_string_lossy().contains(".tmp.") {
                tmps.push(path.display().to_string());
            }
        }
    }
    tmps
}

#[test]
fn test_missing_client_dirs_skipped_cleanly() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    let results = register_all_clients(home, 3774);
    assert_eq!(results.len(), 4);

    for res in &results {
        assert_eq!(
            res.status,
            Ok(RegistrationStatus::SkippedNotDetected),
            "Expected {:?} to be skipped because base dir does not exist",
            res.client
        );
    }

    // Verify nothing was created in home
    let entries: Vec<_> = fs::read_dir(home).unwrap().collect();
    assert!(
        entries.is_empty(),
        "Expected no files or dirs created, found: {:?}",
        entries
    );
}

#[test]
fn test_fresh_creation_of_all_4_client_configs() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    // Create base directories
    fs::create_dir_all(home.join(".config").join("opencode")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".gemini").join("config")).unwrap();

    let results = register_all_clients(home, 3774);
    assert_eq!(results.len(), 4);

    for res in &results {
        assert_eq!(
            res.status,
            Ok(RegistrationStatus::Registered),
            "Expected {:?} to be registered",
            res.client
        );
    }

    // Verify OpenCode
    let opencode_path = home.join(".config").join("opencode").join("opencode.json");
    assert!(opencode_path.exists());
    let opencode_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&opencode_path).unwrap()).unwrap();
    assert_eq!(
        opencode_json["mcp"]["servers"]["discord-context"]["type"],
        "remote"
    );
    assert_eq!(
        opencode_json["mcp"]["servers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );

    // Verify Codex
    let codex_path = home.join(".codex").join("config.toml");
    assert!(codex_path.exists());
    let codex_content = fs::read_to_string(&codex_path).unwrap();
    let codex_doc = codex_content.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(
        codex_doc["mcp_servers"]["discord-context"]["url"].as_str(),
        Some("http://127.0.0.1:3774/mcp")
    );

    // Verify Claude Code
    let claude_path = home.join(".claude.json");
    assert!(claude_path.exists());
    let claude_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&claude_path).unwrap()).unwrap();
    assert_eq!(
        claude_json["mcpServers"]["discord-context"]["type"],
        "http"
    );
    assert_eq!(
        claude_json["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );

    // Verify Antigravity
    let ag_path = home.join(".gemini").join("config").join("mcp_config.json");
    assert!(ag_path.exists());
    let ag_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&ag_path).unwrap()).unwrap();
    assert_eq!(
        ag_json["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );

    assert!(find_tmp_files(home).is_empty());
}

#[test]
fn test_claude_detected_from_claude_json_alone() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    // No ~/.claude dir, but ~/.claude.json exists
    fs::write(home.join(".claude.json"), r#"{"theme":"dark"}"#).unwrap();

    let res = register_claude_code(home, 3774);
    assert_eq!(res.status, Ok(RegistrationStatus::Registered));

    let claude_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(claude_json["theme"], "dark");
    assert_eq!(
        claude_json["mcpServers"]["discord-context"]["type"],
        "http"
    );
    assert_eq!(
        claude_json["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );
}

#[test]
fn test_merging_preserves_unrelated_keys_other_servers_and_toml_comments() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    // OpenCode: existing config with theme and another server
    let opencode_dir = home.join(".config").join("opencode");
    fs::create_dir_all(&opencode_dir).unwrap();
    fs::write(
        opencode_dir.join("opencode.json"),
        r#"{
  "theme": "nord",
  "mcp": {
    "servers": {
      "other-tool": {
        "type": "remote",
        "url": "http://127.0.0.1:9090/mcp"
      }
    }
  }
}"#,
    )
    .unwrap();

    // Codex: existing config with top-level keys, comments, and other server
    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    let original_codex_toml = r#"# Primary user settings
model = "claude-3-7-sonnet"
temperature = 0.2

# Custom MCP servers
[mcp_servers.calculator]
# Calculator comment
url = "http://127.0.0.1:4000/mcp"
"#;
    fs::write(codex_dir.join("config.toml"), original_codex_toml).unwrap();

    // Claude Code: existing config with settings and existing mcp server
    fs::write(
        home.join(".claude.json"),
        r#"{
  "autoUpdaterStatus": "disabled",
  "mcpServers": {
    "git": {
      "type": "stdio",
      "command": "mcp-server-git"
    }
  }
}"#,
    )
    .unwrap();

    // Antigravity: existing config with custom setting and existing mcp server
    let ag_dir = home.join(".gemini").join("config");
    fs::create_dir_all(&ag_dir).unwrap();
    fs::write(
        ag_dir.join("mcp_config.json"),
        r#"{
  "mcpServers": {
    "search": {
      "url": "http://127.0.0.1:5005/mcp"
    }
  },
  "enableTelemetry": false
}"#,
    )
    .unwrap();

    let results = register_all_clients(home, 3774);
    for res in &results {
        assert_eq!(res.status, Ok(RegistrationStatus::Registered));
    }

    // Verify OpenCode preserves theme and other-tool
    let opencode_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(opencode_dir.join("opencode.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(opencode_json["theme"], "nord");
    assert_eq!(
        opencode_json["mcp"]["servers"]["other-tool"]["url"],
        "http://127.0.0.1:9090/mcp"
    );
    assert_eq!(
        opencode_json["mcp"]["servers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );

    // Verify Codex preserves comments, model, temperature, calculator
    let codex_content = fs::read_to_string(codex_dir.join("config.toml")).unwrap();
    assert!(
        codex_content.contains("# Primary user settings"),
        "Missing comment in TOML: {}",
        codex_content
    );
    assert!(
        codex_content.contains("# Calculator comment"),
        "Missing calculator comment in TOML: {}",
        codex_content
    );
    let codex_doc = codex_content.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(codex_doc["model"].as_str(), Some("claude-3-7-sonnet"));
    assert_eq!(
        codex_doc["mcp_servers"]["calculator"]["url"].as_str(),
        Some("http://127.0.0.1:4000/mcp")
    );
    assert_eq!(
        codex_doc["mcp_servers"]["discord-context"]["url"].as_str(),
        Some("http://127.0.0.1:3774/mcp")
    );

    // Verify Claude Code preserves autoUpdaterStatus and git
    let claude_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(claude_json["autoUpdaterStatus"], "disabled");
    assert_eq!(
        claude_json["mcpServers"]["git"]["command"],
        "mcp-server-git"
    );
    assert_eq!(
        claude_json["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );

    // Verify Antigravity preserves enableTelemetry and search
    let ag_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(ag_dir.join("mcp_config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(ag_json["enableTelemetry"], false);
    assert_eq!(
        ag_json["mcpServers"]["search"]["url"],
        "http://127.0.0.1:5005/mcp"
    );
    assert_eq!(
        ag_json["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );
}

#[test]
fn test_idempotency_second_run_reports_already_configured_and_does_not_rewrite() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    fs::create_dir_all(home.join(".config").join("opencode")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".gemini").join("config")).unwrap();

    let first_run = register_all_clients(home, 3774);
    for res in &first_run {
        assert_eq!(res.status, Ok(RegistrationStatus::Registered));
    }

    let opencode_before =
        fs::read_to_string(home.join(".config").join("opencode").join("opencode.json")).unwrap();
    let codex_before = fs::read_to_string(home.join(".codex").join("config.toml")).unwrap();
    let claude_before = fs::read_to_string(home.join(".claude.json")).unwrap();
    let ag_before =
        fs::read_to_string(home.join(".gemini").join("config").join("mcp_config.json")).unwrap();

    let second_run = register_all_clients(home, 3774);
    for res in &second_run {
        assert_eq!(
            res.status,
            Ok(RegistrationStatus::AlreadyConfigured),
            "Expected {:?} to be reported as AlreadyConfigured",
            res.client
        );
    }

    assert_eq!(
        opencode_before,
        fs::read_to_string(home.join(".config").join("opencode").join("opencode.json")).unwrap()
    );
    assert_eq!(
        codex_before,
        fs::read_to_string(home.join(".codex").join("config.toml")).unwrap()
    );
    assert_eq!(
        claude_before,
        fs::read_to_string(home.join(".claude.json")).unwrap()
    );
    assert_eq!(
        ag_before,
        fs::read_to_string(home.join(".gemini").join("config").join("mcp_config.json")).unwrap()
    );
}

#[test]
fn test_port_update_preserves_other_fields() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    fs::create_dir_all(home.join(".config").join("opencode")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".gemini").join("config")).unwrap();

    // Initial registration with 3774
    let init_run = register_all_clients(home, 3774);
    for res in &init_run {
        assert_eq!(res.status, Ok(RegistrationStatus::Registered));
    }

    // Add extra user-configured fields to discord-context in each file
    // 1. OpenCode: add timeout and enabled
    let opencode_path = home.join(".config").join("opencode").join("opencode.json");
    let mut opencode_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&opencode_path).unwrap()).unwrap();
    opencode_json["mcp"]["servers"]["discord-context"]["timeout"] = serde_json::json!(60000);
    opencode_json["mcp"]["servers"]["discord-context"]["enabled"] = serde_json::json!(true);
    fs::write(
        &opencode_path,
        serde_json::to_string_pretty(&opencode_json).unwrap(),
    )
    .unwrap();

    // 2. Codex: add timeout in [mcp_servers.discord-context]
    let codex_path = home.join(".codex").join("config.toml");
    let mut codex_doc = fs::read_to_string(&codex_path)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    codex_doc["mcp_servers"]["discord-context"]["timeout"] = toml_edit::value(45);
    fs::write(&codex_path, codex_doc.to_string()).unwrap();

    // 3. Claude Code: add autoApprove in discord-context
    let claude_path = home.join(".claude.json");
    let mut claude_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&claude_path).unwrap()).unwrap();
    claude_json["mcpServers"]["discord-context"]["autoApprove"] =
        serde_json::json!(["read_dm_with"]);
    fs::write(
        &claude_path,
        serde_json::to_string_pretty(&claude_json).unwrap(),
    )
    .unwrap();

    // 4. Antigravity: add trust in discord-context
    let ag_path = home.join(".gemini").join("config").join("mcp_config.json");
    let mut ag_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&ag_path).unwrap()).unwrap();
    ag_json["mcpServers"]["discord-context"]["trust"] = serde_json::json!(true);
    fs::write(&ag_path, serde_json::to_string_pretty(&ag_json).unwrap()).unwrap();

    // Now update port to 8888
    let updated_run = register_all_clients(home, 8888);
    for res in &updated_run {
        assert_eq!(
            res.status,
            Ok(RegistrationStatus::Updated),
            "Expected {:?} to be reported as Updated",
            res.client
        );
    }

    // Verify OpenCode: URL updated, timeout and enabled preserved
    let opencode_after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&opencode_path).unwrap()).unwrap();
    assert_eq!(
        opencode_after["mcp"]["servers"]["discord-context"]["url"],
        "http://127.0.0.1:8888/mcp"
    );
    assert_eq!(
        opencode_after["mcp"]["servers"]["discord-context"]["type"],
        "remote"
    );
    assert_eq!(
        opencode_after["mcp"]["servers"]["discord-context"]["timeout"],
        60000
    );
    assert_eq!(
        opencode_after["mcp"]["servers"]["discord-context"]["enabled"],
        true
    );

    // Verify Codex: URL updated, timeout preserved
    let codex_after = fs::read_to_string(&codex_path)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert_eq!(
        codex_after["mcp_servers"]["discord-context"]["url"].as_str(),
        Some("http://127.0.0.1:8888/mcp")
    );
    assert_eq!(
        codex_after["mcp_servers"]["discord-context"]["timeout"].as_integer(),
        Some(45)
    );

    // Verify Claude Code: URL updated, autoApprove and type preserved
    let claude_after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&claude_path).unwrap()).unwrap();
    assert_eq!(
        claude_after["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:8888/mcp"
    );
    assert_eq!(
        claude_after["mcpServers"]["discord-context"]["type"],
        "http"
    );
    assert_eq!(
        claude_after["mcpServers"]["discord-context"]["autoApprove"],
        serde_json::json!(["read_dm_with"])
    );

    // Verify Antigravity: URL updated, trust preserved
    let ag_after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&ag_path).unwrap()).unwrap();
    assert_eq!(
        ag_after["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:8888/mcp"
    );
    assert_eq!(
        ag_after["mcpServers"]["discord-context"]["trust"],
        true
    );
}

#[test]
fn test_codex_stdio_replaced_with_url_removes_command_and_args() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    let codex_path = codex_dir.join("config.toml");

    let stdio_toml = r#"[mcp_servers.discord-context]
command = "mcp-discord"
args = ["--stdio", "--verbose"]
"#;
    fs::write(&codex_path, stdio_toml).unwrap();

    let res = register_codex(home, 3774);
    assert_eq!(
        res.status,
        Ok(RegistrationStatus::Updated),
        "Expected Codex stdio server update to report Updated"
    );

    let content = fs::read_to_string(&codex_path).unwrap();
    let doc = content.parse::<toml_edit::DocumentMut>().unwrap();
    let dc = &doc["mcp_servers"]["discord-context"];

    assert_eq!(
        dc["url"].as_str(),
        Some("http://127.0.0.1:3774/mcp"),
        "Expected url to be set"
    );
    assert!(
        dc.get("command").is_none(),
        "Expected command to be removed from Codex config"
    );
    assert!(
        dc.get("args").is_none(),
        "Expected args to be removed from Codex config"
    );
}

#[test]
fn test_codex_inline_table_stdio_replaced_with_url_removes_command_and_args() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    let codex_path = codex_dir.join("config.toml");

    let inline_toml = r#"[mcp_servers]
discord-context = { command = "mcp-discord", args = ["--stdio"] }
"#;
    fs::write(&codex_path, inline_toml).unwrap();

    let res = register_codex(home, 3774);
    assert_eq!(res.status, Ok(RegistrationStatus::Updated));

    let content = fs::read_to_string(&codex_path).unwrap();
    let doc = content.parse::<toml_edit::DocumentMut>().unwrap();
    let dc = &doc["mcp_servers"]["discord-context"];

    assert_eq!(
        dc["url"].as_str(),
        Some("http://127.0.0.1:3774/mcp")
    );
    assert!(dc.get("command").is_none());
    assert!(dc.get("args").is_none());
}

#[test]
fn test_corrupt_configs_fail_fast_without_clobbering_and_valid_clients_succeed() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    let opencode_dir = home.join(".config").join("opencode");
    fs::create_dir_all(&opencode_dir).unwrap();
    let corrupt_json = "{ invalid json content [[[[";
    fs::write(opencode_dir.join("opencode.json"), corrupt_json).unwrap();

    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    let corrupt_toml = "mcp_servers = [unclosed array";
    fs::write(codex_dir.join("config.toml"), corrupt_toml).unwrap();

    // Valid base dirs for Claude Code and Antigravity
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".gemini").join("config")).unwrap();

    let results = register_all_clients(home, 3774);
    assert_eq!(results.len(), 4);

    let opencode_res = results
        .iter()
        .find(|r| r.client == ClientKind::OpenCode)
        .unwrap();
    assert!(
        opencode_res.status.is_err(),
        "Expected error for corrupt OpenCode config"
    );
    assert_eq!(
        fs::read_to_string(opencode_dir.join("opencode.json")).unwrap(),
        corrupt_json,
        "Corrupt OpenCode file must not be overwritten or clobbered"
    );

    let codex_res = results
        .iter()
        .find(|r| r.client == ClientKind::Codex)
        .unwrap();
    assert!(
        codex_res.status.is_err(),
        "Expected error for corrupt Codex config"
    );
    assert_eq!(
        fs::read_to_string(codex_dir.join("config.toml")).unwrap(),
        corrupt_toml,
        "Corrupt Codex file must not be overwritten or clobbered"
    );

    let claude_res = results
        .iter()
        .find(|r| r.client == ClientKind::ClaudeCode)
        .unwrap();
    assert_eq!(claude_res.status, Ok(RegistrationStatus::Registered));

    let ag_res = results
        .iter()
        .find(|r| r.client == ClientKind::Antigravity)
        .unwrap();
    assert_eq!(ag_res.status, Ok(RegistrationStatus::Registered));
}

#[test]
fn test_unexpected_non_object_structure_fails_fast_without_clobbering() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    // OpenCode with root as JSON array
    let opencode_dir = home.join(".config").join("opencode");
    fs::create_dir_all(&opencode_dir).unwrap();
    let non_obj_json = "[1, 2, 3]";
    fs::write(opencode_dir.join("opencode.json"), non_obj_json).unwrap();

    let res = register_opencode(home, 3774);
    assert!(res.status.is_err());
    assert_eq!(
        fs::read_to_string(opencode_dir.join("opencode.json")).unwrap(),
        non_obj_json
    );

    // OpenCode with mcp as a string
    fs::write(
        opencode_dir.join("opencode.json"),
        r#"{"mcp": "not an object"}"#,
    )
    .unwrap();
    let res = register_opencode(home, 3774);
    assert!(res.status.is_err());

    // Codex with mcp_servers as string
    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    let non_table_toml = r#"mcp_servers = "not a table""#;
    fs::write(codex_dir.join("config.toml"), non_table_toml).unwrap();

    let res = register_codex(home, 3774);
    assert!(res.status.is_err());
    assert_eq!(
        fs::read_to_string(codex_dir.join("config.toml")).unwrap(),
        non_table_toml
    );

    // Claude with mcpServers as boolean
    fs::write(home.join(".claude.json"), r#"{"mcpServers": true}"#).unwrap();
    let res = register_claude_code(home, 3774);
    assert!(res.status.is_err());

    // Antigravity with discord-context as number
    let ag_dir = home.join(".gemini").join("config");
    fs::create_dir_all(&ag_dir).unwrap();
    fs::write(
        ag_dir.join("mcp_config.json"),
        r#"{"mcpServers": {"discord-context": 42}}"#,
    )
    .unwrap();
    let res = register_antigravity(home, 3774);
    assert!(res.status.is_err());
}

#[test]
fn test_antigravity_gemini_without_config_dir_is_skipped() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    // Create ~/.gemini but NOT ~/.gemini/config
    fs::create_dir_all(home.join(".gemini")).unwrap();

    let res = register_antigravity(home, 3774);
    assert_eq!(
        res.status,
        Ok(RegistrationStatus::SkippedNotDetected),
        "Expected Antigravity to be skipped when ~/.gemini exists but ~/.gemini/config does not"
    );

    let config_file = home.join(".gemini").join("config").join("mcp_config.json");
    assert!(
        !config_file.exists(),
        "Expected mcp_config.json not to be created"
    );
    assert!(
        !home.join(".gemini").join("config").exists(),
        "Expected config directory not to be created"
    );
}

#[test]
fn test_utf8_bom_stripped_and_registration_succeeds() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    // 1. JSON client: OpenCode with BOM
    let opencode_dir = home.join(".config").join("opencode");
    fs::create_dir_all(&opencode_dir).unwrap();
    let opencode_path = opencode_dir.join("opencode.json");
    let opencode_bom = format!("\u{feff}{{\"theme\": \"dark\"}}");
    fs::write(&opencode_path, opencode_bom.as_bytes()).unwrap();

    let res_opencode = register_opencode(home, 3774);
    assert_eq!(
        res_opencode.status,
        Ok(RegistrationStatus::Registered),
        "Expected OpenCode registration with BOM to succeed as Registered"
    );
    let opencode_content = fs::read_to_string(&opencode_path).unwrap();
    assert!(
        !opencode_content.starts_with('\u{feff}'),
        "Written OpenCode file must not have a BOM"
    );
    let opencode_val: serde_json::Value = serde_json::from_str(&opencode_content)
        .expect("Resulting OpenCode file must be valid JSON");
    assert_eq!(opencode_val["theme"], "dark");
    assert_eq!(
        opencode_val["mcp"]["servers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );

    // 2. TOML client: Codex with BOM
    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    let codex_path = codex_dir.join("config.toml");
    let codex_bom = format!("\u{feff}model = \"gpt-4o\"\n");
    fs::write(&codex_path, codex_bom.as_bytes()).unwrap();

    let res_codex = register_codex(home, 3774);
    assert_eq!(
        res_codex.status,
        Ok(RegistrationStatus::Registered),
        "Expected Codex registration with BOM to succeed as Registered"
    );
    let codex_content = fs::read_to_string(&codex_path).unwrap();
    assert!(
        !codex_content.starts_with('\u{feff}'),
        "Written Codex file must not have a BOM"
    );
    let codex_doc = codex_content
        .parse::<toml_edit::DocumentMut>()
        .expect("Resulting Codex file must be valid TOML");
    assert_eq!(codex_doc["model"].as_str(), Some("gpt-4o"));
    assert_eq!(
        codex_doc["mcp_servers"]["discord-context"]["url"].as_str(),
        Some("http://127.0.0.1:3774/mcp")
    );

    // 3. JSON client: Claude Code with BOM
    let claude_path = home.join(".claude.json");
    let claude_bom = format!("\u{feff}{{\"autoUpdaterStatus\": \"enabled\"}}");
    fs::write(&claude_path, claude_bom.as_bytes()).unwrap();

    let res_claude = register_claude_code(home, 3774);
    assert_eq!(res_claude.status, Ok(RegistrationStatus::Registered));
    let claude_content = fs::read_to_string(&claude_path).unwrap();
    assert!(!claude_content.starts_with('\u{feff}'));
    let claude_val: serde_json::Value = serde_json::from_str(&claude_content)
        .expect("Resulting Claude Code file must be valid JSON");
    assert_eq!(claude_val["autoUpdaterStatus"], "enabled");
    assert_eq!(
        claude_val["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );

    // 4. JSON client: Antigravity with BOM
    let ag_dir = home.join(".gemini").join("config");
    fs::create_dir_all(&ag_dir).unwrap();
    let ag_path = ag_dir.join("mcp_config.json");
    let ag_bom = format!("\u{feff}{{\"enableTelemetry\": true}}");
    fs::write(&ag_path, ag_bom.as_bytes()).unwrap();

    let res_ag = register_antigravity(home, 3774);
    assert_eq!(res_ag.status, Ok(RegistrationStatus::Registered));
    let ag_content = fs::read_to_string(&ag_path).unwrap();
    assert!(!ag_content.starts_with('\u{feff}'));
    let ag_val: serde_json::Value = serde_json::from_str(&ag_content)
        .expect("Resulting Antigravity file must be valid JSON");
    assert_eq!(ag_val["enableTelemetry"], true);
    assert_eq!(
        ag_val["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );
}

#[test]
fn test_antigravity_existing_server_url_same_port_already_configured_and_unchanged() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    let ag_dir = home.join(".gemini").join("config");
    fs::create_dir_all(&ag_dir).unwrap();
    let config_path = ag_dir.join("mcp_config.json");
    let original = r#"{
  "mcpServers": {
    "discord-context": {
      "serverUrl": "http://127.0.0.1:3774/mcp"
    }
  }
}"#;
    fs::write(&config_path, original).unwrap();

    let res = register_antigravity(home, 3774);
    assert_eq!(
        res.status,
        Ok(RegistrationStatus::AlreadyConfigured),
        "Expected AlreadyConfigured for matching serverUrl"
    );

    let content_after = fs::read_to_string(&config_path).unwrap();
    assert_eq!(
        content_after, original,
        "File must remain completely unchanged when AlreadyConfigured"
    );
}

#[test]
fn test_antigravity_existing_server_url_different_port_updates_server_url_and_no_url_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    let ag_dir = home.join(".gemini").join("config");
    fs::create_dir_all(&ag_dir).unwrap();
    let config_path = ag_dir.join("mcp_config.json");
    let original = r#"{
  "mcpServers": {
    "discord-context": {
      "serverUrl": "http://127.0.0.1:3774/mcp"
    }
  }
}"#;
    fs::write(&config_path, original).unwrap();

    let res = register_antigravity(home, 8888);
    assert_eq!(
        res.status,
        Ok(RegistrationStatus::Updated),
        "Expected Updated when port changes"
    );

    let content_after = fs::read_to_string(&config_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content_after).unwrap();
    let dc = &json["mcpServers"]["discord-context"];

    assert_eq!(
        dc["serverUrl"],
        "http://127.0.0.1:8888/mcp",
        "serverUrl must point to the new URL"
    );
    assert!(
        dc.get("url").is_none(),
        "Must not contain a duplicate or stale 'url' key"
    );
}

#[test]
fn test_antigravity_both_url_and_server_url_normalized_to_single_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    let ag_dir = home.join(".gemini").join("config");
    fs::create_dir_all(&ag_dir).unwrap();
    let config_path = ag_dir.join("mcp_config.json");
    let initial = r#"{
  "mcpServers": {
    "discord-context": {
      "url": "http://127.0.0.1:3774/mcp",
      "serverUrl": "http://127.0.0.1:3774/mcp"
    }
  }
}"#;
    fs::write(&config_path, initial).unwrap();

    let res = register_antigravity(home, 3774);
    assert_eq!(
        res.status,
        Ok(RegistrationStatus::Updated),
        "Expected Updated when normalizing multiple endpoint keys"
    );

    let content_after = fs::read_to_string(&config_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content_after).unwrap();
    let dc = &json["mcpServers"]["discord-context"];

    let has_url = dc.get("url").is_some();
    let has_server_url = dc.get("serverUrl").is_some();
    assert!(
        has_url ^ has_server_url,
        "Expected exactly one endpoint key, found url: {}, serverUrl: {}",
        has_url,
        has_server_url
    );
    assert_eq!(dc["serverUrl"], "http://127.0.0.1:3774/mcp");
    assert!(dc.get("url").is_none());
}

#[test]
fn test_opencode_stdio_replaced_with_url_removes_command_and_args_preserves_unrelated() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    let opencode_dir = home.join(".config").join("opencode");
    fs::create_dir_all(&opencode_dir).unwrap();
    let config_path = opencode_dir.join("opencode.json");
    let stdio_json = r#"{
  "mcp": {
    "servers": {
      "discord-context": {
        "command": "discord-mcp-stdio",
        "args": ["--stdio", "--verbose"],
        "timeout": 30000
      }
    }
  }
}"#;
    fs::write(&config_path, stdio_json).unwrap();

    let res = register_opencode(home, 3774);
    assert_eq!(
        res.status,
        Ok(RegistrationStatus::Updated),
        "Expected Updated when replacing stdio"
    );

    let content = fs::read_to_string(&config_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    let dc = &json["mcp"]["servers"]["discord-context"];

    assert_eq!(dc["type"], "remote");
    assert_eq!(dc["url"], "http://127.0.0.1:3774/mcp");
    assert!(dc.get("command").is_none(), "command must be removed");
    assert!(dc.get("args").is_none(), "args must be removed");
    assert_eq!(dc["timeout"], 30000, "unrelated key must be preserved");
}

#[test]
fn test_claude_code_stdio_replaced_with_url_removes_command_and_args_preserves_unrelated() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    let config_path = home.join(".claude.json");
    let stdio_json = r#"{
  "mcpServers": {
    "discord-context": {
      "type": "stdio",
      "command": "discord-mcp-stdio",
      "args": ["--stdio"],
      "autoApprove": ["read_dm_with"]
    }
  }
}"#;
    fs::write(&config_path, stdio_json).unwrap();

    let res = register_claude_code(home, 3774);
    assert_eq!(
        res.status,
        Ok(RegistrationStatus::Updated),
        "Expected Updated when replacing stdio"
    );

    let content = fs::read_to_string(&config_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    let dc = &json["mcpServers"]["discord-context"];

    assert_eq!(dc["type"], "http");
    assert_eq!(dc["url"], "http://127.0.0.1:3774/mcp");
    assert!(dc.get("command").is_none(), "command must be removed");
    assert!(dc.get("args").is_none(), "args must be removed");
    assert_eq!(
        dc["autoApprove"],
        serde_json::json!(["read_dm_with"]),
        "unrelated key must be preserved"
    );
}

#[test]
fn test_antigravity_stdio_replaced_with_url_removes_command_and_args_preserves_unrelated() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    let ag_dir = home.join(".gemini").join("config");
    fs::create_dir_all(&ag_dir).unwrap();
    let config_path = ag_dir.join("mcp_config.json");
    let stdio_json = r#"{
  "mcpServers": {
    "discord-context": {
      "command": "discord-mcp-stdio",
      "args": ["--stdio"],
      "trust": true
    }
  }
}"#;
    fs::write(&config_path, stdio_json).unwrap();

    let res = register_antigravity(home, 3774);
    assert_eq!(
        res.status,
        Ok(RegistrationStatus::Updated),
        "Expected Updated when replacing stdio"
    );

    let content = fs::read_to_string(&config_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    let dc = &json["mcpServers"]["discord-context"];

    assert_eq!(dc["url"], "http://127.0.0.1:3774/mcp");
    assert!(dc.get("command").is_none(), "command must be removed");
    assert!(dc.get("args").is_none(), "args must be removed");
    assert_eq!(dc["trust"], true, "unrelated key must be preserved");
}

#[test]
fn test_concurrent_atomic_writes_to_same_dest_succeed_and_leave_no_tmp_files() {
    let temp_dir = tempfile::tempdir().unwrap();
    let dest = temp_dir.path().join("config.json");

    let dest1 = dest.clone();
    let dest2 = dest.clone();

    let t1 = std::thread::spawn(move || {
        atomic_write(&dest1, "content-from-thread-1").unwrap();
    });
    let t2 = std::thread::spawn(move || {
        atomic_write(&dest2, "content-from-thread-2").unwrap();
    });

    t1.join().unwrap();
    t2.join().unwrap();

    assert!(dest.exists(), "Target file must exist");
    let final_content = fs::read_to_string(&dest).unwrap();
    assert!(
        final_content == "content-from-thread-1" || final_content == "content-from-thread-2",
        "Final content must be from one of the threads, got: {}",
        final_content
    );

    let tmps = find_tmp_files(temp_dir.path());
    assert!(
        tmps.is_empty(),
        "Expected no leftover .tmp files, found: {:?}",
        tmps
    );
}

#[test]
fn test_stray_regular_files_at_client_dirs_skipped() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    // OpenCode: ~/.config is dir, ~/.config/opencode is a regular file
    fs::create_dir_all(home.join(".config")).unwrap();
    fs::write(home.join(".config").join("opencode"), "not a dir").unwrap();

    // Codex: ~/.codex is a regular file
    fs::write(home.join(".codex"), "not a dir").unwrap();

    // Claude Code: ~/.claude is a regular file, and no ~/.claude.json
    fs::write(home.join(".claude"), "not a dir").unwrap();

    // Antigravity: ~/.gemini is dir, ~/.gemini/config is a regular file, and no mcp_config.json
    fs::create_dir_all(home.join(".gemini")).unwrap();
    fs::write(home.join(".gemini").join("config"), "not a dir").unwrap();

    let results = register_all_clients(home, 3774);
    assert_eq!(results.len(), 4);
    for res in &results {
        assert_eq!(
            res.status,
            Ok(RegistrationStatus::SkippedNotDetected),
            "Expected {:?} to be skipped when base/config path is a regular file",
            res.client
        );
    }
}

#[test]
fn test_register_antigravity_at_empty_and_whitespace_mcp_config_json() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();

    // 1. 0-byte file
    let config_path = config_dir.join("mcp_config.json");
    fs::write(&config_path, b"").unwrap();
    assert_eq!(fs::metadata(&config_path).unwrap().len(), 0);

    let res = register_antigravity_at(&config_dir, 3774);
    assert_eq!(res.client, ClientKind::Antigravity);
    assert_eq!(res.status, Ok(RegistrationStatus::Registered));

    let content = fs::read_to_string(&config_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        json["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );

    // 2. Whitespace-only file (spaces, tabs, newlines)
    fs::write(&config_path, "   \r\n \t  \n").unwrap();
    let res_ws = register_antigravity_at(&config_dir, 3774);
    assert_eq!(res_ws.status, Ok(RegistrationStatus::Registered));

    let content_ws = fs::read_to_string(&config_path).unwrap();
    let json_ws: serde_json::Value = serde_json::from_str(&content_ws).unwrap();
    assert_eq!(
        json_ws["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );
}

#[test]
fn test_register_all_clients_with_multiple_t3_antigravity_providers() {
    let temp_dir = tempfile::tempdir().unwrap();
    let home = temp_dir.path();

    // Setup standard clients
    fs::create_dir_all(home.join(".config").join("opencode")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".gemini").join("config")).unwrap();

    // Setup T3 Antigravity provider directories:
    // - provider-1: 0-byte mcp_config.json
    let p1_config = home
        .join(".t3")
        .join("userdata")
        .join("providers")
        .join("antigravity")
        .join("provider-1")
        .join("config");
    fs::create_dir_all(&p1_config).unwrap();
    fs::write(p1_config.join("mcp_config.json"), b"").unwrap();

    // - provider-2: config dir exists, but mcp_config.json is missing
    let p2_config = home
        .join(".t3")
        .join("userdata")
        .join("providers")
        .join("antigravity")
        .join("provider-2")
        .join("config");
    fs::create_dir_all(&p2_config).unwrap();

    // - provider-3: existing valid mcp_config.json with other servers
    let p3_config = home
        .join(".t3")
        .join("userdata")
        .join("providers")
        .join("antigravity")
        .join("provider-3")
        .join("config");
    fs::create_dir_all(&p3_config).unwrap();
    fs::write(
        p3_config.join("mcp_config.json"),
        r#"{"mcpServers": {"existing": {"url": "http://127.0.0.1:1234/mcp"}}}"#,
    )
    .unwrap();

    // - provider-4: no config dir and no mcp_config.json (should be ignored)
    let p4_dir = home
        .join(".t3")
        .join("userdata")
        .join("providers")
        .join("antigravity")
        .join("provider-4");
    fs::create_dir_all(p4_dir).unwrap();

    // - stray file inside providers/antigravity (not a dir, should be ignored)
    fs::write(
        home.join(".t3")
            .join("userdata")
            .join("providers")
            .join("antigravity")
            .join("stray_file.txt"),
        "hello",
    )
    .unwrap();

    // First run
    let results = register_all_clients(home, 3774);
    // 4 standard clients (OpenCode, Codex, Claude Code, Antigravity) + 3 T3 Antigravity providers = 7
    assert_eq!(results.len(), 7);

    for res in &results {
        assert_eq!(
            res.status,
            Ok(RegistrationStatus::Registered),
            "Expected {:?} at {:?} to be Registered",
            res.client,
            res.config_path
        );
    }

    // Verify provider-1 has the server configured
    let p1_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(p1_config.join("mcp_config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        p1_json["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );

    // Verify provider-2 created mcp_config.json
    let p2_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(p2_config.join("mcp_config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        p2_json["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );

    // Verify provider-3 preserved existing server
    let p3_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(p3_config.join("mcp_config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        p3_json["mcpServers"]["existing"]["url"],
        "http://127.0.0.1:1234/mcp"
    );
    assert_eq!(
        p3_json["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:3774/mcp"
    );

    // Verify deterministic ordering of T3 providers
    let t3_results = register_t3_antigravity_providers(home, 3774);
    assert_eq!(t3_results.len(), 3);
    assert_eq!(
        t3_results[0].config_path,
        Some(p1_config.join("mcp_config.json"))
    );
    assert_eq!(
        t3_results[1].config_path,
        Some(p2_config.join("mcp_config.json"))
    );
    assert_eq!(
        t3_results[2].config_path,
        Some(p3_config.join("mcp_config.json"))
    );

    // Second run: idempotency
    let second_run = register_all_clients(home, 3774);
    assert_eq!(second_run.len(), 7);
    for res in &second_run {
        assert_eq!(
            res.status,
            Ok(RegistrationStatus::AlreadyConfigured),
            "Expected {:?} at {:?} to be AlreadyConfigured",
            res.client,
            res.config_path
        );
    }

    // Third run: port update
    let updated_run = register_all_clients(home, 9999);
    assert_eq!(updated_run.len(), 7);
    for res in &updated_run {
        assert_eq!(
            res.status,
            Ok(RegistrationStatus::Updated),
            "Expected {:?} at {:?} to be Updated",
            res.client,
            res.config_path
        );
    }
    let p1_updated: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(p1_config.join("mcp_config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        p1_updated["mcpServers"]["discord-context"]["url"],
        "http://127.0.0.1:9999/mcp"
    );
}
