// Owned host-input/touch injection state extracted from the monolithic backend file.

#[derive(Debug, Clone, Default)]
pub(crate) struct HostInputState {
    queue: VecDeque<ScriptedPointerEvent>,
    events_loaded: u32,
    script_offset: u64,
    script_remainder: String,
    events_consumed: u32,
    events_ignored: u32,
    active_touch: Option<ActivePointerTouch>,
    synthetic_touch_objects: HashMap<u32, SyntheticUiTouchState>,
    synthetic_event_objects: HashMap<u32, SyntheticUiEventState>,
    synthetic_set_objects: HashMap<u32, SyntheticSetState>,
    empty_touch_set: Option<u32>,
    ui_attempts: u32,
    ui_dispatched: u32,
    cocos_attempts: u32,
    cocos_dispatched: u32,
    last_phase: Option<String>,
    last_target: Option<u32>,
    last_x: Option<f32>,
    last_y: Option<f32>,
    last_dispatch: Option<String>,
    last_source: Option<String>,
}
