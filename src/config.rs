use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A single alert rule: fires for messages on `topic` (MQTT `+`/`#` wildcards allowed)
/// whose type matches `match_pattern` (a regex, or an exact match; empty = everything).
/// Mirrors the "rules" concept from the browser plugin's settings page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    #[serde(default = "Rule::new_id")]
    pub id: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default, rename = "match")]
    pub match_pattern: String,
    #[serde(default)]
    pub title: String,
    /// A `data:` URL (inline, base64-encoded image, as produced by the browser plugin's file
    /// upload) or an `http(s)://` URL. Only inline `data:` images are actually decoded and
    /// shown; remote URLs just round-trip so an exported plugin config stays intact.
    #[serde(default)]
    pub gif: String,
}

impl Rule {
    pub fn new() -> Self {
        Self {
            id: Self::new_id(),
            topic: String::new(),
            match_pattern: String::new(),
            title: String::new(),
            gif: String::new(),
        }
    }

    fn new_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

/// Settings for the original, hardcoded DCN protocol: AES-256-CBC-encrypted JSON events on a
/// fixed broker/topic. The broker/topic were previously compile-time constants and still
/// default to those same historical values. The encryption key has no default at all - it was
/// previously a hardcoded constant baked into every build (and the public browser plugin), so
/// defaulting to it wouldn't actually be private; it must be set explicitly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "LegacyConfig::default_broker_url")]
    pub broker_url: String,
    #[serde(default = "LegacyConfig::default_topic")]
    pub topic: String,
    #[serde(default)]
    pub encryption_key: String,
}

impl LegacyConfig {
    fn default_broker_url() -> String {
        "wss://mqttserver.nl/".to_string()
    }

    fn default_topic() -> String {
        "/DCN.Client/events".to_string()
    }
}

impl Default for LegacyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            broker_url: Self::default_broker_url(),
            topic: Self::default_topic(),
            encryption_key: String::new(),
        }
    }
}

/// Settings for the new (plain JSON, unencrypted) MQTT protocol - same shape as the
/// browser plugin's config, minus the browser-only popup/GIF options.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifierConfig {
    #[serde(default)]
    pub broker_url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default = "default_true")]
    pub ignore_retained: bool,
    #[serde(default = "default_true")]
    pub show_notification: bool,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub legacy: LegacyConfig,
}

fn default_true() -> bool {
    true
}

impl Default for NotifierConfig {
    fn default() -> Self {
        Self {
            broker_url: String::new(),
            username: String::new(),
            password: String::new(),
            client_id: String::new(),
            ignore_retained: true,
            show_notification: true,
            rules: Vec::new(),
            legacy: LegacyConfig::default(),
        }
    }
}

impl NotifierConfig {
    /// Compares every field except `legacy`, so the new-protocol connection can tell whether
    /// a settings change actually concerns it (and skip reconnecting otherwise).
    pub fn new_protocol_eq(&self, other: &Self) -> bool {
        self.broker_url == other.broker_url
            && self.username == other.username
            && self.password == other.password
            && self.client_id == other.client_id
            && self.ignore_retained == other.ignore_retained
            && self.show_notification == other.show_notification
            && self.rules == other.rules
    }
}

fn config_path() -> PathBuf {
    home_dir().join(".mqtt-notifier-config.json")
}

fn home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.is_empty() {
            return PathBuf::from(profile);
        }
    }
    PathBuf::from(".")
}

impl NotifierConfig {
    /// Loads the currently applied configuration from `~/.mqtt-notifier-config.json`. This is
    /// the one file the daemon itself reads and writes; separate, user-chosen settings files
    /// (`mqtt-notifier-settings-*.json`, see [`Self::merge_import`]/[`Self::export_json`]) are
    /// only ever loaded into it explicitly, never automatically.
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::merge_import(&Self::default(), &text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(config_path(), json)?;
        Ok(())
    }

    /// Parses a settings file - either the browser plugin's export wrapper
    /// (`{app, version, exportedAt, config}`) or a bare config object - and overlays it onto
    /// `current`. Only keys actually present in the file are overwritten, so fields it omits
    /// (notably `password`, which the plugin leaves out of exports by default) keep their
    /// current value, matching the plugin's own import semantics.
    pub fn merge_import(current: &NotifierConfig, text: &str) -> anyhow::Result<NotifierConfig> {
        let parsed: serde_json::Value = serde_json::from_str(text)?;
        let root = parsed
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("not an MQTT Notifier settings file"))?;

        let imported = if root.get("app").and_then(|v| v.as_str()) == Some("mqtt-notifier") {
            root.get("config")
                .and_then(|v| v.as_object())
                .ok_or_else(|| anyhow::anyhow!("not an MQTT Notifier settings file"))?
        } else {
            root
        };
        if !matches!(imported.get("rules"), Some(serde_json::Value::Array(_))) {
            anyhow::bail!("not an MQTT Notifier settings file (missing \"rules\")");
        }

