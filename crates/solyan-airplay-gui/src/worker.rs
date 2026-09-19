use audio_capture::{start_default_loopback, AudioFormat};
use crossbeam_channel::Sender;
use eframe::egui;
use solyan_airplay_core::discovery::{discover_once, AirPlayReceiver};
use solyan_airplay_core::live::{
    run_live_stream, LiveStreamMode, LiveStreamResult, StreamControl, StreamProgress,
};
use solyan_airplay_core::session::{connect_test, SessionTestResult};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct CaptureProbe {
    pub device_name: String,
    pub chunks: u64,
    pub peak: i16,
}

#[derive(Debug)]
pub enum GuiEvent {
    ScanFinished(Result<Vec<AirPlayReceiver>, String>),
    CaptureFinished(Result<CaptureProbe, String>),
    ConnectFinished(Result<SessionTestResult, String>),
    StreamFinished(Result<LiveStreamResult, String>),
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Runtime::new().map_err(|e| format!("Tokio runtime: {e}"))
}

pub fn spawn_scan(ctx: egui::Context, tx: Sender<GuiEvent>) {
    std::thread::Builder::new()
        .name("solyan-scan".into())
        .spawn(move || {
            let result = match runtime() {
                Ok(rt) => rt
                    .block_on(discover_once(Duration::from_secs(4)))
                    .map_err(|e| format!("{e:#}")),
                Err(e) => Err(e),
            };
            let _ = tx.send(GuiEvent::ScanFinished(result));
            ctx.request_repaint();
        })
        .ok();
}

pub fn spawn_capture_probe(ctx: egui::Context, tx: Sender<GuiEvent>) {
    std::thread::Builder::new()
        .name("solyan-capture-probe".into())
        .spawn(move || {
            let result = (|| -> Result<CaptureProbe, String> {
                let mut capture =
                    start_default_loopback(AudioFormat::default()).map_err(|e| format!("{e:#}"))?;
                let name = capture.device_name.clone();
                let started = Instant::now();
                let mut chunks = 0u64;
                let mut peak = 0i16;

                while started.elapsed() < Duration::from_secs(3) {
                    if let Ok(chunk) = capture.recv_timeout(Duration::from_millis(250)) {
                        chunks += 1;
                        if let Some(p) = chunk.samples.iter().map(|s| s.saturating_abs()).max() {
                            peak = peak.max(p);
                        }
                    }
                }

                capture.stop();
                Ok(CaptureProbe {
                    device_name: name,
                    chunks,
                    peak,
                })
            })();

            let _ = tx.send(GuiEvent::CaptureFinished(result));
            ctx.request_repaint();
        })
        .ok();
}

pub fn spawn_connect_test(
    ctx: egui::Context,
    tx: Sender<GuiEvent>,
    selector: String,
) {
    std::thread::Builder::new()
        .name("solyan-connect-test".into())
        .spawn(move || {
            let result = match runtime() {
                Ok(rt) => rt
                    .block_on(connect_test(
                        Some(&selector),
                        Duration::from_secs(4),
                        Duration::from_secs(20),
                    ))
                    .map_err(|e| format!("{e:#}")),
                Err(e) => Err(e),
            };

            let _ = tx.send(GuiEvent::ConnectFinished(result));
            ctx.request_repaint();
        })
        .ok();
}

pub fn spawn_stream(
    ctx: egui::Context,
    tx: Sender<GuiEvent>,
    progress_tx: Sender<StreamProgress>,
    selectors: Vec<String>,
    mode: LiveStreamMode,
    control: StreamControl,
    render_delay_ms: u32,
) {
    std::thread::Builder::new()
        .name("solyan-live-stream".into())
        .spawn(move || {
            let result = match runtime() {
                Ok(rt) => rt
                    .block_on(run_live_stream(
                        selectors,
                        mode,
                        control,
                        Some(progress_tx),
                        render_delay_ms,
                    ))
                    .map_err(|e| format!("{e:#}")),
                Err(e) => Err(e),
            };

            let _ = tx.send(GuiEvent::StreamFinished(result));
            ctx.request_repaint();
        })
        .ok();
}
