use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD, Engine};
use eframe::egui;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult};

use crate::config::{NotifierConfig, Rule};
use crate::event::{Event, EventType};
use crate::image_util;
use crate::ipc::IpcClient;
use crate::notifier::{self, NotifierPublish};
use crate::state::{OngoingItem, OngoingSnapshot};

const POLL_INTERVAL: Duration = Duration::from_millis(800);

#[derive(PartialEq)]
enum Tab {
    Events,
    Settings,
}

pub struct DcnApp {
    ipc: IpcClient,
    ongoing: Vec<OngoingSnapshot>,
    last_poll: Instant,
    tab: Tab,

    form_type: EventType,
    form_location: String,
    form_duration: u32,
    form_description: String,
    form_url: String,

    notifier_rule_id: Option<String>,
    notifier_message: String,
    notifier_location: String,
    notifier_duration: u32,
    notifier_url: String,

    config: NotifierConfig,
    config_status: Option<(String, Instant)>,
    /// Decoded preview textures for rules' `gif` fields, keyed by rule id. The `String`
    /// alongside each texture is the `gif` value it was decoded from, so edits invalidate it.
    gif_previews: HashMap<String, (String, egui::TextureHandle)>,

    export_include_password: bool,
    backup_status: Option<(String, Instant)>,

    status: Option<(String, Instant)>,
}

impl DcnApp {
    pub fn new(mut ipc: IpcClient) -> Self {
        let config = ipc.get_config().unwrap_or_default();
        let notifier_rule_id = config.rules.first().map(|r| r.id.clone());

        Self {
            ipc,
            ongoing: Vec::new(),
            // Force an immediate poll on the first frame.
            last_poll: Instant::now() - POLL_INTERVAL,
            tab: Tab::Events,
            form_type: EventType::Coffee,
            form_location: String::new(),
            form_duration: 15,
            form_description: String::new(),
            form_url: String::new(),
            notifier_rule_id,
            notifier_message: String::new(),
            notifier_location: String::new(),
            notifier_duration: 0,
            notifier_url: String::new(),
            config,
            config_status: None,
            gif_previews: HashMap::new(),
            export_include_password: false,
            backup_status: None,
            status: None,
        }
    }

    fn poll_ongoing(&mut self) {
        if self.last_poll.elapsed() < POLL_INTERVAL {
            return;
        }
        self.last_poll = Instant::now();
        match self.ipc.get_ongoing() {
            Ok(events) => self.ongoing = events,
            Err(e) => eprintln!("[dialog] failed to fetch ongoing events: {e}"),
        }
    }

    fn publish_legacy_form(&mut self) {
        let organizer = whoami::username().unwrap_or_else(|_| "unknown".to_string());
        let eventurl = if self.form_url.trim().is_empty() {
            None
        } else {
            Some(self.form_url.trim().to_string())
        };

        let event = Event {
            event_type: self.form_type,
            area: 0,
            location: self.form_location.trim().to_string(),
            time: self.form_duration,
            description: self.form_description.trim().to_string(),
            group: 0,
            start: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
            eventurl,
            event_organizer: organizer,
        };

        match self.ipc.publish(event) {
            Ok(()) => {
                self.status = Some(("Event published.".to_string(), Instant::now()));
                self.form_location.clear();
                self.form_description.clear();
                self.form_url.clear();
            }
            Err(e) => {
                self.status = Some((format!("Failed to publish event: {e}"), Instant::now()));
            }
        }
    }

    fn publish_notifier_form(&mut self) {
        let Some(rule) = self
            .config
            .rules
            .iter()
            .find(|r| Some(&r.id) == self.notifier_rule_id.as_ref())
        else {
            self.status = Some(("Select a rule first.".to_string(), Instant::now()));
            return;
        };

        let topic = notifier::send_topic(&rule.topic);
        let location = Some(self.notifier_location.trim().to_string()).filter(|s| !s.is_empty());
        let url = Some(self.notifier_url.trim().to_string()).filter(|s| !s.is_empty());
        let duration_minutes = if self.notifier_duration == 0 { None } else { Some(self.notifier_duration) };
        let publish = NotifierPublish {
            topic,
            msg_type: rule.match_pattern.clone(),
            message: self.notifier_message.clone(),
            location,
            duration_minutes,
            url,
        };

        match self.ipc.publish_notifier(publish) {
            Ok(()) => {
                self.status = Some(("Event published.".to_string(), Instant::now()));
                self.notifier_message.clear();
                self.notifier_location.clear();
                self.notifier_duration = 0;
                self.notifier_url.clear();
            }
            Err(e) => {
                self.status = Some((format!("Failed to publish event: {e}"), Instant::now()));
            }
        }
    }

