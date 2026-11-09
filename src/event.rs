use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize_repr, Deserialize_repr)]
#[repr(u8)]
pub enum EventType {
    #[default]
    Invalid = 0,
    Coffee = 1,
    Cake = 2,
    Lunch = 3,
    Meeting = 4,
    Vrimibo = 5,
    Nyan = 6,
    AdventOfCode = 7,
}

impl EventType {
    /// Types offered in the "create event" dropdown. `Invalid` is excluded -
    /// it only exists to decode events from other, possibly misbehaving,
    /// DCN clients.
    pub const SELECTABLE: [EventType; 7] = [
        EventType::Coffee,
        EventType::Cake,
        EventType::Lunch,
        EventType::Meeting,
        EventType::Vrimibo,
        EventType::Nyan,
        EventType::AdventOfCode,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            EventType::Invalid => "Invalid",
            EventType::Coffee => "Coffee",
            EventType::Cake => "Cake",
            EventType::Lunch => "Lunch",
            EventType::Meeting => "Meeting",
            EventType::Vrimibo => "Vrimibo",
            EventType::Nyan => "Nyan",
            EventType::AdventOfCode => "Advent of Code",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    #[serde(rename = "type")]
    pub event_type: EventType,
    #[serde(default)]
    pub area: i64,
    pub location: String,
    /// Duration in minutes. 0 means the event expires immediately.
    pub time: u32,
    pub description: String,
    #[serde(default)]
    pub group: i64,
    pub start: String,
    #[serde(default)]
    pub eventurl: Option<String>,
    #[serde(rename = "eventOrganizer")]
    pub event_organizer: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_dcn_wire_format() {
        let json = r#"{
            "type":1,
            "area":0,
            "location":"kitchen",
            "time":15,
            "description":"Coffee time",
            "group":0,
            "start":"2023-05-24T16:30:00.345+01:00",
            "eventurl":null,
            "eventOrganizer":"renze"
        }"#;
        let event: Event = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, EventType::Coffee);
        assert_eq!(event.location, "kitchen");
        assert_eq!(event.time, 15);
        assert_eq!(event.eventurl, None);
        assert_eq!(event.event_organizer, "renze");

        let reserialized = serde_json::to_string(&event).unwrap();
        let roundtripped: Event = serde_json::from_str(&reserialized).unwrap();
        assert_eq!(roundtripped.event_type, event.event_type);
    }
}
