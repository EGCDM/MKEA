#[derive(Debug, Clone, Default)]
pub(crate) struct SyntheticNotificationObserverState {
    pub(crate) observer: u32,
    pub(crate) selector_ptr: u32,
    pub(crate) selector_name: String,
    pub(crate) name_ptr: u32,
    pub(crate) object: u32,
    pub(crate) registrations: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SyntheticMoviePlayerState {
    pub(crate) content_url: u32,
    pub(crate) should_autoplay: bool,
    pub(crate) synthetic_view: u32,
    pub(crate) play_count: u32,
    pub(crate) stop_count: u32,
    pub(crate) pause_count: u32,
    pub(crate) prepared: bool,
    pub(crate) is_playing: bool,
    pub(crate) playback_started_tick: u32,
    pub(crate) playback_finish_tick: u32,
    pub(crate) playback_remaining_ticks: u32,
    pub(crate) playback_duration_ticks: u32,
    pub(crate) finish_notifications_posted: u32,
}


#[derive(Debug, Clone, Default)]
pub(crate) struct SyntheticAudioPlayerState {
    pub(crate) content_url: u32,
    pub(crate) content_data: u32,
    pub(crate) delegate: u32,
    pub(crate) prepared: bool,
    pub(crate) is_playing: bool,
    pub(crate) volume: f32,
    pub(crate) number_of_loops: i32,
    pub(crate) play_count: u32,
    pub(crate) pause_count: u32,
    pub(crate) stop_count: u32,
    pub(crate) prepare_count: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SyntheticNotificationState {
    pub(crate) name_ptr: u32,
    pub(crate) object: u32,
    pub(crate) user_info: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ObjcSyncMonitorState {
    pub(crate) owner_thread_id: u32,
    pub(crate) recursion_depth: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct UIKitRuntimeState {
    launched: bool,
    delegate_set: bool,
    pub(crate) window_visible: bool,
    app_active: bool,
    runloop_live: bool,
    timer_armed: bool,
    exit_suppressed: bool,
    launch_count: u32,
    activation_count: u32,
    runloop_ticks: u32,
    runloop_sources: u32,
    last_tick_sources_before: u32,
    last_tick_sources_after: u32,
    idle_ticks_after_completion: u32,
    prng_state: u32,
    prng_seeded: bool,
    prng_last_seed: u32,
    prng_draw_count: u32,
    guest_time_seeded: bool,
    guest_unix_micros: u64,
    sjlj_context_head: u32,
    sjlj_register_count: u32,
    sjlj_unregister_count: u32,
    sjlj_resume_count: u32,
    objc_sync_enter_count: u32,
    objc_sync_exit_count: u32,
    objc_sync_mismatch_count: u32,
    objc_sync_monitors: std::collections::BTreeMap<u32, ObjcSyncMonitorState>,
    roundf_count: u32,
    floorf_count: u32,
    ceilf_count: u32,
    fabsf_count: u32,
    sinf_count: u32,
    cosf_count: u32,
    tanf_count: u32,
    asinf_count: u32,
    acosf_count: u32,
    atanf_count: u32,
    expf_count: u32,
    logf_count: u32,
    sqrtf_count: u32,
    atan2f_count: u32,
    fmodf_count: u32,
    fmaxf_count: u32,
    fminf_count: u32,
    powf_count: u32,
    floor_count: u32,
    ceil_count: u32,
    atan2_count: u32,
    modsi3_count: u32,
    divsi3_count: u32,
    udivsi3_count: u32,
    umodsi3_count: u32,
    strtok_call_count: u32,
    strtok_next_ptr: u32,
    rb_tree_insert_count: u32,
    rb_tree_increment_count: u32,
    rb_tree_decrement_count: u32,
    rb_tree_erase_rebalance_count: u32,
    pub(crate) notification_center_default: u32,
    pub(crate) notification_observers: std::collections::BTreeMap<u32, Vec<SyntheticNotificationObserverState>>,
    pub(crate) synthetic_notifications: std::collections::BTreeMap<u32, SyntheticNotificationState>,
    pub(crate) movie_players: std::collections::BTreeMap<u32, SyntheticMoviePlayerState>,
    pub(crate) audio_players: std::collections::BTreeMap<u32, SyntheticAudioPlayerState>,
}