    fn save_config(&mut self) {
        match self.ipc.set_config(self.config.clone()) {
            Ok(()) => self.config_status = Some(("Saved.".to_string(), Instant::now())),
            Err(e) => self.config_status = Some((format!("Failed to save settings: {e}"), Instant::now())),
        }
    }

    /// Returns the (cached, if unchanged) preview texture for a rule's `gif` field, or `None`
    /// if it's empty or isn't a decodable inline image (e.g. a remote URL - those aren't
    /// fetched, so there's nothing to preview).
    fn gif_preview_texture(&mut self, ctx: &egui::Context, rule_id: &str, gif: &str) -> Option<egui::TextureHandle> {
        if gif.is_empty() {
            self.gif_previews.remove(rule_id);
            return None;
        }
        if let Some((cached_gif, texture)) = self.gif_previews.get(rule_id) {
            if cached_gif == gif {
                return Some(texture.clone());
            }
        }
        let (width, height, rgba) = image_util::decode_for_preview(gif)?;
        let color_image = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
        let texture = ctx.load_texture(format!("gif-preview-{rule_id}"), color_image, egui::TextureOptions::default());
        self.gif_previews.insert(rule_id.to_string(), (gif.to_string(), texture.clone()));
        Some(texture)
    }

    fn export_settings(&mut self) {
        let Some(path) = FileDialog::new()
            .add_filter("JSON", &["json"])
            .set_file_name(NotifierConfig::default_export_file_name())
            .save_file()
        else {
            return;
        };

        let result = self
            .config
            .export_json(self.export_include_password)
            .and_then(|json| std::fs::write(&path, json).map_err(anyhow::Error::from));

        self.backup_status = Some(match result {
            Ok(()) => (format!("Exported to {}", path.display()), Instant::now()),
            Err(e) => (format!("Export failed: {e}"), Instant::now()),
        });
    }

