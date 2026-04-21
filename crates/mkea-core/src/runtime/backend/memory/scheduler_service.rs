impl MemoryArm32Backend {
    // Synthetic scheduler/runloop orchestration stays here, while domain-specific
    // source handling now lives next to the domains it drives.

    fn push_synthetic_runloop_tick(&mut self, origin: &str, handled_source: bool) {
        self.bootstrap_synthetic_runloop();
        self.runtime.ui_runtime.runloop_ticks = self.runtime.ui_runtime.runloop_ticks.saturating_add(1);
        let tick = self.runtime.ui_runtime.runloop_ticks;
        let sources_before = self.runtime.ui_runtime.runloop_sources;
        self.runtime.ui_runtime.last_tick_sources_before = sources_before;

        self.advance_guest_wallclock_for_runloop_tick();
        self.trace_runloop_mode_tick(origin, handled_source);
        self.drive_synthetic_movie_players(origin);
        self.drive_runloop_animation_sources(origin);
        self.drive_runloop_reachability_source();
        self.poll_runloop_stream_sources();
        self.drive_runloop_network_sources();
        self.process_pending_host_input(origin);
        self.maybe_drive_synthetic_scene_progression(origin);
        self.finish_synthetic_runloop_tick(tick, origin, handled_source, sources_before);
    }
}
