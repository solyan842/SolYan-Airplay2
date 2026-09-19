use crate::{theme, worker};
use base64::Engine as _;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Stroke};
use serde::{Deserialize, Serialize};
use solyan_airplay_core::device_profile::DeviceKind;
use solyan_airplay_core::discovery::AirPlayReceiver;
use solyan_airplay_core::live::{LiveStreamMode, StreamControl, StreamProgress};
use std::collections::VecDeque;
use std::time::Duration;
use worker::GuiEvent;

const PREFS_KEY: &str = "solyan-airplay2-prefs";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    volume: f32,
    render_delay_ms: u32,
    video_low_latency: bool,
    experimental_multiroom: bool,
    last_receiver_id: Option<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            volume: 0.80,
            render_delay_ms: 200,
            video_low_latency: false,
            experimental_multiroom: false,
            last_receiver_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activity {
    Idle,
    Scanning,
    AudioTest,
    ConnectionTest,
    Streaming,
    Stopping,
}

impl Activity {
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "Ready",
            Self::Scanning => "Scanning",
            Self::AudioTest => "Testing audio",
            Self::ConnectionTest => "Testing AirPlay",
            Self::Streaming => "Streaming",
            Self::Stopping => "Stopping",
        }
    }

    fn is_streaming(self) -> bool {
        matches!(self, Self::Streaming | Self::Stopping)
    }

    fn is_busy(self) -> bool {
        !matches!(self, Self::Idle)
    }
}

pub struct SolYanAirPlayApp {
    prefs: Preferences,
    devices: Vec<AirPlayReceiver>,
    selected_ids: Vec<String>,
    activity: Activity,
    status: String,
    last_error: Option<String>,
    logs: VecDeque<String>,
    event_tx: Sender<GuiEvent>,
    event_rx: Receiver<GuiEvent>,
    progress_rx: Option<Receiver<StreamProgress>>,
    stream_control: Option<StreamControl>,
    progress: Option<StreamProgress>,
    logo_texture: egui::TextureHandle,
}

