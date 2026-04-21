use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::config_builder::{build_core_config, CoreConfigOverrides};
use minifb::{Key, MouseButton, MouseMode, Scale, Window, WindowOptions};
use serde::Serialize;
use serde_json::json;

const TELEMETRY_SAMPLE_EVERY_MS: u128 = 200;
const STALL_WARN_EVERY_MS: u128 = 1_000;
const NO_PRESENT_THRESHOLD_MS: u128 = 500;

#[derive(Debug, Clone)]
pub struct LiveRunRequest {
    pub manifest: PathBuf,
    pub argv0: Option<String>,
    pub runtime_mode: mkea_core::RuntimeMode,
    pub backend: mkea_core::ExecutionBackendKind,
    pub synthetic_network_faults: bool,
    pub runloop_ticks: Option<u32>,
    pub input_script: Option<PathBuf>,
    pub input_flip_y: bool,
    pub menu_probe_selector: Option<String>,
    pub menu_probe_after: Option<u32>,
    pub out: Option<PathBuf>,
    pub title: String,
    pub window_width: u32,
    pub window_height: u32,
    pub max_instructions: u64,
    pub frame_dump_dir: Option<PathBuf>,
    pub close_when_finished: bool,
}

enum InputRecorder {
    File { writer: BufWriter<File>, host_width: u32, host_height: u32 },
    InMemory { host_width: u32, host_height: u32 },
}

