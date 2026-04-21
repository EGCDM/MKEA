use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct LiveRuntimeTelemetry {
    pub frame_queue_depth: usize,
    pub frame_queue_capacity: usize,
    pub dropped_frames: u64,
    pub last_present: Option<LivePresentTelemetry>,
    pub last_runloop_tick: Option<RunloopTickTelemetry>,
    pub last_input_event: Option<InputEventTelemetry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LivePresentTelemetry {
    pub frame_index: u32,
    pub width: u32,
    pub height: u32,
    pub source: String,
    pub reason: String,
    pub reused_previous: bool,
    pub at_unix_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunloopTickTelemetry {
    pub tick: u32,
    pub origin: String,
    pub handled_source: bool,
    pub sources_before: u32,
    pub sources_after: u32,
    pub at_unix_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputEventTelemetry {
    pub phase: String,
    pub px: f32,
    pub py: f32,
    pub source: String,
    pub at_unix_ms: u128,
}

static LIVE_RUNTIME_TELEMETRY: OnceLock<Mutex<LiveRuntimeTelemetry>> = OnceLock::new();

fn slot() -> &'static Mutex<LiveRuntimeTelemetry> {
    LIVE_RUNTIME_TELEMETRY.get_or_init(|| Mutex::new(LiveRuntimeTelemetry::default()))
}

pub(crate) fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0)
}

pub fn snapshot_live_runtime_telemetry() -> LiveRuntimeTelemetry {
    match slot().lock() {
        Ok(guard) => guard.clone(),
        Err(_) => LiveRuntimeTelemetry::default(),
    }
}

pub fn clear_live_runtime_telemetry() {
    if let Ok(mut guard) = slot().lock() {
        *guard = LiveRuntimeTelemetry::default();
    }
}

pub(crate) fn note_frame_queue(depth: usize, capacity: usize) {
    if let Ok(mut guard) = slot().lock() {
        guard.frame_queue_depth = depth;
        guard.frame_queue_capacity = capacity;
    }
}

pub(crate) fn note_frame_drop(dropped_frames: u64, depth: usize, capacity: usize) {
    if let Ok(mut guard) = slot().lock() {
        guard.dropped_frames = dropped_frames;
        guard.frame_queue_depth = depth;
        guard.frame_queue_capacity = capacity;
    }
}

pub(crate) fn note_present_event(
    frame_index: u32,
    width: u32,
    height: u32,
    source: impl Into<String>,
    reason: impl Into<String>,
    reused_previous: bool,
    queue_depth: usize,
    queue_capacity: usize,
    dropped_frames: u64,
) {
    if let Ok(mut guard) = slot().lock() {
        guard.frame_queue_depth = queue_depth;
        guard.frame_queue_capacity = queue_capacity;
        guard.dropped_frames = dropped_frames;
        guard.last_present = Some(LivePresentTelemetry {
            frame_index,
            width,
            height,
            source: source.into(),
            reason: reason.into(),
            reused_previous,
            at_unix_ms: now_unix_ms(),
        });
    }
}

pub(crate) fn note_runloop_tick(
    tick: u32,
    origin: impl Into<String>,
    handled_source: bool,
    sources_before: u32,
    sources_after: u32,
) {
    if let Ok(mut guard) = slot().lock() {
        guard.last_runloop_tick = Some(RunloopTickTelemetry {
            tick,
            origin: origin.into(),
            handled_source,
            sources_before,
            sources_after,
            at_unix_ms: now_unix_ms(),
        });
    }
}

pub fn note_live_input_event(phase: &str, px: f32, py: f32, source: &str) {
    if let Ok(mut guard) = slot().lock() {
        guard.last_input_event = Some(InputEventTelemetry {
            phase: phase.to_string(),
            px,
            py,
            source: source.to_string(),
            at_unix_ms: now_unix_ms(),
        });
    }
}
