#[derive(Debug, Clone)]
pub(crate) struct UIKitObjectState {
    app: u32,
    delegate: u32,
    window: u32,
    root_controller: u32,
    screen: u32,
    main_runloop: u32,
    default_mode: u32,
    synthetic_timer: u32,
    first_responder: u32,
    view_superviews: std::collections::HashMap<u32, u32>,
    view_subviews: std::collections::HashMap<u32, Vec<u32>>,
    view_frames_bits: std::collections::HashMap<u32, [u32; 4]>,
    view_bounds_bits: std::collections::HashMap<u32, [u32; 4]>,
    view_content_scale_bits: std::collections::HashMap<u32, u32>,
    view_layers: std::collections::HashMap<u32, u32>,
    layer_host_views: std::collections::HashMap<u32, u32>,
}

impl Default for UIKitObjectState {
    fn default() -> Self {
        Self {
            app: HLE_FAKE_UIAPPLICATION,
            delegate: HLE_FAKE_APP_DELEGATE,
            window: HLE_FAKE_UIWINDOW,
            root_controller: HLE_FAKE_ROOT_CONTROLLER,
            screen: HLE_FAKE_MAIN_SCREEN,
            main_runloop: HLE_FAKE_MAIN_RUNLOOP,
            default_mode: HLE_FAKE_DEFAULT_MODE,
            synthetic_timer: HLE_FAKE_SYNTH_TIMER,
            first_responder: HLE_FAKE_ROOT_CONTROLLER,
            view_superviews: std::collections::HashMap::new(),
            view_subviews: std::collections::HashMap::new(),
            view_frames_bits: std::collections::HashMap::new(),
            view_bounds_bits: std::collections::HashMap::new(),
            view_content_scale_bits: std::collections::HashMap::new(),
            view_layers: std::collections::HashMap::new(),
            layer_host_views: std::collections::HashMap::new(),
        }
    }
}