impl InputRecorder {
    fn new(path: Option<&Path>, host_width: u32, host_height: u32) -> Result<Self> {
        if let Some(path) = path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create input dir: {}", parent.display()))?;
            }
            if !path.exists() {
                File::create(path).with_context(|| format!("failed to create input script: {}", path.display()))?;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("failed to open input script for append: {}", path.display()))?;
            Ok(Self::File {
                writer: BufWriter::new(file),
                host_width: host_width.max(1),
                host_height: host_height.max(1),
            })
        } else {
            Ok(Self::InMemory {
                host_width: host_width.max(1),
                host_height: host_height.max(1),
            })
        }
    }

    fn push_pointer(&mut self, phase: &str, px: f32, py: f32) -> Result<()> {
        match self {
            Self::File { writer, host_width, host_height } => {
                let payload = json!({
                    "phase": phase,
                    "pointer_id": 1,
                    "px": px,
                    "py": py,
                    "host_width": *host_width,
                    "host_height": *host_height,
                    "source": "live-window",
                });
                serde_json::to_writer(&mut *writer, &payload)?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
            Self::InMemory { host_width, host_height } => {
                mkea_core::enqueue_live_input(mkea_core::LiveInputPacket {
                    phase: phase.to_string(),
                    pointer_id: 1,
                    px,
                    py,
                    host_width: Some(*host_width),
                    host_height: Some(*host_height),
                    flip_y: None,
                    button: Some(1),
                    buttons: Some(if phase == "up" { 0 } else { 1 }),
                    source: Some("live-window".to_string()),
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
struct LiveTelemetrySample {
    at_unix_ms: u128,
    state: String,
    frame_queue_depth: usize,
    frame_queue_capacity: usize,
    dropped_frames: u64,
    last_frame_index_seen: Option<u32>,
    last_present_frame: Option<u32>,
    last_present_at_unix_ms: Option<u128>,
    last_present_source: Option<String>,
    last_present_reason: Option<String>,
    last_present_reused_previous: Option<bool>,
    last_runloop_tick: Option<u32>,
    last_runloop_at_unix_ms: Option<u128>,
    last_runloop_origin: Option<String>,
    last_input_phase: Option<String>,
    last_input_at_unix_ms: Option<u128>,
    last_input_source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct LiveTelemetryExport {
    manifest: String,
    title: String,
    window_width: u32,
    window_height: u32,
    close_requested: bool,
    last_frame_index_seen: Option<u32>,
    final_state: String,
    no_present_threshold_ms: u128,
    sample_interval_ms: u128,
    stall_warn_interval_ms: u128,
    worker_error: Option<String>,
    samples: Vec<LiveTelemetrySample>,
    final_telemetry: mkea_core::LiveRuntimeTelemetry,
}

pub fn run_live(req: LiveRunRequest) -> Result<()> {
    let input_path = req.input_script.clone();
    let frame_dump_dir = req.frame_dump_dir.clone();

    if let Some(dir) = frame_dump_dir.as_ref() {
        fs::create_dir_all(dir)
            .with_context(|| format!("failed to create frame dump dir: {}", dir.display()))?;
        clear_stale_frame_dumps(dir)?;
    }
    if let Some(path) = input_path.as_ref() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create input dir: {}", parent.display()))?;
        }
        if !path.exists() {
            File::create(path)
                .with_context(|| format!("failed to create input script: {}", path.display()))?;
        }
    }

    mkea_core::clear_stop_request();
    mkea_core::clear_live_input();
    mkea_core::clear_live_runtime_telemetry();

    let transport = if input_path.is_some() { "jsonl-file" } else { "in-memory" };
    println!(
        "live window backend starting: manifest={} input={} transport={}",
        req.manifest.display(),
        input_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<in-memory>".to_string()),
        transport
    );

    let frame_bus: Arc<Mutex<std::collections::VecDeque<mkea_core::LiveFramePacket>>> =
        Arc::new(Mutex::new(std::collections::VecDeque::new()));
    mkea_core::install_live_frame_sink(Some(frame_bus));

    let worker_req = req.clone();
    let worker_input = input_path.clone();
    let worker_dumps = frame_dump_dir.clone();
    let mut worker = Some(thread::spawn(move || -> Result<serde_json::Value> {
        let loaded = mkea_loader::load_build_artifact(&worker_req.manifest)
            .with_context(|| format!("failed to load build manifest: {}", worker_req.manifest.display()))?;

        let mut cfg = build_core_config(
            &loaded,
            CoreConfigOverrides {
                argv0: worker_req.argv0.clone(),
                runtime_mode: Some(worker_req.runtime_mode),
                backend: Some(worker_req.backend),
                synthetic_network_faults: worker_req.synthetic_network_faults,
                runloop_ticks: Some(worker_req.runloop_ticks.unwrap_or(1_000_000)),
                dump_frames: Some(worker_dumps.is_some()),
                frame_dump_dir: worker_dumps.clone(),
                dump_every: Some(1),
                dump_limit: Some(0),
                input_script: worker_input.clone(),
                input_width: Some(worker_req.window_width.max(1)),
                input_height: Some(worker_req.window_height.max(1)),
                input_flip_y: worker_req.input_flip_y,
                menu_probe_selector: worker_req.menu_probe_selector.clone(),
                menu_probe_after: worker_req.menu_probe_after,
                live_host_mode: true,
                max_instructions_floor: Some(worker_req.max_instructions.max(131_072)),
            },
        );

        let (plan, runtime) = mkea_core::plan_bootstrap(&loaded.probe, &loaded.macho_slice, cfg)
            .with_context(|| format!("failed to bootstrap image from {}", worker_req.manifest.display()))?;
        Ok(json!({
            "probe": loaded.probe,
            "bootstrap": plan,
            "runtime": runtime,
        }))
    }));

    let width = req.window_width.max(1) as usize;
    let height = req.window_height.max(1) as usize;
    let mut window = Window::new(
        &req.title,
        width,
        height,
        WindowOptions {
            resize: false,
            scale: Scale::X1,
            ..WindowOptions::default()
        },
    )
    .with_context(|| "failed to create live host window")?;
    window.limit_update_rate(Some(Duration::from_micros(16_600)));

    let mut recorder = InputRecorder::new(input_path.as_deref(), req.window_width, req.window_height)?;
    let mut buffer = checkerboard_buffer(width, height);
    let mut last_frame_index: Option<u32> = None;
    let mut last_frame_dims: Option<(u32, u32)> = None;
    let mut last_mouse_down = false;
    let mut last_mouse_pos: Option<(i32, i32)> = None;
    let mut worker_result: Option<Result<serde_json::Value>> = None;
    let mut telemetry_samples: Vec<LiveTelemetrySample> = Vec::new();
    let mut last_sample_ms: u128 = 0;
    let mut last_stall_log_ms: u128 = 0;
    let mut last_state: String = "booting".to_string();

    let mut close_requested = false;
    while window.is_open() && !window.is_key_down(Key::Escape) {
        if let Some(frame) = mkea_core::take_live_frame() {
            if last_frame_index != Some(frame.frame_index) {
                last_frame_index = Some(frame.frame_index);
                last_frame_dims = Some((frame.width, frame.height));
                buffer = scale_rgba_to_u32(&frame.rgba, frame.width, frame.height, req.window_width, req.window_height);
            }
        } else if let Some(dir) = frame_dump_dir.as_ref() {
            if let Some(frame) = load_latest_frame(dir, last_frame_index)? {
                last_frame_index = Some(frame.frame_index);
                last_frame_dims = Some((frame.width, frame.height));
                buffer = scale_rgba_to_u32(&frame.rgba, frame.width, frame.height, req.window_width, req.window_height);
            }
        }

        if let Some((mx, my)) = window.get_mouse_pos(MouseMode::Clamp) {
            let px = mx.max(0.0).min(req.window_width.saturating_sub(1) as f32);
            let py = my.max(0.0).min(req.window_height.saturating_sub(1) as f32);
            let mouse_down = window.get_mouse_down(MouseButton::Left);
            let mouse_pos = (px.round() as i32, py.round() as i32);
            if mouse_down && !last_mouse_down {
                recorder.push_pointer("down", px, py)?;
            } else if mouse_down && last_mouse_down && last_mouse_pos != Some(mouse_pos) {
                recorder.push_pointer("move", px, py)?;
            } else if !mouse_down && last_mouse_down {
                recorder.push_pointer("up", px, py)?;
            }
            last_mouse_down = mouse_down;
            last_mouse_pos = Some(mouse_pos);
        } else if last_mouse_down {
            recorder.push_pointer(
                "up",
                req.window_width.saturating_sub(1) as f32,
                req.window_height.saturating_sub(1) as f32,
            )?;
            last_mouse_down = false;
            last_mouse_pos = None;
        }

        let now_ms = unix_ms_now();
        let worker_finished = worker_result.is_some() || worker.as_ref().map(|handle| handle.is_finished()).unwrap_or(false);
        let telemetry = mkea_core::snapshot_live_runtime_telemetry();
        let state = classify_live_state(&telemetry, worker_finished, now_ms);
        if last_frame_index.is_some() || state != last_state || should_capture_sample(now_ms, last_sample_ms) {
            telemetry_samples.push(make_telemetry_sample(
                now_ms,
                &state,
                &telemetry,
                last_frame_index,
            ));
            last_sample_ms = now_ms;
        }
        if state.starts_with("no-new-presents") && now_ms.saturating_sub(last_stall_log_ms) >= STALL_WARN_EVERY_MS {
            println!(
                "[live-stall] state={} last_frame_seen={:?} queue={}/{} dropped={} last_present={:?} last_tick={:?} last_input={:?}",
                state,
                last_frame_index,
                telemetry.frame_queue_depth,
                telemetry.frame_queue_capacity,
                telemetry.dropped_frames,
                telemetry.last_present.as_ref().map(|value| (value.frame_index, value.source.clone(), value.reason.clone(), value.at_unix_ms)),
                telemetry.last_runloop_tick.as_ref().map(|value| (value.tick, value.origin.clone(), value.at_unix_ms)),
                telemetry.last_input_event.as_ref().map(|value| (value.phase.clone(), value.source.clone(), value.at_unix_ms)),
            );
            last_stall_log_ms = now_ms;
        }
        update_window_title(&mut window, &req.title, last_frame_index, last_frame_dims, &state, &telemetry);
        last_state = state;

        window
            .update_with_buffer(&buffer, width, height)
            .with_context(|| "failed to update live host window")?;

        if worker_result.is_none() {
            if let Some(handle) = worker.as_ref() {
                if handle.is_finished() {
                    let handle = worker.take().expect("worker handle missing");
                    worker_result = Some(match handle.join() {
                        Ok(result) => result,
                        Err(_) => Err(anyhow::anyhow!("live worker thread panicked")),
                    });
                    if let Some(Ok(_)) = worker_result.as_ref() {
                        if req.close_when_finished {
                            break;
                        }
                    }
                }
            }
        }
    }

    if window.is_key_down(Key::Escape) || !window.is_open() {
        close_requested = true;
    }

    if close_requested {
        println!("live window close requested; waiting for runtime to stop cleanly...");
        mkea_core::request_stop();
    }

    if worker_result.is_none() {
        if let Some(handle) = worker.take() {
            worker_result = Some(match handle.join() {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("live worker thread panicked")),
            });
        }
    }

    let final_telemetry = mkea_core::snapshot_live_runtime_telemetry();
    let final_state = classify_live_state(&final_telemetry, true, unix_ms_now());
    telemetry_samples.push(make_telemetry_sample(
        unix_ms_now(),
        &final_state,
        &final_telemetry,
        last_frame_index,
    ));

    let worker_json = match worker_result.as_ref() {
        Some(Ok(value)) => Some(value.clone()),
        _ => None,
    };
    let worker_error = match worker_result.as_ref() {
        Some(Err(err)) => Some(format!("{err:#}")),
        _ => None,
    };

    if let Some(path) = req.out.as_ref() {
        write_live_report(
            path,
            &req,
            worker_json,
            LiveTelemetryExport {
                manifest: req.manifest.display().to_string(),
                title: req.title.clone(),
                window_width: req.window_width,
                window_height: req.window_height,
                close_requested,
                last_frame_index_seen: last_frame_index,
                final_state: final_state.clone(),
                no_present_threshold_ms: NO_PRESENT_THRESHOLD_MS,
                sample_interval_ms: TELEMETRY_SAMPLE_EVERY_MS,
                stall_warn_interval_ms: STALL_WARN_EVERY_MS,
                worker_error,
                samples: telemetry_samples,
                final_telemetry,
            },
        )?;
        println!("wrote {}", path.display());
    }

    mkea_core::install_live_frame_sink(None);
    mkea_core::clear_live_input();
    mkea_core::clear_stop_request();

    if let Some(result) = worker_result {
        result?;
    }
    Ok(())
}

fn should_capture_sample(now_ms: u128, last_sample_ms: u128) -> bool {
    last_sample_ms == 0 || now_ms.saturating_sub(last_sample_ms) >= TELEMETRY_SAMPLE_EVERY_MS
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0)
}

fn classify_live_state(
    telemetry: &mkea_core::LiveRuntimeTelemetry,
    worker_finished: bool,
    now_ms: u128,
) -> String {
    if worker_finished {
        return "worker-finished".to_string();
    }
    let Some(last_present) = telemetry.last_present.as_ref() else {
        return "waiting-first-present".to_string();
    };
    let age_ms = now_ms.saturating_sub(last_present.at_unix_ms);
    if age_ms >= NO_PRESENT_THRESHOLD_MS {
        return format!("no-new-presents({}ms)", age_ms);
    }
    if last_present.reused_previous {
        return "retained".to_string();
    }
    "live".to_string()
}

fn update_window_title(
    window: &mut Window,
    base_title: &str,
    frame_index: Option<u32>,
    frame_dims: Option<(u32, u32)>,
    state: &str,
    telemetry: &mkea_core::LiveRuntimeTelemetry,
) {
    let queue_depth = telemetry.frame_queue_depth;
    let queue_capacity = telemetry.frame_queue_capacity;
    let dropped = telemetry.dropped_frames;
    let title = match (frame_index, frame_dims) {
        (Some(frame_index), Some((w, h))) => format!(
            "{} - frame {} [{}x{} {} q={}/{} drop={}]",
            base_title, frame_index, w, h, state, queue_depth, queue_capacity, dropped,
        ),
        _ => format!(
            "{} - {} [q={}/{} drop={}]",
            base_title, state, queue_depth, queue_capacity, dropped,
        ),
    };
    window.set_title(&title);
}

fn make_telemetry_sample(
    at_unix_ms: u128,
    state: &str,
    telemetry: &mkea_core::LiveRuntimeTelemetry,
    last_frame_index_seen: Option<u32>,
) -> LiveTelemetrySample {
    LiveTelemetrySample {
        at_unix_ms,
        state: state.to_string(),
        frame_queue_depth: telemetry.frame_queue_depth,
        frame_queue_capacity: telemetry.frame_queue_capacity,
        dropped_frames: telemetry.dropped_frames,
        last_frame_index_seen,
        last_present_frame: telemetry.last_present.as_ref().map(|value| value.frame_index),
        last_present_at_unix_ms: telemetry.last_present.as_ref().map(|value| value.at_unix_ms),
        last_present_source: telemetry.last_present.as_ref().map(|value| value.source.clone()),
        last_present_reason: telemetry.last_present.as_ref().map(|value| value.reason.clone()),
        last_present_reused_previous: telemetry.last_present.as_ref().map(|value| value.reused_previous),
        last_runloop_tick: telemetry.last_runloop_tick.as_ref().map(|value| value.tick),
        last_runloop_at_unix_ms: telemetry.last_runloop_tick.as_ref().map(|value| value.at_unix_ms),
        last_runloop_origin: telemetry.last_runloop_tick.as_ref().map(|value| value.origin.clone()),
        last_input_phase: telemetry.last_input_event.as_ref().map(|value| value.phase.clone()),
        last_input_at_unix_ms: telemetry.last_input_event.as_ref().map(|value| value.at_unix_ms),
        last_input_source: telemetry.last_input_event.as_ref().map(|value| value.source.clone()),
    }
}

fn write_live_report(
    path: &Path,
    req: &LiveRunRequest,
    worker_json: Option<serde_json::Value>,
    live: LiveTelemetryExport,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create report dir: {}", parent.display()))?;
    }

    let mut root = worker_json.unwrap_or_else(|| json!({}));
    if !root.is_object() {
        root = json!({ "runtime": root });
    }
    if let Some(obj) = root.as_object_mut() {
        obj.insert("manifest_path".to_string(), json!(req.manifest.display().to_string()));
        obj.insert("live".to_string(), serde_json::to_value(live)?);
    }
    let text = serde_json::to_string_pretty(&root)?;
    fs::write(path, text)
        .with_context(|| format!("failed to write live runtime report: {}", path.display()))?;
    Ok(())
}

#[derive(Debug, Clone)]
struct DecodedFrame {
    frame_index: u32,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn clear_stale_frame_dumps(dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)
        .with_context(|| format!("failed to list frame dump dir: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.starts_with("frame_") && name.ends_with(".png") {
            let _ = fs::remove_file(&path);
        }
    }
    Ok(())
}

fn load_latest_frame(dir: &Path, last_frame_index: Option<u32>) -> Result<Option<DecodedFrame>> {
    let mut newest: Option<(u32, PathBuf)> = None;
    for entry in fs::read_dir(dir)
        .with_context(|| format!("failed to read frame dump dir: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !name.starts_with("frame_") || !name.ends_with(".png") {
            continue;
        }
        let idx = name.trim_start_matches("frame_").trim_end_matches(".png").parse::<u32>().unwrap_or(0);
        match newest {
            Some((best, _)) if idx <= best => {}
            _ => newest = Some((idx, path)),
        }
    }
    let Some((frame_index, path)) = newest else {
        return Ok(None);
    };
    if last_frame_index == Some(frame_index) {
        return Ok(None);
    }
    let frame = decode_png_rgba(&path).with_context(|| format!("failed to decode frame dump: {}", path.display()))?;
    Ok(Some(DecodedFrame {
        frame_index,
        width: frame.0,
        height: frame.1,
        rgba: frame.2,
    }))
}

fn decode_png_rgba(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let file = File::open(path).with_context(|| format!("failed to open png: {}", path.display()))?;
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().with_context(|| format!("failed to read png header: {}", path.display()))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .with_context(|| format!("failed to decode png frame: {}", path.display()))?;
    let bytes = &buf[..info.buffer_size()];
    let mut rgba = Vec::with_capacity((info.width * info.height * 4) as usize);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(bytes),
        png::ColorType::Rgb => {
            for chunk in bytes.chunks_exact(3) {
                rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 0xff]);
            }
        }
        png::ColorType::Grayscale => {
            for &value in bytes {
                rgba.extend_from_slice(&[value, value, value, 0xff]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for chunk in bytes.chunks_exact(2) {
                rgba.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
        }
        png::ColorType::Indexed => {
            return Err(anyhow::anyhow!("indexed pngs are not supported by the live preview"));
        }
    }
    Ok((info.width, info.height, rgba))
}

fn scale_rgba_to_u32(src: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u32> {
    let sw = src_w.max(1) as usize;
    let sh = src_h.max(1) as usize;
    let dw = dst_w.max(1) as usize;
    let dh = dst_h.max(1) as usize;
    let mut out = vec![0u32; dw * dh];
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            let src_idx = (sy * sw + sx) * 4;
            if src_idx + 3 >= src.len() {
                continue;
            }
            let r = src[src_idx] as u32;
            let g = src[src_idx + 1] as u32;
            let b = src[src_idx + 2] as u32;
            out[y * dw + x] = (r << 16) | (g << 8) | b;
        }
    }
    out
}

fn checkerboard_buffer(width: usize, height: usize) -> Vec<u32> {
    let mut out = vec![0u32; width * height];
    for y in 0..height {
        for x in 0..width {
            let dark = ((x / 16) + (y / 16)) % 2 == 0;
            let c = if dark { 0x1a1a22 } else { 0x2b2b36 };
            out[y * width + x] = c;
        }
    }
    out
}
