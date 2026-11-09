//! Matching/formatting logic for the new, unencrypted MQTT protocol that the browser plugin
//! ("MQTT Notifier") implements. Messages are plain JSON `{"type": "...", "message": "..."}`
//! (or arbitrary text/JSON from other senders), matched against user-configured [`crate::config::Rule`]s.

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::Rule;

/// A publish request for the new protocol, sent from the dialog to the daemon over IPC.
/// `location`, `duration_minutes` and `url` are optional extensions beyond what the browser
/// plugin sends, mirroring the legacy event's location/time/eventurl fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifierPublish {
    pub topic: String,
    #[serde(default)]
    pub msg_type: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub duration_minutes: Option<u32>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParsedMessage {
    pub structured: bool,
    pub msg_type: String,
    pub message: String,
    pub raw: String,
    pub location: Option<String>,
    pub duration_minutes: Option<u32>,
    pub url: Option<String>,
}

/// Messages in our format are JSON: `{"type": "...", "message": "..."}`, optionally with
/// `location`, `duration` (minutes) and `url` - our own extension, absent from messages sent
/// by the browser plugin itself. Anything else (plain text, other JSON) is treated as a raw
/// message.
pub fn parse_payload(raw: &str) -> ParsedMessage {
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) {
        if map.contains_key("type") || map.contains_key("message") {
            let msg_type = map.get("type").and_then(value_to_string).unwrap_or_default();
            let message = map.get("message").and_then(value_to_string).unwrap_or_default();
            let location = map.get("location").and_then(value_to_string).filter(|s| !s.is_empty());
            let duration_minutes = map.get("duration").and_then(|v| v.as_u64()).map(|v| v as u32);
            let url = map.get("url").and_then(value_to_string).filter(|s| !s.is_empty());
            return ParsedMessage {
                structured: true,
                msg_type,
                message,
                raw: raw.to_string(),
                location,
                duration_minutes,
                url,
            };
        }
    }
    ParsedMessage {
        structured: false,
        msg_type: String::new(),
        message: raw.to_string(),
        raw: raw.to_string(),
        location: None,
        duration_minutes: None,
        url: None,
    }
}

fn value_to_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// `filter` may use the MQTT wildcards `+` (single level) and `#` (any remaining levels).
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    let f: Vec<&str> = filter.split('/').collect();
    let t: Vec<&str> = topic.split('/').collect();
    for (i, level) in f.iter().enumerate() {
        if *level == "#" {
            return true;
        }
        if i >= t.len() {
            return false;
        }
        if *level != "+" && *level != t[i] {
            return false;
        }
    }
    f.len() == t.len()
}

/// The type must match completely, so rule "nyan" doesn't fire for type "nyan_surf";
/// raw messages are searched, as they have no type to compare against.
pub fn rule_matches(pattern: &str, msg: &ParsedMessage) -> bool {
    if pattern.is_empty() {
        return true;
    }
    if msg.structured {
        full_match(pattern, &msg.msg_type)
    } else {
        payload_matches(pattern, &msg.raw)
    }
}

fn full_match(pattern: &str, text: &str) -> bool {
    // Identical text always matches, so a message sent for a regex rule (type "alarm|fault") triggers that rule.
    if text == pattern {
        return true;
    }
    match Regex::new(&format!("^(?:{pattern})$")) {
        Ok(re) => re.is_match(text),
        Err(_) => text == pattern,
    }
}

fn payload_matches(pattern: &str, text: &str) -> bool {
    match Regex::new(pattern) {
        Ok(re) => re.is_match(text),
        Err(_) => text.contains(pattern),
    }
}

