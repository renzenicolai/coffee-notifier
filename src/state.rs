use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::NotifierConfig;
use crate::event::Event;

/// New-protocol messages that don't specify a `duration` are shown in "Ongoing events" for
/// this fixed window instead.
const NOTIFIER_ONGOING_DURATION: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OngoingItem {
    Legacy {
        event: Event,
    },
    Notifier {
        title: String,
        message: String,
        location: Option<String>,
        url: Option<String>,
    },
}

/// A point-in-time, wire-friendly view of an [`OngoingEvent`] for the IPC protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OngoingSnapshot {
    pub id: Uuid,
    pub item: OngoingItem,
    pub remaining_secs: u64,
    pub progress: f32,
    /// Whether the remaining-time progress bar is meaningful. New-protocol messages have no
    /// duration of their own (they're just shown for a fixed window), so it isn't for those.
    pub show_duration: bool,
}

pub struct OngoingEvent {
    pub id: Uuid,
    pub item: OngoingItem,
    pub received_at: Instant,
    pub duration: Duration,
    /// Whether `duration` reflects a real, protocol-supplied duration (legacy events, or
    /// new-protocol messages that set their optional `duration` field) as opposed to just the
    /// fixed display window used when no duration was given.
    pub show_duration: bool,
}

impl OngoingEvent {
    pub fn elapsed(&self) -> Duration {
        self.received_at.elapsed()
    }

    pub fn is_expired(&self) -> bool {
        self.elapsed() >= self.duration
    }

    pub fn progress(&self) -> f32 {
        let total = self.duration.as_secs_f32();
        if total <= 0.0 {
            return 1.0;
        }
        (self.elapsed().as_secs_f32() / total).clamp(0.0, 1.0)
    }

    pub fn remaining(&self) -> Duration {
        self.duration.saturating_sub(self.elapsed())
    }
}

pub struct SharedState {
    pub ongoing: Mutex<Vec<OngoingEvent>>,
    /// The new protocol's settings - kept here (in addition to the watch channel the daemon
    /// uses to tell the MQTT task to reconnect) so the IPC thread can answer `GetConfig`
    /// without needing an async context.
    pub config: Mutex<NotifierConfig>,
}

impl SharedState {
    pub fn new(config: NotifierConfig) -> Self {
        Self {
            ongoing: Mutex::new(Vec::new()),
            config: Mutex::new(config),
        }
    }

    pub fn push(&self, event: Event) {
        // Events without a duration expire immediately, so there's no point tracking them.
        if event.time == 0 {
            return;
        }
        let duration = Duration::from_secs(u64::from(event.time) * 60);
        self.push_item(OngoingItem::Legacy { event }, duration, true);
    }

    /// Tracks a matched new-protocol message as an ongoing event. `duration_minutes` is the
    /// message's optional `duration` field: `None` (not specified) shows it for a fixed window
    /// without a remaining-time readout; `Some(0)` matches the legacy "0 = expires
    /// immediately" convention and isn't tracked at all; `Some(n)` for `n > 0` behaves like a
    /// real duration.
    pub fn push_notifier(&self, title: String, message: String, location: Option<String>, url: Option<String>, duration_minutes: Option<u32>) {
        let item = OngoingItem::Notifier { title, message, location, url };
        match duration_minutes {
            Some(0) => {}
            Some(mins) => self.push_item(item, Duration::from_secs(u64::from(mins) * 60), true),
            None => self.push_item(item, NOTIFIER_ONGOING_DURATION, false),
        }
    }

    fn push_item(&self, item: OngoingItem, duration: Duration, show_duration: bool) {
        let mut ongoing = self.ongoing.lock().unwrap();
        ongoing.push(OngoingEvent {
            id: Uuid::new_v4(),
            item,
            received_at: Instant::now(),
            duration,
            show_duration,
        });
    }

    pub fn evict_expired(&self) {
        let mut ongoing = self.ongoing.lock().unwrap();
        ongoing.retain(|e| !e.is_expired());
    }

    pub fn snapshot(&self) -> Vec<OngoingSnapshot> {
        self.evict_expired();
        let ongoing = self.ongoing.lock().unwrap();
        ongoing
            .iter()
            .map(|e| OngoingSnapshot {
                id: e.id,
                item: e.item.clone(),
                remaining_secs: e.remaining().as_secs(),
                progress: e.progress(),
                show_duration: e.show_duration,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notifier_entry(state: &SharedState) -> OngoingSnapshot {
        let snapshot = state.snapshot();
        assert_eq!(snapshot.len(), 1);
        snapshot.into_iter().next().unwrap()
    }

    #[test]
    fn notifier_without_duration_shows_for_fixed_window_without_countdown() {
        let state = SharedState::new(NotifierConfig::default());
        state.push_notifier("Coffee".to_string(), "Kitchen".to_string(), None, None, None);
        let snapshot = notifier_entry(&state);
        assert!(!snapshot.show_duration);
        // Allow for the tiny amount of real time that elapses between push and snapshot.
        assert!(snapshot.remaining_secs >= NOTIFIER_ONGOING_DURATION.as_secs() - 1);
    }

    #[test]
    fn notifier_with_duration_behaves_like_a_real_duration() {
        let state = SharedState::new(NotifierConfig::default());
        state.push_notifier("Meeting".to_string(), "".to_string(), None, None, Some(15));
        let snapshot = notifier_entry(&state);
        assert!(snapshot.show_duration);
        assert!(snapshot.remaining_secs >= 15 * 60 - 1);
    }

    #[test]
    fn notifier_with_zero_duration_is_not_tracked() {
        let state = SharedState::new(NotifierConfig::default());
        state.push_notifier("Gone".to_string(), "".to_string(), None, None, Some(0));
        assert!(state.snapshot().is_empty());
    }

    #[test]
    fn legacy_event_without_duration_is_not_tracked() {
        let state = SharedState::new(NotifierConfig::default());
        state.push(Event {
            event_type: crate::event::EventType::Coffee,
            area: 0,
            location: "kitchen".to_string(),
            time: 0,
            description: String::new(),
            group: 0,
            start: String::new(),
            eventurl: None,
            event_organizer: "renze".to_string(),
        });
        assert!(state.snapshot().is_empty());
    }
}
