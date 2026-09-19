use crate::{theme, worker};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Stroke};
use serde::{Deserialize, Serialize};
use solyan_airplay_core::device_profile::DeviceKind;
use solyan_airplay_core::discovery::{detect_homepod_pairs, AirPlayReceiver, HomePodPair};
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
    experimental_multiroom: bool,
    last_receiver_id: Option<String>,
    vietnamese: bool,
    light_theme: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            volume: 0.80,
            render_delay_ms: 350,
            experimental_multiroom: false,
            last_receiver_id: None,
            vietnamese: true,
            light_theme: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activity {
    Idle,
    Scanning,
    AudioTest,
    ConnectionTest,
    PreparingStream,
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
            Self::PreparingStream => "Preparing",
            Self::Streaming => "Streaming",
            Self::Stopping => "Stopping",
        }
    }

    fn is_streaming(self) -> bool {
        matches!(self, Self::PreparingStream | Self::Streaming | Self::Stopping)
    }

    fn is_busy(self) -> bool {
        !matches!(self, Self::Idle)
    }
}

pub struct SolYanAirPlayApp {
    prefs: Preferences,
    devices: Vec<AirPlayReceiver>,
    homepod_pairs: Vec<HomePodPair>,
    selected_pair_id: Option<String>,
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
    logo_light_texture: egui::TextureHandle,
    logo_dark_texture: egui::TextureHandle,
    startup_window_forced: bool,
}