pub fn fill_template(template: &str, topic: &str, msg: &ParsedMessage) -> String {
    let mut result = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < template.len() {
        if bytes[i] == b'{' {
            if let Some(end) = template[i..].find('}') {
                let key = &template[i + 1..i + end];
                let replacement = match key {
                    "topic" => Some(topic),
                    "type" => Some(msg.msg_type.as_str()),
                    "message" => Some(msg.message.as_str()),
                    "payload" => Some(msg.raw.as_str()),
                    _ => None,
                };
                if let Some(value) = replacement {
                    result.push_str(value);
                    i += end + 1;
                    continue;
                }
            }
        }
        let ch = template[i..].chars().next().unwrap();
        result.push(ch);
        i += ch.len_utf8();
    }
    result
}

/// A topic filter can't be published to; wildcards become a concrete topic that still matches
/// the filter: "machine/+/status" -> "machine/notifier/status", "fun/#" -> "fun" (which "#"
/// also matches, since it covers its own parent level).
pub fn send_topic(filter: &str) -> String {
    let mut levels: Vec<&str> = filter.trim().split('/').collect();
    if levels.last() == Some(&"#") && levels.len() > 1 {
        levels.pop();
    }
    levels
        .iter()
        .map(|level| if *level == "+" || *level == "#" { "notifier" } else { level })
        .collect::<Vec<_>>()
        .join("/")
}

/// A title with placeholders ("{type}: {message}") reads badly as a name; fall back to the
/// type, then the topic.
pub fn rule_label(rule: &Rule) -> &str {
    if !rule.title.is_empty() && !rule.title.contains('{') {
        return &rule.title;
    }
    if !rule.match_pattern.is_empty() {
        return &rule.match_pattern;
    }
    &rule.topic
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_wildcards() {
        assert!(topic_matches("machine/+/status", "machine/notifier/status"));
        assert!(!topic_matches("machine/+/status", "machine/a/b/status"));
        assert!(topic_matches("fun/#", "fun"));
        assert!(topic_matches("fun/#", "fun/times/here"));
        assert!(!topic_matches("fun/#", "other"));
        assert!(topic_matches("exact/topic", "exact/topic"));
        assert!(!topic_matches("exact/topic", "exact/topic/extra"));
    }

    #[test]
    fn structured_payload_matches_type_exactly() {
        let msg = parse_payload(r#"{"type":"nyan_surf","message":"hi"}"#);
        assert!(msg.structured);
        assert!(!rule_matches("nyan", &msg));
        assert!(rule_matches("nyan_surf", &msg));
        assert!(rule_matches("nyan_surf|other", &msg));
        assert!(rule_matches("", &msg));
    }

    #[test]
    fn parses_optional_location_duration_and_url() {
        let msg = parse_payload(r#"{"type":"meeting","message":"standup","location":"room 2","duration":15,"url":"https://example.com"}"#);
        assert_eq!(msg.location, Some("room 2".to_string()));
        assert_eq!(msg.duration_minutes, Some(15));
        assert_eq!(msg.url, Some("https://example.com".to_string()));
    }

    #[test]
    fn omitted_optional_fields_are_none() {
        let msg = parse_payload(r#"{"type":"meeting","message":"standup"}"#);
        assert_eq!(msg.location, None);
        assert_eq!(msg.duration_minutes, None);
        assert_eq!(msg.url, None);
    }

    #[test]
    fn raw_payload_is_searched_not_matched_exactly() {
        let msg = parse_payload("hello from a raw sender");
        assert!(!msg.structured);
        assert!(rule_matches("raw", &msg));
        assert!(!rule_matches("nope", &msg));
    }

    #[test]
    fn fill_template_replaces_known_placeholders_only() {
        let msg = parse_payload(r#"{"type":"alarm","message":"fire"}"#);
        let out = fill_template("{type}: {message} on {topic} ({unknown})", "machine/1", &msg);
        assert_eq!(out, "alarm: fire on machine/1 ({unknown})");
    }

    #[test]
    fn send_topic_resolves_wildcards() {
        assert_eq!(send_topic("machine/+/status"), "machine/notifier/status");
        assert_eq!(send_topic("fun/#"), "fun");
        assert_eq!(send_topic("plain/topic"), "plain/topic");
    }
}
