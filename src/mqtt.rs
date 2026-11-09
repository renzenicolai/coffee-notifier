use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use rumqttc::{AsyncClient, Event as MqttEvent, MqttOptions, Packet, QoS, Transport};
use tokio::sync::{mpsc, watch};

use crate::config::{LegacyConfig, NotifierConfig};
use crate::crypto::{decrypt_dcn, encrypt_dcn};
use crate::event::Event;
use crate::notifications::{notify_event, notify_mqtt};
use crate::notifier::{self, NotifierPublish};
use crate::state::SharedState;

/// Runs forever, (re)connecting to the legacy broker whenever the connection drops or the
/// legacy settings change. Does nothing (just waits for settings changes) while disabled.
pub async fn run(
    shared: Arc<SharedState>,
    mut outbox: mpsc::UnboundedReceiver<Event>,
    mut config_rx: watch::Receiver<NotifierConfig>,
) {
    loop {
        let cfg = config_rx.borrow().clone().legacy;
        if !cfg.enabled {
            let _ = config_rx.changed().await;
            continue;
        }
        if let Err(e) = connect_and_run(&shared, &cfg, &mut outbox, &mut config_rx).await {
            eprintln!("[mqtt] connection error: {e}");
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                changed = config_rx.changed() => { let _ = changed; }
            }
        }
    }
}

/// Returns `Ok(())` once the settings changed (the caller should reconnect with the new
/// config), or `Err` on a connection error (the caller retries after a delay).
async fn connect_and_run(
    shared: &Arc<SharedState>,
    cfg: &LegacyConfig,
    outbox: &mut mpsc::UnboundedReceiver<Event>,
    config_rx: &mut watch::Receiver<NotifierConfig>,
) -> anyhow::Result<()> {
    let client_id = format!("dcn-notify-{}", uuid::Uuid::new_v4());
    let mut options = MqttOptions::new(client_id, cfg.broker_url.clone(), 443);
    options.set_transport(Transport::wss_with_default_config());
    options.set_keep_alive(Duration::from_secs(30));

    let (client, mut eventloop) = AsyncClient::new(options, 10);
    client.subscribe(&cfg.topic, QoS::AtLeastOnce).await?;

    loop {
        tokio::select! {
            notif = eventloop.poll() => {
                if let MqttEvent::Incoming(Packet::Publish(publish)) = notif? {
                    handle_publish(shared, &cfg.encryption_key, &publish.payload);
                }
            }
            Some(event) = outbox.recv() => {
                if let Ok(json) = serde_json::to_string(&event) {
                    match encrypt_dcn(&cfg.encryption_key, &json) {
                        Ok(payload) => {
                            if let Err(e) = client.publish(&cfg.topic, QoS::AtLeastOnce, false, payload).await {
                                eprintln!("[mqtt] publish error: {e}");
                            }
                        }
                        Err(e) => eprintln!("[mqtt] failed to encrypt outgoing event: {e}"),
                    }
                } else {
                    eprintln!("[mqtt] failed to serialize outgoing event");
                }
            }
            changed = config_rx.changed() => {
                changed?;
                // Only reconnect if the legacy settings actually changed - a new-protocol-only
                // save would otherwise needlessly drop this connection.
                if config_rx.borrow().legacy != *cfg {
                    return Ok(());
                }
            }
        }
    }
}

fn handle_publish(shared: &Arc<SharedState>, encryption_key: &str, payload: &[u8]) {
    let plaintext = match decrypt_dcn(encryption_key, payload) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[mqtt] failed to decrypt incoming message: {e}");
            return;
        }
    };

    let event: Event = match serde_json::from_str(&plaintext) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[mqtt] failed to parse event json: {e}");
            return;
        }
    };

    notify_event(&event);
    shared.push(event);
}

/// Runs forever, (re)connecting to the user-configured broker for the new protocol whenever
/// the connection drops, or whenever the settings change (mirrors the browser plugin's
/// `chrome.storage.onChanged` -> `connect()` behavior).
pub async fn run_notifier(
    shared: Arc<SharedState>,
    mut outbox: mpsc::UnboundedReceiver<NotifierPublish>,
    mut config_rx: watch::Receiver<NotifierConfig>,
) {
    loop {
        let cfg = config_rx.borrow().clone();
        match connect_and_run_notifier(&shared, &cfg, &mut outbox, &mut config_rx).await {
            Ok(()) => {
                // connect_and_run_notifier only returns Ok after the config actually changed.
            }
            Err(e) => {
                eprintln!("[mqtt-new] connection error: {e}");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    changed = config_rx.changed() => { let _ = changed; }
                }
            }
        }
    }
}

fn unique_topics(cfg: &NotifierConfig) -> Vec<String> {
    let mut seen = HashSet::new();
    cfg.rules
        .iter()
        .map(|r| r.topic.trim().to_string())
        .filter(|t| !t.is_empty() && seen.insert(t.clone()))
        .collect()
}