impl SolYanAirPlayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let prefs: Preferences = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, PREFS_KEY))
            .unwrap_or_default();

        theme::apply(&cc.egui_ctx, prefs.light_theme);

        let load_logo = |name: &str, png: &[u8]| -> Option<egui::TextureHandle> {
            let icon = eframe::icon_data::from_png_bytes(png).ok()?;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            Some(cc.egui_ctx.load_texture(name, image, egui::TextureOptions::LINEAR))
        };

        // Startup must never depend on decorative assets being perfect.
        // The main/light logo is also validated by build.rs for the Windows icon.
        let logo_light_texture = load_logo(
            "solyan-airplay-logo-light",
            include_bytes!("../assets/solyan-airplay-logo.png"),
        )
        .unwrap_or_else(|| {
            let image = egui::ColorImage::new(
                [1, 1],
                vec![theme::ACCENT],
            );
            cc.egui_ctx.load_texture(
                "solyan-airplay-logo-fallback",
                image,
                egui::TextureOptions::LINEAR,
            )
        });

        let logo_dark_texture = load_logo(
            "solyan-airplay-logo-dark",
            include_bytes!("../assets/solyan-airplay-logo-dark.png"),
        )
        .unwrap_or_else(|| logo_light_texture.clone());

        let (event_tx, event_rx) = unbounded();
        let mut app = Self {
            prefs,
            devices: Vec::new(),
            homepod_pairs: Vec::new(),
            selected_pair_id: None,
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
            logo_light_texture,
            logo_dark_texture,
            startup_window_forced: false,
        };

        app.log("SolYan AirPlay2 v0.2.13 GUI initialized.");
        app.start_scan(cc.egui_ctx.clone());
        app
    }

    fn log(&mut self, line: impl Into<String>) {
        if self.logs.len() >= 250 {
            self.logs.pop_front();
        }
        self.logs.push_back(line.into());
    }

    fn tr<'a>(&self, en: &'a str, vi: &'a str) -> &'a str {
        if self.prefs.vietnamese { vi } else { en }
    }

    fn export_log(&mut self) {
        let body = self.logs.iter().cloned().collect::<Vec<_>>().join("\r\n");
        let base = std::env::var("USERPROFILE")
            .map(std::path::PathBuf::from)
            .or_else(|_| std::env::current_dir())
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        let desktop = base.join("Desktop");
        let dir = if desktop.is_dir() { desktop } else { base };
        let path = dir.join("SolYan-AirPlay2-v0.2.13-log.txt");
        match std::fs::write(&path, body) {
            Ok(()) => {
                self.status = if self.prefs.vietnamese {
                    format!("Đã xuất log: {}", path.display())
                } else {
                    format!("Log exported: {}", path.display())
                };
            }
            Err(err) => {
                self.last_error = Some(if self.prefs.vietnamese {
                    format!("Không thể xuất log: {err}")
                } else {
                    format!("Could not export log: {err}")
                });
            }
        }
    }

    fn start_scan(&mut self, ctx: egui::Context) {
        if self.activity.is_streaming() {
            return;
        }
        self.activity = Activity::Scanning;
        self.status = self.tr("Scanning for AirPlay devices...", "Đang tìm thiết bị AirPlay...").into();
        self.last_error = None;
        self.log("Scanning mDNS _airplay._tcp.local...");
        worker::spawn_scan(ctx, self.event_tx.clone());
    }

    fn start_capture_test(&mut self, ctx: egui::Context) {
        if self.activity != Activity::Idle {
            return;
        }
        self.activity = Activity::AudioTest;
        self.status = self.tr("Testing Windows audio...", "Đang kiểm tra âm thanh Windows...").into();
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
        self.status = self.tr("Testing AirPlay connection...", "Đang kiểm tra kết nối AirPlay...").into();
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

        let pair_selected = self.selected_pair_id.is_some();
        let mode = if pair_selected {
            if self.selected_ids.len() != 2 {
                self.last_error = Some(
                    "HomePod stereo pair requires exactly two discovered members.".into(),
                );
                return;
            }
            LiveStreamMode::MultiroomExperimental
        } else if self.prefs.experimental_multiroom && self.selected_ids.len() >= 2 {
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
        self.activity = Activity::PreparingStream;
        self.last_error = None;

        let target_count = self.selected_ids.len();
        self.status = if self.selected_pair_id.is_some() {
            self.tr(
                "Preparing HomePod stereo pair — checking PTP, audio capture and packet flow...",
                "Đang chuẩn bị cặp HomePod — kiểm tra PTP, thu âm và luồng packet...",
            ).into()
        } else if mode == LiveStreamMode::MultiroomExperimental {
            format!(
                "{} {target_count} {}",
                self.tr("Preparing PTP group for", "Đang chuẩn bị nhóm PTP cho"),
                self.tr("speakers — waiting for audio readiness...", "loa — chờ hệ thống âm thanh sẵn sàng...")
            )
        } else {
            self.tr(
                "Preparing AirPlay stream — checking session, WASAPI and packet flow...",
                "Đang chuẩn bị luồng AirPlay — kiểm tra session, WASAPI và packet...",
            ).into()
        };

        let effective_render_delay_ms = self.prefs.render_delay_ms;

        self.log(match mode {
            LiveStreamMode::Single => {
                format!(
                    "Starting single-speaker stream (render lead {} ms).",
                    effective_render_delay_ms
                )
            }
            LiveStreamMode::MultiroomExperimental => {
                if let Some(pair_id) = &self.selected_pair_id {
                    format!(
                        "Starting HomePod stereo pair route {} with {} members using PTP.",
                        pair_id, target_count
                    )
                } else {
                    format!(
                        "Starting EXPERIMENTAL multiroom stream to {target_count} speakers using PTP."
                    )
                }
            },
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
            self.status = self.tr("Stopping stream...", "Đang dừng phát...").into();
            self.log("Stop requested.");
        }
    }

    fn set_selected(&mut self, id: String) {
        self.selected_pair_id = None;
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
        if let Some(pair_id) = &self.selected_pair_id {
            if let Some(pair) = self.homepod_pairs.iter().find(|p| &p.id == pair_id) {
                return vec![pair.name.clone()];
            }
        }

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

    fn set_selected_pair(&mut self, pair_id: String) {
        let Some(pair) = self.homepod_pairs.iter().find(|p| p.id == pair_id) else {
            return;
        };

        self.selected_pair_id = Some(pair.id.clone());
        self.selected_ids = pair.member_ids.clone();
        self.prefs.last_receiver_id = Some(pair.leader_id.clone());
        self.log(format!(
            "HomePod stereo pair selected: {} [{}].",
            pair.name,
            pair.member_names.join(" + ")
        ));
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
                        self.homepod_pairs = detect_homepod_pairs(&devices);

                        let group_debug = devices
                            .iter()
                            .filter(|d| d.device_kind().is_homepod())
                            .map(|d| {
                                format!(
                                    "PAIR META: '{}' id={} gid={} pgid={} tsid={} gpn={} igl={} gcgl={} pgcgl={} hgid={} household={}",
                                    d.name,
                                    d.id,
                                    d.group_id.as_deref().unwrap_or("-"),
                                    d.parent_group_id.as_deref().unwrap_or("-"),
                                    d.tight_sync_id.as_deref().unwrap_or("-"),
                                    d.group_public_name.as_deref().unwrap_or("-"),
                                    d.is_group_leader,
                                    d.group_contains_discoverable_leader,
                                    d.parent_group_contains_discoverable_leader,
                                    d.home_group_id.as_deref().unwrap_or("-"),
                                    d.household_id.as_deref().unwrap_or("-"),
                                )
                            })
                            .collect::<Vec<_>>();
                        for line in group_debug {
                            self.log(line);
                        }

                        self.devices = devices;

                        if let Some(pair_id) = &self.selected_pair_id {
                            if !self.homepod_pairs.iter().any(|p| &p.id == pair_id) {
                                self.selected_pair_id = None;
                            }
                        }

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
                        if self.selected_pair_id.is_none()
                            && !self.prefs.experimental_multiroom
                            && self.selected_ids.len() > 1
                        {
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
                                "{} AirPlay device(s) — {} HomePod, {} Apple TV, {} HomePod pair(s).",
                                self.devices.len(),
                                homepods,
                                apple_tvs,
                                self.homepod_pairs.len()
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
            let logo = if self.prefs.light_theme {
                &self.logo_light_texture
            } else {
                &self.logo_dark_texture
            };
            ui.image((logo.id(), egui::vec2(46.0, 46.0)));
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("SolYan AirPlay2")
                        .size(26.0)
                        .strong()
                        .color(theme::text()),
                );
                ui.label(
                    RichText::new("Stream Beyond Boundaries · Native Windows AirPlay 2")
                        .size(13.0)
                        .color(theme::muted()),
                );
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (text, color) = if self.last_error.is_some() {
                    (self.tr("Attention", "Chú ý"), theme::RED)
                } else if self.activity == Activity::Streaming {
                    (self.tr("Live", "Đang phát"), theme::GREEN)
                } else if self.activity == Activity::PreparingStream {
                    (self.tr("Preparing", "Đang chuẩn bị"), theme::BLUE)
                } else if self.activity == Activity::Stopping {
                    (self.tr("Stopping", "Đang dừng"), theme::ACCENT)
                } else if self.activity.is_busy() {
                    (self.activity.label(), theme::BLUE)
                } else {
                    (self.tr("Ready", "Sẵn sàng"), theme::GREEN)
                };

                egui::Frame::new()
                    .fill(color.gamma_multiply(0.16))
                    .stroke(Stroke::new(1.0, color.gamma_multiply(0.55)))
                    .corner_radius(egui::CornerRadius::same(20))
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).color(color).strong());
                    });

                if ui.small_button(self.tr("Export log", "Xuất log TXT")).clicked() {
                    self.export_log();
                }

                egui::Frame::new()
                    .fill(theme::accent_soft().gamma_multiply(0.72))
                    .stroke(Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.75)))
                    .corner_radius(egui::CornerRadius::same(16))
                    .inner_margin(egui::Margin::symmetric(4, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            let vi_fill = if self.prefs.vietnamese { theme::ACCENT } else { theme::card_color() };
                            let en_fill = if self.prefs.vietnamese { theme::card_color() } else { theme::ACCENT };
                            let vi_text = if self.prefs.vietnamese { Color32::WHITE } else { theme::muted() };
                            let en_text = if self.prefs.vietnamese { theme::muted() } else { Color32::WHITE };

                            if ui.add(
                                egui::Button::new(RichText::new("VI").strong().color(vi_text))
                                    .fill(vi_fill)
                                    .stroke(Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.5)))
                                    .corner_radius(egui::CornerRadius::same(12))
                                    .min_size(egui::vec2(38.0, 28.0)),
                            ).clicked() {
                                self.prefs.vietnamese = true;
                            }

                            if ui.add(
                                egui::Button::new(RichText::new("ENG").strong().color(en_text))
                                    .fill(en_fill)
                                    .stroke(Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.5)))
                                    .corner_radius(egui::CornerRadius::same(12))
                                    .min_size(egui::vec2(48.0, 28.0)),
                            ).clicked() {
                                self.prefs.vietnamese = false;
                            }
                        });
                    });

                egui::Frame::new()
                    .fill(theme::sidebar())
                    .stroke(Stroke::new(1.0, theme::border()))
                    .corner_radius(egui::CornerRadius::same(16))
                    .inner_margin(egui::Margin::symmetric(4, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;

                            let light_fill = if self.prefs.light_theme { theme::ACCENT } else { theme::card_color() };
                            let dark_fill = if self.prefs.light_theme { theme::card_color() } else { theme::ACCENT };
                            let light_text = if self.prefs.light_theme { Color32::WHITE } else { theme::muted() };
                            let dark_text = if self.prefs.light_theme { theme::muted() } else { Color32::WHITE };

                            if ui.add(
                                egui::Button::new(RichText::new(self.tr("Light", "Sáng")).strong().color(light_text))
                                    .fill(light_fill)
                                    .stroke(Stroke::new(1.0, theme::border()))
                                    .corner_radius(egui::CornerRadius::same(12))
                                    .min_size(egui::vec2(52.0, 28.0)),
                            ).clicked() && !self.prefs.light_theme {
                                self.prefs.light_theme = true;
                                theme::apply(ui.ctx(), true);
                            }

                            if ui.add(
                                egui::Button::new(RichText::new(self.tr("Dark", "Tối")).strong().color(dark_text))
                                    .fill(dark_fill)
                                    .stroke(Stroke::new(1.0, theme::border()))
                                    .corner_radius(egui::CornerRadius::same(12))
                                    .min_size(egui::vec2(48.0, 28.0)),
                            ).clicked() && self.prefs.light_theme {
                                self.prefs.light_theme = false;
                                theme::apply(ui.ctx(), false);
                            }
                        });
                    });
            });
        });

        ui.add_space(6.0);
        ui.label(RichText::new(&self.status).color(theme::muted()));
        if let Some(error) = &self.last_error {
            ui.add_space(3.0);
            ui.label(RichText::new(error).color(theme::RED));
        }
    }

    fn draw_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.tr("AIRPLAY DEVICES", "THIẾT BỊ AIRPLAY")).size(12.0).strong().color(theme::muted()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let enabled = !self.activity.is_streaming();
                if ui
                    .add_enabled(enabled, egui::Button::new(self.tr("Scan", "Quét")))
                    .clicked()
                {
                    self.start_scan(ui.ctx().clone());
                }
            });
        });

        ui.label(
            RichText::new(self.tr(
                "Single devices. Select one, then press Start.",
                "Thiết bị đơn. Chọn thiết bị rồi nhấn Chạy.",
            ))
            .size(10.5)
            .color(theme::muted()),
        );
        ui.add_space(8.0);

        let mut clicked_id: Option<String> = None;
        // Keep the device list usable at both low and high resolutions.
        // The whole sidebar can also scroll, so this list must never consume
        // an unbounded parent height.
        let viewport_h = ui.ctx().content_rect().height();
        let list_height = (viewport_h * 0.42).clamp(160.0, 390.0);
        egui::ScrollArea::vertical()
            .id_salt("speaker-list")
            .auto_shrink([false, false])
            .max_height(list_height)
            .show(ui, |ui| {
                if self.devices.is_empty() {
                    theme::sidebar_card().show(ui, |ui| {
                        ui.label(RichText::new(self.tr("No receiver yet", "Chưa có thiết bị")).strong().color(theme::text()));
                        ui.label(
                            RichText::new(self.tr("Scan to find HomePod and AirPlay devices.", "Nhấn Quét để tìm HomePod và thiết bị AirPlay."))
                                .size(12.0)
                                .color(theme::muted()),
                        );
                    });
                }

                for (index, device) in self.devices.iter().enumerate() {
                    let selected = self.selected_pair_id.is_none()
                        && self.selected_ids.iter().any(|id| id == &device.id);
                    let fill = if selected {
                        theme::accent_soft()
                    } else {
                        theme::sidebar()
                    };
                    let border = if selected { theme::ACCENT } else { theme::border() };
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
                                            .color(theme::text()),
                                    );
                                    ui.label(
                                        RichText::new(device.friendly_model_name())
                                            .size(12.0)
                                            .strong()
                                            .color(if selected { theme::ACCENT } else { theme::text() }),
                                    );
                                    ui.label(
                                        RichText::new(format!("{}  •  {}", device.model, ip))
                                            .size(10.5)
                                            .color(theme::muted()),
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
                        .interact(Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand);

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
        ui.add_space(10.0);
        self.draw_homepod_pair_section(ui);

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        let old = self.prefs.experimental_multiroom;
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut self.prefs.experimental_multiroom,
                RichText::new("Multiroom").strong().color(theme::text()),
            );
            ui.label(
                RichText::new("EXPERIMENTAL")
                    .size(9.5)
                    .strong()
                    .color(theme::ACCENT),
            );
        });
        ui.label(
            RichText::new(self.tr("Select 2+ devices. First one is the leader.", "Chọn từ 2 thiết bị. Thiết bị đầu tiên là trưởng nhóm."))
                .size(11.5)
                .color(theme::muted()),
        );

        if self.selected_pair_id.is_none()
            && old
            && !self.prefs.experimental_multiroom
            && self.selected_ids.len() > 1
        {
            self.selected_ids.truncate(1);
        }
    }


    fn draw_homepod_pair_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(self.tr("HOMEPOD PAIR", "CẶP HOMEPOD"))
                    .size(12.0)
                    .strong()
                    .color(theme::muted()),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let enabled = !self.activity.is_streaming();
                if ui
                    .add_enabled(enabled, egui::Button::new(self.tr("Search Pair", "Tìm cặp")))
                    .clicked()
                {
                    self.log("Searching Apple group metadata (tsid / pgid / gid) for HomePod pairs.");
                    self.start_scan(ui.ctx().clone());
                }
            });
        });

        ui.label(
            RichText::new(self.tr(
                "Stereo HomePod pair from the Home app.",
                "Cặp HomePod Stereo đã ghép trong ứng dụng Home.",
            ))
            .size(10.5)
            .color(theme::muted()),
        );
        ui.add_space(7.0);
        let mut clicked_pair_id: Option<String> = None;

        if self.homepod_pairs.is_empty() {
            egui::Frame::new()
                .fill(theme::sidebar())
                .stroke(Stroke::new(1.0, theme::border()))
                .corner_radius(egui::CornerRadius::same(12))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(self.tr("No HomePod stereo pair detected", "Chưa tìm thấy cặp HomePod Stereo"))
                            .size(12.0)
                            .strong()
                            .color(theme::text()),
                    );
                    ui.label(
                        RichText::new(self.tr(
                            "Stereo pairs from the Home app. Select a pair to stream in stereo.",
                            "Cặp HomePod Stereo từ ứng dụng Home. Chọn cặp để phát stereo.",
                        ))
                        .size(10.0)
                        .color(theme::muted()),
                    );
                });
        } else {
            for pair in &self.homepod_pairs {
                let selected = self.selected_pair_id.as_deref() == Some(pair.id.as_str());
                let fill = if selected { theme::accent_soft() } else { theme::sidebar() };
                let border = if selected { theme::ACCENT } else { theme::border() };

                let response = egui::Frame::new()
                    .fill(fill)
                    .stroke(Stroke::new(1.0, border))
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            stereo_pair_icon(ui, selected);
                            ui.add_space(10.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(&pair.name)
                                        .size(14.5)
                                        .strong()
                                        .color(theme::text()),
                                );
                                ui.label(
                                    RichText::new(pair.member_names.join("  +  "))
                                        .size(10.5)
                                        .color(theme::muted()),
                                );
                                ui.horizontal_wrapped(|ui| {
                                    capability_badge(ui, "STEREO PAIR", true, true);
                                    capability_badge(ui, "PTP", true, true);
                                    if pair.tight_sync_id.is_some() {
                                        capability_badge(ui, "TIGHT SYNC", true, false);
                                    } else if pair.parent_group_id.is_some() {
                                        capability_badge(ui, "PARENT GROUP", true, false);
                                    }
                                });
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if selected {
                                    ui.label(
                                        RichText::new("SELECTED")
                                            .size(9.5)
                                            .strong()
                                            .color(theme::ACCENT),
                                    );
                                }
                            });
                        });
                    })
                    .response
                    .interact(Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);

                if response.clicked() && !self.activity.is_streaming() {
                    clicked_pair_id = Some(pair.id.clone());
                }
                ui.add_space(7.0);
            }
        }

        if let Some(pair_id) = clicked_pair_id {
            self.set_selected_pair(pair_id);
        }
    }

    fn draw_stream_card(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            let narrow = ui.available_width() < 660.0;

            if narrow {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(if self.activity == Activity::PreparingStream {
                            self.tr("PREPARING STREAM", "ĐANG CHUẨN BỊ")
                        } else if self.activity == Activity::Streaming {
                            self.tr("NOW STREAMING", "ĐANG PHÁT")
                        } else {
                            self.tr("AIRPLAY STREAM", "PHÁT AIRPLAY")
                        })
                            .size(11.0).strong().color(theme::muted()),
                    );
                    let names = self.selected_name_list();
                    let title = if names.is_empty() {
                        self.tr("No speaker selected", "Chưa chọn thiết bị").to_string()
                    } else {
                        names.join("  +  ")
                    };
                    ui.label(RichText::new(title).size(21.0).strong().color(theme::text()));
                    ui.add_space(6.0);
                    self.draw_start_stop_button(ui);
                });
            } else {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(if self.activity == Activity::PreparingStream {
                            self.tr("PREPARING STREAM", "ĐANG CHUẨN BỊ")
                        } else if self.activity == Activity::Streaming {
                            self.tr("NOW STREAMING", "ĐANG PHÁT")
                        } else {
                            self.tr("AIRPLAY STREAM", "PHÁT AIRPLAY")
                        })
                                .size(11.0).strong().color(theme::muted()),
                        );
                        let names = self.selected_name_list();
                        let title = if names.is_empty() {
                            self.tr("No speaker selected", "Chưa chọn thiết bị").to_string()
                        } else {
                            names.join("  +  ")
                        };
                        ui.label(RichText::new(title).size(22.0).strong().color(theme::text()));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.draw_start_stop_button(ui);
                    });
                });
            }

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(12.0);

            let protocol = if self.selected_pair_id.is_some()
                || (self.prefs.experimental_multiroom && self.selected_ids.len() >= 2)
            {
                "ALAC / PTP"
            } else {
                "ALAC / NTP"
            };
            let timing = if self.selected_pair_id.is_some() {
                "HomePod stereo-pair timing"
            } else if protocol.contains("PTP") {
                "Experimental group timing"
            } else {
                "Realtime compatibility path"
            };

            if ui.available_width() < 620.0 {
                metric(ui, "Source", "Windows system audio", "WASAPI loopback");
                ui.add_space(8.0);
                metric(ui, "Protocol", protocol, timing);
                ui.add_space(8.0);
                metric(
                    ui,
                    "Render lead",
                    &format!("{} ms", self.prefs.render_delay_ms),
                    "Retransmit headroom",
                );
            } else {
                ui.columns(3, |columns| {
                    metric(&mut columns[0], "Source", "Windows system audio", "WASAPI loopback");
                    metric(&mut columns[1], "Protocol", protocol, timing);
                    metric(
                        &mut columns[2],
                        "Render lead",
                        &format!("{} ms", self.prefs.render_delay_ms),
                        "Retransmit headroom",
                    );
                });
            }
        });
    }

    fn draw_start_stop_button(&mut self, ui: &mut egui::Ui) {
        if self.activity.is_streaming() {
            let stop = ui.add(
                egui::Button::new(
                    RichText::new(self.tr("Stop", "Dừng")).strong().color(Color32::WHITE),
                )
                .fill(theme::RED.gamma_multiply(0.86))
                .stroke(Stroke::new(1.5, theme::RED))
                .corner_radius(egui::CornerRadius::same(9))
                .min_size(egui::vec2(122.0, 42.0)),
            );
            if stop.clicked()
                && matches!(self.activity, Activity::PreparingStream | Activity::Streaming)
            {
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
                    RichText::new(self.tr("Start", "Chạy")).strong().color(Color32::WHITE),
                )
                .fill(theme::ACCENT)
                .stroke(Stroke::new(1.5, theme::ACCENT))
                .corner_radius(egui::CornerRadius::same(9))
                .min_size(egui::vec2(148.0, 42.0)),
            );
            if start.clicked() {
                self.start_stream(ui.ctx().clone());
            }
        }
    }

    fn draw_controls_card(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.label(
                RichText::new(self.tr("Playback", "Điều khiển"))
                    .size(17.0).strong().color(theme::text()),
            );
            ui.label(
                RichText::new(self.tr(
                    "Adjust volume and latency. Use 200 ms for faster response; increase to 350–500 ms for more stability.",
                    "Điều chỉnh âm lượng và độ trễ. 200 ms cho phản hồi nhanh; tăng 350–500 ms nếu cần ổn định hơn.",
                ))
                .size(10.5)
                .color(theme::muted()),
            );
            ui.add_space(10.0);

            ui.label(
                RichText::new(self.tr("Volume", "Âm lượng"))
                    .size(12.0).strong().color(theme::text()),
            );
            ui.horizontal_wrapped(|ui| {
                let slider_w = (ui.available_width() - 100.0).clamp(180.0, 420.0);
                let response = ui.scope(|ui| {
                    ui.spacing_mut().slider_width = slider_w;
                    let visuals = ui.visuals_mut();
                    visuals.slider_trailing_fill = true;
                    visuals.widgets.inactive.bg_fill = theme::control_bg();
                    visuals.widgets.inactive.bg_stroke = Stroke::new(1.2, theme::control_border());
                    visuals.widgets.hovered.bg_fill = theme::accent_soft();
                    visuals.widgets.hovered.bg_stroke = Stroke::new(1.8, theme::ACCENT);
                    visuals.widgets.active.bg_fill = theme::ACCENT;
                    ui.add(
                        egui::Slider::new(&mut self.prefs.volume, 0.0..=1.0)
                            .show_value(false),
                    )
                }).inner
                  .on_hover_text(self.tr("Drag to change volume", "Kéo để thay đổi âm lượng"));

                egui::Frame::new()
                    .fill(theme::accent_soft())
                    .stroke(Stroke::new(1.2, theme::ACCENT))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!("{:.0}%", self.prefs.volume * 100.0))
                                .size(14.0).strong().color(theme::ACCENT),
                        );
                    });

                if response.changed() {
                    if let Some(control) = &self.stream_control {
                        control.set_volume(self.prefs.volume);
                    }
                }
            });

            ui.add_space(10.0);
            ui.label(
                RichText::new(self.tr("Latency", "Độ trễ"))
                    .size(12.0).strong().color(theme::text()),
            );

            let enabled = !self.activity.is_streaming();
            ui.add_enabled_ui(enabled, |ui| {
                let slider_w = ui.available_width().clamp(200.0, 420.0);
                ui.scope(|ui| {
                    ui.spacing_mut().slider_width = slider_w;
                    let visuals = ui.visuals_mut();
                    visuals.slider_trailing_fill = true;
                    visuals.widgets.inactive.bg_fill = theme::control_bg();
                    visuals.widgets.inactive.bg_stroke = Stroke::new(1.2, theme::control_border());
                    visuals.widgets.hovered.bg_fill = theme::accent_soft();
                    visuals.widgets.hovered.bg_stroke = Stroke::new(1.8, theme::ACCENT);
                    visuals.widgets.active.bg_fill = theme::ACCENT;
                    ui.add(
                        egui::Slider::new(&mut self.prefs.render_delay_ms, 0..=600)
                            .suffix(" ms"),
                    )
                    .on_hover_text(self.tr("Drag to change latency", "Kéo để thay đổi độ trễ"));
                });

                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    let preset = |ui: &mut egui::Ui, label: &str, selected: bool| {
                        let fill = if selected { theme::accent_soft() } else { theme::control_bg() };
                        let stroke = if selected { theme::ACCENT } else { theme::control_border() };
                        ui.add(
                            egui::Button::new(RichText::new(label).strong().color(theme::text()))
                                .fill(fill)
                                .stroke(Stroke::new(1.2, stroke))
                                .corner_radius(egui::CornerRadius::same(8))
                                .min_size(egui::vec2(86.0, 32.0)),
                        )
                    };
                    if preset(ui, "Low 200", self.prefs.render_delay_ms == 200).clicked() {
                        self.prefs.render_delay_ms = 200;
                    }
                    if preset(ui, "Stable 350", self.prefs.render_delay_ms == 350).clicked() {
                        self.prefs.render_delay_ms = 350;
                    }
                    if preset(ui, "Safe 500", self.prefs.render_delay_ms == 500).clicked() {
                        self.prefs.render_delay_ms = 500;
                    }
                });
            });

            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                let enabled = self.activity == Activity::Idle;
                let test_audio = egui::Button::new(
                    RichText::new(self.tr("Test Windows audio", "Test âm thanh")).strong(),
                )
                .fill(theme::control_bg())
                .stroke(Stroke::new(1.2, theme::control_border()))
                .corner_radius(egui::CornerRadius::same(8))
                .min_size(egui::vec2(128.0, 34.0));
                if ui.add_enabled(enabled, test_audio).clicked() {
                    self.start_capture_test(ui.ctx().clone());
                }

                let test_airplay = egui::Button::new(
                    RichText::new(self.tr("Test AirPlay session", "Test AirPlay")).strong(),
                )
                .fill(theme::control_bg())
                .stroke(Stroke::new(1.2, theme::control_border()))
                .corner_radius(egui::CornerRadius::same(8))
                .min_size(egui::vec2(116.0, 34.0));
                if ui
                    .add_enabled(enabled && !self.selected_ids.is_empty(), test_airplay)
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
                        .color(theme::text()),
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
                    RichText::new(self.tr("Enable to stream to multiple devices.", "Bật để phát tới nhiều thiết bị."))
                    .color(theme::muted()),
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
                RichText::new(self.tr("PTP synchronized group playback.", "Phát nhóm đồng bộ bằng PTP."))
                .size(11.5)
                .color(theme::muted()),
            );
        });
    }

    fn draw_footer(&self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(theme::sidebar())
            .stroke(Stroke::new(1.0, theme::border()))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(12, 7))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("© 2026 SolYan")
                            .size(12.0)
                            .strong()
                            .color(theme::ACCENT),
                    );
                    ui.label(
                        RichText::new("· SolYan AirPlay2 v0.2.13")
                            .size(11.5)
                            .strong()
                            .color(theme::text()),
                    );
                    ui.label(
                        RichText::new(self.tr("· Developer: SolYan ·", "· Tác giả: SolYan ·"))
                            .size(11.5)
                            .strong()
                            .color(theme::text()),
                    );
                    ui.hyperlink_to(
                        RichText::new("https://www.youtube.com/@SolYan-Music")
                            .size(11.5)
                            .strong()
                            .color(theme::ACCENT),
                        "https://www.youtube.com/@SolYan-Music",
                    );
                });
            });
    }
}