impl SolYanAirPlayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);

        let prefs = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, PREFS_KEY))
            .unwrap_or_default();

        let logo_png = base64::engine::general_purpose::STANDARD
            .decode(concat!(include_str!("../assets/solyan-airplay-logo.0.b64"), include_str!("../assets/solyan-airplay-logo.1.b64"), include_str!("../assets/solyan-airplay-logo.2.b64"), include_str!("../assets/solyan-airplay-logo.3.b64")))
            .expect("embedded SolYan AirPlay logo must decode");
        let icon = eframe::icon_data::from_png_bytes(&logo_png)
            .expect("embedded SolYan AirPlay logo must be a valid PNG");
        let logo_image = egui::ColorImage::from_rgba_unmultiplied(
            [icon.width as usize, icon.height as usize],
            &icon.rgba,
        );
        let logo_texture = cc.egui_ctx.load_texture(
            "solyan-airplay-logo",
            logo_image,
            egui::TextureOptions::LINEAR,
        );

        let (event_tx, event_rx) = unbounded();
        let mut app = Self {
            prefs,
            devices: Vec::new(),
            selected_ids: Vec::new(),
            activity: Activity::Idle,
            status: "Ready to discover HomePod and AirPlay receivers.".into(),
            last_error: None,
            logs: VecDeque::new(),
            event_tx,
            event_rx,
            progress_rx: None,
            stream_control: None,
            progress: None,
            logo_texture,
        };

        app.log("SolYan AirPlay2 v0.1.8 GUI initialized.");
        app.start_scan(cc.egui_ctx.clone());
        app
    }

    fn log(&mut self, line: impl Into<String>) {
        if self.logs.len() >= 250 {
            self.logs.pop_front();
        }
        self.logs.push_back(line.into());
    }

    fn start_scan(&mut self, ctx: egui::Context) {
        if self.activity.is_streaming() {
            return;
        }
        self.activity = Activity::Scanning;
        self.status = "Searching the local network for AirPlay receivers...".into();
        self.last_error = None;
        self.log("Scanning mDNS _airplay._tcp.local...");
        worker::spawn_scan(ctx, self.event_tx.clone());
    }

    fn start_capture_test(&mut self, ctx: egui::Context) {
        if self.activity != Activity::Idle {
            return;
        }
        self.activity = Activity::AudioTest;
        self.status = "Testing Windows WASAPI loopback for 3 seconds...".into();
        self.last_error = None;
        self.log("Starting WASAPI loopback probe.");
        worker::spawn_capture_probe(ctx, self.event_tx.clone());
    }

    fn start_connection_test(&mut self, ctx: egui::Context) {
        if self.activity != Activity::Idle {
            return;
        }
        let Some(selector) = self.selected_ids.first().cloned() else {
            self.last_error = Some("Select a receiver first.".into());
            return;
        };

        self.activity = Activity::ConnectionTest;
        self.status = "Pairing and testing the encrypted AirPlay session...".into();
        self.last_error = None;
        self.log("Starting /info → transient pairing → encrypted RTSP SETUP test.");
        worker::spawn_connect_test(ctx, self.event_tx.clone(), selector);
    }

    fn start_stream(&mut self, ctx: egui::Context) {
        if self.activity != Activity::Idle {
            return;
        }
        if self.selected_ids.is_empty() {
            self.last_error = Some("Select a receiver first.".into());
            return;
        }

        let mode = if self.prefs.experimental_multiroom && self.selected_ids.len() >= 2 {
            LiveStreamMode::MultiroomExperimental
        } else {
            if self.selected_ids.len() != 1 {
                self.last_error = Some(
                    "Single-speaker mode requires exactly one selected receiver.".into(),
                );
                return;
            }
            LiveStreamMode::Single
        };

        let control = StreamControl::new(self.prefs.volume);
        let (progress_tx, progress_rx) = bounded(12);

        self.stream_control = Some(control.clone());
        self.progress_rx = Some(progress_rx);
        self.progress = None;
        self.activity = Activity::Streaming;
        self.last_error = None;

        let target_count = self.selected_ids.len();
        self.status = if mode == LiveStreamMode::MultiroomExperimental {
            format!("Building experimental PTP group for {target_count} speakers...")
        } else {
            "Connecting and starting realtime ALAC stream...".into()
        };

        let effective_render_delay_ms = if self.prefs.video_low_latency {
            0
        } else {
            self.prefs.render_delay_ms
        };

        self.log(match mode {
            LiveStreamMode::Single => {
                format!(
                    "Starting single-speaker stream (profile={}, render lead {} ms).",
                    if self.prefs.video_low_latency { "Video" } else { "Music" },
                    effective_render_delay_ms
                )
            }
            LiveStreamMode::MultiroomExperimental => format!(
                "Starting EXPERIMENTAL multiroom stream to {target_count} speakers using PTP."
            ),
        });

        worker::spawn_stream(
            ctx,
            self.event_tx.clone(),
            progress_tx,
            self.selected_ids.clone(),
            mode,
            control,
            effective_render_delay_ms,
        );
    }

    fn stop_stream(&mut self) {
        if let Some(control) = &self.stream_control {
            control.stop();
            self.activity = Activity::Stopping;
            self.status = "Stopping stream and closing AirPlay session...".into();
            self.log("Stop requested.");
        }
    }

    fn set_selected(&mut self, id: String) {
        if self.prefs.experimental_multiroom {
            if let Some(pos) = self.selected_ids.iter().position(|v| v == &id) {
                self.selected_ids.remove(pos);
            } else {
                self.selected_ids.push(id);
            }
        } else {
            self.selected_ids.clear();
            self.selected_ids.push(id);
        }

        self.prefs.last_receiver_id = self.selected_ids.first().cloned();
    }

    fn selected_name_list(&self) -> Vec<String> {
        self.selected_ids
            .iter()
            .filter_map(|id| {
                self.devices
                    .iter()
                    .find(|d| &d.id == id)
                    .map(|d| d.name.clone())
            })
            .collect()
    }

    fn handle_event(&mut self, event: GuiEvent) {
        match event {
            GuiEvent::ScanFinished(result) => {
                self.activity = Activity::Idle;
                match result {
                    Ok(devices) => {
                        self.log(format!("Discovery complete: {} receiver(s).", devices.len()));
                        let mut devices = devices;
                        devices.sort_by_key(|device| device_sort_rank(device.device_kind()));
                        self.devices = devices;

                        self.selected_ids
                            .retain(|id| self.devices.iter().any(|d| &d.id == id));

                        if self.selected_ids.is_empty() {
                            if let Some(last) = &self.prefs.last_receiver_id {
                                if self.devices.iter().any(|d| &d.id == last) {
                                    self.selected_ids.push(last.clone());
                                }
                            }
                        }
                        if self.selected_ids.is_empty() && self.devices.len() == 1 {
                            self.selected_ids.push(self.devices[0].id.clone());
                        }
                        if !self.prefs.experimental_multiroom && self.selected_ids.len() > 1 {
                            self.selected_ids.truncate(1);
                        }

                        self.status = if self.devices.is_empty() {
                            "No AirPlay receiver found on this network.".into()
                        } else {
                            let homepods = self
                                .devices
                                .iter()
                                .filter(|device| device.device_kind().is_homepod())
                                .count();
                            let apple_tvs = self
                                .devices
                                .iter()
                                .filter(|device| device.device_kind().is_apple_tv())
                                .count();
                            format!(
                                "{} AirPlay device(s) — {} HomePod, {} Apple TV.",
                                self.devices.len(),
                                homepods,
                                apple_tvs
                            )
                        };
                    }
                    Err(error) => {
                        self.last_error = Some(error.clone());
                        self.status = "Discovery failed.".into();
                        self.log(format!("Discovery error: {error}"));
                    }
                }
            }
            GuiEvent::CaptureFinished(result) => {
                self.activity = Activity::Idle;
                match result {
                    Ok(probe) => {
                        self.status = format!(
                            "WASAPI OK — {} chunks captured from {}.",
                            probe.chunks, probe.device_name
                        );
                        self.log(format!(
                            "WASAPI PASS: device='{}', chunks={}, peak={}.",
                            probe.device_name, probe.chunks, probe.peak
                        ));
                        if probe.chunks == 0 {
                            self.last_error =
                                Some("No PCM was captured. Play audio and test again.".into());
                        }
                    }
                    Err(error) => {
                        self.last_error = Some(error.clone());
                        self.status = "WASAPI test failed.".into();
                        self.log(format!("WASAPI error: {error}"));
                    }
                }
            }
            GuiEvent::ConnectFinished(result) => {
                self.activity = Activity::Idle;
                match result {
                    Ok(info) => {
                        self.status = format!("AirPlay session verified with {}.", info.name);
                        self.log(format!(
                            "SESSION PASS: {} ({}) @ {}.",
                            info.name, info.model, info.address
                        ));
                    }
                    Err(error) => {
                        self.last_error = Some(error.clone());
                        self.status = "AirPlay session test failed.".into();
                        self.log(format!("Session error: {error}"));
                    }
                }
            }
            GuiEvent::StreamFinished(result) => {
                self.activity = Activity::Idle;
                self.stream_control = None;
                self.progress_rx = None;
                match result {
                    Ok(info) => {
                        self.status = format!(
                            "Stream stopped cleanly after {:.1}s.",
                            info.elapsed.as_secs_f64()
                        );
                        self.log(format!(
                            "Stream closed: targets={}, captured={}, silence={}, late={}, transitions={}, dropped={}.",
                            info.target_names.join(", "),
                            info.captured_chunks,
                            info.silence_chunks,
                            info.late_polls,
                            info.silence_transitions,
                            info.dropped_chunks
                        ));
                    }
                    Err(error) => {
                        self.last_error = Some(error.clone());
                        self.status = "Streaming stopped with an error.".into();
                        self.log(format!("Stream error: {error}"));
                    }
                }
            }
        }
    }

    fn draw_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.image((self.logo_texture.id(), egui::vec2(54.0, 54.0)));
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("SolYan AirPlay2")
                        .size(26.0)
                        .strong()
                        .color(theme::TEXT),
                );
                ui.label(
                    RichText::new("Stream Beyond Boundaries · Native Windows AirPlay 2")
                        .size(13.0)
                        .color(theme::MUTED),
                );
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (text, color) = if self.last_error.is_some() {
                    ("Attention", theme::RED)
                } else if self.activity == Activity::Streaming {
                    ("Live", theme::GREEN)
                } else if self.activity == Activity::Stopping {
                    ("Stopping", theme::ACCENT)
                } else if self.activity.is_busy() {
                    (self.activity.label(), theme::BLUE)
                } else {
                    ("Ready", theme::GREEN)
                };

                egui::Frame::new()
                    .fill(color.gamma_multiply(0.16))
                    .stroke(Stroke::new(1.0, color.gamma_multiply(0.55)))
                    .corner_radius(egui::CornerRadius::same(20))
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).color(color).strong());
                    });
            });
        });

        ui.add_space(8.0);
        ui.label(RichText::new(&self.status).color(theme::MUTED));
        if let Some(error) = &self.last_error {
            ui.add_space(4.0);
            ui.label(RichText::new(error).color(theme::RED));
        }
    }

    fn draw_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("AIRPLAY DEVICES").size(12.0).strong().color(theme::MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let enabled = !self.activity.is_streaming();
                if ui
                    .add_enabled(enabled, egui::Button::new("Scan"))
                    .clicked()
                {
                    self.start_scan(ui.ctx().clone());
                }
            });
        });

        ui.add_space(8.0);

        let mut clicked_id: Option<String> = None;
        let list_height = (ui.available_height() - 112.0).max(150.0);
        egui::ScrollArea::vertical()
            .id_salt("speaker-list")
            .auto_shrink([false, false])
            .max_height(list_height)
            .show(ui, |ui| {
                if self.devices.is_empty() {
                    theme::sidebar_card().show(ui, |ui| {
                        ui.label(RichText::new("No receiver yet").strong().color(theme::TEXT));
                        ui.label(
                            RichText::new("Scan the local network to find HomePod or AirPlay receivers.")
                                .size(12.0)
                                .color(theme::MUTED),
                        );
                    });
                }

                for (index, device) in self.devices.iter().enumerate() {
                    let selected = self.selected_ids.iter().any(|id| id == &device.id);
                    let fill = if selected {
                        theme::ACCENT_SOFT
                    } else {
                        theme::SIDEBAR
                    };
                    let border = if selected { theme::ACCENT } else { theme::BORDER };
                    let ip = device.preferred_address().unwrap_or("No address");
                    let kind = device.device_kind();

                    let response = egui::Frame::new()
                        .fill(fill)
                        .stroke(Stroke::new(1.0, border))
                        .corner_radius(egui::CornerRadius::same(12))
                        .inner_margin(egui::Margin::same(13))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                device_icon(ui, kind, selected);
                                ui.add_space(10.0);
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new(&device.name)
                                            .strong()
                                            .size(15.0)
                                            .color(theme::TEXT),
                                    );
                                    ui.label(
                                        RichText::new(device.friendly_model_name())
                                            .size(12.0)
                                            .strong()
                                            .color(if selected { theme::ACCENT } else { theme::TEXT }),
                                    );
                                    ui.label(
                                        RichText::new(format!("{}  •  {}", device.model, ip))
                                            .size(10.5)
                                            .color(theme::MUTED),
                                    );
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if self.prefs.experimental_multiroom
                                        && selected
                                        && self.selected_ids.first() == Some(&device.id)
                                    {
                                        ui.label(
                                            RichText::new("LEADER")
                                                .size(10.0)
                                                .strong()
                                                .color(theme::ACCENT),
                                        );
                                    } else if selected {
                                        ui.label(
                                            RichText::new("SELECTED")
                                                .size(10.0)
                                                .strong()
                                                .color(theme::ACCENT),
                                        );
                                    }
                                });
                            });
                            ui.add_space(7.0);
                            ui.horizontal_wrapped(|ui| {
                                capability_badge(ui, "Audio", device.supports_audio, false);
                                capability_badge(ui, "AP2", device.supports_airplay2, false);
                                capability_badge(ui, "PTP", device.supports_ptp, true);
                            });
                        })
                        .response
                        .interact(Sense::click());

                    if response.clicked() && !self.activity.is_streaming() {
                        clicked_id = Some(device.id.clone());
                    }
                    if index + 1 < self.devices.len() {
                        ui.add_space(8.0);
                    }
                }
            });

        if let Some(id) = clicked_id {
            self.set_selected(id);
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        let old = self.prefs.experimental_multiroom;
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut self.prefs.experimental_multiroom,
                RichText::new("Multiroom").strong().color(theme::TEXT),
            );
            ui.label(
                RichText::new("EXPERIMENTAL")
                    .size(9.5)
                    .strong()
                    .color(theme::ACCENT),
            );
        });
        ui.label(
            RichText::new("Select 2+ speakers. First selected speaker is the group leader.")
                .size(11.5)
                .color(theme::MUTED),
        );

        if old && !self.prefs.experimental_multiroom && self.selected_ids.len() > 1 {
            self.selected_ids.truncate(1);
        }
    }

    fn draw_stream_card(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("NOW STREAMING")
                            .size(11.0)
                            .strong()
                            .color(theme::MUTED),
                    );
                    let names = self.selected_name_list();
                    let title = if names.is_empty() {
                        "No speaker selected".to_string()
                    } else {
                        names.join("  +  ")
                    };
                    ui.label(RichText::new(title).size(22.0).strong().color(theme::TEXT));
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.activity.is_streaming() {
                        let stop = ui.add_sized(
                            [112.0, 42.0],
                            egui::Button::new(
                                RichText::new("Stop").strong().color(Color32::WHITE),
                            )
                            .fill(theme::RED.gamma_multiply(0.78)),
                        );
                        if stop.clicked() && self.activity == Activity::Streaming {
                            self.stop_stream();
                        }
                    } else {
                        let can_start = self.activity == Activity::Idle
                            && !self.selected_ids.is_empty()
                            && (!self.prefs.experimental_multiroom
                                || self.selected_ids.len() == 1
                                || self.selected_ids.len() >= 2);
                        let start = ui.add_enabled(
                            can_start,
                            egui::Button::new(
                                RichText::new("Start streaming")
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .fill(theme::ACCENT)
                            .min_size(egui::vec2(148.0, 42.0)),
                        );
                        if start.clicked() {
                            self.start_stream(ui.ctx().clone());
                        }
                    }
                });
            });

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(14.0);

            ui.columns(3, |columns| {
                metric(
                    &mut columns[0],
                    "Source",
                    "Windows system audio",
                    "WASAPI loopback",
                );

                let protocol = if self.prefs.experimental_multiroom && self.selected_ids.len() >= 2 {
                    "ALAC / PTP"
                } else {
                    "ALAC / NTP"
                };
                metric(
                    &mut columns[1],
                    "Protocol",
                    protocol,
                    if protocol.contains("PTP") {
                        "Experimental group timing"
                    } else {
                        "Realtime compatibility path"
                    },
                );

                let effective_render_delay_ms = if self.prefs.video_low_latency {
                    0
                } else {
                    self.prefs.render_delay_ms
                };
                metric(
                    &mut columns[2],
                    "Render lead",
                    &format!("{} ms", effective_render_delay_ms),
                    if self.prefs.video_low_latency {
                        "Video low-latency profile"
                    } else {
                        "Retransmit headroom"
                    },
                );
            });
        });
    }

    fn draw_controls_card(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.label(RichText::new("Playback").size(17.0).strong().color(theme::TEXT));
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.label(RichText::new("Profile").color(theme::MUTED));
                let enabled = !self.activity.is_streaming();
                ui.add_enabled_ui(enabled, |ui| {
                    if ui.selectable_label(!self.prefs.video_low_latency, "Music").clicked() {
                        self.prefs.video_low_latency = false;
                    }
                    if ui.selectable_label(self.prefs.video_low_latency, "Video Low Latency").clicked() {
                        self.prefs.video_low_latency = true;
                    }
                });
            });
            if self.prefs.video_low_latency {
                ui.label(
                    RichText::new(
                        "Video mode removes SolYan's extra render lead. HomePod realtime ALAC can still have receiver-side latency; true frame-accurate sync requires video compensation or a lower-latency AirPlay transport.",
                    )
                    .size(11.0)
                    .color(theme::ACCENT),
                );
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Volume").color(theme::MUTED));
                ui.add_space(8.0);

                let response = ui.scope(|ui| {
                    let visuals = ui.visuals_mut();
                    visuals.slider_trailing_fill = true;
                    visuals.selection.bg_fill = theme::ACCENT;
                    visuals.widgets.inactive.bg_fill = Color32::from_rgb(48, 49, 55);
                    visuals.widgets.hovered.bg_fill = Color32::from_rgb(58, 59, 66);
                    visuals.widgets.active.bg_fill = theme::ACCENT_SOFT;
                    ui.spacing_mut().slider_width = 360.0;
                    ui.add(
                        egui::Slider::new(&mut self.prefs.volume, 0.0..=1.0)
                            .show_value(false),
                    )
                }).inner;

                ui.add_space(8.0);
                egui::Frame::new()
                    .fill(theme::ACCENT_SOFT)
                    .stroke(Stroke::new(1.0, theme::ACCENT))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!("{:.0}%", self.prefs.volume * 100.0))
                                .size(14.0)
                                .strong()
                                .color(theme::ACCENT),
                        );
                    });

                if response.changed() {
                    if let Some(control) = &self.stream_control {
                        control.set_volume(self.prefs.volume);
                    }
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Latency").color(theme::MUTED));
                let enabled = !self.activity.is_streaming() && !self.prefs.video_low_latency;
                ui.add_enabled_ui(enabled, |ui| {
                    ui.add(
                        egui::Slider::new(&mut self.prefs.render_delay_ms, 0..=600)
                            .suffix(" ms"),
                    );
                    if ui.small_button("Low 200").clicked() {
                        self.prefs.render_delay_ms = 200;
                    }
                    if ui.small_button("Stable 350").clicked() {
                        self.prefs.render_delay_ms = 350;
                    }
                    if ui.small_button("Safe 500").clicked() {
                        self.prefs.render_delay_ms = 500;
                    }
                });
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let enabled = self.activity == Activity::Idle;
                if ui
                    .add_enabled(enabled, egui::Button::new("Test Windows audio"))
                    .clicked()
                {
                    self.start_capture_test(ui.ctx().clone());
                }

                if ui
                    .add_enabled(
                        enabled && !self.selected_ids.is_empty(),
                        egui::Button::new("Test AirPlay session"),
                    )
                    .clicked()
                {
                    self.start_connection_test(ui.ctx().clone());
                }
            });
        });
    }

    fn draw_multiroom_card(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Multiroom")
                        .size(17.0)
                        .strong()
                        .color(theme::TEXT),
                );
                ui.label(
                    RichText::new("EXPERIMENTAL")
                        .size(10.0)
                        .strong()
                        .color(theme::ACCENT),
                );
            });

            ui.add_space(8.0);
            if !self.prefs.experimental_multiroom {
                ui.label(
                    RichText::new(
                        "Disabled by default. Enable Multiroom in the speaker panel when ready to validate PTP synchronization on real HomePods.",
                    )
                    .color(theme::MUTED),
                );
                return;
            }

            let names = self.selected_name_list();
            let leader = names.first().map(String::as_str).unwrap_or("Not selected");
            ui.columns(2, |columns| {
                metric(
                    &mut columns[0],
                    "Selected speakers",
                    &names.len().to_string(),
                    "2 or more required for group mode",
                );
                metric(
                    &mut columns[1],
                    "Group leader",
                    leader,
                    "Owns the primary PTP/BMCA connection",
                );
            });

            ui.add_space(10.0);
            ui.label(
                RichText::new(
                    "Engine path is already reserved: connect_group → SETPEERS → shared PTP timing → per-device RTP/retransmit. Keep this off for daily use until hardware validation passes.",
                )
                .size(11.5)
                .color(theme::MUTED),
            );
        });
    }

    fn draw_diagnostics_card(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Diagnostics")
                        .size(17.0)
                        .strong()
                        .color(theme::TEXT),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Clear log").clicked() {
                        self.logs.clear();
                    }
                });
            });

            if let Some(progress) = &self.progress {
                ui.add_space(10.0);
                ui.columns(4, |columns| {
                    metric(
                        &mut columns[0],
                        "Packets",
                        &progress.packets_sent.to_string(),
                        "RTP audio packets",
                    );
                    metric(
                        &mut columns[1],
                        "Retransmit",
                        &progress.retransmit_requested.to_string(),
                        &format!("fulfilled {}", progress.retransmit_fulfilled),
                    );
                    metric(
                        &mut columns[2],
                        "Underruns",
                        &progress.underruns.to_string(),
                        &format!(
                            "loss {:.3}% • drift {:+.1} ppm",
                            progress.loss_percent,
                            progress.drift_ppm
                        ),
                    );
                    metric(
                        &mut columns[3],
                        "Captured",
                        &progress.captured_chunks.to_string(),
                        &format!(
                            "silence {} • late {} • transitions {} • dropped {}",
                            progress.silence_chunks,
                            progress.late_polls,
                            progress.silence_transitions,
                            progress.dropped_chunks
                        ),
                    );
                });
            }

            ui.add_space(10.0);
            let log_height = (ui.available_height() - 8.0).max(150.0);
            egui::ScrollArea::vertical()
                .id_salt("diagnostic-log")
                .max_height(log_height)
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &self.logs {
                        ui.label(
                            RichText::new(line)
                                .monospace()
                                .size(11.0)
                                .color(theme::MUTED),
                        );
                    }
                });
        });
    }

    fn draw_footer(&self, ui: &mut egui::Ui) {
        ui.separator();
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("© 2026 SolYan · SolYan AirPlay2 v0.1.8 · Designed & developed by SolYan")
                    .size(10.5)
                    .color(theme::MUTED),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.hyperlink_to(
                    RichText::new("youtube.com/@SolYan-Music")
                        .size(10.5)
                        .color(theme::ACCENT),
                    "https://www.youtube.com/@SolYan-Music",
                );
            });
        });
    }
}