        let mut merged = serde_json::to_value(current)?;
        let merged_obj = merged.as_object_mut().expect("NotifierConfig always serializes to an object");
        for (key, value) in imported {
            merged_obj.insert(key.clone(), value.clone());
        }
        Ok(serde_json::from_value(merged)?)
    }

    /// Serializes this config into the browser plugin's export file shape
    /// (`{app, version, exportedAt, config}`), optionally omitting the password - mirroring
    /// the plugin's "Include password in export" checkbox.
    pub fn export_json(&self, include_password: bool) -> anyhow::Result<String> {
        let mut config = serde_json::to_value(self)?;
        if !include_password {
            if let Some(obj) = config.as_object_mut() {
                obj.remove("password");
            }
        }
        let wrapper = serde_json::json!({
            "app": "mqtt-notifier",
            "version": 1,
            "exportedAt": chrono::Utc::now().to_rfc3339(),
            "config": config,
        });
        Ok(serde_json::to_string_pretty(&wrapper)?)
    }

    /// The browser plugin's default export file name: `mqtt-notifier-settings-YYYY-MM-DD.json`.
    pub fn default_export_file_name() -> String {
        format!("mqtt-notifier-settings-{}.json", chrono::Local::now().format("%Y-%m-%d"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_historical_broker_and_topic_but_key_is_empty() {
        let cfg = NotifierConfig::default();
        assert!(cfg.legacy.enabled);
        assert_eq!(cfg.legacy.broker_url, "wss://mqttserver.nl/");
        assert_eq!(cfg.legacy.topic, "/DCN.Client/events");
        // The key was previously a hardcoded constant baked into every build; it now has no
        // default and must be configured explicitly.
        assert_eq!(cfg.legacy.encryption_key, "");
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let cfg: NotifierConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.legacy.topic, "/DCN.Client/events");
        assert!(cfg.rules.is_empty());
    }

    #[test]
    fn camel_case_wire_format_matches_plugin() {
        let json = r#"{
            "brokerUrl": "wss://broker.example.com/mqtt",
            "username": "u",
            "password": "p",
            "clientId": "c",
            "ignoreRetained": false,
            "showNotification": false,
            "rules": [{"id": "r1", "topic": "a/+/b", "match": "alarm", "title": "{topic}"}]
        }"#;
        let cfg: NotifierConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.broker_url, "wss://broker.example.com/mqtt");
        assert!(!cfg.ignore_retained);
        assert_eq!(cfg.rules[0].match_pattern, "alarm");
        // The legacy section wasn't present, so it still gets its defaults.
        assert!(cfg.legacy.enabled);
    }

    #[test]
    fn merge_import_accepts_plugin_export_wrapper() {
        let current = NotifierConfig::default();
        let json = r#"{
            "app": "mqtt-notifier",
            "version": 1,
            "exportedAt": "2026-01-01T00:00:00.000Z",
            "config": {"brokerUrl": "ws://broker/mqtt", "rules": []}
        }"#;
        let merged = NotifierConfig::merge_import(&current, json).unwrap();
        assert_eq!(merged.broker_url, "ws://broker/mqtt");
    }

    #[test]
    fn merge_import_keeps_current_password_when_absent_from_file() {
        let current = NotifierConfig {
            password: "secret".to_string(),
            username: "old-user".to_string(),
            ..NotifierConfig::default()
        };

        // Exported without a password (the plugin's default), but with a new username.
        let json = r#"{"username": "new-user", "rules": []}"#;
        let merged = NotifierConfig::merge_import(&current, json).unwrap();
        assert_eq!(merged.password, "secret");
        assert_eq!(merged.username, "new-user");
    }

    #[test]
    fn merge_import_rejects_unrelated_json() {
        let current = NotifierConfig::default();
        assert!(NotifierConfig::merge_import(&current, r#"{"foo": "bar"}"#).is_err());
    }

    #[test]
    fn export_then_import_round_trips() {
        let mut cfg = NotifierConfig {
            broker_url: "wss://broker.example.com/mqtt".to_string(),
            password: "secret".to_string(),
            ..NotifierConfig::default()
        };
        cfg.rules.push(Rule {
            id: "r1".to_string(),
            topic: "a/+/b".to_string(),
            match_pattern: "alarm".to_string(),
            title: "{topic}".to_string(),
            gif: "data:image/gif;base64,AAAA".to_string(),
        });

        let exported = cfg.export_json(true).unwrap();
        let imported = NotifierConfig::merge_import(&NotifierConfig::default(), &exported).unwrap();
        assert_eq!(imported.broker_url, cfg.broker_url);
        assert_eq!(imported.password, "secret");
        assert_eq!(imported.rules, cfg.rules);
    }

    #[test]
    fn export_without_password_omits_it_entirely() {
        let mut cfg = NotifierConfig {
            password: "secret".to_string(),
            ..NotifierConfig::default()
        };
        cfg.rules.push(Rule::new());

        let exported = cfg.export_json(false).unwrap();
        assert!(!exported.contains("secret"));

        // Importing it on top of a config that still has the password keeps that password.
        let current = NotifierConfig {
            password: "unchanged".to_string(),
            ..NotifierConfig::default()
        };
        let imported = NotifierConfig::merge_import(&current, &exported).unwrap();
        assert_eq!(imported.password, "unchanged");
    }

    #[test]
    fn new_protocol_eq_ignores_legacy_section() {
        let mut a = NotifierConfig::default();
        let mut b = NotifierConfig::default();
        b.legacy.enabled = false;
        assert!(a.new_protocol_eq(&b));
        a.broker_url = "ws://changed".to_string();
        assert!(!a.new_protocol_eq(&b));
    }
}