impl eframe::App for SolYanAirPlayApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.startup_window_forced {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            self.startup_window_forced = true;
        }

        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_event(event);
        }

        if let Some(rx) = self.progress_rx.clone() {
            while let Ok(progress) = rx.try_recv() {
                if self.activity == Activity::PreparingStream
                    && progress.captured_chunks > 0
                    && progress.packets_sent > 0
                {
                    self.activity = Activity::Streaming;
                    self.status = self.tr(
                        "AirPlay ready — audio capture and packet flow verified.",
                        "AirPlay đã sẵn sàng — đã xác nhận thu âm và luồng packet.",
                    ).into();
                    self.log(format!(
                        "STREAM READY: captured_chunks={}, packets_sent={}, targets={}.",
                        progress.captured_chunks,
                        progress.packets_sent,
                        progress.target_count
                    ));
                }
                self.progress = Some(progress);
            }
        }

        if self.activity.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.painter()
            .rect_filled(ui.max_rect(), egui::CornerRadius::ZERO, theme::bg());

        ui.add_space(4.0);
        self.draw_header(ui);
        ui.add_space(10.0);

        // Footer is fixed. Everything above it gets its own viewport and can
        // scroll instead of being clipped on 720p / small windows.
        let available = ui.available_rect_before_wrap();
        let footer_height = 44.0f32;
        let footer_gap = 8.0f32;
        let footer_top = (available.bottom() - footer_height).max(available.top());
        let body_bottom = (footer_top - footer_gap).max(available.top());

        let body_rect = egui::Rect::from_min_max(
            available.min,
            egui::pos2(available.right(), body_bottom),
        );
        let footer_rect = egui::Rect::from_min_max(
            egui::pos2(available.left(), footer_top),
            available.max,
        );

        let draw_right = |this: &mut SolYanAirPlayApp, ui: &mut egui::Ui| {
            this.draw_stream_card(ui);
            ui.add_space(10.0);
            this.draw_controls_card(ui);
            ui.add_space(10.0);
            this.draw_multiroom_card(ui);
            ui.add_space(10.0);
            egui::Frame::new()
                .fill(theme::accent_soft().gamma_multiply(0.45))
                .stroke(Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.35)))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(this.tr(
                            "AirPlay may introduce latency or A/V sync offset due to protocol buffering and synchronization.",
                            "AirPlay có thể xảy ra trễ hoặc lệch tiếng/hình do cơ chế đệm và đồng bộ của giao thức.",
                        ))
                        .size(10.5)
                        .color(theme::muted()),
                    );
                });
            ui.add_space(6.0);
        };

        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(body_rect)
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                let body_w = body_rect.width();
                let body_h = body_rect.height();

                // At narrow widths use one natural vertical page. At normal desktop
                // widths keep the two-column design with independent scrolling.
                if body_w < 980.0 {
                    egui::ScrollArea::vertical()
                        .id_salt("responsive-single-column")
                        .auto_shrink([false, false])
                        .max_height(body_h)
                        .show(ui, |ui| {
                            theme::sidebar_card().show(ui, |ui| {
                                self.draw_sidebar(ui);
                            });
                            ui.add_space(10.0);
                            draw_right(self, ui);
                        });
                } else {
                    ui.horizontal(|ui| {
                        let sidebar_w = (body_w * 0.34).clamp(340.0, 560.0);
                        let right_w = (body_w - sidebar_w - 10.0).max(420.0);

                        ui.allocate_ui_with_layout(
                            egui::vec2(sidebar_w, body_h),
                            Layout::top_down(Align::Min),
                            |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("sidebar-viewport")
                                    .auto_shrink([false, false])
                                    .max_height(body_h)
                                    .show(ui, |ui| {
                                        theme::sidebar_card().show(ui, |ui| {
                                            self.draw_sidebar(ui);
                                        });
                                        ui.add_space(4.0);
                                    });
                            },
                        );

                        ui.add_space(10.0);

                        ui.allocate_ui_with_layout(
                            egui::vec2(right_w, body_h),
                            Layout::top_down(Align::Min),
                            |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("main-viewport")
                                    .auto_shrink([false, false])
                                    .max_height(body_h)
                                    .show(ui, |ui| {
                                        draw_right(self, ui);
                                    });
                            },
                        );
                    });
                }
            },
        );

        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(footer_rect)
                .layout(Layout::top_down(Align::Min)),
            |ui| self.draw_footer(ui),
        );
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
            .color(theme::muted()),
    );
    ui.label(RichText::new(value).size(16.0).strong().color(theme::text()));
    ui.label(RichText::new(detail).size(10.5).color(theme::muted()));
}