impl eframe::App for SolYanAirPlayApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_event(event);
        }

        if let Some(rx) = &self.progress_rx {
            while let Ok(progress) = rx.try_recv() {
                self.progress = Some(progress);
            }
        }

        if self.activity.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.painter()
            .rect_filled(ui.max_rect(), egui::CornerRadius::ZERO, theme::BG);

        ui.add_space(4.0);
        self.draw_header(ui);
        ui.add_space(12.0);

        let footer_height = 30.0;
        let body_height = (ui.available_height() - footer_height).max(420.0);

        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), body_height),
            Layout::left_to_right(Align::Min),
            |ui| {
                let available_height = ui.available_height();
                let device_panel_width = (ui.available_width() * 0.35).clamp(460.0, 650.0);

                ui.allocate_ui_with_layout(
                    egui::vec2(device_panel_width, available_height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        theme::sidebar_card().show(ui, |ui| {
                            ui.set_min_height((available_height - 4.0).max(200.0));
                            self.draw_sidebar(ui);
                        });
                    },
                );

                ui.add_space(10.0);

                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), available_height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        self.draw_stream_card(ui);
                        ui.add_space(10.0);
                        self.draw_controls_card(ui);
                        ui.add_space(10.0);
                        self.draw_multiroom_card(ui);
                        ui.add_space(10.0);

                        // Diagnostics receives every pixel left in the right pane.
                        let diag_height = ui.available_height().max(190.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), diag_height),
                            Layout::top_down(Align::Min),
                            |ui| {
                                theme::card().show(ui, |ui| {
                                    ui.set_min_height((diag_height - 4.0).max(180.0));
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new("Diagnostics")
                                                .size(17.0)
                                                .strong()
                                                .color(theme::TEXT),
                                        );
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            if ui.small_button("Clear log").clicked() {
                                                self.logs.clear();
                                            }
                                        });
                                    });

                                    if let Some(progress) = &self.progress {
                                        ui.add_space(10.0);
                                        ui.columns(4, |columns| {
                                            metric(
                                                &mut columns[0],
                                                "Packets",
                                                &progress.packets_sent.to_string(),
                                                "RTP audio packets",
                                            );
                                            metric(
                                                &mut columns[1],
                                                "Retransmit",
                                                &progress.retransmit_requested.to_string(),
                                                &format!("fulfilled {}", progress.retransmit_fulfilled),
                                            );
                                            metric(
                                                &mut columns[2],
                                                "Underruns",
                                                &progress.underruns.to_string(),
                                                &format!("loss {:.3}%", progress.loss_percent),
                                            );
                                            metric(
                                                &mut columns[3],
                                                "Captured",
                                                &progress.captured_chunks.to_string(),
                                                &format!(
                                                    "silence {} · late {} · transitions {} · dropped {}",
                                                    progress.silence_chunks,
                                                    progress.late_polls,
                                                    progress.silence_transitions,
                                                    progress.dropped_chunks
                                                ),
                                            );
                                        });
                                    }

                                    ui.add_space(10.0);
                                    egui::ScrollArea::vertical()
                                        .id_salt("diagnostic-log-full")
                                        .auto_shrink([false, false])
                                        .stick_to_bottom(true)
                                        .show(ui, |ui| {
                                            for line in &self.logs {
                                                ui.label(
                                                    RichText::new(line)
                                                        .monospace()
                                                        .size(11.0)
                                                        .color(theme::MUTED),
                                                );
                                            }
                                        });
                                });
                            },
                        );
                    },
                );
            },
        );

        self.draw_footer(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, PREFS_KEY, &self.prefs);
    }
}

