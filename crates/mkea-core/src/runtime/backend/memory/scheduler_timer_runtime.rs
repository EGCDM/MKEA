impl MemoryArm32Backend {
    // Scheduler-owned timer/selectors behavior: cocos scheduled selectors,
    // foundation timers, delayed selectors, and passive plan materialization.

    fn capture_scheduler_callsite_provenance(&self, origin: &str) -> SchedulerCallsiteProvenance {
        SchedulerCallsiteProvenance {
            origin: origin.to_string(),
            pc: self.cpu.regs[15],
            lr: self.cpu.regs[14],
            exec_pc: self.exec.current_exec_pc,
            tick: self.runtime.ui_runtime.runloop_ticks,
        }
    }

    fn format_scheduler_callsite_provenance(&self, provenance: &SchedulerCallsiteProvenance) -> String {
        format!(
            "origin={} pc=0x{:08x} lr=0x{:08x} execPc=0x{:08x} tick={}",
            provenance.origin,
            provenance.pc,
            provenance.lr,
            provenance.exec_pc,
            provenance.tick,
        )
    }

    fn register_cocos_scheduled_selector(&mut self, target: u32, selector_name: &str, interval_bits: u32, repeats_left: Option<u32>, origin: &str) {
        if target == 0 || selector_name.trim().is_empty() {
            return;
        }
        let clean_selector = selector_name.trim_matches('\0').to_string();
        let interval_ticks = self.cocos_schedule_interval_ticks(interval_bits);
        let next_tick = self.runtime.ui_runtime.runloop_ticks.saturating_add(interval_ticks);
        let key = (target, clean_selector.clone());
        let prev_fires = self.runtime.scheduler.timers.cocos_selectors.get(&key).map(|entry| entry.fires).unwrap_or(0);
        self.runtime.scheduler.timers.cocos_selectors.insert(key, SyntheticCocosScheduledSelector {
            target,
            selector_name: clean_selector.clone(),
            interval_ticks,
            next_tick,
            repeats_left,
            fires: prev_fires,
        });
        self.diag.trace.push(format!(
            "     ↳ hle cocos.schedule target={} selector={} intervalTicks={} nextTick={} repeats={:?} origin={}",
            self.describe_ptr(target),
            clean_selector,
            interval_ticks,
            next_tick,
            repeats_left,
            origin,
        ));
    }

    fn unschedule_cocos_selector(&mut self, target: u32, selector_name: &str, origin: &str) {
        if target == 0 || selector_name.trim().is_empty() {
            return;
        }
        let clean_selector = selector_name.trim_matches('\0').to_string();
        let removed = self.runtime.scheduler.timers.cocos_selectors.remove(&(target, clean_selector.clone())).is_some();
        self.diag.trace.push(format!(
            "     ↳ hle cocos.unschedule target={} selector={} removed={} origin={}",
            self.describe_ptr(target),
            clean_selector,
            if removed { "YES" } else { "NO" },
            origin,
        ));
    }

    fn cocos_has_scheduled_selector_for_target(&self, target: u32) -> bool {
        self.runtime.scheduler.timers.cocos_selectors.values().any(|entry| entry.target == target)
    }

    fn cocos_scheduled_target_is_live_in_active_graph(&self, target: u32) -> bool {
        let target = target & 0xFFFF_FFFF;
        if target == 0 {
            return false;
        }

        let mut roots = [0u32; 4];
        let mut root_count = 0usize;
        let mut push_root = |value: u32, roots: &mut [u32; 4], root_count: &mut usize| {
            let value = value & 0xFFFF_FFFF;
            if value == 0 || roots[..*root_count].contains(&value) {
                return;
            }
            if *root_count < roots.len() {
                roots[*root_count] = value;
                *root_count += 1;
            }
        };

        push_root(self.runtime.ui_cocos.running_scene, &mut roots, &mut root_count);
        if let Some(destination) = self.resolve_transition_render_destination(self.runtime.ui_cocos.running_scene) {
            push_root(destination, &mut roots, &mut root_count);
        }
        push_root(self.runtime.scene.auto_scene_inferred_root, &mut roots, &mut root_count);
        push_root(self.runtime.scene.auto_scene_cached_root, &mut roots, &mut root_count);

        if roots[..root_count].contains(&target) {
            return true;
        }

        let mut cursor = target;
        for _ in 0..64 {
            let Some(state) = self.runtime.graphics.synthetic_sprites.get(&cursor) else {
                break;
            };
            let parent = state.parent & 0xFFFF_FFFF;
            if parent == 0 || parent == cursor {
                break;
            }
            if roots[..root_count].contains(&parent) {
                return true;
            }
            cursor = parent;
        }
        false
    }

    fn invoke_timer_style_selector_with_aliases(
        &mut self,
        target: u32,
        selector_name: &str,
        arg2_with_colon: u32,
        origin: &str,
        channel: &str,
    ) -> (bool, String) {
        let clean = selector_name.trim_matches('\0').trim();
        if clean.is_empty() {
            return (false, String::new());
        }
        let mut candidates = Vec::with_capacity(2);
        candidates.push(clean.to_string());
        if clean.ends_with(':') {
            let trimmed = clean.trim_end_matches(':').to_string();
            if !trimmed.is_empty() && trimmed != clean {
                candidates.push(trimmed);
            }
        } else {
            candidates.push(format!("{clean}:"));
        }
        candidates.dedup();
        for candidate in candidates {
            let arg2 = if candidate.ends_with(':') { arg2_with_colon } else { 0 };
            if self.invoke_objc_selector_now_resolved(target, &candidate, arg2, 0, 180_000, origin) {
                if candidate != clean {
                    self.diag.trace.push(format!(
                        "     ↳ hle {}.selector-alias target={} requested={} used={} arg2={} origin={}",
                        channel,
                        self.describe_ptr(target),
                        clean,
                        candidate,
                        self.describe_ptr(arg2),
                        origin,
                    ));
                }
                return (true, candidate);
            }
        }
        (false, clean.to_string())
    }

    fn fire_due_cocos_scheduled_selectors(&mut self, origin: &str) {
        if self.runtime.scheduler.timers.cocos_selectors.is_empty() {
            return;
        }
        let now_tick = self.runtime.ui_runtime.runloop_ticks;
        let mut due: Vec<(u32, String)> = self.runtime.scheduler
            .timers.cocos_selectors
            .iter()
            .filter_map(|((target, selector_name), entry)| {
                (entry.next_tick <= now_tick).then(|| (*target, selector_name.clone()))
            })
            .collect();
        if due.is_empty() {
            return;
        }
        due.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        let dt_bits = if self.runtime.ui_cocos.animation_interval_bits != 0 {
            self.runtime.ui_cocos.animation_interval_bits
        } else {
            (1.0f32 / 60.0f32).to_bits()
        };
        for (target, selector_name) in due {
            let key = (target, selector_name.clone());
            let Some(mut entry) = self.runtime.scheduler.timers.cocos_selectors.remove(&key) else {
                continue;
            };
            let target_live = self.cocos_scheduled_target_is_live_in_active_graph(target);
            if !target_live {
                self.diag.trace.push(format!(
                    "     ↳ hle cocos.schedule.prune-stale target={} selector={} tick={} fires={} origin={} reason=detached-from-active-scene",
                    self.describe_ptr(target),
                    selector_name,
                    now_tick,
                    entry.fires,
                    origin,
                ));
                continue;
            }
            let (invoked, used_selector) =
                self.invoke_timer_style_selector_with_aliases(target, &selector_name, dt_bits, origin, "cocos.schedule");
            entry.fires = entry.fires.saturating_add(1);
            if let Some(left) = entry.repeats_left.as_mut() {
                if *left > 0 {
                    *left -= 1;
                }
            }
            let exhausted = matches!(entry.repeats_left, Some(0));
            self.diag.trace.push(format!(
                "     ↳ hle cocos.schedule.fire target={} selector={} tick={} invoked={} fires={} exhausted={} origin={}",
                self.describe_ptr(target),
                used_selector,
                now_tick,
                if invoked { "YES" } else { "NO" },
                entry.fires,
                if exhausted { "YES" } else { "NO" },
                origin,
            ));
            if !exhausted {
                entry.next_tick = now_tick.saturating_add(entry.interval_ticks.max(1));
                self.runtime.scheduler.timers.cocos_selectors.insert(key, entry);
            }
        }
    }


    fn is_foundation_frame_driver_selector(selector_name: &str) -> bool {
        matches!(
            selector_name.trim_matches('\0').trim(),
            "mainLoop" | "mainLoop:" | "drawScene" | "drawScene:" | "drawFrame" | "drawFrame:"
        )
    }

    fn foundation_timer_is_duplicate_frame_driver_target(&self, target: u32) -> bool {
        if target == 0 {
            return false;
        }
        target == self.runtime.ui_cocos.cocos_director
            || target == self.runtime.ui_cocos.opengl_view
            || target == self.runtime.ui_objects.root_controller
    }

    fn foundation_timer_is_duplicate_frame_driver(
        &self,
        target: u32,
        selector_name: &str,
        repeats: bool,
    ) -> bool {
        repeats
            && self.runtime.ui_cocos.display_link_armed
            && Self::is_foundation_frame_driver_selector(selector_name)
            && self.foundation_timer_is_duplicate_frame_driver_target(target)
    }

    fn foundation_timer_is_duplicate_frame_driver_entry(
        &self,
        entry: &SyntheticFoundationTimer,
    ) -> bool {
        self.foundation_timer_is_duplicate_frame_driver(entry.target, &entry.selector_name, entry.repeats)
    }

    fn count_attached_foundation_timers(&self) -> usize {
        self.runtime
            .scheduler
            .timers
            .foundation_timers
            .values()
            .filter(|entry| entry.attached && !self.foundation_timer_is_duplicate_frame_driver_entry(entry))
            .count()
    }

    fn register_foundation_timer(
        &mut self,
        timer_obj: u32,
        target: u32,
        selector_name: &str,
        interval_bits: u32,
        repeats: bool,
        user_info: u32,
        attached: bool,
        origin: &str,
    ) -> u32 {
        if target == 0 || selector_name.trim().is_empty() {
            return 0;
        }
        let timer = if timer_obj != 0 {
            timer_obj
        } else {
            let label = format!("NSTimer.synthetic#{}", self.runtime.scheduler.timers.foundation_timers.len());
            let obj = self.alloc_synthetic_ui_object(label.clone());
            self.diag.object_labels.entry(obj).or_insert(label);
            obj
        };
        let clean_selector = selector_name.trim_matches('\0').to_string();
        let interval_ticks = self.cocos_schedule_interval_ticks(interval_bits);
        let next_tick = self.runtime.ui_runtime.runloop_ticks.saturating_add(interval_ticks.max(1));
        let created_from = self.capture_scheduler_callsite_provenance(origin);
        let suppress_duplicate_frame_driver =
            self.foundation_timer_is_duplicate_frame_driver(target, &clean_selector, repeats);
        let effective_attached = attached && !suppress_duplicate_frame_driver;
        let attached_from = effective_attached.then(|| created_from.clone());
        let provenance_desc = self.format_scheduler_callsite_provenance(&created_from);
        self.runtime.scheduler.timers.foundation_timers.insert(timer, SyntheticFoundationTimer {
            timer_obj: timer,
            target,
            selector_name: clean_selector.clone(),
            interval_ticks,
            next_tick,
            repeats,
            user_info,
            attached: effective_attached,
            fires: 0,
            created_from,
            attached_from,
        });
        if effective_attached {
            self.runtime.ui_runtime.timer_armed = true;
            self.recalc_runloop_sources();
        }
        self.diag.trace.push(format!(
            "     ↳ hle foundation.timer.register timer={} target={} selector={} intervalTicks={} repeats={} userInfo={} attached={} create={} origin={}",
            self.describe_ptr(timer),
            self.describe_ptr(target),
            clean_selector,
            interval_ticks,
            if repeats { "YES" } else { "NO" },
            self.describe_ptr(user_info),
            if effective_attached { "YES" } else { "NO" },
            provenance_desc,
            origin,
        ));
        if suppress_duplicate_frame_driver {
            self.diag.trace.push(format!(
                "     ↳ hle foundation.timer.suppress timer={} target={} selector={} reason=displaylink-frame-driver-duplicate origin={}",
                self.describe_ptr(timer),
                self.describe_ptr(target),
                clean_selector,
                origin,
            ));
        }
        timer
    }

    fn attach_foundation_timer(&mut self, timer_obj: u32, origin: &str) -> bool {
        let runloop_ticks = self.runtime.ui_runtime.runloop_ticks;
        let attach_provenance = self.capture_scheduler_callsite_provenance(origin);
        let create_desc = {
            let Some(entry) = self.runtime.scheduler.timers.foundation_timers.get(&timer_obj) else {
                return false;
            };
            self.format_scheduler_callsite_provenance(&entry.created_from)
        };
        let attach_desc = self.format_scheduler_callsite_provenance(&attach_provenance);
        let suppress_duplicate_frame_driver = {
            let Some(entry) = self.runtime.scheduler.timers.foundation_timers.get(&timer_obj) else {
                return false;
            };
            self.foundation_timer_is_duplicate_frame_driver_entry(entry)
        };
        let (target, selector_name, next_tick) = {
            let Some(entry) = self.runtime.scheduler.timers.foundation_timers.get_mut(&timer_obj) else {
                return false;
            };
            entry.attached = !suppress_duplicate_frame_driver;
            entry.next_tick = runloop_ticks.saturating_add(entry.interval_ticks.max(1));
            entry.attached_from = (!suppress_duplicate_frame_driver).then(|| attach_provenance.clone());
            (entry.target, entry.selector_name.clone(), entry.next_tick)
        };
        if !suppress_duplicate_frame_driver {
            self.runtime.ui_runtime.timer_armed = true;
        }
        self.recalc_runloop_sources();
        let timer_desc = self.describe_ptr(timer_obj);
        let target_desc = self.describe_ptr(target);
        self.diag.trace.push(format!(
            "     ↳ hle foundation.timer.attach timer={} target={} selector={} nextTick={} create={} attach={} origin={}",
            timer_desc,
            target_desc,
            selector_name,
            next_tick,
            create_desc,
            attach_desc,
            origin,
        ));
        if suppress_duplicate_frame_driver {
            self.diag.trace.push(format!(
                "     ↳ hle foundation.timer.suppress timer={} target={} selector={} reason=displaylink-frame-driver-duplicate attachOrigin={}",
                timer_desc,
                target_desc,
                selector_name,
                origin,
            ));
        }
        !suppress_duplicate_frame_driver
    }

    fn invalidate_foundation_timer(&mut self, timer_obj: u32, origin: &str) -> bool {
        let removed = self.runtime.scheduler.timers.foundation_timers.remove(&timer_obj).is_some();
        if self.runtime.scheduler.timers.foundation_timers.is_empty() {
            self.runtime.ui_runtime.timer_armed = false;
        }
        self.recalc_runloop_sources();
        self.diag.trace.push(format!(
            "     ↳ hle foundation.timer.invalidate timer={} removed={} origin={}",
            self.describe_ptr(timer_obj),
            if removed { "YES" } else { "NO" },
            origin,
        ));
        removed
    }

    fn fire_foundation_timer_now(&mut self, timer_obj: u32, origin: &str) -> bool {
        let Some(mut entry) = self.runtime.scheduler.timers.foundation_timers.remove(&timer_obj) else {
            return false;
        };
        let requested_selector = entry.selector_name.clone();
        let create_desc = self.format_scheduler_callsite_provenance(&entry.created_from);
        let attach_desc = entry
            .attached_from
            .as_ref()
            .map(|value| self.format_scheduler_callsite_provenance(value))
            .unwrap_or_else(|| "<detached>".to_string());
        let suppress_duplicate_frame_driver = self.foundation_timer_is_duplicate_frame_driver_entry(&entry);
        let (invoked, used_selector) = if suppress_duplicate_frame_driver {
            (false, requested_selector.clone())
        } else {
            self.invoke_timer_style_selector_with_aliases(
                entry.target,
                &requested_selector,
                entry.user_info,
                origin,
                "foundation.timer",
            )
        };
        entry.fires = entry.fires.saturating_add(1);
        self.diag.trace.push(format!(
            "     ↳ hle foundation.timer.fire timer={} target={} selector={} requestedSelector={} invoked={} fires={} repeats={} create={} attach={} origin={}",
            self.describe_ptr(timer_obj),
            self.describe_ptr(entry.target),
            used_selector,
            requested_selector,
            if invoked { "YES" } else { "NO" },
            entry.fires,
            if entry.repeats { "YES" } else { "NO" },
            create_desc,
            attach_desc,
            origin,
        ));
        if suppress_duplicate_frame_driver {
            self.diag.trace.push(format!(
                "     ↳ hle foundation.timer.suppress timer={} target={} selector={} reason=displaylink-frame-driver-duplicate fireOrigin={}",
                self.describe_ptr(timer_obj),
                self.describe_ptr(entry.target),
                requested_selector,
                origin,
            ));
        }
        if entry.repeats {
            entry.next_tick = self.runtime.ui_runtime.runloop_ticks.saturating_add(entry.interval_ticks.max(1));
            entry.attached = !suppress_duplicate_frame_driver;
            self.runtime.scheduler.timers.foundation_timers.insert(timer_obj, entry);
        } else {
            if self.runtime.scheduler.timers.foundation_timers.is_empty() {
                self.runtime.ui_runtime.timer_armed = false;
            }
            self.recalc_runloop_sources();
        }
        invoked
    }

    fn fire_due_foundation_timers(&mut self, origin: &str) {
        if self.runtime.scheduler.timers.foundation_timers.is_empty() {
            return;
        }
        let now_tick = self.runtime.ui_runtime.runloop_ticks;
        let mut due: Vec<u32> = self.runtime.scheduler.timers.foundation_timers.iter()
            .filter_map(|(timer, entry)| (entry.attached && entry.next_tick <= now_tick).then_some(*timer))
            .collect();
        due.sort_unstable();
        for timer in due {
            let _ = self.fire_foundation_timer_now(timer, origin);
        }
    }

    fn schedule_delayed_selector(&mut self, target: u32, selector_name: &str, object_arg: u32, delay_bits: u32, origin: &str) {
        if target == 0 || selector_name.trim().is_empty() {
            return;
        }
        let delay_ticks = self.cocos_schedule_interval_ticks(delay_bits);
        let next_tick = self.runtime.ui_runtime.runloop_ticks.saturating_add(delay_ticks.max(1));
        self.runtime.scheduler.timers.delayed_selectors.push(SyntheticDelayedSelector {
            target,
            selector_name: selector_name.trim_matches('\0').to_string(),
            object_arg,
            next_tick,
            fires: 0,
        });
        self.note_scheduler_selector_handoff(
            origin,
            target,
            target,
            selector_name.trim_matches('\0'),
            object_arg,
            "delayed selector registered",
        );
        self.runtime.ui_runtime.timer_armed = true;
        self.recalc_runloop_sources();
        self.diag.trace.push(format!(
            "     ↳ hle delayed-selector.register target={} selector={} object={} delayTicks={} nextTick={} origin={}",
            self.describe_ptr(target),
            selector_name.trim_matches('\0'),
            self.describe_ptr(object_arg),
            delay_ticks,
            next_tick,
            origin,
        ));
    }

    fn fire_due_delayed_selectors(&mut self, origin: &str) {
        if self.runtime.scheduler.timers.delayed_selectors.is_empty() {
            return;
        }
        let now_tick = self.runtime.ui_runtime.runloop_ticks;
        let mut ready = Vec::new();
        let mut pending = Vec::new();
        for entry in self.runtime.scheduler.timers.delayed_selectors.drain(..) {
            if entry.next_tick <= now_tick {
                ready.push(entry);
            } else {
                pending.push(entry);
            }
        }
        self.runtime.scheduler.timers.delayed_selectors = pending;
        for mut entry in ready {
            let arg2 = if entry.selector_name.ends_with(':') { entry.object_arg } else { 0 };
            self.note_scheduler_selector_handoff(
                origin,
                entry.target,
                entry.target,
                &entry.selector_name,
                arg2,
                "delayed selector fired",
            );
            let invoked = self.invoke_objc_selector_now_resolved(entry.target, &entry.selector_name, arg2, 0, 180_000, origin);
            entry.fires = entry.fires.saturating_add(1);
            self.diag.trace.push(format!(
                "     ↳ hle delayed-selector.fire target={} selector={} object={} tick={} invoked={} fires={} origin={}",
                self.describe_ptr(entry.target),
                entry.selector_name,
                self.describe_ptr(entry.object_arg),
                now_tick,
                if invoked { "YES" } else { "NO" },
                entry.fires,
                origin,
            ));
        }
        if self.runtime.scheduler.timers.foundation_timers.is_empty() && self.runtime.scheduler.timers.delayed_selectors.is_empty() {
            self.recalc_runloop_sources();
        }
    }

    fn delayed_selector_is_loading_relevant(&self, entry: &SyntheticDelayedSelector) -> bool {
        if matches!(entry.selector_name.as_str(), "foo" | "foo:") {
            return true;
        }
        let target_label = self.diag.object_labels.get(&entry.target).map(|v| v.as_str()).unwrap_or("");
        self.active_profile().is_loading_scene_or_manager_label(target_label)
    }

    fn passive_loading_plan_is_relevant(&self, plan: &PassiveLoadingActionPlan) -> bool {
        if matches!(plan.selector_name.as_str(), "foo" | "foo:") {
            return true;
        }
        let owner_label = self.diag.object_labels.get(&plan.owner).map(|v| v.as_str()).unwrap_or("");
        let target_label = self.diag.object_labels.get(&plan.target).map(|v| v.as_str()).unwrap_or("");
        self.active_profile().is_loading_scene_label(owner_label)
            || self.active_profile().is_loading_scene_or_manager_label(target_label)
    }

    fn has_loading_relevant_delayed_selectors(&self) -> bool {
        self.runtime.scheduler.timers.delayed_selectors
            .iter()
            .any(|entry| self.delayed_selector_is_loading_relevant(entry))
    }

    fn has_loading_relevant_passive_plan(&self) -> bool {
        self.runtime.scheduler.actions.passive_loading_plan
            .as_ref()
            .map(|plan| self.passive_loading_plan_is_relevant(plan))
            .unwrap_or(false)
    }

    fn materialize_passive_loading_action_plan(&mut self, origin: &str, force: bool) -> bool {
        let Some(plan) = self.runtime.scheduler.actions.passive_loading_plan.clone() else {
            return false;
        };
        if !self.passive_loading_plan_is_relevant(&plan) {
            self.push_callback_trace(format!(
                "action.passive.drop owner={} target={} selector={} object={} nextTick={} path={} force={} origin={}",
                self.describe_ptr(plan.owner),
                self.describe_ptr(plan.target),
                plan.selector_name,
                self.describe_ptr(plan.object_arg),
                plan.next_tick,
                plan.path,
                if force { "YES" } else { "NO" },
                origin,
            ));
            self.runtime.scheduler.actions.passive_loading_plan = None;
            return false;
        }
        let now_tick = self.runtime.ui_runtime.runloop_ticks;
        if !force && plan.next_tick > now_tick {
            return false;
        }
        if force && self.runtime.ui_runtime.runloop_ticks < plan.next_tick {
            self.runtime.ui_runtime.runloop_ticks = plan.next_tick;
        }
        let scheduled_tick = self.runtime.ui_runtime.runloop_ticks.max(plan.next_tick);
        let duplicate = self.runtime.scheduler.timers.delayed_selectors.iter().any(|entry| {
            entry.target == plan.target
                && entry.selector_name == plan.selector_name
                && entry.object_arg == plan.object_arg
        });
        self.push_callback_trace(format!(
            "action.passive.materialize owner={} target={} selector={} object={} scheduledTick={} path={} force={} duplicate={} origin={}",
            self.describe_ptr(plan.owner),
            self.describe_ptr(plan.target),
            plan.selector_name,
            self.describe_ptr(plan.object_arg),
            scheduled_tick,
            plan.path,
            if force { "YES" } else { "NO" },
            if duplicate { "YES" } else { "NO" },
            origin,
        ));
        self.runtime.scheduler.actions.passive_loading_plan = None;
        if duplicate {
            return false;
        }
        self.runtime.scheduler.timers.delayed_selectors.push(SyntheticDelayedSelector {
            target: plan.target,
            selector_name: plan.selector_name,
            object_arg: plan.object_arg,
            next_tick: scheduled_tick,
            fires: 0,
        });
        self.runtime.ui_runtime.timer_armed = true;
        self.recalc_runloop_sources();
        true
    }

    fn flush_loading_delayed_selectors_before_shutdown(&mut self, origin: &str, max_extra_ticks: u32) -> u32 {
        let has_relevant = self.has_loading_relevant_delayed_selectors() || self.has_loading_relevant_passive_plan();
        if !has_relevant {
            return 0;
        }

        let start_tick = self.runtime.ui_runtime.runloop_ticks;
        let pending_before = self.runtime.scheduler.timers.delayed_selectors.len();
        let mut extra_ticks = 0u32;
        while extra_ticks < max_extra_ticks
            && (self.has_loading_relevant_delayed_selectors() || self.has_loading_relevant_passive_plan())
        {
            self.push_synthetic_runloop_tick(origin, true);
            extra_ticks = extra_ticks.saturating_add(1);
        }

        let mut remaining_relevant = self.runtime.scheduler
            .timers.delayed_selectors
            .iter()
            .filter(|entry| self.delayed_selector_is_loading_relevant(entry))
            .count();
        if remaining_relevant > 0 {
            if let Some(force_tick) = self.runtime.scheduler
                .timers.delayed_selectors
                .iter()
                .filter(|entry| self.delayed_selector_is_loading_relevant(entry))
                .map(|entry| entry.next_tick)
                .max()
            {
                self.runtime.ui_runtime.runloop_ticks = self.runtime.ui_runtime.runloop_ticks.max(force_tick);
                self.fire_due_delayed_selectors(&format!("{}-force", origin));
            }
            remaining_relevant = self.runtime.scheduler
                .timers.delayed_selectors
                .iter()
                .filter(|entry| self.delayed_selector_is_loading_relevant(entry))
                .count();
        }

        let mut passive_fired = false;
        if remaining_relevant == 0 && self.materialize_passive_loading_action_plan(origin, true) {
            self.fire_due_delayed_selectors(&format!("{}-passive", origin));
            passive_fired = true;
            remaining_relevant = self.runtime.scheduler
                .timers.delayed_selectors
                .iter()
                .filter(|entry| self.delayed_selector_is_loading_relevant(entry))
                .count();
        }

        self.push_callback_trace(format!(
            "shutdown.delayed-selector.flush origin={} startTick={} endTick={} extraTicks={} pendingBefore={} pendingAfter={} remainingRelevant={} passiveFired={}",
            origin,
            start_tick,
            self.runtime.ui_runtime.runloop_ticks,
            extra_ticks,
            pending_before,
            self.runtime.scheduler.timers.delayed_selectors.len(),
            remaining_relevant,
            if passive_fired { "YES" } else { "NO" },
        ));
        self.runtime.ui_runtime.runloop_ticks.saturating_sub(start_tick)
    }

}
