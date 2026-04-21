use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use super::telemetry::note_live_input_event;

#[derive(Debug, Clone)]
pub struct LiveInputPacket {
    pub phase: String,
    pub pointer_id: u32,
    pub px: f32,
    pub py: f32,
    pub host_width: Option<u32>,
    pub host_height: Option<u32>,
    pub flip_y: Option<bool>,
    pub button: Option<u32>,
    pub buttons: Option<u32>,
    pub source: Option<String>,
}

static LIVE_INPUT_QUEUE: OnceLock<Mutex<VecDeque<LiveInputPacket>>> = OnceLock::new();

fn queue() -> &'static Mutex<VecDeque<LiveInputPacket>> {
    LIVE_INPUT_QUEUE.get_or_init(|| Mutex::new(VecDeque::new()))
}

pub fn enqueue_live_input(packet: LiveInputPacket) {
    let source = packet.source.clone().unwrap_or_else(|| "inputbus".to_string());
    note_live_input_event(&packet.phase, packet.px, packet.py, &source);
    if let Ok(mut guard) = queue().lock() {
        guard.push_back(packet);
    }
}

pub fn drain_live_input(limit: usize) -> Vec<LiveInputPacket> {
    let mut out = Vec::new();
    let take = limit.max(1);
    if let Ok(mut guard) = queue().lock() {
        while out.len() < take {
            let Some(packet) = guard.pop_front() else {
                break;
            };
            out.push(packet);
        }
    }
    out
}

pub fn clear_live_input() {
    if let Ok(mut guard) = queue().lock() {
        guard.clear();
    }
}