fn metric(ui: &mut egui::Ui, label: &str, value: &str, detail: &str) {
    ui.label(
        RichText::new(label.to_uppercase())
            .size(10.0)
            .strong()
            .color(theme::MUTED),
    );
    ui.label(RichText::new(value).size(16.0).strong().color(theme::TEXT));
    ui.label(RichText::new(detail).size(10.5).color(theme::MUTED));
}

fn capability_badge(ui: &mut egui::Ui, label: &str, enabled: bool, experimental: bool) {
    let color = if experimental && enabled {
        theme::ACCENT
    } else if enabled {
        theme::GREEN
    } else {
        theme::MUTED
    };

    egui::Frame::new()
        .fill(color.gamma_multiply(if enabled { 0.12 } else { 0.07 }))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.42)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(7, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(9.5).strong().color(color));
        });
}


fn device_sort_rank(kind: DeviceKind) -> u8 {
    match kind {
        DeviceKind::HomePodMini => 0,
        DeviceKind::HomePod2 => 1,
        DeviceKind::HomePod1 => 2,
        DeviceKind::HomePodOther => 3,
        DeviceKind::AppleTv4K3 => 10,
        DeviceKind::AppleTv4K2 => 11,
        DeviceKind::AppleTv4K1 => 12,
        DeviceKind::AppleTvHd4 => 13,
        DeviceKind::AppleTv3 => 14,
        DeviceKind::AppleTv2 => 15,
        DeviceKind::AppleTvOther => 16,
        DeviceKind::AirPlaySpeaker => 30,
    }
}

