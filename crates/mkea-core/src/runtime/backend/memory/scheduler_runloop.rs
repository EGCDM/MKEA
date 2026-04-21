const TRACE_VERIFY_BUILD_ID: &str = "TRACE_VERIFY_V2_20260416B";

impl MemoryArm32Backend {
    // Core synthetic runloop bookkeeping and high-level bootstrap wiring.

    fn trace_build_id(&self) -> &'static str {
        TRACE_VERIFY_BUILD_ID
    }

    fn emit_trace_build_banner(&mut self, origin: &str) {
        if self.runtime.scheduler.trace.build_banner_emitted {
            return;
        }
        self.runtime.scheduler.trace.build_banner_emitted = true;
        let banner = format!(
            "trace.verify build={} origin={} runloopLive={} appActive={} timerArmed={} sources={}",
            self.trace_build_id(),
            origin,
            if self.runtime.ui_runtime.runloop_live { "YES" } else { "NO" },
            if self.runtime.ui_runtime.app_active { "YES" } else { "NO" },
            if self.runtime.ui_runtime.timer_armed { "YES" } else { "NO" },
            self.current_runloop_source_count(),
        );
        self.diag.trace.push(format!("     ↳ {}", banner));
        self.push_scheduler_trace(banner.clone());
        self.push_callback_trace(banner);
    }

    fn current_runloop_source_count(&self) -> u32 {
        let mut sources = 0;
        if self.runtime.ui_runtime.timer_armed
            || self.count_attached_foundation_timers() > 0
            || !self.runtime.scheduler.timers.delayed_selectors.is_empty()
        {
            sources += 1;
        }
        if self.runtime.ui_cocos.display_link_armed {
            sources += 1;
        }
        if self.runtime.ui_network.reachability_scheduled {
            sources += 1;
        }
        if self.runtime.ui_network.read_stream_scheduled {
            sources += 1;
        }
        if self.runtime.ui_network.write_stream_scheduled {
            sources += 1;
        }
        if self.runtime.ui_network.network_armed && !self.runtime.ui_network.network_completed {
            sources += 1;
        }
        if self.runtime.ui_network.network_timeout_armed {
            sources += 1;
        }
        sources
    }

    fn recalc_runloop_sources(&mut self) {
        self.runtime.ui_runtime.runloop_sources = self.current_runloop_source_count();
    }

    fn bootstrap_synthetic_runloop(&mut self) {
        self.install_uikit_labels();
        self.bootstrap_synthetic_graphics();
        self.emit_trace_build_banner("bootstrap_synthetic_runloop");
        self.runtime.ui_runtime.runloop_live = true;
        self.runtime.ui_runtime.timer_armed = true;
        self.runtime.ui_cocos.display_link_armed = true;
        // Do not auto-arm synthetic network or reachability here.
        // Those sources must only become live after the guest explicitly
        // constructs/schedules them; otherwise we fabricate bootstrap traffic
        // and delegate callbacks that never existed in guest code.
        self.runtime.ui_runtime.window_visible = true;
        if !self.runtime.ui_runtime.app_active {
            self.runtime.ui_runtime.activation_count = self.runtime.ui_runtime.activation_count.saturating_add(1);
        }
        self.runtime.ui_runtime.app_active = true;
        self.recalc_runloop_sources();
        if self.runtime.ui_objects.first_responder == 0 {
            self.runtime.ui_objects.first_responder = self.runtime.ui_objects.root_controller;
        }
    }

    fn trace_runloop_mode_tick(&mut self, origin: &str, handled_source: bool) {
        self.emit_trace_build_banner(origin);
        self.diag.trace.push(format!(
            "     ↳ hle CFRunLoopRunInMode(mode={}, seconds=0.016667, returnAfter=0, origin={}) -> {}",
            self.describe_ptr(self.runtime.ui_objects.default_mode),
            origin,
            if handled_source { "HandledSource" } else { "TimedOut" },
        ));
        if self.runtime.ui_runtime.timer_armed {
            self.diag.trace.push(format!(
                "     ↳ hle NSTimer.fire {} target={} selector=applicationDidBecomeActive: foundationTimers={} delayedSelectors={}",
                self.describe_ptr(self.runtime.ui_objects.synthetic_timer),
                self.describe_ptr(self.runtime.ui_objects.delegate),
                self.count_attached_foundation_timers(),
                self.runtime.scheduler.timers.delayed_selectors.len(),
            ));
        }
    }

    fn finish_synthetic_runloop_tick(
        &mut self,
        tick: u32,
        origin: &str,
        handled_source: bool,
        sources_before: u32,
    ) {
        let sources_after = self.current_runloop_source_count();
        self.runtime.ui_runtime.runloop_sources = sources_after;
        self.runtime.ui_runtime.last_tick_sources_after = sources_after;
        self.diag.trace.push(format!(
            "     ↳ runloop tick#{} main={} sources={}->{} window={} firstResponder={}",
            tick,
            self.describe_ptr(self.runtime.ui_objects.main_runloop),
            sources_before,
            sources_after,
            self.describe_ptr(self.runtime.ui_objects.window),
            self.describe_ptr(self.runtime.ui_objects.first_responder),
        ));
        note_runloop_tick(tick, origin, handled_source, sources_before, sources_after);
    }
}
