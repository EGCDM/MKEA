impl MemoryArm32Backend {
    // Scheduler-owned cocos action and passive loading plan behavior.

    fn ensure_synthetic_cocos_action_state(&mut self, action: u32) -> &mut SyntheticCocosAction {
        self.runtime.scheduler.actions.cocos_actions.entry(action).or_insert_with(|| SyntheticCocosAction::default())
    }

    fn note_synthetic_cocos_action_callfunc(
        &mut self,
        action: u32,
        class_hint: &str,
        target: u32,
        selector_name: &str,
        object_arg: u32,
        origin: &str,
    ) {
        let state = self.ensure_synthetic_cocos_action_state(action);
        state.class_name = class_hint.to_string();
        state.kind = if class_hint.contains("CCCallFuncND") {
            "callfuncnd".to_string()
        } else if class_hint.contains("CCCallFuncN") {
            "callfuncn".to_string()
        } else {
            "callfunc".to_string()
        };
        state.target = target;
        state.selector_name = Some(selector_name.to_string());
        state.object_arg = object_arg;
        self.diag.object_labels
            .insert(action, format!("{}.instance(synth)", if class_hint.is_empty() { "CCCallFunc" } else { class_hint }));
        self.push_callback_trace(format!(
            "action.callfunc action={} class={} target={} selector={} object={} origin={}",
            self.describe_ptr(action),
            if class_hint.is_empty() { "CCCallFunc" } else { class_hint },
            self.describe_ptr(target),
            selector_name,
            self.describe_ptr(object_arg),
            origin,
        ));
    }

    fn note_synthetic_cocos_action_delay(&mut self, action: u32, class_hint: &str, duration_bits: u32, origin: &str) {
        let state = self.ensure_synthetic_cocos_action_state(action);
        state.class_name = class_hint.to_string();
        state.kind = "delay".to_string();
        state.duration_bits = duration_bits;
        self.diag.object_labels
            .insert(action, format!("{}.instance(synth)", if class_hint.is_empty() { "CCDelayTime" } else { class_hint }));
        self.push_callback_trace(format!(
            "action.delay action={} class={} duration={:.3} ticks={} origin={}",
            self.describe_ptr(action),
            if class_hint.is_empty() { "CCDelayTime" } else { class_hint },
            Self::f32_from_bits(duration_bits),
            self.cocos_schedule_interval_ticks(duration_bits),
            origin,
        ));
    }

    fn note_synthetic_cocos_action_sequence(&mut self, action: u32, class_hint: &str, children: Vec<u32>, origin: &str) {
        let state = self.ensure_synthetic_cocos_action_state(action);
        state.class_name = class_hint.to_string();
        state.kind = if class_hint.contains("CCSpawn") { "spawn".to_string() } else { "sequence".to_string() };
        state.children = children.clone();
        self.diag.object_labels
            .insert(action, format!("{}.instance(synth)", if class_hint.is_empty() { "CCSequence" } else { class_hint }));
        let child_labels = children
            .iter()
            .map(|child| self.describe_ptr(*child))
            .collect::<Vec<_>>()
            .join(",");
        self.push_callback_trace(format!(
            "action.sequence action={} class={} children=[{}] origin={}",
            self.describe_ptr(action),
            if class_hint.is_empty() { "CCSequence" } else { class_hint },
            child_labels,
            origin,
        ));
    }


    fn note_synthetic_cocos_interval_action(
        &mut self,
        action: u32,
        class_hint: &str,
        kind: &str,
        duration_bits: u32,
        scale_x_bits: Option<u32>,
        scale_y_bits: Option<u32>,
        origin: &str,
    ) {
        let state = self.ensure_synthetic_cocos_action_state(action);
        state.class_name = class_hint.to_string();
        state.kind = kind.to_string();
        state.duration_bits = duration_bits;
        if let (Some(scale_x_bits), Some(scale_y_bits)) = (scale_x_bits, scale_y_bits) {
            state.interval_scale_x_bits = scale_x_bits;
            state.interval_scale_y_bits = scale_y_bits;
            state.interval_scale_explicit = true;
        } else {
            state.interval_scale_x_bits = 0;
            state.interval_scale_y_bits = 0;
            state.interval_scale_explicit = false;
        }
        self.diag.object_labels.insert(
            action,
            format!(
                "{}.instance(synth)",
                if class_hint.is_empty() { "CCIntervalAction" } else { class_hint }
            ),
        );
        let scale_note = if let (Some(scale_x_bits), Some(scale_y_bits)) = (scale_x_bits, scale_y_bits) {
            format!(" scale=({:.3},{:.3})", Self::f32_from_bits(scale_x_bits), Self::f32_from_bits(scale_y_bits))
        } else {
            String::new()
        };
        self.push_callback_trace(format!(
            "action.interval action={} class={} kind={} duration={:.3} ticks={}{} origin={}",
            self.describe_ptr(action),
            if class_hint.is_empty() { "CCIntervalAction" } else { class_hint },
            kind,
            Self::f32_from_bits(duration_bits),
            self.cocos_schedule_interval_ticks(duration_bits),
            scale_note,
            origin,
        ));
    }

    fn synthetic_cocos_action_duration_ticks(&self, action: u32, seen: &mut HashSet<u32>) -> u32 {
        if action == 0 || !seen.insert(action) {
            return 0;
        }
        let Some(state) = self.runtime.scheduler.actions.cocos_actions.get(&action) else {
            return 0;
        };
        match state.kind.as_str() {
            "delay" => self.cocos_schedule_interval_ticks(state.duration_bits),
            "sequence" => state.children.iter().copied().map(|child| self.synthetic_cocos_action_duration_ticks(child, seen)).sum(),
            "spawn" => state.children.iter().copied().map(|child| self.synthetic_cocos_action_duration_ticks(child, seen)).max().unwrap_or(0),
            _ => 0,
        }
    }

    fn resolve_synthetic_cocos_action_plan(
        &self,
        action: u32,
        fallback_target: u32,
        seen: &mut HashSet<u32>,
    ) -> Option<SyntheticCocosActionPlan> {
        if action == 0 || !seen.insert(action) {
            return None;
        }
        let state = self.runtime.scheduler.actions.cocos_actions.get(&action)?;
        match state.kind.as_str() {
            "callfunc" | "callfuncn" | "callfuncnd" => {
                let selector_name = state.selector_name.clone()?;
                let target = if state.target != 0 { state.target } else { fallback_target };
                if target == 0 {
                    return None;
                }
                Some(SyntheticCocosActionPlan {
                    target,
                    selector_name,
                    object_arg: state.object_arg,
                    delay_ticks: 0,
                    path: format!(
                        "{} -> target={} selector={} object={}",
                        if state.class_name.is_empty() { "CCCallFunc" } else { &state.class_name },
                        self.describe_ptr(target),
                        state.selector_name.as_deref().unwrap_or("<none>"),
                        self.describe_ptr(state.object_arg),
                    ),
                })
            }
            "sequence" => {
                let mut delay = 0u32;
                for child in &state.children {
                    let mut nested_seen = seen.clone();
                    if let Some(mut plan) = self.resolve_synthetic_cocos_action_plan(*child, fallback_target, &mut nested_seen) {
                        plan.delay_ticks = plan.delay_ticks.saturating_add(delay);
                        plan.path = format!(
                            "{} -> {}",
                            if state.class_name.is_empty() { "CCSequence" } else { &state.class_name },
                            plan.path,
                        );
                        return Some(plan);
                    }
                    let mut delay_seen = seen.clone();
                    delay = delay.saturating_add(self.synthetic_cocos_action_duration_ticks(*child, &mut delay_seen));
                }
                None
            }
            "spawn" => {
                for child in &state.children {
                    let mut nested_seen = seen.clone();
                    if let Some(mut plan) = self.resolve_synthetic_cocos_action_plan(*child, fallback_target, &mut nested_seen) {
                        plan.path = format!(
                            "{} -> {}",
                            if state.class_name.is_empty() { "CCSpawn" } else { &state.class_name },
                            plan.path,
                        );
                        return Some(plan);
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn queue_synthetic_cocos_action_plan(&mut self, owner: u32, action: u32, origin: &str) -> Option<String> {
        if owner == 0 || action == 0 {
            return None;
        }
        let owner_label = self.diag.object_labels.get(&owner).cloned().unwrap_or_default();
        let owner_is_loading = self.active_profile().is_loading_scene_label(&owner_label);
        let mut seen = HashSet::new();
        let plan = self.resolve_synthetic_cocos_action_plan(action, owner, &mut seen)?;
        let target_is_relevant = self
            .diag.object_labels
            .get(&plan.target)
            .map(|v| self.active_profile().is_loading_mission_scene_label(v) || v.contains("MissionManager"))
            .unwrap_or(false);
        if !owner_is_loading && !target_is_relevant {
            return None;
        }
        let owner_desc = self.describe_ptr(owner);
        let action_desc = self.describe_ptr(action);
        let target_desc = self.describe_ptr(plan.target);
        let object_desc = self.describe_ptr(plan.object_arg);
        let next_tick = self.runtime.ui_runtime.runloop_ticks.saturating_add(plan.delay_ticks.max(1));
        if let Some(prev) = self.runtime.scheduler.actions.passive_loading_plan.as_ref() {
            if prev.owner == owner
                && prev.target == plan.target
                && prev.selector_name == plan.selector_name
                && prev.object_arg == plan.object_arg
                && prev.delay_ticks == plan.delay_ticks
            {
                let note = format!(
                    "action passive already recorded owner={} action={} selector={} target={} delayTicks={} nextTick={}",
                    owner_desc,
                    action_desc,
                    &plan.selector_name,
                    target_desc,
                    plan.delay_ticks,
                    prev.next_tick,
                );
                self.push_callback_trace(format!("action.passive.skip {} origin={}", note, origin));
                return Some(note);
            }
        }
        self.runtime.scheduler.actions.passive_loading_plan = Some(PassiveLoadingActionPlan {
            owner,
            target: plan.target,
            selector_name: plan.selector_name.clone(),
            object_arg: plan.object_arg,
            delay_ticks: plan.delay_ticks,
            next_tick,
            path: plan.path.clone(),
        });
        self.runtime.ui_runtime.timer_armed = true;
        self.recalc_runloop_sources();
        if let Some(action_state) = self.runtime.scheduler.actions.cocos_actions.get_mut(&action) {
            action_state.last_owner = owner;
        }
        self.push_callback_trace(format!(
            "action.passive.record owner={} action={} target={} selector={} object={} delayTicks={} nextTick={} path={} origin={}",
            owner_desc,
            action_desc,
            target_desc,
            &plan.selector_name,
            object_desc,
            plan.delay_ticks,
            next_tick,
            plan.path,
            origin,
        ));
        Some(format!(
            "recorded passive action owner={} action={} selector={} target={} delayTicks={} nextTick={}",
            owner_desc,
            action_desc,
            &plan.selector_name,
            target_desc,
            plan.delay_ticks,
            next_tick,
        ))
    }

    fn note_passive_loading_callfunc(&mut self, target: u32, selector_name: &str, object_arg: u32, origin: &str) {
        let target_label = self.diag.object_labels.get(&target).cloned().unwrap_or_default();
        let relevant = self.active_profile().is_loading_scene_or_manager_label(&target_label);
        self.push_callback_trace(format!(
            "action.passive.callfunc target={} selector={} object={} relevant={} origin={}",
            self.describe_ptr(target),
            selector_name,
            self.describe_ptr(object_arg),
            if relevant { "YES" } else { "NO" },
            origin,
        ));
        if relevant {
            self.runtime.scheduler.actions.last_loading_callfunc = Some((target, selector_name.to_string(), object_arg));
        }
    }

    fn note_passive_loading_delay(&mut self, delay_bits: u32, origin: &str) {
        self.runtime.scheduler.actions.last_delay_bits = Some(delay_bits);
        self.push_callback_trace(format!(
            "action.passive.delay duration={:.3} delayBits=0x{:08x} delayTicks={} origin={}",
            Self::f32_from_bits(delay_bits),
            delay_bits,
            self.cocos_schedule_interval_ticks(delay_bits),
            origin,
        ));
    }

    fn note_passive_loading_sequence(&mut self, owner_hint: u32, origin: &str) -> Option<String> {
        let (target, selector_name, object_arg) = self.runtime.scheduler.actions.last_loading_callfunc.clone()?;
        let delay_bits = self.runtime.scheduler.actions.last_delay_bits.unwrap_or((0.25f32).to_bits());
        let delay_ticks = self.cocos_schedule_interval_ticks(delay_bits).max(1);
        let owner = if owner_hint != 0 { owner_hint } else { target };
        let next_tick = self.runtime.ui_runtime.runloop_ticks.saturating_add(delay_ticks);
        let next_tick = if let Some(prev) = self.runtime.scheduler.actions.passive_loading_plan.as_ref() {
            if prev.target == target
                && prev.selector_name == selector_name
                && prev.object_arg == object_arg
                && prev.delay_ticks == delay_ticks
            {
                prev.next_tick.min(next_tick)
            } else {
                next_tick
            }
        } else {
            next_tick
        };
        self.runtime.scheduler.actions.passive_loading_plan = Some(PassiveLoadingActionPlan {
            owner,
            target,
            selector_name: selector_name.clone(),
            object_arg,
            delay_ticks,
            next_tick,
            path: "passive-sequence".to_string(),
        });
        self.runtime.ui_runtime.timer_armed = true;
        self.recalc_runloop_sources();
        let note = format!(
            "action.passive.sequence owner={} target={} selector={} object={} delayTicks={} nextTick={} origin={}",
            self.describe_ptr(owner),
            self.describe_ptr(target),
            selector_name,
            self.describe_ptr(object_arg),
            delay_ticks,
            next_tick,
            origin,
        );
        self.push_callback_trace(note.clone());
        Some(note)
    }

    fn bind_passive_loading_plan_owner(&mut self, owner: u32, origin: &str) -> Option<String> {
        let owner_desc = self.describe_ptr(owner);
        let (target, selector_name, delay_ticks, next_tick, path) = {
            let plan = self.runtime.scheduler.actions.passive_loading_plan.as_mut()?;
            plan.owner = owner;
            let candidate_tick = self.runtime.ui_runtime.runloop_ticks.saturating_add(plan.delay_ticks.max(1));
            plan.next_tick = plan.next_tick.min(candidate_tick);
            (
                plan.target,
                plan.selector_name.clone(),
                plan.delay_ticks,
                plan.next_tick,
                plan.path.clone(),
            )
        };
        let target_desc = self.describe_ptr(target);
        self.runtime.ui_runtime.timer_armed = true;
        self.recalc_runloop_sources();
        let note = format!(
            "action.passive.bind owner={} target={} selector={} delayTicks={} nextTick={} path={} origin={}",
            owner_desc,
            target_desc,
            selector_name,
            delay_ticks,
            next_tick,
            path,
            origin,
        );
        self.push_callback_trace(note.clone());
        Some(note)
    }

    fn queue_synthetic_cocos_interval_action(&mut self, owner: u32, action: u32, origin: &str) -> Option<String> {
        if owner == 0 || action == 0 {
            return None;
        }
        let state = self.runtime.scheduler.actions.cocos_actions.get(&action)?.clone();
        let kind = state.kind.clone();
        if !kind.starts_with("interval") {
            return None;
        }
        let duration_ticks = self.cocos_schedule_interval_ticks(state.duration_bits).max(1);
        let owner_desc = self.describe_ptr(owner);
        let action_desc = self.describe_ptr(action);
        let class_name = if state.class_name.is_empty() {
            "CCIntervalAction".to_string()
        } else {
            state.class_name.clone()
        };
        self.runtime.scheduler.actions.active_interval_actions.insert(
            action,
            ActiveCocosIntervalAction {
                action,
                owner,
                class_name: class_name.clone(),
                kind: kind.clone(),
                start_tick: self.runtime.ui_runtime.runloop_ticks,
                duration_ticks,
                started: false,
                last_step_tick: 0,
                step_count: 0,
                host_scale_ready: false,
                host_start_scale_x_bits: 0,
                host_start_scale_y_bits: 0,
                host_end_scale_x_bits: 0,
                host_end_scale_y_bits: 0,
            },
        );
        if let Some(action_state) = self.runtime.scheduler.actions.cocos_actions.get_mut(&action) {
            action_state.last_owner = owner;
            action_state.queued_count = action_state.queued_count.saturating_add(1);
        }
        self.runtime.ui_runtime.timer_armed = true;
        self.recalc_runloop_sources();
        let note = format!(
            "action.interval.queue owner={} action={} class={} kind={} durationTicks={} origin={}",
            owner_desc,
            action_desc,
            class_name,
            kind,
            duration_ticks,
            origin,
        );
        self.push_callback_trace(note.clone());
        Some(note)
    }

    fn synthetic_cocos_node_scale_bits(&self, node: u32) -> (u32, u32) {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return (1.0f32.to_bits(), 1.0f32.to_bits());
        };
        let sx = if state.scale_explicit && state.scale_x_bits != 0 {
            state.scale_x_bits
        } else {
            1.0f32.to_bits()
        };
        let sy = if state.scale_explicit && state.scale_y_bits != 0 {
            state.scale_y_bits
        } else {
            1.0f32.to_bits()
        };
        (sx, sy)
    }

    fn apply_synthetic_cocos_node_scale_bits(&mut self, node: u32, scale_x_bits: u32, scale_y_bits: u32, reason: &str) {
        if node == 0 {
            return;
        }
        let changed = {
            let state = self.ensure_synthetic_sprite_state(node);
            let changed = !state.scale_explicit
                || state.scale_x_bits != scale_x_bits
                || state.scale_y_bits != scale_y_bits;
            state.scale_x_bits = scale_x_bits;
            state.scale_y_bits = scale_y_bits;
            state.scale_explicit = true;
            changed
        };
        if changed {
            let revision = self.invalidate_synthetic_widget_content(node, reason);
            self.push_callback_trace(format!(
                "action.interval.applyScale owner={} scale=({:.3},{:.3}) reason={} revision={}",
                self.describe_ptr(node),
                Self::f32_from_bits(scale_x_bits),
                Self::f32_from_bits(scale_y_bits),
                reason,
                revision,
            ));
        }
    }

    fn ensure_host_scale_interval_ready(&mut self, entry: &mut ActiveCocosIntervalAction) -> bool {
        if entry.host_scale_ready {
            return true;
        }
        let Some(action_state) = self.runtime.scheduler.actions.cocos_actions.get(&entry.action).cloned() else {
            return false;
        };
        if !action_state.interval_scale_explicit {
            return false;
        }
        let now_tick = self.runtime.ui_runtime.runloop_ticks;
        if let Some(state) = self.runtime.graphics.synthetic_sprites.get(&entry.owner) {
            let guest_tick = state.last_guest_scale_tick;
            if guest_tick != 0 && guest_tick.saturating_add(1) >= entry.start_tick && guest_tick <= now_tick {
                self.push_callback_trace(format!(
                    "action.interval.hostScale.defer action={} owner={} kind={} reason=guest-scale-write guestTick={} startTick={} nowTick={}",
                    self.describe_ptr(entry.action),
                    self.describe_ptr(entry.owner),
                    entry.kind,
                    guest_tick,
                    entry.start_tick,
                    now_tick,
                ));
                return false;
            }
        }
        let (start_x_bits, start_y_bits) = self.synthetic_cocos_node_scale_bits(entry.owner);
        let start_x = Self::f32_from_bits(start_x_bits);
        let start_y = Self::f32_from_bits(start_y_bits);
        let target_x = Self::f32_from_bits(action_state.interval_scale_x_bits);
        let target_y = Self::f32_from_bits(action_state.interval_scale_y_bits);
        let (end_x, end_y) = if entry.kind.contains("scale-by") {
            (start_x * target_x, start_y * target_y)
        } else {
            (target_x, target_y)
        };
        if !end_x.is_finite() || !end_y.is_finite() {
            return false;
        }
        entry.host_scale_ready = true;
        entry.host_start_scale_x_bits = start_x_bits;
        entry.host_start_scale_y_bits = start_y_bits;
        entry.host_end_scale_x_bits = end_x.to_bits();
        entry.host_end_scale_y_bits = end_y.to_bits();
        self.push_callback_trace(format!(
            "action.interval.hostScale action={} owner={} kind={} start=({:.3},{:.3}) end=({:.3},{:.3}) durationTicks={}",
            self.describe_ptr(entry.action),
            self.describe_ptr(entry.owner),
            entry.kind,
            start_x,
            start_y,
            end_x,
            end_y,
            entry.duration_ticks,
        ));
        true
    }

    fn drive_active_synthetic_cocos_interval_actions(&mut self, origin: &str) {
        if self.runtime.scheduler.actions.active_interval_actions.is_empty() {
            return;
        }
        let now_tick = self.runtime.ui_runtime.runloop_ticks;
        let dt_bits = if self.runtime.ui_cocos.animation_interval_bits != 0 {
            self.runtime.ui_cocos.animation_interval_bits
        } else {
            (1.0f32 / 60.0f32).to_bits()
        };
        let action_ids = self
            .runtime
            .scheduler
            .actions
            .active_interval_actions
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for action in action_ids {
            let Some(mut entry) = self.runtime.scheduler.actions.active_interval_actions.remove(&action) else {
                continue;
            };
            if entry.last_step_tick == now_tick {
                self.runtime.scheduler.actions.active_interval_actions.insert(action, entry);
                continue;
            }
            let owner_live = self.cocos_scheduled_target_is_live_in_active_graph(entry.owner);
            if !owner_live {
                self.push_callback_trace(format!(
                    "action.interval.drop action={} owner={} class={} tick={} reason=detached origin={}",
                    self.describe_ptr(entry.action),
                    self.describe_ptr(entry.owner),
                    entry.class_name,
                    now_tick,
                    origin,
                ));
                continue;
            }
            if !entry.started {
                let started = self.invoke_objc_selector_now(
                    entry.action,
                    "startWithTarget:",
                    entry.owner,
                    0,
                    180_000,
                    &format!("{}:startWithTarget", origin),
                );
                self.push_callback_trace(format!(
                    "action.interval.start action={} owner={} class={} tick={} invoked={} origin={}",
                    self.describe_ptr(entry.action),
                    self.describe_ptr(entry.owner),
                    entry.class_name,
                    now_tick,
                    if started { "YES" } else { "NO" },
                    origin,
                ));
                entry.started = true;
            }

            let elapsed = now_tick.saturating_sub(entry.start_tick).saturating_add(1);
            let denom = entry.duration_ticks.max(1);
            let progress = ((elapsed as f32) / (denom as f32)).clamp(0.0, 1.0);
            let mut stepped = false;
            let mut done_r0 = 0u32;
            let host_driven_scale = entry.kind.starts_with("interval-scale") && self.ensure_host_scale_interval_ready(&mut entry);
            if host_driven_scale {
                let start_x = Self::f32_from_bits(entry.host_start_scale_x_bits);
                let start_y = Self::f32_from_bits(entry.host_start_scale_y_bits);
                let end_x = Self::f32_from_bits(entry.host_end_scale_x_bits);
                let end_y = Self::f32_from_bits(entry.host_end_scale_y_bits);
                let cur_x = (start_x + (end_x - start_x) * progress).to_bits();
                let cur_y = (start_y + (end_y - start_y) * progress).to_bits();
                self.apply_synthetic_cocos_node_scale_bits(entry.owner, cur_x, cur_y, "action.interval.scale");
                stepped = true;
            } else {
                stepped = self.invoke_objc_selector_now(
                    entry.action,
                    "step:",
                    dt_bits,
                    0,
                    250_000,
                    &format!("{}:step", origin),
                );
                if !stepped {
                    stepped = self.invoke_objc_selector_now(
                        entry.action,
                        "update:",
                        progress.to_bits(),
                        0,
                        250_000,
                        &format!("{}:update", origin),
                    );
                }
                done_r0 = self
                    .invoke_objc_selector_now_capture_r0(
                        entry.action,
                        "isDone",
                        0,
                        0,
                        120_000,
                        &format!("{}:isDone", origin),
                    )
                    .unwrap_or(0);
            }
            let fallback_done = elapsed >= entry.duration_ticks.max(1);
            let done = if host_driven_scale { fallback_done } else { done_r0 != 0 || fallback_done };
            entry.last_step_tick = now_tick;
            entry.step_count = entry.step_count.saturating_add(1);
            if done {
                let _ = self.invoke_objc_selector_now(
                    entry.action,
                    "stop",
                    0,
                    0,
                    120_000,
                    &format!("{}:stop", origin),
                );
                self.push_callback_trace(format!(
                    "action.interval.finish action={} owner={} class={} tick={} stepped={} steps={} doneR0={} origin={}",
                    self.describe_ptr(entry.action),
                    self.describe_ptr(entry.owner),
                    entry.class_name,
                    now_tick,
                    if stepped { "YES" } else { "NO" },
                    entry.step_count,
                    self.describe_ptr(done_r0),
                    origin,
                ));
                continue;
            }
            self.push_callback_trace(format!(
                "action.interval.step action={} owner={} class={} tick={} stepped={} steps={} dt={:.6} origin={}",
                self.describe_ptr(entry.action),
                self.describe_ptr(entry.owner),
                entry.class_name,
                now_tick,
                if stepped { "YES" } else { "NO" },
                entry.step_count,
                Self::f32_from_bits(dt_bits),
                origin,
            ));
            self.runtime
                .scheduler
                .actions
                .active_interval_actions
                .insert(action, entry);
        }
    }

}