    fn import_settings(&mut self) {
        let Some(path) = FileDialog::new().add_filter("JSON", &["json"]).pick_file() else {
            return;
        };

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                self.backup_status = Some((format!("Failed to read {}: {e}", path.display()), Instant::now()));
                return;
            }
        };

        let merged = match NotifierConfig::merge_import(&self.config, &text) {
            Ok(merged) => merged,
            Err(e) => {
                self.backup_status = Some((format!("Import failed: {e}"), Instant::now()));
                return;
            }
        };

        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let confirmed = MessageDialog::new()
            .set_title("Import settings")
            .set_description(format!(
                "Replace the current settings with \"{file_name}\" ({} rule(s))?",
                merged.rules.len()
            ))
            .set_buttons(MessageButtons::OkCancel)
            .show()
            == MessageDialogResult::Ok;
        if !confirmed {
            return;
        }

        self.gif_previews.clear();
        self.notifier_rule_id = merged.rules.first().map(|r| r.id.clone());
        let rule_count = merged.rules.len();
        self.config = merged.clone();

        self.backup_status = Some(match self.ipc.set_config(merged) {
            Ok(()) => (format!("Imported {rule_count} rule(s)."), Instant::now()),
            Err(e) => (format!("Failed to apply imported settings: {e}"), Instant::now()),
        });
    }

    fn show_events_tab(&mut self, ui: &mut egui::Ui) {
        if self.config.legacy.enabled {
            ui.heading("Create event (legacy)");

            egui::Grid::new("create-event-form")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Type:");
                    egui::ComboBox::from_id_salt("create-event-type")
                        .selected_text(self.form_type.label())
                        .show_ui(ui, |ui| {
                            for t in EventType::SELECTABLE {
                                ui.selectable_value(&mut self.form_type, t, t.label());
                            }
                        });
                    ui.end_row();

                    ui.label("Location:");
                    ui.text_edit_singleline(&mut self.form_location);
                    ui.end_row();

                    ui.label("Duration (minutes):");
                    ui.add(egui::DragValue::new(&mut self.form_duration).range(0..=100_000));
                    ui.end_row();

                    ui.label("Description:");
                    ui.text_edit_multiline(&mut self.form_description);
                    ui.end_row();

                    ui.label("URL:");
                    ui.text_edit_singleline(&mut self.form_url);
                    ui.end_row();
                });

            ui.add_space(4.0);
            if ui.button("Publish event (legacy)").clicked() {
                self.publish_legacy_form();
            }

            ui.separator();
        }
        ui.heading("Publish event");

        if self.config.rules.is_empty() {
            ui.label("No rules configured yet - add one in Settings.");
        } else {
            egui::Grid::new("publish-notifier-form")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Rule:");
                    let selected_label = self
                        .config
                        .rules
                        .iter()
                        .find(|r| Some(&r.id) == self.notifier_rule_id.as_ref())
                        .map(|r| notifier::rule_label(r).to_string())
                        .unwrap_or_else(|| "Select a rule".to_string());
                    egui::ComboBox::from_id_salt("publish-notifier-rule")
                        .selected_text(selected_label)
                        .show_ui(ui, |ui| {
                            for rule in &self.config.rules {
                                let label = notifier::rule_label(rule).to_string();
                                ui.selectable_value(&mut self.notifier_rule_id, Some(rule.id.clone()), label);
                            }
                        });
                    ui.end_row();

                    ui.label("Message:");
                    ui.text_edit_multiline(&mut self.notifier_message);
                    ui.end_row();

                    ui.label("Location:");
                    ui.text_edit_singleline(&mut self.notifier_location);
                    ui.end_row();

                    ui.label("Duration (minutes):");
                    ui.add(egui::DragValue::new(&mut self.notifier_duration).range(0..=100_000));
                    ui.end_row();

                    ui.label("URL:");
                    ui.text_edit_singleline(&mut self.notifier_url);
                    ui.end_row();
                });

            ui.add_space(4.0);
            if ui.button("Publish event").clicked() {
                self.publish_notifier_form();
            }
        }

        if let Some((message, shown_at)) = &self.status {
            if shown_at.elapsed() < Duration::from_secs(4) {
                ui.label(message.clone());
            } else {
                self.status = None;
            }
        }

        ui.separator();
        ui.heading("Ongoing events");

        if self.ongoing.is_empty() {
            ui.label("No ongoing events.");
        } else {
            for snapshot in &self.ongoing {
                ui.push_id(snapshot.id, |ui| {
                    ui.group(|ui| {
                        match &snapshot.item {
                            OngoingItem::Legacy { event } => {
                                ui.label(format!("{} — {}", event.event_type.label(), event.location));
                                ui.label(format!("Organizer: {}", event.event_organizer));
                                if !event.description.is_empty() {
                                    ui.label(&event.description);
                                }
                            }
                            OngoingItem::Notifier { title, message, location, url } => {
                                ui.label(title);
                                if !message.is_empty() {
                                    ui.label(message);
                                }
                                if let Some(location) = location {
                                    ui.label(format!("Location: {location}"));
                                }
                                if let Some(url) = url {
                                    ui.hyperlink(url);
                                }
                            }
                        }
                        if snapshot.show_duration {
                            let remaining_mins = snapshot.remaining_secs.div_ceil(60);
                            ui.add(
                                egui::ProgressBar::new(snapshot.progress)
                                    .text(format!("{remaining_mins} min remaining")),
                            );
                        }
                    });
                });
            }
        }
    }

    fn show_settings_tab(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        // Preview textures need `&mut self`, so decode them before the rules loop below
        // borrows `self.config.rules` mutably.
        let rule_textures: Vec<Option<egui::TextureHandle>> = (0..self.config.rules.len())
            .map(|i| {
                let id = self.config.rules[i].id.clone();
                let gif = self.config.rules[i].gif.clone();
                self.gif_preview_texture(&ctx, &id, &gif)
            })
            .collect();

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Legacy protocol");
            egui::Grid::new("legacy-settings-form")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    let enabled = self.config.legacy.enabled;
                    ui.label("Enabled:");
                    ui.checkbox(&mut self.config.legacy.enabled, "");
                    ui.end_row();

                    ui.label("Broker URL:");
                    ui.add_enabled(enabled, egui::TextEdit::singleline(&mut self.config.legacy.broker_url));
                    ui.end_row();

                    ui.label("Topic:");
                    ui.add_enabled(enabled, egui::TextEdit::singleline(&mut self.config.legacy.topic));
                    ui.end_row();

                    ui.label("Encryption key:");
                    ui.add_enabled(enabled, egui::TextEdit::singleline(&mut self.config.legacy.encryption_key));
                    ui.end_row();
                });

            ui.separator();
            ui.heading("New protocol broker");
            egui::Grid::new("notifier-broker-form")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("WebSocket URL:");
                    ui.text_edit_singleline(&mut self.config.broker_url);
                    ui.end_row();

                    ui.label("Username:");
                    ui.text_edit_singleline(&mut self.config.username);
                    ui.end_row();

                    ui.label("Password:");
                    ui.add(egui::TextEdit::singleline(&mut self.config.password).password(true));
                    ui.end_row();

                    ui.label("Client ID:");
                    ui.text_edit_singleline(&mut self.config.client_id);
                    ui.end_row();
                });

            ui.add_space(4.0);
            ui.checkbox(&mut self.config.show_notification, "Show system notification");
            ui.checkbox(&mut self.config.ignore_retained, "Ignore retained messages");

            ui.separator();
            ui.heading("New protocol rules");
            ui.label(
                "The first matching rule wins. Topic supports the MQTT wildcards + and #. \
                 \"Type\" must match a message's type exactly (a regular expression such as \
                 alarm|fault also works; empty = every message). Title placeholders: {topic}, \
                 {type}, {message}, {payload}. A rule's image is shown as a static preview in \
                 the matching notification (desktop notifications can't animate GIFs).",
            );

            let mut remove_index = None;
            for (i, (rule, texture)) in self.config.rules.iter_mut().zip(rule_textures.iter()).enumerate() {
                ui.push_id(&rule.id, |ui| {
                    ui.group(|ui| {
                        egui::Grid::new(("rule-form", i))
                            .num_columns(2)
                            .spacing([12.0, 8.0])
                            .show(ui, |ui| {
                                ui.label("Topic:");
                                ui.text_edit_singleline(&mut rule.topic);
                                ui.end_row();

                                ui.label("Type:");
                                ui.text_edit_singleline(&mut rule.match_pattern);
                                ui.end_row();

                                ui.label("Title:");
                                ui.text_edit_singleline(&mut rule.title);
                                ui.end_row();
                            });

                        ui.horizontal(|ui| {
                            if let Some(texture) = texture {
                                ui.add(egui::Image::new(texture).max_size(egui::vec2(96.0, 72.0)));
                            }
                            ui.vertical(|ui| {
                                if rule.gif.starts_with("data:") {
                                    let approx_kb = rule.gif.len() * 3 / 4 / 1024;
                                    ui.label(format!("Uploaded image (~{approx_kb} KB)"));
                                } else {
                                    ui.horizontal(|ui| {
                                        ui.label("Image URL:");
                                        ui.text_edit_singleline(&mut rule.gif);
                                    });
                                }
                                ui.horizontal(|ui| {
                                    if ui.button("Choose file…").clicked() {
                                        if let Some(path) =
                                            FileDialog::new().add_filter("Images", &["gif", "png", "jpg", "jpeg", "webp"]).pick_file()
                                        {
                                            match std::fs::read(&path) {
                                                Ok(bytes) => {
                                                    let mime = mime_for_path(&path);
                                                    rule.gif = format!("data:{mime};base64,{}", STANDARD.encode(bytes));
                                                }
                                                Err(e) => eprintln!("[dialog] failed to read {}: {e}", path.display()),
                                            }
                                        }
                                    }
                                    if !rule.gif.is_empty() && ui.button("Clear").clicked() {
                                        rule.gif.clear();
                                    }
                                });
                            });
                        });

                        if ui.button("Remove").clicked() {
                            remove_index = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = remove_index {
                let removed = self.config.rules.remove(i);
                self.gif_previews.remove(&removed.id);
            }
            if ui.button("+ Add rule").clicked() {
                self.config.rules.push(Rule::new());
            }

            ui.separator();
            if ui.button("Save").clicked() {
                self.save_config();
            }
            if let Some((message, shown_at)) = &self.config_status {
                if shown_at.elapsed() < Duration::from_secs(4) {
                    ui.label(message.clone());
                } else {
                    self.config_status = None;
                }
            }

            ui.separator();
            ui.heading("Backup");
            ui.label(
                "Export writes the settings currently shown above to a JSON file \
                 (mqtt-notifier-settings-YYYY-MM-DD.json by default). Import replaces them - \
                 the applied configuration is still only ~/.mqtt-notifier-config.json.",
            );
            ui.horizontal(|ui| {
                if ui.button("Export settings…").clicked() {
                    self.export_settings();
                }
                if ui.button("Import settings…").clicked() {
                    self.import_settings();
                }
                ui.checkbox(&mut self.export_include_password, "Include password in export");
            });
            if let Some((message, shown_at)) = &self.backup_status {
                if shown_at.elapsed() < Duration::from_secs(6) {
                    ui.label(message.clone());
                } else {
                    self.backup_status = None;
                }
            }
        });
    }
}

fn mime_for_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
        "gif" => "image/gif",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

impl eframe::App for DcnApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_ongoing();

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Events, "Events");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Events => self.show_events_tab(ui),
            Tab::Settings => self.show_settings_tab(ui),
        });

        ctx.request_repaint_after(POLL_INTERVAL);
    }
}