fn capability_badge(ui: &mut egui::Ui, label: &str, enabled: bool, experimental: bool) {
    let color = if experimental && enabled {
        theme::ACCENT
    } else if enabled {
        theme::GREEN
    } else {
        theme::muted()
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



fn stereo_pair_icon(ui: &mut egui::Ui, selected: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(58.0, 48.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let fg = if selected { theme::ACCENT } else { theme::text() };
    let soft = fg.gamma_multiply(0.14);
    let center = rect.center();

    let left = egui::Rect::from_center_size(
        egui::pos2(center.x - 10.0, center.y),
        egui::vec2(25.0, 34.0),
    );
    let right = egui::Rect::from_center_size(
        egui::pos2(center.x + 10.0, center.y),
        egui::vec2(25.0, 34.0),
    );

    painter.rect_filled(left, egui::CornerRadius::same(9), soft);
    painter.rect_filled(right, egui::CornerRadius::same(9), soft);
    painter.circle_filled(
        egui::pos2(left.center().x, left.top() + 5.0),
        2.5,
        fg.gamma_multiply(0.78),
    );
    painter.circle_filled(
        egui::pos2(right.center().x, right.top() + 5.0),
        2.5,
        fg.gamma_multiply(0.78),
    );
    painter.text(
        egui::pos2(center.x, center.y + 4.0),
        egui::Align2::CENTER_CENTER,
        "PAIR",
        egui::FontId::proportional(8.0),
        fg,
    );
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
    let fg = if selected { theme::ACCENT } else { theme::text() };
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
