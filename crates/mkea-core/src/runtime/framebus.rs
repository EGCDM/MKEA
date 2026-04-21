use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use super::telemetry::{note_frame_drop, note_frame_queue, note_present_event};

pub const LIVE_FRAME_QUEUE_CAPACITY: usize = 32;
const LIVE_FRAME_QUEUE_WAIT_ROUNDS: usize = 32;
const LIVE_FRAME_QUEUE_WAIT_MS: u64 = 2;

#[derive(Debug, Clone)]
pub struct LiveFramePacket {
    pub frame_index: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub source: String,
    pub reason: String,
    pub reused_previous: bool,
}

static LIVE_FRAME_SINK: OnceLock<Mutex<Option<Arc<Mutex<VecDeque<LiveFramePacket>>>>>> = OnceLock::new();
static LIVE_FRAME_DROP_COUNT: OnceLock<Mutex<u64>> = OnceLock::new();

fn sink_slot() -> &'static Mutex<Option<Arc<Mutex<VecDeque<LiveFramePacket>>>>> {
    LIVE_FRAME_SINK.get_or_init(|| Mutex::new(None))
}

fn drop_counter() -> &'static Mutex<u64> {
    LIVE_FRAME_DROP_COUNT.get_or_init(|| Mutex::new(0))
}

pub fn install_live_frame_sink(sink: Option<Arc<Mutex<VecDeque<LiveFramePacket>>>>) {
    if let Ok(mut dropped) = drop_counter().lock() {
        *dropped = 0;
    }
    if let Ok(mut slot) = sink_slot().lock() {
        if let Some(queue) = sink.as_ref() {
            if let Ok(mut guard) = queue.lock() {
                guard.clear();
            }
        }
        *slot = sink;
    }
    note_frame_queue(0, LIVE_FRAME_QUEUE_CAPACITY);
}

pub fn take_live_frame() -> Option<LiveFramePacket> {
    let sink = sink_slot().lock().ok()?.clone()?;
    let mut guard = sink.lock().ok()?;
    let packet = guard.pop_front();
    note_frame_queue(guard.len(), LIVE_FRAME_QUEUE_CAPACITY);
    packet
}

pub fn live_frame_queue_depth() -> usize {
    let sink = match sink_slot().lock() {
        Ok(slot) => slot.clone(),
        Err(_) => None,
    };
    let Some(sink) = sink else {
        return 0;
    };
    let depth = match sink.lock() {
        Ok(guard) => guard.len(),
        Err(_) => 0,
    };
    depth
}

fn current_drop_count() -> u64 {
    match drop_counter().lock() {
        Ok(dropped) => *dropped,
        Err(_) => 0,
    }
}

fn note_drop_and_queue(depth: usize) {
    let dropped = current_drop_count();
    note_frame_drop(dropped, depth, LIVE_FRAME_QUEUE_CAPACITY);
}

fn push_packet(guard: &mut VecDeque<LiveFramePacket>, packet: LiveFramePacket, force_drop_oldest: bool) {
    if force_drop_oldest && guard.len() >= LIVE_FRAME_QUEUE_CAPACITY {
        guard.pop_front();
        if let Ok(mut dropped) = drop_counter().lock() {
            *dropped = dropped.saturating_add(1);
        }
        note_drop_and_queue(guard.len());
    }

    let frame_index = packet.frame_index;
    let width = packet.width;
    let height = packet.height;
    let source = packet.source.clone();
    let reason = packet.reason.clone();
    let reused_previous = packet.reused_previous;
    guard.push_back(packet);

    let dropped_frames = current_drop_count();
    note_present_event(
        frame_index,
        width,
        height,
        source,
        reason,
        reused_previous,
        guard.len(),
        LIVE_FRAME_QUEUE_CAPACITY,
        dropped_frames,
    );
}

pub(crate) fn publish_live_frame(packet: LiveFramePacket) {
    let sink = match sink_slot().lock() {
        Ok(slot) => slot.clone(),
        Err(_) => None,
    };
    let Some(sink) = sink else {
        return;
    };

    let mut packet = Some(packet);
    for _ in 0..LIVE_FRAME_QUEUE_WAIT_ROUNDS {
        let should_wait = match sink.lock() {
            Ok(mut guard) => {
                if guard.len() < LIVE_FRAME_QUEUE_CAPACITY {
                    let packet = packet.take().expect("live frame packet missing during enqueue");
                    push_packet(&mut guard, packet, false);
                    return;
                }
                note_frame_queue(guard.len(), LIVE_FRAME_QUEUE_CAPACITY);
                true
            }
            Err(_) => return,
        };
        if should_wait {
            thread::sleep(Duration::from_millis(LIVE_FRAME_QUEUE_WAIT_MS));
        }
    }

    if let Ok(mut guard) = sink.lock() {
        let packet = packet.take().expect("live frame packet missing during forced enqueue");
        push_packet(&mut guard, packet, true);
    };
}
