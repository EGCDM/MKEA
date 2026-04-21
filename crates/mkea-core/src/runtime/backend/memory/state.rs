#[derive(Debug, Clone)]
pub(crate) struct SyntheticHeapState {
    synthetic_heap_cursor: u32,
    synthetic_heap_end: u32,
    synthetic_heap_allocations: HashMap<u32, SyntheticHeapAllocation>,
    synthetic_heap_allocations_total: u32,
    synthetic_heap_frees: u32,
    synthetic_heap_reallocs: u32,
    synthetic_heap_bytes_active: u32,
    synthetic_heap_bytes_peak: u32,
    synthetic_heap_last_alloc_ptr: Option<u32>,
    synthetic_heap_last_alloc_size: Option<u32>,
    synthetic_heap_last_freed_ptr: Option<u32>,
    synthetic_heap_last_realloc_old_ptr: Option<u32>,
    synthetic_heap_last_realloc_new_ptr: Option<u32>,
    synthetic_heap_last_realloc_size: Option<u32>,
    synthetic_heap_last_error: Option<String>,
    synthetic_string_backing: HashMap<u32, SyntheticStringBacking>,
    synthetic_blob_backing: HashMap<u32, SyntheticBlobBacking>,
}

impl SyntheticHeapState {
    fn new(heap_base: u32, heap_size: u32) -> Self {
        Self {
            synthetic_heap_cursor: heap_base,
            synthetic_heap_end: heap_base.saturating_add(heap_size),
            synthetic_heap_allocations: HashMap::new(),
            synthetic_heap_allocations_total: 0,
            synthetic_heap_frees: 0,
            synthetic_heap_reallocs: 0,
            synthetic_heap_bytes_active: 0,
            synthetic_heap_bytes_peak: 0,
            synthetic_heap_last_alloc_ptr: None,
            synthetic_heap_last_alloc_size: None,
            synthetic_heap_last_freed_ptr: None,
            synthetic_heap_last_realloc_old_ptr: None,
            synthetic_heap_last_realloc_new_ptr: None,
            synthetic_heap_last_realloc_size: None,
            synthetic_heap_last_error: None,
            synthetic_string_backing: HashMap::new(),
            synthetic_blob_backing: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BackendGraphicsState {
    synthetic_framebuffer: Vec<u8>,
    synthetic_last_readback_rgba: Vec<u8>,
    synthetic_last_bgra_swizzle_rgba: Vec<u8>,
    synthetic_previous_readback_checksum: Option<u32>,
    synthetic_unique_frame_checksums: VecDeque<u32>,
    synthetic_bitmap_contexts: HashMap<u32, SyntheticBitmapContext>,
    synthetic_images: HashMap<u32, SyntheticImage>,
    synthetic_dictionaries: HashMap<u32, SyntheticDictionary>,
    synthetic_arrays: HashMap<u32, SyntheticArray>,
    synthetic_textures: HashMap<u32, SyntheticTexture>,
    synthetic_texture_atlases: HashMap<u32, SyntheticTextureAtlasState>,
    synthetic_sprites: HashMap<u32, SyntheticSpriteState>,
    synthetic_gl_texture_name_cursor: u32,
    guest_gl_texture_name_cursor: u32,
    guest_gl_textures: HashMap<u32, GuestGlTextureObject>,
    current_bound_texture_name: u32,
    gl_texture_2d_enabled: bool,
    gl_blend_enabled: bool,
    gl_current_color: [u8; 4],
    gl_blend_src_factor: u32,
    gl_blend_dst_factor: u32,
    gl_tex_env_mode: u32,
    synthetic_ui_object_cursor: u32,
    cocos_texture_cache_object: u32,
    cocos_texture_cache_entries: HashMap<String, u32>,
    cocos_audio_manager_object: u32,
    cocos_sound_engine_object: u32,
    current_uigraphics_context: u32,
    uigraphics_stack: Vec<u32>,
    last_uikit_image_object: u32,
    current_clear_rgba: [u8; 4],
    guest_framebuffer_dirty: bool,
    guest_draws_since_present: u32,
    uikit_framebuffer_dirty: bool,
    gl_vertex_array: GlClientArrayState,
    gl_color_array: GlClientArrayState,
    gl_texcoord_array: GlClientArrayState,
    current_bound_framebuffer: u32,
    current_bound_renderbuffer: u32,
    gl_call_counts: HashMap<String, u32>,
    recent_gl_calls: Vec<String>,
    synthetic_splash_destinations: HashMap<u32, u32>,
    texture_ptr_last_request_path: HashMap<u32, String>,
    texture_ptr_last_request_key: HashMap<u32, String>,
}

impl BackendGraphicsState {
    fn new() -> Self {
        Self {
            synthetic_framebuffer: Vec::new(),
            synthetic_last_readback_rgba: Vec::new(),
            synthetic_last_bgra_swizzle_rgba: Vec::new(),
            synthetic_previous_readback_checksum: None,
            synthetic_unique_frame_checksums: VecDeque::new(),
            synthetic_bitmap_contexts: HashMap::new(),
            synthetic_images: HashMap::new(),
            synthetic_dictionaries: HashMap::new(),
            synthetic_arrays: HashMap::new(),
            synthetic_textures: HashMap::new(),
            synthetic_texture_atlases: HashMap::new(),
            synthetic_sprites: HashMap::new(),
            synthetic_gl_texture_name_cursor: 1,
            guest_gl_texture_name_cursor: 0x0001_0000,
            guest_gl_textures: HashMap::new(),
            current_bound_texture_name: 0,
            gl_texture_2d_enabled: false,
            gl_blend_enabled: false,
            gl_current_color: [255, 255, 255, 255],
            gl_blend_src_factor: GL_ONE,
            gl_blend_dst_factor: GL_ZERO,
            gl_tex_env_mode: GL_MODULATE,
            synthetic_ui_object_cursor: 0x6fff0800,
            cocos_texture_cache_object: 0,
            cocos_texture_cache_entries: HashMap::new(),
            cocos_audio_manager_object: 0,
            cocos_sound_engine_object: 0,
            current_uigraphics_context: 0,
            uigraphics_stack: Vec::new(),
            last_uikit_image_object: 0,
            current_clear_rgba: [18, 24, 40, 255],
            guest_framebuffer_dirty: false,
            guest_draws_since_present: 0,
            uikit_framebuffer_dirty: false,
            gl_vertex_array: GlClientArrayState::default(),
            gl_color_array: GlClientArrayState::default(),
            gl_texcoord_array: GlClientArrayState::default(),
            current_bound_framebuffer: HLE_FAKE_GL_FRAMEBUFFER,
            current_bound_renderbuffer: HLE_FAKE_GL_RENDERBUFFER,
            gl_call_counts: HashMap::new(),
            recent_gl_calls: Vec::new(),
            synthetic_splash_destinations: HashMap::new(),
            texture_ptr_last_request_path: HashMap::new(),
            texture_ptr_last_request_key: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BackendOpenAlBufferState {
    format: u32,
    frequency: u32,
    byte_len: u32,
    preview: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(crate) struct BackendOpenAlSourceState {
    ints: HashMap<u32, i32>,
    floats: HashMap<u32, f32>,
    vectors: HashMap<u32, Vec<f32>>,
    queued_buffers: VecDeque<u32>,
    processed_buffers: VecDeque<u32>,
    state: u32,
}

impl Default for BackendOpenAlSourceState {
    fn default() -> Self {
        Self {
            ints: HashMap::new(),
            floats: HashMap::new(),
            vectors: HashMap::new(),
            queued_buffers: VecDeque::new(),
            processed_buffers: VecDeque::new(),
            state: 0x1011,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SyntheticFileUrlState {
    original_path: String,
    host_path: Option<String>,
    is_directory: bool,
    absolute_string: String,
    path_extension: String,
    last_path_component: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SyntheticDataProviderState {
    url_object: u32,
    path: String,
    byte_len: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SyntheticAudioFileMetadata {
    container_type: u32,
    data_format_id: u32,
    sample_rate: f64,
    channels_per_frame: u32,
    bits_per_channel: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    format_flags: u32,
    audio_data_offset: u64,
    audio_data_byte_count: u64,
    audio_data_packet_count: u64,
    packet_size_upper_bound: u32,
    maximum_packet_size: u32,
    estimated_duration_seconds: f64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SyntheticAudioPacketEntry {
    file_offset: u64,
    byte_count: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SyntheticAudioFileState {
    url_object: u32,
    path: String,
    byte_len: u32,
    metadata: SyntheticAudioFileMetadata,
    packet_table: Vec<SyntheticAudioPacketEntry>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AudioStreamBasicDescriptionState {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BackendAudioQueuePropertyListenerState {
    property_id: u32,
    callback_ptr: u32,
    user_data_ptr: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BackendAudioQueueBufferState {
    queue_ptr: u32,
    buffer_ptr: u32,
    audio_data_ptr: u32,
    audio_data_capacity: u32,
    last_byte_size: u32,
    user_data_ptr: u32,
    packet_descs_ptr: u32,
    packet_desc_capacity: u32,
    packet_desc_count: u32,
    enqueued: bool,
    callback_inflight: bool,
    freed: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BackendAudioQueueHandleState {
    handle_ptr: u32,
    callback_ptr: u32,
    user_data_ptr: u32,
    callback_runloop: u32,
    callback_runloop_mode: u32,
    flags: u32,
    format: Option<AudioStreamBasicDescriptionState>,
    parameters: HashMap<u32, f32>,
    properties: HashMap<u32, Vec<u8>>,
    property_listeners: Vec<BackendAudioQueuePropertyListenerState>,
    allocated_buffers: Vec<u32>,
    queued_buffers: VecDeque<u32>,
    is_running: bool,
    start_count: u32,
    prime_count: u32,
    stop_count: u32,
    dispose_count: u32,
}

#[derive(Debug, Clone)]
pub(crate) enum BackendAudioQueuePendingInvocation {
    OutputCallback { queue_ptr: u32, buffer_ptr: u32 },
    PropertyListener {
        queue_ptr: u32,
        property_id: u32,
        callback_ptr: u32,
        user_data_ptr: u32,
    },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BackendAudioQueueCallbackResumeState {
    origin_label: String,
    resume_lr: u32,
    return_status: u32,
    current: Option<BackendAudioQueuePendingInvocation>,
    pending: VecDeque<BackendAudioQueuePendingInvocation>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BackendAudioQueueState {
    next_queue_serial: u32,
    next_buffer_serial: u32,
    queues: HashMap<u32, BackendAudioQueueHandleState>,
    buffers: HashMap<u32, BackendAudioQueueBufferState>,
    callback_resume: Option<BackendAudioQueueCallbackResumeState>,
}

impl BackendAudioQueueState {
    fn new() -> Self {
        Self {
            next_queue_serial: 1,
            next_buffer_serial: 1,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PendingAudioIvarSnapshot {
    pub(crate) owner_class: String,
    pub(crate) name: String,
    pub(crate) offset: u32,
    pub(crate) value: u32,
    pub(crate) value_desc: String,
    pub(crate) value_class: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingAudioSelectorReturn {
    pub(crate) selector: String,
    pub(crate) receiver: u32,
    pub(crate) receiver_class: String,
    pub(crate) resource: Option<String>,
    pub(crate) imp: u32,
    pub(crate) return_pc: u32,
    pub(crate) return_thumb: bool,
    pub(crate) dispatch_pc: u32,
    pub(crate) receiver_ivars_before: Vec<PendingAudioIvarSnapshot>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BackendAudioTraceState {
    openal_device_open_calls: u32,
    openal_context_create_calls: u32,
    openal_make_current_calls: u32,
    openal_buffers_generated: u32,
    openal_sources_generated: u32,
    openal_buffer_upload_calls: u32,
    openal_bytes_uploaded: u64,
    openal_queue_calls: u32,
    openal_unqueue_calls: u32,
    openal_play_calls: u32,
    openal_stop_calls: u32,
    openal_last_buffer_format: Option<String>,
    openal_last_source_state: Option<String>,
    audioqueue_create_calls: u32,
    audioqueue_allocate_calls: u32,
    audioqueue_enqueue_calls: u32,
    audioqueue_enqueued_bytes: u64,
    audioqueue_prime_calls: u32,
    audioqueue_start_calls: u32,
    audioqueue_stop_calls: u32,
    audioqueue_dispose_calls: u32,
    audioqueue_output_callback_dispatches: u32,
    audioqueue_property_callback_dispatches: u32,
    audioqueue_last_format: Option<String>,
    audioqueue_last_queue: Option<u32>,
    audioqueue_last_buffer: Option<u32>,
    audioqueue_last_buffer_preview_hex: Option<String>,
    audioqueue_last_buffer_preview_ascii: Option<String>,
    audiofile_open_calls: u32,
    audiofile_read_bytes_calls: u32,
    audiofile_read_packets_calls: u32,
    audiofile_bytes_served: u64,
    systemsound_create_calls: u32,
    systemsound_play_calls: u32,
    systemsound_dispose_calls: u32,
    objc_audio_player_alloc_calls: u32,
    objc_audio_player_init_url_calls: u32,
    objc_audio_player_init_data_calls: u32,
    objc_audio_player_prepare_calls: u32,
    objc_audio_player_play_calls: u32,
    objc_audio_player_pause_calls: u32,
    objc_audio_player_stop_calls: u32,
    objc_audio_player_set_volume_calls: u32,
    objc_audio_player_set_loops_calls: u32,
    objc_audio_engine_shared_calls: u32,
    objc_audio_manager_shared_calls: u32,
    objc_audio_manager_soundengine_calls: u32,
    objc_audio_manager_soundengine_nil_results: u32,
    objc_audio_engine_preload_calls: u32,
    objc_audio_bgm_preload_calls: u32,
    objc_audio_engine_play_calls: u32,
    objc_audio_bgm_play_calls: u32,
    objc_audio_engine_stop_calls: u32,
    objc_audio_engine_effect_calls: u32,
    objc_audio_engine_async_load_progress_calls: u32,
    objc_audio_engine_async_load_progress_nil_receivers: u32,
    objc_audio_engine_playsound_calls: u32,
    objc_audio_engine_playsound_nil_receivers: u32,
    objc_audio_fallback_dispatches: u32,
    objc_audio_last_class: Option<String>,
    objc_audio_last_selector: Option<String>,
    objc_audio_last_resource: Option<String>,
    objc_audio_last_result: Option<String>,
    next_systemsound_id: u32,
    next_objc_audio_effect_id: u32,
    systemsounds: HashMap<u32, String>,
    unsupported_events: u32,
    recent_events: Vec<String>,
    pending_selector_returns: Vec<PendingAudioSelectorReturn>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BackendOpenAlState {
    device_ptr: u32,
    context_ptr: u32,
    current_context: u32,
    next_buffer_id: u32,
    next_source_id: u32,
    last_al_error: u32,
    last_alc_error: u32,
    distance_model: u32,
    listener_floats: HashMap<u32, f32>,
    listener_vectors: HashMap<u32, Vec<f32>>,
    buffers: HashMap<u32, BackendOpenAlBufferState>,
    sources: HashMap<u32, BackendOpenAlSourceState>,
}

impl BackendOpenAlState {
    fn new() -> Self {
        Self {
            next_buffer_id: 1,
            next_source_id: 1,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BackendSceneState {
    auto_scene_cached_root: u32,
    auto_scene_cached_source: Option<String>,
    auto_scene_inferred_root: u32,
    auto_scene_inferred_source: Option<String>,
    auto_scene_last_present_signature: Option<u64>,
    scene_progress_trace: Vec<String>,
    sprite_watch_trace: Vec<String>,
    graph_trace: Vec<String>,
    synthetic_last_running_scene: u32,
    synthetic_running_scene_ticks: u32,
    synthetic_touch_injections: u32,
    synthetic_scene_transitions: u32,
    synthetic_menu_probe_attempts: u32,
    synthetic_menu_probe_fired: bool,
    synthetic_menu_probe_inflight: bool,
    traversal_guard_frame_token: u64,
    traversal_first_cycle_frame_token: u64,
    traversal_first_cycle_message: Option<String>,
    traversal_invalidate_nodes: HashSet<u32>,
    traversal_adopt_nodes: HashSet<u32>,
    selector_dispatch_guard_frame_token: u64,
    selector_dispatch_depth: u32,
    selector_dispatch_max_depth: u32,
    selector_dispatch_stack: Vec<String>,
    lifecycle_dispatch_guard_frame_token: u64,
    lifecycle_dispatch_depth: u32,
    lifecycle_dispatch_max_depth: u32,
    lifecycle_dispatch_stack: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BackendSchedulerState {
    trace: SchedulerTraceState,
    timers: SchedulerTimerState,
    invocations: SchedulerInvocationState,
    actions: SchedulerActionState,
    loading: LoadingBootstrapState,
}


#[derive(Debug, Clone, Default)]
pub(crate) struct BackendCpuState {
    pub(crate) regs: [u32; 16],
    pub(crate) thumb: bool,
    pub(crate) flags: ArmFlags,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BackendAddressSpaceState {
    pub(crate) mapped: Vec<BackendRegion>,
    pub(crate) trampoline_addr: u32,
    pub(crate) trampoline_size: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct BackendRuntimeState {
    pub(crate) heap: SyntheticHeapState,
    pub(crate) fs: FsState,
    pub(crate) objc: ObjcState,
    pub(crate) graphics: BackendGraphicsState,
    pub(crate) scene: BackendSceneState,
    pub(crate) host_input: HostInputState,
    pub(crate) scheduler: BackendSchedulerState,
    pub(crate) openal: BackendOpenAlState,
    pub(crate) audio_queue: BackendAudioQueueState,
    pub(crate) audio_trace: BackendAudioTraceState,
    // Split the old UIKit aggregate into focused ownership zones so the
    // top-level runtime stops reintroducing a second giant state blob.
    pub(crate) ui_objects: UIKitObjectState,
    pub(crate) ui_runtime: UIKitRuntimeState,
    pub(crate) ui_graphics: GraphicsState,
    pub(crate) ui_network: NetworkState,
    pub(crate) ui_cocos: CocosState,
}

impl BackendRuntimeState {
    fn new(cfg: &CoreConfig, bundle_root: Option<PathBuf>) -> Self {
        let mut ui_graphics = GraphicsState::default();
        if cfg.preferred_surface_width > 0 {
            ui_graphics.graphics_surface_width = cfg.preferred_surface_width.max(1);
        }
        if cfg.preferred_surface_height > 0 {
            ui_graphics.graphics_surface_height = cfg.preferred_surface_height.max(1);
        }
        if ui_graphics.graphics_surface_width > 0 && ui_graphics.graphics_surface_height > 0 {
            ui_graphics.graphics_viewport_width = ui_graphics.graphics_surface_width;
            ui_graphics.graphics_viewport_height = ui_graphics.graphics_surface_height;
        }

        Self {
            heap: SyntheticHeapState::new(cfg.heap_base, cfg.heap_size),
            fs: FsState {
                bundle_root,
                ..FsState::default()
            },
            objc: ObjcState {
                selector_pool_cursor: cfg.selector_pool_base,
                selector_pool_end: cfg.selector_pool_base.saturating_add(cfg.selector_pool_size),
                ..Default::default()
            },
            graphics: BackendGraphicsState::new(),
            scene: BackendSceneState::default(),
            host_input: HostInputState::default(),
            scheduler: BackendSchedulerState::default(),
            openal: BackendOpenAlState::new(),
            audio_queue: BackendAudioQueueState::new(),
            audio_trace: BackendAudioTraceState::default(),
            ui_objects: UIKitObjectState::default(),
            ui_runtime: UIKitRuntimeState::default(),
            ui_graphics,
            ui_network: NetworkState::default(),
            ui_cocos: CocosState::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BackendDiagnosticsState {
    pub(crate) writes: Vec<(u32, usize)>,
    pub(crate) first_instruction_addr: Option<u32>,
    pub(crate) first_instruction: Option<u32>,
    pub(crate) entry_bytes_present: bool,
    pub(crate) executed_instructions: u64,
    pub(crate) stop_reason: String,
    pub(crate) status: String,
    pub(crate) trace: Vec<String>,
    pub(crate) symbol_labels: HashMap<u32, String>,
    pub(crate) object_labels: HashMap<u32, String>,
}

impl BackendDiagnosticsState {
    fn new() -> Self {
        Self {
            writes: Vec::new(),
            first_instruction_addr: None,
            first_instruction: None,
            entry_bytes_present: false,
            executed_instructions: 0,
            stop_reason: "not-started".to_string(),
            status: "not-started".to_string(),
            trace: Vec::new(),
            symbol_labels: HashMap::new(),
            object_labels: HashMap::from([
                (HLE_FAKE_UIAPPLICATION, "UIApplication.sharedApplication".to_string()),
                (HLE_FAKE_APP_DELEGATE, "UIApplication.delegate".to_string()),
                (HLE_FAKE_UIWINDOW, "UIWindow.main".to_string()),
                (HLE_FAKE_ROOT_CONTROLLER, "UIViewController.root".to_string()),
                (HLE_FAKE_MAIN_SCREEN, "UIScreen.mainScreen".to_string()),
                (HLE_FAKE_MAIN_RUNLOOP, "NSRunLoop.mainRunLoop".to_string()),
                (HLE_FAKE_DEFAULT_MODE, "kCFRunLoopDefaultMode".to_string()),
                (HLE_FAKE_SYNTH_TIMER, "NSTimer.synthetic#0".to_string()),
                (HLE_FAKE_SYNTH_DISPLAYLINK, "CADisplayLink.synthetic#0".to_string()),
                (HLE_FAKE_NSSTRING_URL_ABSOLUTE, "NSString.synthetic.url.absoluteString".to_string()),
                (HLE_FAKE_NSSTRING_URL_HOST, "NSString.synthetic.url.host".to_string()),
                (HLE_FAKE_NSSTRING_URL_PATH, "NSString.synthetic.url.path".to_string()),
                (HLE_FAKE_NSSTRING_HTTP_METHOD, "NSString.synthetic.request.method".to_string()),
                (HLE_FAKE_NSSTRING_MIME_TYPE, "NSString.synthetic.response.mimeType".to_string()),
                (HLE_FAKE_NSSTRING_ERROR_DOMAIN, "NSString.synthetic.error.domain".to_string()),
                (HLE_FAKE_NSSTRING_ERROR_DESCRIPTION, "NSString.synthetic.error.localizedDescription".to_string()),
                (HLE_FAKE_READ_STREAM, "CFReadStream.synthetic#0".to_string()),
                (HLE_FAKE_WRITE_STREAM, "CFWriteStream.synthetic#0".to_string()),
                (HLE_FAKE_EAGL_CONTEXT, "EAGLContext.synthetic#0".to_string()),
                (HLE_FAKE_CAEAGL_LAYER, "CAEAGLLayer.synthetic#0".to_string()),
                (HLE_FAKE_GL_FRAMEBUFFER, "GLFramebuffer.synthetic#0".to_string()),
                (HLE_FAKE_GL_RENDERBUFFER, "GLRenderbuffer.synthetic#0".to_string()),
                (HLE_FAKE_UIGRAPHICS_CONTEXT, "UIGraphicsContext.synthetic#bootstrap".to_string()),
                (HLE_FAKE_UIIMAGE, "UIImage.synthetic#bootstrap".to_string()),
                (HLE_FAKE_MAIN_BUNDLE, "NSBundle.mainBundle".to_string()),
            ]),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CpuExecutionState {
    pub(crate) current_exec_pc: u32,
    pub(crate) current_exec_word: u32,
    pub(crate) current_exec_thumb: bool,
    pub(crate) vfp_d_regs: [u64; 32],
    pub(crate) vfp_multi_ops: u32,
    pub(crate) vfp_load_multi_ops: u32,
    pub(crate) vfp_store_multi_ops: u32,
    pub(crate) vfp_pc_base_ops: u32,
    pub(crate) vfp_pc_base_load_ops: u32,
    pub(crate) vfp_pc_base_store_ops: u32,
    pub(crate) vfp_single_range_ops: u32,
    pub(crate) vfp_exact_opcode_hits: u32,
    pub(crate) vfp_exact_override_hits: u32,
    pub(crate) vfp_single_transfer_ops: u32,
    pub(crate) vfp_double_transfer_ops: u32,
    pub(crate) vfp_last_op: Option<String>,
    pub(crate) vfp_last_start_addr: Option<u32>,
    pub(crate) vfp_last_end_addr: Option<u32>,
    pub(crate) vfp_last_pc_base_addr: Option<u32>,
    pub(crate) vfp_last_pc_base_word: Option<u32>,
    pub(crate) vfp_last_single_range: Option<String>,
    pub(crate) vfp_last_exact_opcode: Option<String>,
    pub(crate) vfp_last_exact_decoder_branch: Option<String>,
    pub(crate) vfp_last_transfer_mode: Option<String>,
    pub(crate) vfp_last_transfer_start_reg: Option<u32>,
    pub(crate) vfp_last_transfer_end_reg: Option<u32>,
    pub(crate) vfp_last_transfer_count: Option<u32>,
    pub(crate) vfp_last_transfer_precision: Option<String>,
    pub(crate) vfp_last_transfer_addr: Option<u32>,
    pub(crate) vfp_last_exact_reason: Option<String>,
    pub(crate) arm_reg_shift_operand2_ops: u32,
    pub(crate) arm_extra_load_store_ops: u32,
    pub(crate) arm_extra_load_store_loads: u32,
    pub(crate) arm_extra_load_store_stores: u32,
    pub(crate) arm_last_reg_shift: Option<String>,
    pub(crate) arm_last_extra_load_store: Option<String>,
    pub(crate) arm_exact_epilogue_site_hits: u32,
    pub(crate) arm_exact_epilogue_repairs: u32,
    pub(crate) arm_exact_epilogue_last_pc: Option<u32>,
    pub(crate) arm_exact_epilogue_last_before_sp: Option<u32>,
    pub(crate) arm_exact_epilogue_last_after_sp: Option<u32>,
    pub(crate) arm_exact_epilogue_last_r0: Option<u32>,
    pub(crate) arm_exact_epilogue_last_r7: Option<u32>,
    pub(crate) arm_exact_epilogue_last_r8: Option<u32>,
    pub(crate) arm_exact_epilogue_last_lr: Option<u32>,
    pub(crate) arm_exact_epilogue_last_repair: Option<String>,
}
