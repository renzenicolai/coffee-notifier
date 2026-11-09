use notify_rust::Notification;

use crate::event::Event;

pub fn notify_event(event: &Event) {
    let summary = format!("DCN: {}", event.event_type.label());

    let mut body = format!(
        "Organizer: {}\nLocation: {}",
        event.event_organizer, event.location
    );
    if !event.description.is_empty() {
        body.push('\n');
        body.push_str(&event.description);
    }
    if event.time > 0 {
        body.push_str(&format!("\nDuration: {} min", event.time));
    }

    let mut notification = Notification::new();
    notification
        .appname("DCN Notify")
        .summary(&summary)
        .body(&body);

    let url = event.eventurl.clone();
    if url.is_some() {
        notification.action("default", "Open");
    }

    match notification.show() {
        Ok(handle) => {
            if let Some(url) = url {
                std::thread::spawn(move || {
                    handle.wait_for_action(|action| {
                        if action == "default" {
                            let _ = open::that(&url);
                        }
                    });
                });
            }
        }
        Err(e) => eprintln!("failed to show notification: {e}"),
    }
}

/// Notification for a message received over the new (plain JSON) MQTT protocol: a title, a
/// body, optionally the matched rule's GIF (shown as a static preview image - desktop
/// notifications can't animate), and optionally a URL (our own extension beyond what the
/// browser plugin sends), openable the same way as a legacy event's.
pub fn notify_mqtt(title: &str, body: &str, gif: &str, url: Option<&str>) {
    let mut notification = Notification::new();
    notification.appname("DCN Notify").summary(title).body(body);

    if let Some(path) = crate::image_util::materialize_preview(gif) {
        notification.image_path(&path.to_string_lossy());
    }

    let url = url.map(|u| u.to_string());
    if url.is_some() {
        notification.action("default", "Open");
    }

    match notification.show() {
        Ok(handle) => {
            if let Some(url) = url {
                std::thread::spawn(move || {
                    handle.wait_for_action(|action| {
                        if action == "default" {
                            let _ = open::that(&url);
                        }
                    });
                });
            }
        }
        Err(e) => eprintln!("failed to show notification: {e}"),
    }
}