fn build_notifier_client(cfg: &NotifierConfig, topics: &[String]) -> anyhow::Result<(AsyncClient, rumqttc::EventLoop)> {
    let transport = if cfg.broker_url.starts_with("wss://") {
        Transport::wss_with_default_config()
    } else if cfg.broker_url.starts_with("ws://") {
        Transport::ws()
    } else {
        anyhow::bail!("broker URL must start with ws:// or wss://");
    };

    let client_id = if cfg.client_id.trim().is_empty() {
        format!("mqtt-notifier-{}", &uuid::Uuid::new_v4().to_string()[..8])
    } else {
        cfg.client_id.trim().to_string()
    };

    let mut options = MqttOptions::new(client_id, cfg.broker_url.clone(), 443);
    options.set_transport(transport);
    options.set_keep_alive(Duration::from_secs(20));
    if !cfg.username.is_empty() || !cfg.password.is_empty() {
        options.set_credentials(cfg.username.clone(), cfg.password.clone());
    }

    let (client, eventloop) = AsyncClient::new(options, 10);
    let _ = topics; // subscriptions are issued by the caller once connected
    Ok((client, eventloop))
}

/// Returns `Ok(())` once the settings changed (the caller should reconnect with the new
/// config), or `Err` on a connection error (the caller retries after a delay).
async fn connect_and_run_notifier(
    shared: &Arc<SharedState>,
    cfg: &NotifierConfig,
    outbox: &mut mpsc::UnboundedReceiver<NotifierPublish>,
    config_rx: &mut watch::Receiver<NotifierConfig>,
) -> anyhow::Result<()> {
    let topics = unique_topics(cfg);
    if cfg.broker_url.trim().is_empty() || topics.is_empty() {
        // Nothing to connect to yet - wait until the settings say otherwise.
        let _ = config_rx.changed().await;
        return Ok(());
    }

    let (client, mut eventloop) = build_notifier_client(cfg, &topics)?;
    for topic in &topics {
        client.subscribe(topic, QoS::AtMostOnce).await?;
    }

    loop {
        tokio::select! {
            notif = eventloop.poll() => {
                if let MqttEvent::Incoming(Packet::Publish(publish)) = notif? {
                    handle_notifier_publish(shared, cfg, publish.retain, &publish.topic, &publish.payload);
                }
            }
            Some(req) = outbox.recv() => {
                publish_notifier(&client, &req).await;
            }
            changed = config_rx.changed() => {
                changed?;
                // Only reconnect if a new-protocol-relevant setting actually changed - a
                // legacy-only save would otherwise needlessly drop this connection.
                if !config_rx.borrow().new_protocol_eq(cfg) {
                    return Ok(());
                }
            }
        }
    }
}

async fn publish_notifier(client: &AsyncClient, req: &NotifierPublish) {
    if req.topic.is_empty() || req.topic.contains(['+', '#']) {
        eprintln!("[mqtt-new] refusing to publish to invalid topic {:?}", req.topic);
        return;
    }
    let mut payload = serde_json::Map::new();
    if !req.msg_type.is_empty() {
        payload.insert("type".to_string(), serde_json::json!(req.msg_type));
    }
    payload.insert("message".to_string(), serde_json::json!(req.message));
    if let Some(location) = req.location.as_deref().filter(|s| !s.is_empty()) {
        payload.insert("location".to_string(), serde_json::json!(location));
    }
    if let Some(minutes) = req.duration_minutes {
        payload.insert("duration".to_string(), serde_json::json!(minutes));
    }
    if let Some(url) = req.url.as_deref().filter(|s| !s.is_empty()) {
        payload.insert("url".to_string(), serde_json::json!(url));
    }

    let payload = serde_json::Value::Object(payload).to_string();
    if let Err(e) = client.publish(&req.topic, QoS::AtLeastOnce, false, payload).await {
        eprintln!("[mqtt-new] publish error: {e}");
    }
}

fn handle_notifier_publish(shared: &Arc<SharedState>, cfg: &NotifierConfig, retain: bool, topic: &str, payload: &[u8]) {
    // Retained messages arrive on every (re)connect; alerting on them would repeat old alarms.
    if retain && cfg.ignore_retained {
        return;
    }

    let raw = String::from_utf8_lossy(payload).into_owned();
    let msg = notifier::parse_payload(&raw);
    let Some(rule) = cfg
        .rules
        .iter()
        .find(|r| notifier::topic_matches(r.topic.trim(), topic) && notifier::rule_matches(&r.match_pattern, &msg))
    else {
        return;
    };

    let title_template = if rule.title.is_empty() { "MQTT: {topic}" } else { &rule.title };
    let title = notifier::fill_template(title_template, topic, &msg);
    let mut body = if !msg.message.is_empty() {
        msg.message.clone()
    } else if !msg.msg_type.is_empty() {
        msg.msg_type.clone()
    } else {
        topic.to_string()
    };
    if let Some(location) = &msg.location {
        body.push_str(&format!("\nLocation: {location}"));
    }
    if let Some(minutes) = msg.duration_minutes.filter(|m| *m > 0) {
        body.push_str(&format!("\nDuration: {minutes} min"));
    }

    // Always shown in "Ongoing events" - the system notification is separately gated below.
    shared.push_notifier(title.clone(), body.clone(), msg.location.clone(), msg.url.clone(), msg.duration_minutes);

    if cfg.show_notification {
        notify_mqtt(&title, &body, &rule.gif, msg.url.as_deref());
    }
}