fn device_icon(ui: &mut egui::Ui, kind: DeviceKind, selected: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let fg = if selected { theme::ACCENT } else { theme::TEXT };
    let soft = fg.gamma_multiply(0.16);
    let center = rect.center();

    if kind.is_homepod() {
        let size = if matches!(kind, DeviceKind::HomePodMini) {
            egui::vec2(31.0, 31.0)
        } else {
            egui::vec2(30.0, 39.0)
        };
        let body = egui::Rect::from_center_size(center, size);
        painter.rect_filled(
            body,
            egui::CornerRadius::same(if matches!(kind, DeviceKind::HomePodMini) { 12 } else { 10 }),
            soft,
        );
        painter.circle_filled(
            egui::pos2(center.x, body.top() + 5.5),
            3.2,
            fg.gamma_multiply(0.75),
        );
        painter.text(
            egui::pos2(center.x, center.y + 5.0),
            egui::Align2::CENTER_CENTER,
            kind.icon_text(),
            egui::FontId::proportional(8.5),
            fg,
        );
    } else if kind.is_apple_tv() {
        let body = egui::Rect::from_center_size(center, egui::vec2(38.0, 25.0));
        painter.rect_filled(body, egui::CornerRadius::same(7), soft);
        painter.circle_filled(
            egui::pos2(body.right() - 5.0, body.bottom() - 4.5),
            1.5,
            fg.gamma_multiply(0.8),
        );
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            kind.icon_text(),
            egui::FontId::proportional(9.0),
            fg,
        );
    } else {
        let body = egui::Rect::from_center_size(center, egui::vec2(28.0, 38.0));
        painter.rect_filled(body, egui::CornerRadius::same(7), soft);
        painter.circle_filled(egui::pos2(center.x, center.y - 7.0), 4.0, fg.gamma_multiply(0.7));
        painter.circle_filled(egui::pos2(center.x, center.y + 8.0), 7.0, fg.gamma_multiply(0.7));
        painter.text(
            egui::pos2(center.x, body.bottom() + 5.0),
            egui::Align2::CENTER_CENTER,
            kind.icon_text(),
            egui::FontId::proportional(7.5),
            fg,
        );
    }
}
