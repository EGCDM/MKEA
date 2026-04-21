#[derive(Debug, Clone)]
pub(crate) struct SyntheticCocosScheduledSelector {
    target: u32,
    selector_name: String,
    interval_ticks: u32,
    next_tick: u32,
    repeats_left: Option<u32>,
    fires: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SchedulerCallsiteProvenance {
    origin: String,
    pc: u32,
    lr: u32,
    exec_pc: u32,
    tick: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct SyntheticFoundationTimer {
    timer_obj: u32,
    target: u32,
    selector_name: String,
    interval_ticks: u32,
    next_tick: u32,
    repeats: bool,
    user_info: u32,
    attached: bool,
    fires: u32,
    created_from: SchedulerCallsiteProvenance,
    attached_from: Option<SchedulerCallsiteProvenance>,
}

#[derive(Debug, Clone)]
pub(crate) struct SyntheticDelayedSelector {
    target: u32,
    selector_name: String,
    object_arg: u32,
    next_tick: u32,
    fires: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SyntheticMethodSignature {
    selector_name: Option<String>,
    objc_types: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SyntheticInvocation {
    signature: u32,
    target: u32,
    selector_ptr: u32,
    selector_name: Option<String>,
    arguments: HashMap<u32, u32>,
    retained_arguments: bool,
    invoke_count: u32,
    last_result: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SyntheticCocosAction {
    class_name: String,
    kind: String,
    target: u32,
    selector_name: Option<String>,
    object_arg: u32,
    duration_bits: u32,
    interval_scale_x_bits: u32,
    interval_scale_y_bits: u32,
    interval_scale_explicit: bool,
    children: Vec<u32>,
    queued_count: u32,
    execute_count: u32,
    last_owner: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveCocosIntervalAction {
    action: u32,
    owner: u32,
    class_name: String,
    kind: String,
    start_tick: u32,
    duration_ticks: u32,
    started: bool,
    last_step_tick: u32,
    step_count: u32,
    host_scale_ready: bool,
    host_start_scale_x_bits: u32,
    host_start_scale_y_bits: u32,
    host_end_scale_x_bits: u32,
    host_end_scale_y_bits: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct SyntheticCocosActionPlan {
    target: u32,
    selector_name: String,
    object_arg: u32,
    delay_ticks: u32,
    path: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PassiveLoadingActionPlan {
    owner: u32,
    target: u32,
    selector_name: String,
    object_arg: u32,
    delay_ticks: u32,
    next_tick: u32,
    path: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SchedulerTraceState {
    events: Vec<String>,
    callbacks: Vec<String>,
    build_banner_emitted: bool,
    window_scene: u32,
    window_start_tick: u32,
    window_end_tick: u32,
    window_origin: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SchedulerTimerState {
    cocos_selectors: HashMap<(u32, String), SyntheticCocosScheduledSelector>,
    foundation_timers: HashMap<u32, SyntheticFoundationTimer>,
    delayed_selectors: Vec<SyntheticDelayedSelector>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SchedulerInvocationState {
    method_signatures: HashMap<u32, SyntheticMethodSignature>,
    invocations: HashMap<u32, SyntheticInvocation>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SchedulerActionState {
    cocos_actions: HashMap<u32, SyntheticCocosAction>,
    active_interval_actions: HashMap<u32, ActiveCocosIntervalAction>,
    passive_loading_plan: Option<PassiveLoadingActionPlan>,
    last_loading_callfunc: Option<(u32, String, u32)>,
    last_delay_bits: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LoadingBootstrapState {
    scene_startup_attempts: HashMap<u32, u32>,
    scene_bootstrap_state: HashMap<u32, u32>,
}
