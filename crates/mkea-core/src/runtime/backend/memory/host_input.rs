impl MemoryArm32Backend {
// Host input ingestion and synthetic touch dispatch.

    fn bootstrap_host_input_script(&mut self) {
        let Some(path) = self.tuning.host_input_script_path.clone() else {
            return;
        };
        match Self::load_host_input_script(&path) {
            Ok(queue) => {
                self.runtime.host_input.events_loaded = queue.len() as u32;
                self.runtime.host_input.queue = queue;
                self.runtime.host_input.script_offset = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                self.runtime.host_input.script_remainder.clear();
                self.runtime.host_input.last_dispatch = Some(format!(
                    "loaded:{}:{}",
                    path.display(),
                    self.runtime.host_input.events_loaded
                ));
                self.diag.trace.push(format!(
                    "loaded host input script {} events={} host={}x{} flipY={}",
                    path.display(),
                    self.runtime.host_input.events_loaded,
                    self.tuning.host_input_width,
                    self.tuning.host_input_height,
                    if self.tuning.host_input_flip_y { "YES" } else { "NO" },
                ));
            }
            Err(err) => {
                self.runtime.host_input.last_dispatch = Some(format!("load-error:{}", err));
                self.diag.trace.push(format!(
                    "host input script load failed {}: {}",
                    path.display(),
                    err,
                ));
            }
        }
    }

    fn load_host_input_script(path: &Path) -> CoreResult<VecDeque<ScriptedPointerEvent>> {
        let text = fs::read_to_string(path)
            .map_err(|err| CoreError::Backend(format!("failed to read input script {}: {}", path.display(), err)))?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(VecDeque::new());
        }

        let jsonl_fallback = || -> CoreResult<VecDeque<ScriptedPointerEvent>> {
            let mut items = VecDeque::new();
            for (index, line) in text.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let event: ScriptedPointerEvent = serde_json::from_str(line).map_err(|err| {
                    CoreError::Backend(format!(
                        "invalid input script line {} in {}: {}",
                        index + 1,
                        path.display(),
                        err,
                    ))
                })?;
                items.push_back(event);
            }
            Ok(items)
        };

        if trimmed.starts_with('[') {
            let items: Vec<ScriptedPointerEvent> = serde_json::from_str(trimmed)
                .map_err(|err| CoreError::Backend(format!("invalid input script array {}: {}", path.display(), err)))?;
            return Ok(items.into());
        }

        if trimmed.starts_with('{') {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(events) = value.get("events") {
                    let items: Vec<ScriptedPointerEvent> = serde_json::from_value(events.clone())
                        .map_err(|err| CoreError::Backend(format!("invalid input script events {}: {}", path.display(), err)))?;
                    return Ok(items.into());
                }
                if let Ok(one) = serde_json::from_value::<ScriptedPointerEvent>(value.clone()) {
                    return Ok(VecDeque::from(vec![one]));
                }
            }
            return jsonl_fallback();
        }

        jsonl_fallback()
    }

    fn poll_host_input_script_file(&mut self) {
        let Some(path) = self.tuning.host_input_script_path.clone() else {
            return;
        };
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        if (bytes.len() as u64) < self.runtime.host_input.script_offset {
            self.runtime.host_input.script_offset = 0;
            self.runtime.host_input.script_remainder.clear();
        }
        let start = self.runtime.host_input.script_offset as usize;
        if start >= bytes.len() {
            return;
        }
        let tail = &bytes[start..];
        self.runtime.host_input.script_offset = bytes.len() as u64;

        let mut chunk = String::new();
        if !self.runtime.host_input.script_remainder.is_empty() {
            chunk.push_str(&self.runtime.host_input.script_remainder);
            self.runtime.host_input.script_remainder.clear();
        }
        chunk.push_str(&String::from_utf8_lossy(tail));
        let ends_with_newline = chunk.ends_with('\n') || chunk.ends_with('\r');
        let mut lines: Vec<&str> = chunk.lines().collect();
        if !ends_with_newline {
            if let Some(last) = lines.pop() {
                self.runtime.host_input.script_remainder = last.to_string();
            }
        }

        for line in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match serde_json::from_str::<ScriptedPointerEvent>(line) {
                Ok(event) => {
                    self.runtime.host_input.events_loaded = self.runtime.host_input.events_loaded.saturating_add(1);
                    self.runtime.host_input.queue.push_back(event);
                }
                Err(err) => {
                    self.runtime.host_input.events_ignored = self.runtime.host_input.events_ignored.saturating_add(1);
                    self.runtime.host_input.last_dispatch = Some(format!("poll-parse-error:{}", err));
                    self.diag.trace.push(format!(
                        "     ↳ host input poll ignored malformed line path={} err={} line={}",
                        path.display(),
                        err,
                        line,
                    ));
                }
            }
        }
    }
    fn canonical_host_input_phase(phase: &str) -> Option<&'static str> {
        let phase = phase.trim();
        if phase.is_empty() {
            return None;
        }
        match phase.to_ascii_lowercase().as_str() {
            "down" | "begin" | "start" | "press" => Some("down"),
            "move" | "drag" | "hover" => Some("move"),
            "up" | "end" | "release" => Some("up"),
            _ => None,
        }
    }

    fn normalize_host_pointer_event(&self, event: &ScriptedPointerEvent) -> Option<(u32, f32, f32, String)> {
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1);
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1);
        let mut host_w = event.host_width.unwrap_or(self.tuning.host_input_width);
        let mut host_h = event.host_height.unwrap_or(self.tuning.host_input_height);
        if host_w == 0 {
            host_w = surface_w;
        }
        if host_h == 0 {
            host_h = surface_h;
        }
        if host_w == 0 || host_h == 0 || !event.px.is_finite() || !event.py.is_finite() {
            return None;
        }
        let mut x = event.px * surface_w as f32 / host_w as f32;
        let mut y = event.py * surface_h as f32 / host_h as f32;
        let flip_y = event.flip_y.unwrap_or(self.tuning.host_input_flip_y);
        if flip_y {
            y = surface_h as f32 - y;
        }
        let max_x = (surface_w.saturating_sub(1)) as f32;
        let max_y = (surface_h.saturating_sub(1)) as f32;
        x = x.clamp(0.0, max_x.max(0.0));
        y = y.clamp(0.0, max_y.max(0.0));
        let pointer_id = if event.pointer_id == 0 { 1 } else { event.pointer_id };
        let source = event.source.clone().unwrap_or_else(|| format!("script@{}x{}", host_w, host_h));
        Some((pointer_id, x, y, source))
    }

    fn synthetic_node_hit_bounds(&self, node: u32) -> Option<(f32, f32, f32, f32)> {
        let state = self.runtime.graphics.synthetic_sprites.get(&node)?;
        if !state.visible {
            return None;
        }
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let mut draw_w = state.width;
        let mut draw_h = state.height;
        if state.texture != 0 {
            if let Some(texture) = self.runtime.graphics.synthetic_textures.get(&state.texture) {
                if draw_w == 0 {
                    draw_w = texture.width;
                }
                if draw_h == 0 {
                    draw_h = texture.height;
                }
            }
        }
        if draw_w == 0 || draw_h == 0 {
            if let Some(text_backing) = self.string_backing(node) {
                let scale = Self::synthetic_text_scale_for_height(draw_h.max(14));
                let (text_w, text_h) = Self::synthetic_text_dimensions_5x7(&text_backing.text, scale);
                if draw_w == 0 {
                    draw_w = text_w.max(1);
                }
                if draw_h == 0 {
                    draw_h = text_h.max(1);
                }
            }
            if label.contains("CCColorLayer") || label.contains("CCScene") || label.contains("MenuLayer") || label.contains("FirstScene") || Self::is_transition_like_label(&label) {
                if draw_w == 0 {
                    draw_w = self.runtime.ui_graphics.graphics_surface_width.max(1);
                }
                if draw_h == 0 {
                    draw_h = self.runtime.ui_graphics.graphics_surface_height.max(1);
                }
            }
        }
        if draw_w == 0 || draw_h == 0 {
            return None;
        }

        let Some((draw_x, draw_y, mapped_w, mapped_h)) = self.synthetic_surface_rect_for_node(node, draw_w.max(1), draw_h.max(1)) else {
            return None;
        };
        Some((draw_x as f32, draw_y as f32, mapped_w as f32, mapped_h as f32))
    }

    fn synthetic_node_contains_guest_point(&self, node: u32, x: f32, y: f32) -> bool {
        let Some((left, top, width, height)) = self.synthetic_node_hit_bounds(node) else {
            return false;
        };
        x >= left && y >= top && x <= (left + width) && y <= (top + height)
    }

    fn synthetic_touch_target_priority(&self, node: u32, state: &SyntheticSpriteState, depth: u32) -> i32 {
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let mut score = 0i32;
        if state.touch_enabled {
            score += 16;
        }
        if state.callback_selector != 0 {
            score += 160;
        }
        if state.callback_target != 0 {
            score += 24;
        }
        if state.children == 0 {
            score += 12;
        }
        if state.texture != 0 {
            score += 6;
        }
        if label.contains("GUIButton") {
            score += 240;
        } else if label.contains("GUICheckBox") {
            score += 220;
        } else if label.contains("MenuItem") {
            score += 210;
        } else if label.contains("EquipBoard") || label.contains("StageBoard") {
            score += 180;
        } else if label.contains("GUI") {
            score += 120;
        } else if label.contains("Label") {
            score -= 80;
        } else if label.contains("MenuLayer") || label.contains("CCScene") || label.contains("CocosNode") {
            score -= 40;
        }
        score += (depth as i32) * 4;
        score += state.z_order.clamp(-32, 32);
        score
    }

    fn synthetic_touch_proxy_target_for_node(&self, node: u32) -> Option<u32> {
        let mut current = node;
        let mut hops = 0u32;
        while current != 0 && hops < 64 {
            let state = self.runtime.graphics.synthetic_sprites.get(&current)?;
            if state.visible && state.touch_enabled {
                return Some(current);
            }
            current = state.parent;
            hops = hops.saturating_add(1);
        }
        None
    }

    fn find_synthetic_touch_target_at(&self, root: u32, x: f32, y: f32) -> Option<u32> {
        if root == 0 {
            return None;
        }
        let mut stack = vec![(root, 0u32)];
        let mut seen = HashSet::new();
        let mut best: Option<(i32, u32, u32)> = None;
        while let Some((node, depth)) = stack.pop() {
            if node == 0 || !seen.insert(node) {
                continue;
            }
            let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
                continue;
            };
            if state.children != 0 {
                if let Some(arr) = self.runtime.graphics.synthetic_arrays.get(&state.children) {
                    for child in arr.items.iter().rev() {
                        if *child != 0 {
                            stack.push((*child, depth.saturating_add(1)));
                        }
                    }
                }
            }
            if !state.visible || !self.synthetic_node_contains_guest_point(node, x, y) {
                continue;
            }
            let Some(candidate) = self.synthetic_touch_proxy_target_for_node(node) else {
                continue;
            };
            let Some(candidate_state) = self.runtime.graphics.synthetic_sprites.get(&candidate) else {
                continue;
            };
            let score = self.synthetic_touch_target_priority(candidate, candidate_state, depth);
            let replace = match best {
                Some((best_score, best_depth, best_node)) => {
                    score > best_score
                        || (score == best_score && depth > best_depth)
                        || (score == best_score && depth == best_depth && candidate > best_node)
                }
                None => true,
            };
            if replace {
                best = Some((score, depth, candidate));
            }
        }
        best.map(|(_, _, node)| node)
    }

    fn synthetic_callback_for_node(&self, node: u32) -> Option<(u32, u32, String)> {
        let state = self.runtime.graphics.synthetic_sprites.get(&node)?;
        if state.callback_selector == 0 {
            return None;
        }
        let callback_target = if state.callback_target != 0 { state.callback_target } else { node };
        let selector_name = self
            .objc_read_selector_name(state.callback_selector)
            .unwrap_or_else(|| format!("0x{:08x}", state.callback_selector));
        Some((callback_target, state.callback_selector, selector_name))
    }

    fn synthetic_callback_selector_name(&self, callback_selector: u32) -> Option<String> {
        if callback_selector == 0 {
            return None;
        }
        let selector_name = self
            .objc_read_selector_name(callback_selector)
            .unwrap_or_else(|| format!("0x{:08x}", callback_selector));
        let trimmed = selector_name.trim_matches('\0').trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn selector_trace_mentions_selector(dispatch_selector: &str, callback_selector: &str) -> bool {
        let dispatch = dispatch_selector.trim_matches('\0').trim();
        let callback = callback_selector.trim_matches('\0').trim();
        if dispatch.is_empty() || callback.is_empty() {
            return false;
        }
        dispatch
            .split('+')
            .map(|part| part.trim_matches('\0').trim().trim_start_matches("widget:").trim())
            .any(|part| !part.is_empty() && part == callback)
    }

    fn is_touch_phase_selector(selector: &str) -> bool {
        matches!(
            selector.trim_matches('\0').trim(),
            "touchesBegan:withEvent:"
                | "touchesMoved:withEvent:"
                | "touchesEnded:withEvent:"
                | "touchesCancelled:withEvent:"
                | "touchesBegan"
                | "touchesMoved"
                | "touchesEnded"
                | "touchesCancelled"
                | "ccTouchesBegan:withEvent:"
                | "ccTouchesMoved:withEvent:"
                | "ccTouchesEnded:withEvent:"
                | "ccTouchesCancelled:withEvent:"
                | "ccTouchBegan:withEvent:"
                | "ccTouchMoved:withEvent:"
                | "ccTouchEnded:withEvent:"
                | "ccTouchCancelled:withEvent:"
        )
    }

    fn should_post_dispatch_hit_callback(
        &self,
        _target: u32,
        callback_target: u32,
        callback_selector: u32,
        selector_name: &str,
        _ui_dispatch_target: u32,
        _cocos_dispatch_target: u32,
        ui_selector: Option<&str>,
        cocos_selector: Option<&str>,
    ) -> bool {
        if callback_target == 0 || callback_selector == 0 {
            return false;
        }
        let Some(callback_name) = self.synthetic_callback_selector_name(callback_selector) else {
            return false;
        };
        if Self::is_touch_phase_selector(&callback_name) {
            return false;
        }
        if Self::selector_trace_mentions_selector(selector_name, &callback_name) {
            return false;
        }
        if ui_selector.map(|value| value.trim_matches('\0').trim()) == Some(callback_name.as_str()) {
            return false;
        }
        if cocos_selector.map(|value| value.trim_matches('\0').trim()) == Some(callback_name.as_str()) {
            return false;
        }
        true
    }

    fn synthetic_parent_for_node(&self, node: u32) -> u32 {
        self.runtime
            .graphics
            .synthetic_sprites
            .get(&node)
            .map(|state| state.parent)
            .unwrap_or(0)
    }

    fn synthetic_node_label(&self, node: u32) -> String {
        self.diag.object_labels.get(&node).cloned().unwrap_or_default()
    }

    fn scene_widget_touch_state_value(phase: &str) -> u32 {
        match phase {
            "down" => 1,
            "move" => 2,
            "up" => 3,
            _ => 0,
        }
    }

    fn is_gui_button_like_label(label: &str) -> bool {
        label.contains("GUIButton")
    }

    fn is_gui_checkbox_like_label(label: &str) -> bool {
        label.contains("GUICheckBox")
    }

    fn is_menu_item_like_label(label: &str) -> bool {
        label.contains("MenuItem")
    }

    fn resolve_shared_touch_event_for_touch(&mut self, touch_object: u32, origin: &str) -> u32 {
        if touch_object == 0 {
            return 0;
        }
        let Some(controller_class) = self.objc_lookup_class_by_name("TouchController") else {
            return 0;
        };
        let controller = self
            .invoke_objc_selector_now_capture_r0(controller_class, "sharedTouchController", 0, 0, 120_000, origin)
            .unwrap_or(0);
        if controller == 0 {
            return 0;
        }
        self.invoke_objc_selector_now_capture_r0(controller, "getTouchEvent:", touch_object, 0, 120_000, origin)
            .unwrap_or(0)
    }

    fn find_synthetic_ancestor_responding_to_selector(&mut self, start: u32, selector: &str) -> u32 {
        let mut current = start;
        let mut seen = HashSet::new();
        for _ in 0..32 {
            if current == 0 || !seen.insert(current) {
                break;
            }
            if self.objc_lookup_imp_for_receiver(current, selector).is_some() {
                return current;
            }
            current = self.synthetic_parent_for_node(current);
        }
        0
    }

    fn dispatch_scene_widget_activation(
        &mut self,
        target: u32,
        touch_object: u32,
        phase: &str,
        inside: bool,
        origin: &str,
    ) -> Option<String> {
        if target == 0 {
            return None;
        }
        let label = self.synthetic_node_label(target);
        if label.is_empty() {
            return None;
        }

        let touch_state = Self::scene_widget_touch_state_value(phase);
        let touch_event = self.resolve_shared_touch_event_for_touch(touch_object, origin);
        if touch_event != 0 {
            let _ = self.invoke_objc_selector_now(touch_event, "setTouchUI:", 1, 0, 120_000, origin);
            let _ = self.invoke_objc_selector_now(touch_event, "setTouchState:", touch_state, 0, 120_000, origin);
        }

        if Self::is_menu_item_like_label(&label) {
            let selected = phase == "down";
            let selector = if selected {
                "selected"
            } else if inside {
                "activate"
            } else {
                "unselected"
            };
            if self.invoke_objc_selector_now(target, selector, 0, 0, 120_000, origin) {
                if phase == "up" {
                    let _ = self.invoke_objc_selector_now(target, "unselected", 0, 0, 120_000, origin);
                }
                return Some(selector.to_string());
            }
        }

        if Self::is_gui_button_like_label(&label) || Self::is_gui_checkbox_like_label(&label) || label.contains("GUIBase") || label.contains("Board") {
            if touch_event != 0
                && self
                    .invoke_objc_selector_now_capture_r0(target, "handleTouch:touchState:", touch_event, touch_state, 120_000, origin)
                    .is_some()
            {
                let mut suffix = String::from("handleTouch:touchState:");
                if phase == "up" && inside {
                    let action_selector = if Self::is_gui_checkbox_like_label(&label) {
                        "checkboxClicked:"
                    } else {
                        "buttonClicked:"
                    };
                    let action_target = self.find_synthetic_ancestor_responding_to_selector(target, action_selector);
                    if action_target != 0
                        && self.invoke_objc_selector_now(action_target, action_selector, target, 0, 120_000, origin)
                    {
                        suffix.push('+');
                        suffix.push_str(action_selector);
                    }
                }
                return Some(suffix);
            }
            if phase == "up" && inside {
                let action_selector = if Self::is_gui_checkbox_like_label(&label) {
                    "checkboxClicked:"
                } else {
                    "buttonClicked:"
                };
                let action_target = self.find_synthetic_ancestor_responding_to_selector(target, action_selector);
                if action_target != 0
                    && self.invoke_objc_selector_now(action_target, action_selector, target, 0, 120_000, origin)
                {
                    return Some(action_selector.to_string());
                }
            }
        }

        None
    }

    fn cocos_touch_dispatch_selectors_for_phase(phase: &str) -> &'static [&'static str] {
        match phase {
            "down" => &[
                "ccTouchesBegan:withEvent:",
                "ccTouchBegan:withEvent:",
            ],
            "move" => &[
                "ccTouchesMoved:withEvent:",
                "ccTouchMoved:withEvent:",
            ],
            "up" => &[
                "ccTouchesEnded:withEvent:",
                "ccTouchEnded:withEvent:",
            ],
            _ => &[],
        }
    }

    fn uikit_touch_dispatch_selectors_for_phase(phase: &str) -> &'static [&'static str] {
        match phase {
            "down" => &[
                "touchesBegan:withEvent:",
                "touchesBegan:",
            ],
            "move" => &[
                "touchesMoved:withEvent:",
                "touchesMoved:",
            ],
            "up" => &[
                "touchesEnded:withEvent:",
                "touchesEnded:",
            ],
            _ => &[],
        }
    }

    fn synthetic_touch_dispatch_target_supports_phase(&mut self, target: u32, phase: &str) -> bool {
        if target == 0 {
            return false;
        }
        Self::cocos_touch_dispatch_selectors_for_phase(phase)
            .iter()
            .any(|selector| self.objc_lookup_imp_for_receiver(target, selector).is_some())
    }

    fn uikit_touch_dispatch_target_supports_phase(&mut self, target: u32, phase: &str) -> bool {
        if target == 0 {
            return false;
        }
        Self::uikit_touch_dispatch_selectors_for_phase(phase)
            .iter()
            .any(|selector| self.objc_lookup_imp_for_receiver(target, selector).is_some())
    }

    fn find_synthetic_touch_dispatch_target(&mut self, target: u32, root: u32) -> Option<u32> {
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();

        let mut current = target;
        for _ in 0..32 {
            if current == 0 || !seen.insert(current) {
                break;
            }
            candidates.push(current);
            current = self.synthetic_parent_for_node(current);
        }

        for extra in [
            root,
            self.runtime.ui_cocos.effect_scene,
            self.runtime.ui_cocos.running_scene,
            self.runtime.scene.auto_scene_cached_root,
            self.runtime.scene.auto_scene_inferred_root,
            self.runtime.ui_objects.first_responder,
            self.runtime.ui_cocos.opengl_view,
            self.runtime.ui_objects.root_controller,
        ] {
            if extra != 0 && seen.insert(extra) {
                candidates.push(extra);
            }
        }

        candidates
            .into_iter()
            .find(|candidate| self.synthetic_touch_dispatch_target_supports_phase(*candidate, "down"))
    }


    fn find_uikit_touch_dispatch_target_for_view(&mut self, hit_view: u32, phase: &str) -> u32 {
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();

        let mut push = |value: u32, out: &mut Vec<u32>, seen: &mut HashSet<u32>| {
            if value != 0 && seen.insert(value) {
                out.push(value);
            }
        };

        let mut current = hit_view;
        for _ in 0..32 {
            if current == 0 {
                break;
            }
            push(current, &mut candidates, &mut seen);
            let next = self.ui_next_responder(current);
            if next == 0 || next == current {
                break;
            }
            current = next;
        }

        push(self.runtime.ui_objects.first_responder, &mut candidates, &mut seen);
        push(self.runtime.ui_cocos.opengl_view, &mut candidates, &mut seen);
        push(self.runtime.ui_objects.root_controller, &mut candidates, &mut seen);
        push(self.runtime.ui_objects.window, &mut candidates, &mut seen);
        push(self.runtime.ui_objects.app, &mut candidates, &mut seen);

        candidates
            .into_iter()
            .find(|candidate| self.uikit_touch_dispatch_target_supports_phase(*candidate, phase))
            .unwrap_or(0)
    }

    fn find_uikit_touch_dispatch_target_at(&mut self, x: f32, y: f32, phase: &str) -> (u32, u32) {
        let hit_view = self
            .ui_hit_test_window_point(self.runtime.ui_objects.window, x, y)
            .or_else(|| {
                let responder = self.runtime.ui_objects.first_responder;
                (responder != 0 && self.ui_object_is_view_like(responder)).then_some(responder)
            })
            .or_else(|| {
                let view = self.runtime.ui_cocos.opengl_view;
                (view != 0 && self.ui_object_is_view_like(view)).then_some(view)
            })
            .unwrap_or(0);

        let dispatch_target = self.find_uikit_touch_dispatch_target_for_view(hit_view, phase);
        let routed_hit_view = if hit_view != 0 {
            hit_view
        } else if dispatch_target != 0 {
            dispatch_target
        } else {
            self.runtime.ui_cocos.opengl_view
        };
        (dispatch_target, routed_hit_view)
    }

    fn synthetic_primary_touch_for_event(&self, event_object: u32) -> u32 {
        self.runtime
            .host_input
            .synthetic_event_objects
            .get(&event_object)
            .map(|state| state.primary_touch)
            .unwrap_or(0)
    }

    fn synthetic_phase_name_for_event(&self, event_object: u32) -> Option<&'static str> {
        let touch = self.synthetic_primary_touch_for_event(event_object);
        let state = self.runtime.host_input.synthetic_touch_objects.get(&touch)?;
        Some(match state.phase {
            SyntheticUiTouchPhase::Began => "down",
            SyntheticUiTouchPhase::Moved | SyntheticUiTouchPhase::Stationary => "move",
            SyntheticUiTouchPhase::Ended | SyntheticUiTouchPhase::Cancelled => "up",
        })
    }

    fn dispatch_uikit_event_object_via_window_chain(
        &mut self,
        phase: &str,
        event_object: u32,
        origin: &str,
    ) -> Option<(u32, u32, String)> {
        let touch_object = self.synthetic_primary_touch_for_event(event_object);
        if touch_object == 0 {
            return None;
        }
        let touch_set = self
            .runtime
            .host_input
            .synthetic_event_objects
            .get(&event_object)
            .map(|state| state.touch_set)
            .unwrap_or(0);
        let touch_state = self.runtime.host_input.synthetic_touch_objects.get(&touch_object)?.clone();
        let window = if touch_state.window != 0 {
            touch_state.window
        } else {
            self.runtime.ui_objects.window
        };
        let hit_view = if phase == "down" || touch_state.hit_view == 0 {
            self.ui_hit_test_window_point(window, touch_state.current_x, touch_state.current_y)
                .or_else(|| {
                    let responder = self.runtime.ui_objects.first_responder;
                    (responder != 0 && self.ui_object_is_view_like(responder)).then_some(responder)
                })
                .or_else(|| {
                    let view = self.runtime.ui_cocos.opengl_view;
                    (view != 0 && self.ui_object_is_view_like(view)).then_some(view)
                })
                .unwrap_or(0)
        } else {
            touch_state.hit_view
        };
        let dispatch_anchor = if hit_view != 0 {
            hit_view
        } else if touch_state.hit_view != 0 {
            touch_state.hit_view
        } else {
            window
        };
        let dispatch_target = self.find_uikit_touch_dispatch_target_for_view(dispatch_anchor, phase);
        if let Some(state) = self.runtime.host_input.synthetic_touch_objects.get_mut(&touch_object) {
            if window != 0 && (phase == "down" || state.window == 0) {
                state.window = window;
            }
            if hit_view != 0 {
                if phase == "down" || state.view == 0 {
                    state.view = hit_view;
                }
                if phase == "down" || state.hit_view == 0 {
                    state.hit_view = hit_view;
                }
            } else if dispatch_target != 0 && state.view == 0 {
                state.view = dispatch_target;
            }
        }
        if dispatch_target == 0 {
            return None;
        }
        self.runtime.ui_objects.first_responder = dispatch_target;
        let dispatched = self.dispatch_uikit_touch_phase(
            dispatch_target,
            phase,
            touch_object,
            touch_set,
            event_object,
            origin,
        )?;
        Some((dispatch_target, hit_view, dispatched))
    }

    fn dispatch_uikit_event_via_window_send_event(
        &mut self,
        phase: &str,
        event_object: u32,
        origin: &str,
    ) -> Option<(u32, u32, String)> {
        let touch_object = self.synthetic_primary_touch_for_event(event_object);
        if touch_object == 0 {
            return None;
        }
        let window_receiver = self
            .runtime
            .host_input
            .synthetic_touch_objects
            .get(&touch_object)
            .map(|state| if state.window != 0 { state.window } else { self.runtime.ui_objects.window })
            .unwrap_or(self.runtime.ui_objects.window);
        if let Some(state) = self.runtime.host_input.synthetic_touch_objects.get_mut(&touch_object) {
            if state.window == 0 {
                state.window = window_receiver;
            }
        }

        if window_receiver != 0 {
            if self.invoke_objc_selector_now(window_receiver, "sendEvent:", event_object, 0, 120_000, origin) {
                let hit_view = self
                    .runtime
                    .host_input
                    .synthetic_touch_objects
                    .get(&touch_object)
                    .map(|state| if state.hit_view != 0 { state.hit_view } else { state.view })
                    .unwrap_or(0);
                let dispatch_target = self.find_uikit_touch_dispatch_target_for_view(
                    if hit_view != 0 { hit_view } else { window_receiver },
                    phase,
                );
                if dispatch_target != 0 || hit_view != 0 {
                    return Some((
                        if dispatch_target != 0 { dispatch_target } else { window_receiver },
                        hit_view,
                        "sendEvent:".to_string(),
                    ));
                }
            }
        }

        self.dispatch_uikit_event_object_via_window_chain(phase, event_object, origin)
    }

    fn ui_object_contains_guest_point(&self, object: u32, x: f32, y: f32) -> bool {
        if object == 0 || !self.ui_object_is_view_like(object) {
            return false;
        }
        let frame_bits = self.ui_frame_bits_for_object(object);
        let bounds_bits = self.ui_bounds_bits_for_object(object);
        let left = Self::f32_from_bits(frame_bits[0]);
        let top = Self::f32_from_bits(frame_bits[1]);
        let mut width = Self::f32_from_bits(bounds_bits[2]);
        let mut height = Self::f32_from_bits(bounds_bits[3]);
        if !width.is_finite() || width <= 0.0 {
            width = Self::f32_from_bits(frame_bits[2]);
        }
        if !height.is_finite() || height <= 0.0 {
            height = Self::f32_from_bits(frame_bits[3]);
        }
        if !left.is_finite() || !top.is_finite() || !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return false;
        }
        x >= left && y >= top && x <= (left + width) && y <= (top + height)
    }

    fn host_touch_target_contains_guest_point(&self, target: u32, x: f32, y: f32) -> bool {
        if target == 0 {
            return false;
        }
        if self.runtime.graphics.synthetic_sprites.contains_key(&target) {
            return self.synthetic_node_contains_guest_point(target, x, y);
        }
        if self.ui_object_contains_guest_point(target, x, y) {
            return true;
        }
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1) as f32;
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
        x >= 0.0 && y >= 0.0 && x <= surface_w && y <= surface_h
    }

    fn ensure_synthetic_touch_dispatch_target_ready(&mut self, target: u32, origin: &str) {
        if target == 0 {
            return;
        }
        let already_enabled = self
            .runtime
            .graphics
            .synthetic_sprites
            .get(&target)
            .map(|state| state.touch_enabled)
            .unwrap_or(false);
        if already_enabled {
            return;
        }
        self.ensure_synthetic_sprite_state(target).touch_enabled = true;
        self.runtime.ui_objects.first_responder = target;
        let enabled = self.invoke_objc_selector_now(target, "setIsTouchEnabled:", 1, 0, 120_000, origin);
        let registered = self.invoke_objc_selector_now(target, "registerWithTouchDispatcher", 0, 0, 120_000, origin);
        self.diag.trace.push(format!(
            "     ↳ host touch.bootstrap target={} setIsTouchEnabled={} registerWithTouchDispatcher={} origin={}",
            self.describe_ptr(target),
            if enabled { "YES" } else { "NO" },
            if registered { "YES" } else { "NO" },
            origin,
        ));
    }

    fn attach_named_objc_class_if_available(&mut self, receiver: u32, class_name: &str) {
        if receiver == 0 || class_name.is_empty() {
            return;
        }
        if let Some(class_ptr) = self.objc_lookup_class_by_name(class_name) {
            self.objc_attach_receiver_class(receiver, class_ptr, class_name);
        }
    }


    fn alloc_synthetic_input_object(&mut self, label: impl Into<String>, size: u32) -> u32 {
        let obj = self.alloc_synthetic_ui_object(label);
        let reserve = size.max(0x20).saturating_add(0x1f) & !0x1f;
        let baseline = obj.saturating_add(0x20);
        let desired = obj.saturating_add(reserve);
        if self.runtime.graphics.synthetic_ui_object_cursor < desired {
            self.runtime.graphics.synthetic_ui_object_cursor = desired;
        }
        if self.runtime.graphics.synthetic_ui_object_cursor < baseline {
            self.runtime.graphics.synthetic_ui_object_cursor = baseline;
        }
        obj
    }

    fn synthetic_touch_timestamp_words(value: f64) -> (u32, u32) {
        let bits = value.to_bits().to_le_bytes();
        (
            u32::from_le_bytes([bits[0], bits[1], bits[2], bits[3]]),
            u32::from_le_bytes([bits[4], bits[5], bits[6], bits[7]]),
        )
    }

    fn sync_synthetic_touch_object_memory(&mut self, touch_object: u32) {
        let Some(state) = self.runtime.host_input.synthetic_touch_objects.get(&touch_object).cloned() else {
            return;
        };
        let phase = state.phase.as_uikit_value();
        let tap_count = state.tap_count.max(1);
        let pointer_id = state.pointer_id.max(1);
        let (ts_lo, ts_hi) = Self::synthetic_touch_timestamp_words(state.timestamp_secs);
        let writes = [
            (0x04u32, state.window),
            (0x08, state.view),
            (0x0c, state.hit_view),
            (0x10, phase),
            (0x14, tap_count),
            (0x18, state.current_x.to_bits()),
            (0x1c, state.current_y.to_bits()),
            (0x20, state.previous_x.to_bits()),
            (0x24, state.previous_y.to_bits()),
            (0x28, state.began_x.to_bits()),
            (0x2c, state.began_y.to_bits()),
            (0x30, ts_lo),
            (0x34, ts_hi),
            (0x38, pointer_id),
            (0x3c, phase),
            (0x40, state.window),
            (0x44, state.view),
            (0x48, state.current_x.to_bits()),
            (0x4c, state.current_y.to_bits()),
            (0x50, state.previous_x.to_bits()),
            (0x54, state.previous_y.to_bits()),
            (0x58, ts_lo),
            (0x5c, ts_hi),
            (0x60, state.hit_view),
            (0x64, pointer_id),
        ];
        for (off, value) in writes {
            let _ = self.write_u32_le(touch_object.wrapping_add(off), value);
        }
    }

    fn sync_synthetic_event_object_memory(&mut self, event_object: u32) {
        let Some(event) = self.runtime.host_input.synthetic_event_objects.get(&event_object).cloned() else {
            return;
        };
        let touch_state = self.runtime.host_input.synthetic_touch_objects.get(&event.primary_touch).cloned();
        let (window, view, hit_view, ts_lo, ts_hi) = if let Some(state) = touch_state {
            let (lo, hi) = Self::synthetic_touch_timestamp_words(state.timestamp_secs);
            (state.window, state.view, state.hit_view, lo, hi)
        } else {
            (0, 0, 0, 0, 0)
        };
        let writes = [
            (0x04u32, event.touch_set),
            (0x08, event.primary_touch),
            (0x0c, event.event_type),
            (0x10, event.event_subtype),
            (0x14, window),
            (0x18, view),
            (0x1c, hit_view),
            (0x20, ts_lo),
            (0x24, ts_hi),
            (0x28, event.touch_set),
            (0x2c, event.touch_set),
            (0x30, if event.primary_touch != 0 { 1 } else { 0 }),
        ];
        for (off, value) in writes {
            let _ = self.write_u32_le(event_object.wrapping_add(off), value);
        }
    }

    fn sync_synthetic_touch_set_memory(&mut self, set_object: u32) {
        let Some(state) = self.runtime.host_input.synthetic_set_objects.get(&set_object).cloned() else {
            return;
        };
        let first = state.items.first().copied().unwrap_or(0);
        let count = state.items.len() as u32;
        let writes = [
            (0x04u32, count),
            (0x08, first),
            (0x0c, state.mutation_count),
            (0x10, first),
            (0x14, count),
            (0x18, state.mutation_count),
        ];
        for (off, value) in writes {
            let _ = self.write_u32_le(set_object.wrapping_add(off), value);
        }
    }

    fn sync_synthetic_touch_payload_memory(&mut self, touch_object: u32, touch_set: u32, event_object: u32) {
        self.sync_synthetic_touch_object_memory(touch_object);
        self.sync_synthetic_touch_set_memory(touch_set);
        self.sync_synthetic_event_object_memory(event_object);
    }

    fn host_touch_timestamp_secs(&self) -> f64 {
        (self.runtime.ui_runtime.runloop_ticks as f64) / 60.0
    }

    fn host_touch_view_for_target(&self, target: u32, dispatch_target: u32) -> u32 {
        if self.ui_object_is_view_like(dispatch_target) {
            dispatch_target
        } else if self.ui_object_is_view_like(target) {
            target
        } else if self.runtime.ui_objects.window != 0 {
            self.runtime.ui_objects.window
        } else if self.runtime.ui_cocos.opengl_view != 0 {
            self.runtime.ui_cocos.opengl_view
        } else if dispatch_target != 0 {
            dispatch_target
        } else {
            target
        }
    }

    fn upsert_synthetic_touch_set(&mut self, set_object: u32, items: &[u32]) {
        if set_object == 0 {
            return;
        }
        let entry = self.runtime.host_input.synthetic_set_objects.entry(set_object).or_default();
        if entry.items != items {
            entry.items.clear();
            entry.items.extend_from_slice(items);
            entry.mutation_count = entry.mutation_count.saturating_add(1);
        }
        self.sync_synthetic_touch_set_memory(set_object);
    }

    fn ensure_synthetic_empty_touch_set(&mut self) -> u32 {
        if let Some(existing) = self.runtime.host_input.empty_touch_set.filter(|value| *value != 0) {
            return existing;
        }
        let object = self.alloc_synthetic_input_object(format!(
            "NSSet.synthetic.empty#{}",
            self.runtime.host_input.synthetic_set_objects.len()
        ), 0x40);
        self.attach_named_objc_class_if_available(object, "NSSet");
        self.runtime.host_input.synthetic_set_objects.entry(object).or_default();
        self.runtime.host_input.empty_touch_set = Some(object);
        object
    }

    fn create_synthetic_touch_payload(
        &mut self,
        pointer_id: u32,
        target: u32,
        dispatch_target: u32,
        hit_view: u32,
        x: f32,
        y: f32,
    ) -> (u32, u32, u32) {
        let touch_object = self.alloc_synthetic_input_object(format!(
            "UITouch.synthetic#{}",
            self.runtime.host_input.synthetic_touch_objects.len()
        ), 0x80);
        let touch_set = self.alloc_synthetic_input_object(format!(
            "NSSet.synthetic#{}",
            self.runtime.host_input.synthetic_set_objects.len()
        ), 0x40);
        let event_object = self.alloc_synthetic_input_object(format!(
            "UIEvent.synthetic#{}",
            self.runtime.host_input.synthetic_event_objects.len()
        ), 0x60);
        self.attach_named_objc_class_if_available(touch_object, "UITouch");
        self.attach_named_objc_class_if_available(touch_set, "NSSet");
        self.attach_named_objc_class_if_available(event_object, "UIEvent");

        let view = if hit_view != 0 {
            hit_view
        } else {
            self.host_touch_view_for_target(target, dispatch_target)
        };
        self.runtime.host_input.synthetic_touch_objects.insert(
            touch_object,
            SyntheticUiTouchState {
                pointer_id,
                phase: SyntheticUiTouchPhase::Began,
                tap_count: 1,
                timestamp_secs: self.host_touch_timestamp_secs(),
                window: self.runtime.ui_objects.window,
                view,
                hit_view: if hit_view != 0 { hit_view } else { target },
                began_x: x,
                began_y: y,
                previous_x: x,
                previous_y: y,
                current_x: x,
                current_y: y,
            },
        );
        self.upsert_synthetic_touch_set(touch_set, &[touch_object]);
        self.runtime.host_input.synthetic_event_objects.insert(
            event_object,
            SyntheticUiEventState {
                touch_set,
                primary_touch: touch_object,
                event_type: 0,
                event_subtype: 0,
            },
        );
        self.sync_synthetic_touch_payload_memory(touch_object, touch_set, event_object);
        (touch_object, touch_set, event_object)
    }

    fn update_synthetic_touch_payload(
        &mut self,
        touch_object: u32,
        touch_set: u32,
        event_object: u32,
        target: u32,
        dispatch_target: u32,
        hit_view: u32,
        phase: &str,
        x: f32,
        y: f32,
    ) {
        let phase_value = SyntheticUiTouchPhase::from_host_phase(phase);
        let timestamp_secs = self.host_touch_timestamp_secs();
        let default_view = self.host_touch_view_for_target(target, dispatch_target);
        let window = self.runtime.ui_objects.window;
        let mut touch_ptr = 0u32;
        if let Some(state) = self.runtime.host_input.synthetic_touch_objects.get_mut(&touch_object) {
            state.phase = phase_value;
            state.timestamp_secs = timestamp_secs;
            state.previous_x = state.current_x;
            state.previous_y = state.current_y;
            state.current_x = x;
            state.current_y = y;
            if hit_view != 0 {
                if phase == "down" || state.hit_view == 0 {
                    state.hit_view = hit_view;
                }
                if phase == "down" || state.view == 0 {
                    state.view = hit_view;
                }
            } else {
                state.hit_view = target;
                if state.view == 0 {
                    state.view = default_view;
                }
            }
            if state.window == 0 {
                state.window = window;
            }
            touch_ptr = touch_object;
        }
        if touch_ptr != 0 {
            self.upsert_synthetic_touch_set(touch_set, &[touch_ptr]);
            if let Some(event) = self.runtime.host_input.synthetic_event_objects.get_mut(&event_object) {
                event.touch_set = touch_set;
                if event.primary_touch == 0 {
                    event.primary_touch = touch_ptr;
                }
            }
            self.sync_synthetic_touch_payload_memory(touch_object, touch_set, event_object);
        }
    }

    fn synthetic_touch_selector_args(
        selector: &str,
        touch_object: u32,
        touch_set: u32,
        event_object: u32,
    ) -> (u32, u32) {
        let singular_touch = selector.starts_with("ccTouch") && !selector.starts_with("ccTouches");
        let arg2 = if singular_touch { touch_object } else { touch_set };
        let arg3 = if selector.ends_with(":withEvent:") { event_object } else { 0 };
        (arg2, arg3)
    }

    fn dispatch_selector_family_touch_phase(
        &mut self,
        dispatch_target: u32,
        selectors: &[&str],
        touch_object: u32,
        touch_set: u32,
        event_object: u32,
        origin: &str,
    ) -> Option<String> {
        if dispatch_target == 0 {
            return None;
        }
        for selector in selectors {
            let (arg2, arg3) = Self::synthetic_touch_selector_args(selector, touch_object, touch_set, event_object);
            let result = self.invoke_objc_selector_now_capture_r0(dispatch_target, selector, arg2, arg3, 120_000, origin);
            if let Some(ret) = result {
                if *selector == "ccTouchBegan:withEvent:" && ret == 0 {
                    continue;
                }
                return Some((*selector).to_string());
            }
        }
        None
    }

    fn dispatch_synthetic_touch_phase(
        &mut self,
        dispatch_target: u32,
        phase: &str,
        touch_object: u32,
        touch_set: u32,
        event_object: u32,
        origin: &str,
    ) -> Option<String> {
        self.dispatch_selector_family_touch_phase(
            dispatch_target,
            Self::cocos_touch_dispatch_selectors_for_phase(phase),
            touch_object,
            touch_set,
            event_object,
            origin,
        )
    }

    fn dispatch_uikit_touch_phase(
        &mut self,
        dispatch_target: u32,
        phase: &str,
        touch_object: u32,
        touch_set: u32,
        event_object: u32,
        origin: &str,
    ) -> Option<String> {
        self.dispatch_selector_family_touch_phase(
            dispatch_target,
            Self::uikit_touch_dispatch_selectors_for_phase(phase),
            touch_object,
            touch_set,
            event_object,
            origin,
        )
    }

    fn synthetic_touch_point_bits_global(&self, touch_object: u32, previous: bool) -> [u32; 2] {
        let Some(state) = self.runtime.host_input.synthetic_touch_objects.get(&touch_object) else {
            return [0, 0];
        };
        let x = if previous { state.previous_x } else { state.current_x };
        let y = if previous { state.previous_y } else { state.current_y };
        [x.to_bits(), y.to_bits()]
    }

    fn synthetic_touch_point_bits_for_view(&self, touch_object: u32, view: u32, previous: bool) -> [u32; 2] {
        let Some(state) = self.runtime.host_input.synthetic_touch_objects.get(&touch_object) else {
            return [0, 0];
        };
        let mut x = if previous { state.previous_x } else { state.current_x };
        let mut y = if previous { state.previous_y } else { state.current_y };
        if view != 0 {
            if self.runtime.graphics.synthetic_sprites.contains_key(&view) {
                if let Some((left, top, _, _)) = self.synthetic_node_hit_bounds(view) {
                    x -= left;
                    y -= top;
                }
            } else {
                let frame_bits = self.ui_frame_bits_for_object(view);
                let bounds_bits = self.ui_bounds_bits_for_object(view);
                x = x - Self::f32_from_bits(frame_bits[0]) + Self::f32_from_bits(bounds_bits[0]);
                y = y - Self::f32_from_bits(frame_bits[1]) + Self::f32_from_bits(bounds_bits[1]);
            }
        }
        [x.to_bits(), y.to_bits()]
    }

    fn maybe_dispatch_synthetic_input_objc_msgsend(
        &mut self,
        index: u64,
        current_pc: u32,
        receiver: u32,
        selector: &str,
        arg2: u32,
        arg3: u32,
        receiver_desc: &str,
        arg2_desc: &str,
        arg3_desc: &str,
    ) -> CoreResult<Option<StepControl>> {
        if let Some(touch) = self.runtime.host_input.synthetic_touch_objects.get(&receiver).cloned() {
            if matches!(selector,
                "locationInView:" | "previousLocationInView:" | "locationInWindow" | "previousLocationInWindow"
                | "_locationInView:" | "_previousLocationInView:" | "_locationInWindow" | "_previousLocationInWindow"
            ) {
                let previous = matches!(selector, "previousLocationInView:" | "previousLocationInWindow" | "_previousLocationInView:" | "_previousLocationInWindow");
                let in_window = matches!(selector, "locationInWindow" | "previousLocationInWindow" | "_locationInWindow" | "_previousLocationInWindow");
                let point_bits = if in_window {
                    self.synthetic_touch_point_bits_global(receiver, previous)
                } else {
                    self.synthetic_touch_point_bits_for_view(receiver, arg2, previous)
                };
                let detail = format!(
                    "hle objc_msgSend(receiver={}, sel={}, arg2={}, arg3={}, result=CGPoint({:.3},{:.3}))",
                    receiver_desc,
                    selector,
                    arg2_desc,
                    arg3_desc,
                    Self::f32_from_bits(point_bits[0]),
                    Self::f32_from_bits(point_bits[1]),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, "objc_msgSend", &detail));
                self.cpu.regs[0] = point_bits[0];
                self.cpu.regs[1] = point_bits[1];
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }

            if selector == "timestamp" {
                let value = touch.timestamp_secs;
                let bits = value.to_bits().to_le_bytes();
                self.vfp_set_d_f64(0, value);
                self.cpu.regs[0] = u32::from_le_bytes([bits[0], bits[1], bits[2], bits[3]]);
                self.cpu.regs[1] = u32::from_le_bytes([bits[4], bits[5], bits[6], bits[7]]);
                let detail = format!(
                    "hle objc_msgSend(receiver={}, sel={}, arg2={}, arg3={}, result=NSTimeInterval({:.3}))",
                    receiver_desc,
                    selector,
                    arg2_desc,
                    arg3_desc,
                    value,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, "objc_msgSend", &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }

            let result = match selector {
                "view" | "gestureView" | "_view" => touch.view,
                "window" | "_window" => touch.window,
                "phase" | "_phase" => touch.phase.as_uikit_value(),
                "tapCount" | "_tapCount" => touch.tap_count,
                "locationOnScreen" => touch.window,
                "isTap" => {
                    let dx = (touch.current_x - touch.began_x).abs();
                    let dy = (touch.current_y - touch.began_y).abs();
                    if dx <= 12.0 && dy <= 12.0 { 1 } else { 0 }
                }
                _ => 0,
            };
            if matches!(selector, "view" | "gestureView" | "_view" | "window" | "_window" | "phase" | "_phase" | "tapCount" | "_tapCount" | "isTap") {
                return self.finish_objc_msgsend_hle_dispatch(
                    index,
                    current_pc,
                    "objc_msgSend",
                    receiver_desc,
                    selector,
                    arg2_desc,
                    arg3_desc,
                    result,
                    None,
                );
            }
        }

        if let Some(event) = self.runtime.host_input.synthetic_event_objects.get(&receiver).cloned() {
            let touch_view = self
                .runtime
                .host_input
                .synthetic_touch_objects
                .get(&event.primary_touch)
                .map(|state| state.view)
                .unwrap_or(0);
            let touch_window = self
                .runtime
                .host_input
                .synthetic_touch_objects
                .get(&event.primary_touch)
                .map(|state| state.window)
                .unwrap_or(0);
            let touch_hit_view = self
                .runtime
                .host_input
                .synthetic_touch_objects
                .get(&event.primary_touch)
                .map(|state| state.hit_view)
                .unwrap_or(0);
            let touch_timestamp = self
                .runtime
                .host_input
                .synthetic_touch_objects
                .get(&event.primary_touch)
                .map(|state| state.timestamp_secs)
                .unwrap_or(0.0);
            if selector == "timestamp" {
                let bits = touch_timestamp.to_bits().to_le_bytes();
                self.vfp_set_d_f64(0, touch_timestamp);
                self.cpu.regs[0] = u32::from_le_bytes([bits[0], bits[1], bits[2], bits[3]]);
                self.cpu.regs[1] = u32::from_le_bytes([bits[4], bits[5], bits[6], bits[7]]);
                let detail = format!(
                    "hle objc_msgSend(receiver={}, sel={}, arg2={}, arg3={}, result=NSTimeInterval({:.3}))",
                    receiver_desc,
                    selector,
                    arg2_desc,
                    arg3_desc,
                    touch_timestamp,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, "objc_msgSend", &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            let result = match selector {
                "allTouches" | "_touches" | "touches" => event.touch_set,
                "touchesForView:" => {
                    if arg2 == 0 || arg2 == touch_view || arg2 == touch_window || arg2 == touch_hit_view {
                        event.touch_set
                    } else {
                        self.ensure_synthetic_empty_touch_set()
                    }
                }
                "touchesForWindow:" => {
                    if arg2 == 0 || arg2 == touch_window {
                        event.touch_set
                    } else {
                        self.ensure_synthetic_empty_touch_set()
                    }
                }
                "type" => event.event_type,
                "subtype" => event.event_subtype,
                _ => 0,
            };
            if matches!(selector, "allTouches" | "_touches" | "touches" | "touchesForView:" | "touchesForWindow:" | "type" | "subtype") {
                return self.finish_objc_msgsend_hle_dispatch(
                    index,
                    current_pc,
                    "objc_msgSend",
                    receiver_desc,
                    selector,
                    arg2_desc,
                    arg3_desc,
                    result,
                    None,
                );
            }
        }

        if let Some(set_state) = self.runtime.host_input.synthetic_set_objects.get(&receiver).cloned() {
            let mut note: Option<String> = None;
            let result = match selector {
                "count" => set_state.items.len() as u32,
                "anyObject" => set_state.items.first().copied().unwrap_or(0),
                "member:" => set_state.items.iter().copied().find(|item| *item == arg2).unwrap_or(0),
                "containsObject:" => {
                    if set_state.items.iter().any(|item| *item == arg2) { 1 } else { 0 }
                }
                "allObjects" => {
                    let array = self.alloc_synthetic_array(format!(
                        "NSArray.fromNSSet#{}",
                        self.runtime.graphics.synthetic_arrays.len()
                    ));
                    for item in set_state.items.iter().copied() {
                        let _ = self.synthetic_array_push(array, item);
                    }
                    note = Some(format!("set allObjects count={}", set_state.items.len()));
                    array
                }
                "countByEnumeratingWithState:objects:count:" => {
                    let state_ptr = arg2;
                    let objects_ptr = arg3;
                    let count = self.peek_stack_u32(0).unwrap_or(0).max(1) as usize;
                    let prior_state = if state_ptr != 0 { self.read_u32_le(state_ptr).unwrap_or(0) } else { 0 };
                    if state_ptr == 0 || objects_ptr == 0 || prior_state != 0 || set_state.items.is_empty() {
                        if state_ptr != 0 {
                            let _ = self.write_u32_le(state_ptr, 1);
                        }
                        note = Some(format!("set fast-enum -> 0 state={} count={}", prior_state, set_state.items.len()));
                        0
                    } else {
                        let n = set_state.items.len().min(count);
                        for (i, item) in set_state.items.iter().take(n).enumerate() {
                            let _ = self.write_u32_le(objects_ptr.wrapping_add((i as u32).saturating_mul(4)), *item);
                        }
                        let _ = self.write_u32_le(state_ptr, 1);
                        let _ = self.write_u32_le(state_ptr.wrapping_add(4), objects_ptr);
                        let mutation_ptr = state_ptr.wrapping_add(28);
                        let _ = self.write_u32_le(state_ptr.wrapping_add(8), mutation_ptr);
                        for i in 0..4u32 {
                            let value = if i == 0 { n as u32 } else { 0 };
                            let _ = self.write_u32_le(state_ptr.wrapping_add(12 + i.saturating_mul(4)), value);
                        }
                        let _ = self.write_u32_le(mutation_ptr, set_state.mutation_count);
                        note = Some(format!(
                            "set fast-enum -> {} state={} objects={}",
                            n,
                            self.describe_ptr(state_ptr),
                            self.describe_ptr(objects_ptr)
                        ));
                        n as u32
                    }
                }
                _ => 0,
            };
            if matches!(selector, "count" | "anyObject" | "member:" | "containsObject:" | "allObjects" | "countByEnumeratingWithState:objects:count:") {
                return self.finish_objc_msgsend_hle_dispatch(
                    index,
                    current_pc,
                    "objc_msgSend",
                    receiver_desc,
                    selector,
                    arg2_desc,
                    arg3_desc,
                    result,
                    note,
                );
            }
        }

        Ok(None)
    }

    fn maybe_dispatch_synthetic_input_objc_msgsend_stret(
        &mut self,
        index: u64,
        current_pc: u32,
        out_ptr: u32,
        receiver: u32,
        selector: &str,
        arg1: u32,
        receiver_desc: &str,
        arg1_desc: &str,
    ) -> CoreResult<Option<StepControl>> {
        if self.runtime.host_input.synthetic_touch_objects.contains_key(&receiver)
            && matches!(selector,
                "locationInView:" | "previousLocationInView:" | "locationInWindow" | "previousLocationInWindow"
                | "_locationInView:" | "_previousLocationInView:" | "_locationInWindow" | "_previousLocationInWindow"
            )
        {
            let previous = matches!(selector, "previousLocationInView:" | "previousLocationInWindow" | "_previousLocationInView:" | "_previousLocationInWindow");
            let in_window = matches!(selector, "locationInWindow" | "previousLocationInWindow" | "_locationInWindow" | "_previousLocationInWindow");
            let point_bits = if in_window {
                self.synthetic_touch_point_bits_global(receiver, previous)
            } else {
                self.synthetic_touch_point_bits_for_view(receiver, arg1, previous)
            };
            self.write_cg_point_to_guest_bits(out_ptr, point_bits)?;
            let detail = format!(
                "hle objc_msgSend_stret(out={}, receiver={}, sel={}, arg1={}, wrote=CGPoint({:.3},{:.3}))",
                self.describe_ptr(out_ptr),
                receiver_desc,
                selector,
                arg1_desc,
                Self::f32_from_bits(point_bits[0]),
                Self::f32_from_bits(point_bits[1]),
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, "objc_msgSend_stret", &detail));
            self.cpu.regs[0] = out_ptr;
            self.cpu.regs[15] = self.cpu.regs[14] & !1;
            self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
            return Ok(Some(StepControl::Continue));
        }
        Ok(None)
    }

    fn poll_host_input_bus(&mut self) {
        for packet in drain_live_input(64) {
            self.runtime.host_input.events_loaded = self.runtime.host_input.events_loaded.saturating_add(1);
            self.runtime.host_input.queue.push_back(packet.into());
        }
    }

    fn process_pending_host_input(&mut self, origin: &str) {
        self.poll_host_input_bus();
        self.poll_host_input_script_file();
        let Some(event) = self.runtime.host_input.queue.pop_front() else {
            return;
        };
        self.runtime.host_input.events_consumed = self.runtime.host_input.events_consumed.saturating_add(1);
        let Some(phase) = Self::canonical_host_input_phase(&event.phase).map(str::to_string) else {
            self.runtime.host_input.events_ignored = self.runtime.host_input.events_ignored.saturating_add(1);
            self.runtime.host_input.last_dispatch = Some(format!("ignored-phase:{}", event.phase));
            self.diag.trace.push(format!("     ↳ host input ignored invalid phase={} origin={}", event.phase, origin));
            return;
        };
        let Some((pointer_id, x, y, source)) = self.normalize_host_pointer_event(&event) else {
            self.runtime.host_input.events_ignored = self.runtime.host_input.events_ignored.saturating_add(1);
            self.runtime.host_input.last_dispatch = Some("ignored-normalize".to_string());
            self.diag.trace.push(format!("     ↳ host input ignored normalize-failed phase={} origin={}", phase, origin));
            return;
        };
        self.runtime.host_input.last_phase = Some(phase.clone());
        self.runtime.host_input.last_x = Some(x);
        self.runtime.host_input.last_y = Some(y);
        self.runtime.host_input.last_source = Some(source.clone());
        crate::runtime::note_live_input_event(&phase, x, y, &source);

        match phase.as_str() {
            "down" => self.dispatch_host_pointer_down(pointer_id, x, y, origin, &source),
            "move" => self.dispatch_host_pointer_move(pointer_id, x, y, origin, &source),
            "up" => self.dispatch_host_pointer_up(pointer_id, x, y, origin, &source),
            _ => {
                self.runtime.host_input.events_ignored = self.runtime.host_input.events_ignored.saturating_add(1);
            }
        }
    }

    fn resolve_host_touch_root(&self) -> u32 {
        self.resolve_active_scene_root_for_input()
    }

    fn find_scene_progress_fallback_target(&self, root: u32) -> Option<u32> {
        if root == 0 {
            return None;
        }
        let label = self.diag.object_labels.get(&root).cloned().unwrap_or_default();
        if label.contains("HowToScene") {
            return self
                .find_synthetic_menu_item_by_selector(root, "nextCallback")
                .or_else(|| self.find_synthetic_menu_item_by_selector(root, "playCallback"));
        }
        if self.loading_scene_has_continue_prompt(root) {
            return self
                .find_synthetic_menu_item_by_selector(root, "nextCallback")
                .or_else(|| self.find_synthetic_menu_item_by_selector(root, "playCallback"))
                .or(Some(root));
        }
        None
    }

    fn try_invoke_host_touch_fallback(
        &mut self,
        pointer_id: u32,
        x: f32,
        y: f32,
        origin: &str,
        source: &str,
        reason: &str,
    ) -> bool {
        let root = self.resolve_host_touch_root();
        let target = self
            .find_synthetic_touch_target_at(root, x, y)
            .or_else(|| self.find_scene_progress_fallback_target(root));
        let Some(target) = target else {
            return false;
        };
        self.runtime.scene.synthetic_touch_injections = self.runtime.scene.synthetic_touch_injections.saturating_add(1);
        self.set_synthetic_menu_item_pressed(target, true);

        let (callback_target, callback_selector, callback_selector_name) = self
            .synthetic_callback_for_node(target)
            .unwrap_or((target, 0, String::new()));
        // Do not pre-seed selector_name with the authored callback selector here.
        // This string is used only to detect selectors that were *already dispatched*
        // during the current touch path. Pre-filling it with e.g. NewGame:/Options:/etc.
        // makes should_post_dispatch_hit_callback() incorrectly think the real menu
        // callback has already run after a mere activate/sendEvent phase.
        let mut selector_name = String::new();
        let touch_dispatch_target = self.find_synthetic_touch_dispatch_target(target, root).unwrap_or(0);
        let (touch_object, touch_set, event_object) = self.create_synthetic_touch_payload(
            pointer_id,
            target,
            touch_dispatch_target,
            0,
            x,
            y,
        );

        let mut invoked = false;
        let mut effective_callback_target = callback_target;
        let mut dispatched_phase_selector: Option<String> = None;
        let mut post_dispatch_selector: Option<String> = None;

        if touch_dispatch_target != 0 {
            effective_callback_target = touch_dispatch_target;
            self.ensure_synthetic_touch_dispatch_target_ready(touch_dispatch_target, reason);
            if let Some(dispatched) = self.dispatch_synthetic_touch_phase(
                touch_dispatch_target,
                "down",
                touch_object,
                touch_set,
                event_object,
                reason,
            ) {
                selector_name = dispatched.clone();
                self.update_synthetic_touch_payload(
                    touch_object,
                    touch_set,
                    event_object,
                    target,
                    touch_dispatch_target,
                    0,
                    "up",
                    x,
                    y,
                );
                dispatched_phase_selector = self.dispatch_synthetic_touch_phase(
                    touch_dispatch_target,
                    "up",
                    touch_object,
                    touch_set,
                    event_object,
                    reason,
                );
                if let Some(dispatched_up) = dispatched_phase_selector.as_deref() {
                    selector_name = dispatched_up.to_string();
                }
                invoked = true;
            }
        }

        if self.should_post_dispatch_hit_callback(
            target,
            callback_target,
            callback_selector,
            &selector_name,
            0,
            touch_dispatch_target,
            None,
            dispatched_phase_selector.as_deref(),
        ) {
            effective_callback_target = callback_target;
            let post_selector = self
                .synthetic_callback_selector_name(callback_selector)
                .unwrap_or_else(|| {
                    if callback_selector_name.is_empty() {
                        format!("0x{:08x}", callback_selector)
                    } else {
                        callback_selector_name.clone()
                    }
                });
            if self.invoke_objc_selector_now(
                callback_target,
                &post_selector,
                target,
                0,
                120_000,
                "host-touch-post-activate",
            ) {
                self.diag.trace.push(format!(
                    "     ↳ host touch.post-callback target={} callbackTarget={} selector={} reason={} dispatchedTarget={} invoked=YES",
                    self.describe_ptr(target),
                    self.describe_ptr(callback_target),
                    post_selector,
                    reason,
                    self.describe_ptr(touch_dispatch_target),
                ));
                post_dispatch_selector = Some(post_selector);
                invoked = true;
            }
        }

        if !invoked && callback_selector != 0 {
            effective_callback_target = callback_target;
            let callback_selector_name = self
                .synthetic_callback_selector_name(callback_selector)
                .unwrap_or_else(|| format!("0x{:08x}", callback_selector));
            if selector_name.is_empty() || !Self::selector_trace_mentions_selector(&selector_name, &callback_selector_name) {
                selector_name = callback_selector_name.clone();
            }
            invoked = self.invoke_objc_selector_now(
                callback_target,
                &callback_selector_name,
                target,
                0,
                120_000,
                reason,
            );
        }

        if let Some(post_selector) = post_dispatch_selector.as_deref() {
            if selector_name.is_empty() {
                selector_name = format!("post:{}", post_selector);
            } else if selector_name != post_selector {
                selector_name = format!("{}+post:{}", selector_name, post_selector);
            }
        }

        self.set_synthetic_menu_item_pressed(target, false);
        if !invoked {
            return false;
        }

        self.runtime.host_input.cocos_dispatched = self.runtime.host_input.cocos_dispatched.saturating_add(1);
        self.runtime.host_input.last_target = Some(target);
        self.runtime.host_input.last_dispatch = Some(format!(
            "cocos-touch-fallback:{}:{}:{}:{:.1}:{:.1}",
            self.describe_ptr(target),
            if selector_name.is_empty() { "<none>" } else { &selector_name },
            if invoked { "invoked" } else { "idle" },
            x,
            y,
        ));
        self.diag.trace.push(format!(
            "     ↳ host touch.fallback target={} selector={} callbackTarget={} touch={} set={} event={} pointer={} invoked={} x={:.1} y={:.1} source={} origin={} reason={} injections={}",
            self.describe_ptr(target),
            if selector_name.is_empty() { "<none>".to_string() } else { selector_name },
            self.describe_ptr(effective_callback_target),
            self.describe_ptr(touch_object),
            self.describe_ptr(touch_set),
            self.describe_ptr(event_object),
            pointer_id,
            if invoked { "YES" } else { "NO" },
            x,
            y,
            source,
            origin,
            reason,
            self.runtime.scene.synthetic_touch_injections,
        ));
        invoked
    }

    fn dispatch_host_pointer_down(&mut self, pointer_id: u32, x: f32, y: f32, origin: &str, source: &str) {
        self.runtime.host_input.ui_attempts = self.runtime.host_input.ui_attempts.saturating_add(1);

        let root = self.resolve_host_touch_root();
        let hit_target = self
            .find_synthetic_touch_target_at(root, x, y)
            .or_else(|| self.find_scene_progress_fallback_target(root));
        let (uikit_target, uikit_hit_view) = self.find_uikit_touch_dispatch_target_at(x, y, "down");
        let ui_surface_only = uikit_hit_view == 0
            || uikit_hit_view == self.runtime.ui_cocos.opengl_view
            || uikit_hit_view == self.runtime.ui_objects.window;
        let cocos_dispatch_target = hit_target
            .and_then(|node| self.find_synthetic_touch_dispatch_target(node, root));
        let enable_cocos_route = cocos_dispatch_target.is_some() && (uikit_target == 0 || ui_surface_only);

        // When UIKit only sees the fullscreen GL surface but synthetic scene hit-testing already
        // resolved a concrete cocos widget (for example a CCMenuItemImage), keep that synthetic
        // node as the logical target even if we could not yet prove a dedicated cocos touch
        // dispatcher target. Otherwise the active touch gets pinned to EAGLView, the UI route
        // reports success, but button activation never reaches the actual menu item.
        let prefer_synthetic_logical_target = hit_target.is_some() && ui_surface_only;
        let logical_target = if enable_cocos_route || prefer_synthetic_logical_target {
            hit_target
                .or_else(|| (uikit_hit_view != 0).then_some(uikit_hit_view))
                .or_else(|| (uikit_target != 0).then_some(uikit_target))
                .or_else(|| (root != 0).then_some(root))
                .or_else(|| (self.runtime.ui_cocos.opengl_view != 0).then_some(self.runtime.ui_cocos.opengl_view))
        } else {
            (uikit_hit_view != 0)
                .then_some(uikit_hit_view)
                .or_else(|| (uikit_target != 0).then_some(uikit_target))
                .or(hit_target)
                .or_else(|| (root != 0).then_some(root))
                .or_else(|| (self.runtime.ui_cocos.opengl_view != 0).then_some(self.runtime.ui_cocos.opengl_view))
        };

        let Some(target) = logical_target else {
            self.runtime.host_input.events_ignored = self.runtime.host_input.events_ignored.saturating_add(1);
            self.runtime.host_input.last_target = None;
            self.runtime.host_input.last_dispatch = Some(format!("miss-down:{}:{:.1}:{:.1}", self.describe_ptr(root), x, y));
            self.diag.trace.push(format!(
                "     ↳ host touch.down miss root={} x={:.1} y={:.1} source={} origin={}",
                self.describe_ptr(root),
                x,
                y,
                source,
                origin,
            ));
            return;
        };

        let (callback_target, callback_selector, selector_name) = hit_target
            .and_then(|node| self.synthetic_callback_for_node(node))
            .unwrap_or((target, 0, String::new()));

        let payload_dispatch_target = if uikit_target != 0 {
            uikit_target
        } else if enable_cocos_route {
            cocos_dispatch_target.unwrap_or(0)
        } else {
            0
        };
        let (touch_object, touch_set, event_object) = self.create_synthetic_touch_payload(
            pointer_id,
            target,
            payload_dispatch_target,
            uikit_hit_view,
            x,
            y,
        );

        let mut effective_ui_dispatch_target = uikit_target;
        let mut effective_ui_hit_view = uikit_hit_view;
        let mut ui_selector: Option<String> = None;
        let mut cocos_selector: Option<String> = None;

        if uikit_target != 0 {
            self.runtime.ui_objects.first_responder = uikit_target;
            if let Some((dispatch_target, hit_view, selector)) =
                self.dispatch_uikit_event_via_window_send_event("down", event_object, "host-touch-uikit")
            {
                effective_ui_dispatch_target = dispatch_target;
                if hit_view != 0 {
                    effective_ui_hit_view = hit_view;
                }
                ui_selector = Some(selector);
                self.runtime.host_input.ui_dispatched = self.runtime.host_input.ui_dispatched.saturating_add(1);
            }
        }

        let effective_cocos_dispatch_target = if enable_cocos_route {
            cocos_dispatch_target.unwrap_or(0)
        } else {
            0
        };
        if effective_cocos_dispatch_target != 0 {
            self.runtime.host_input.cocos_attempts = self.runtime.host_input.cocos_attempts.saturating_add(1);
            self.ensure_synthetic_touch_dispatch_target_ready(effective_cocos_dispatch_target, "host-touch-cocos");
            cocos_selector = self.dispatch_synthetic_touch_phase(
                effective_cocos_dispatch_target,
                "down",
                touch_object,
                touch_set,
                event_object,
                "host-touch-cocos",
            );
            if cocos_selector.is_some() {
                self.runtime.host_input.cocos_dispatched = self.runtime.host_input.cocos_dispatched.saturating_add(1);
            }
        }
        let widget_selector = self.dispatch_scene_widget_activation(
            target,
            touch_object,
            "down",
            true,
            "host-touch-scene-widget",
        );
        if widget_selector.is_some() {
            self.runtime.host_input.cocos_dispatched = self.runtime.host_input.cocos_dispatched.saturating_add(1);
        }

        let dispatch_kind = match (effective_ui_dispatch_target != 0, effective_cocos_dispatch_target != 0) {
            (true, true) => ActivePointerDispatchKind::Hybrid,
            (true, false) => ActivePointerDispatchKind::UIKit,
            (false, true) => ActivePointerDispatchKind::Cocos,
            (false, false) => ActivePointerDispatchKind::Cocos,
        };
        let mut dispatch_summary = match (ui_selector.as_deref(), cocos_selector.as_deref()) {
            (Some(ui), Some(cocos)) if ui != cocos => format!("{}+{}", ui, cocos),
            (Some(ui), _) => ui.to_string(),
            (_, Some(cocos)) => cocos.to_string(),
            _ => (!selector_name.is_empty())
                .then_some(selector_name.clone())
                .unwrap_or_else(|| "<none>".to_string()),
        };
        if let Some(widget_selector) = widget_selector.as_deref() {
            if dispatch_summary == "<none>" || dispatch_summary == "idle" {
                dispatch_summary = format!("widget:{}", widget_selector);
            } else if !dispatch_summary.contains(widget_selector) {
                dispatch_summary = format!("{}+widget:{}", dispatch_summary, widget_selector);
            }
        }

        self.runtime.scene.synthetic_touch_injections = self.runtime.scene.synthetic_touch_injections.saturating_add(1);
        self.runtime.ui_objects.first_responder = if effective_cocos_dispatch_target != 0 {
            effective_cocos_dispatch_target
        } else if effective_ui_dispatch_target != 0 {
            effective_ui_dispatch_target
        } else {
            target
        };
        self.set_synthetic_menu_item_pressed(target, true);
        self.runtime.host_input.active_touch = Some(ActivePointerTouch {
            pointer_id,
            target,
            callback_target,
            callback_selector,
            dispatch_kind,
            touch_dispatch_target: if effective_ui_dispatch_target != 0 {
                effective_ui_dispatch_target
            } else {
                effective_cocos_dispatch_target
            },
            touch_hit_view: effective_ui_hit_view,
            ui_dispatch_target: effective_ui_dispatch_target,
            ui_hit_view: effective_ui_hit_view,
            cocos_dispatch_target: effective_cocos_dispatch_target,
            touch_object,
            touch_set,
            event_object,
            last_x: x,
            last_y: y,
            source: source.to_string(),
        });
        self.runtime.host_input.last_target = Some(target);
        self.runtime.host_input.last_dispatch = Some(format!(
            "{}-touch-begin:{}:{}:{:.1}:{:.1}",
            dispatch_kind.as_str(),
            self.describe_ptr(target),
            dispatch_summary,
            x,
            y,
        ));
        self.diag.trace.push(format!(
            "     ↳ host touch.down target={} selector={} callbackTarget={} dispatchKind={} uiDispatchTarget={} cocosDispatchTarget={} touch={} set={} event={} x={:.1} y={:.1} source={} origin={} injections={}",
            self.describe_ptr(target),
            dispatch_summary,
            self.describe_ptr(callback_target),
            dispatch_kind.as_str(),
            self.describe_ptr(effective_ui_dispatch_target),
            self.describe_ptr(effective_cocos_dispatch_target),
            self.describe_ptr(touch_object),
            self.describe_ptr(touch_set),
            self.describe_ptr(event_object),
            x,
            y,
            source,
            origin,
            self.runtime.scene.synthetic_touch_injections,
        ));
    }

    fn dispatch_host_pointer_move(&mut self, pointer_id: u32, x: f32, y: f32, origin: &str, source: &str) {
        self.runtime.host_input.ui_attempts = self.runtime.host_input.ui_attempts.saturating_add(1);
        let Some(mut active) = self.runtime.host_input.active_touch.clone() else {
            self.runtime.host_input.events_ignored = self.runtime.host_input.events_ignored.saturating_add(1);
            self.runtime.host_input.last_dispatch = Some("move-without-active-touch".to_string());
            return;
        };
        if active.pointer_id != pointer_id {
            self.runtime.host_input.events_ignored = self.runtime.host_input.events_ignored.saturating_add(1);
            self.runtime.host_input.last_dispatch = Some(format!("move-pointer-mismatch:{}!= {}", pointer_id, active.pointer_id));
            return;
        }
        let inside = self.host_touch_target_contains_guest_point(active.target, x, y);
        self.set_synthetic_menu_item_pressed(active.target, inside);
        let payload_dispatch_target = if active.ui_dispatch_target != 0 {
            active.ui_dispatch_target
        } else {
            active.cocos_dispatch_target
        };
        self.update_synthetic_touch_payload(
            active.touch_object,
            active.touch_set,
            active.event_object,
            active.target,
            payload_dispatch_target,
            active.ui_hit_view,
            "move",
            x,
            y,
        );

        let mut ui_selector: Option<String> = None;
        if active.ui_dispatch_target != 0 {
            if let Some((dispatch_target, hit_view, selector)) =
                self.dispatch_uikit_event_via_window_send_event("move", active.event_object, "host-touch-uikit")
            {
                active.ui_dispatch_target = dispatch_target;
                active.touch_dispatch_target = dispatch_target;
                if hit_view != 0 {
                    active.ui_hit_view = hit_view;
                    active.touch_hit_view = hit_view;
                }
                ui_selector = Some(selector);
                self.runtime.host_input.ui_dispatched = self.runtime.host_input.ui_dispatched.saturating_add(1);
            }
        }

        let mut cocos_selector: Option<String> = None;
        if active.cocos_dispatch_target != 0 {
            self.runtime.host_input.cocos_attempts = self.runtime.host_input.cocos_attempts.saturating_add(1);
            cocos_selector = self.dispatch_synthetic_touch_phase(
                active.cocos_dispatch_target,
                "move",
                active.touch_object,
                active.touch_set,
                active.event_object,
                "host-touch-cocos",
            );
            if cocos_selector.is_some() {
                self.runtime.host_input.cocos_dispatched = self.runtime.host_input.cocos_dispatched.saturating_add(1);
            }
        }
        let widget_selector = self.dispatch_scene_widget_activation(
            active.target,
            active.touch_object,
            "move",
            inside,
            "host-touch-scene-widget",
        );
        if widget_selector.is_some() {
            self.runtime.host_input.cocos_dispatched = self.runtime.host_input.cocos_dispatched.saturating_add(1);
        }

        active.last_x = x;
        active.last_y = y;
        self.runtime.host_input.active_touch = Some(active.clone());
        let mut dispatch_summary = match (ui_selector.as_deref(), cocos_selector.as_deref()) {
            (Some(ui), Some(cocos)) if ui != cocos => format!("{}+{}", ui, cocos),
            (Some(ui), _) => ui.to_string(),
            (_, Some(cocos)) => cocos.to_string(),
            _ => "idle".to_string(),
        };
        if let Some(widget_selector) = widget_selector.as_deref() {
            if dispatch_summary == "idle" || dispatch_summary == "<none>" {
                dispatch_summary = format!("widget:{}", widget_selector);
            } else if !dispatch_summary.contains(widget_selector) {
                dispatch_summary = format!("{}+widget:{}", dispatch_summary, widget_selector);
            }
        }
        self.runtime.host_input.last_target = Some(active.target);
        self.runtime.host_input.last_dispatch = Some(format!(
            "{}-touch-move:{}:{}:{}:{:.1}:{:.1}",
            active.dispatch_kind.as_str(),
            self.describe_ptr(active.target),
            if inside { "inside" } else { "outside" },
            dispatch_summary,
            x,
            y,
        ));
        self.diag.trace.push(format!(
            "     ↳ host touch.move target={} inside={} dispatchKind={} uiDispatchTarget={} cocosDispatchTarget={} selector={} touch={} set={} event={} x={:.1} y={:.1} source={} origin={}",
            self.describe_ptr(active.target),
            if inside { "YES" } else { "NO" },
            active.dispatch_kind.as_str(),
            self.describe_ptr(active.ui_dispatch_target),
            self.describe_ptr(active.cocos_dispatch_target),
            dispatch_summary,
            self.describe_ptr(active.touch_object),
            self.describe_ptr(active.touch_set),
            self.describe_ptr(active.event_object),
            x,
            y,
            source,
            origin,
        ));
    }

    fn dispatch_host_pointer_up(&mut self, pointer_id: u32, x: f32, y: f32, origin: &str, source: &str) {
        self.runtime.host_input.ui_attempts = self.runtime.host_input.ui_attempts.saturating_add(1);
        let Some(mut active) = self.runtime.host_input.active_touch.take() else {
            if self.try_invoke_host_touch_fallback(pointer_id, x, y, origin, source, "host-touch-fallback") {
                return;
            }
            self.runtime.host_input.events_ignored = self.runtime.host_input.events_ignored.saturating_add(1);
            self.runtime.host_input.last_dispatch = Some("up-without-active-touch".to_string());
            return;
        };
        if active.pointer_id != pointer_id {
            self.runtime.host_input.active_touch = Some(active);
            self.runtime.host_input.events_ignored = self.runtime.host_input.events_ignored.saturating_add(1);
            self.runtime.host_input.last_dispatch = Some(format!("up-pointer-mismatch:{}", pointer_id));
            return;
        }

        self.runtime.scene.synthetic_touch_injections = self.runtime.scene.synthetic_touch_injections.saturating_add(1);
        let payload_dispatch_target = if active.ui_dispatch_target != 0 {
            active.ui_dispatch_target
        } else {
            active.cocos_dispatch_target
        };
        self.update_synthetic_touch_payload(
            active.touch_object,
            active.touch_set,
            active.event_object,
            active.target,
            payload_dispatch_target,
            active.ui_hit_view,
            "up",
            x,
            y,
        );
        self.set_synthetic_menu_item_pressed(active.target, false);
        let inside = self.host_touch_target_contains_guest_point(active.target, x, y);
        let mut invoked = false;
        // selector_name tracks selectors that were actually dispatched during this
        // pointer-up processing. Keep it empty initially; otherwise the authored callback
        // selector (for example NewGame:) suppresses the post-activate callback handoff.
        let mut selector_name = String::new();
        let mut post_dispatch_selector: Option<String> = None;
        let mut ui_selector: Option<String> = None;
        if active.ui_dispatch_target != 0 {
            if let Some((dispatch_target, hit_view, selector)) =
                self.dispatch_uikit_event_via_window_send_event("up", active.event_object, "host-touch-uikit")
            {
                active.ui_dispatch_target = dispatch_target;
                active.touch_dispatch_target = dispatch_target;
                if hit_view != 0 {
                    active.ui_hit_view = hit_view;
                    active.touch_hit_view = hit_view;
                }
                ui_selector = Some(selector.clone());
                if selector_name.is_empty() {
                    selector_name = selector;
                }
                invoked = true;
                self.runtime.host_input.ui_dispatched = self.runtime.host_input.ui_dispatched.saturating_add(1);
            }
        }

        let mut cocos_selector: Option<String> = None;
        if active.cocos_dispatch_target != 0 {
            self.runtime.host_input.cocos_attempts = self.runtime.host_input.cocos_attempts.saturating_add(1);
            cocos_selector = self.dispatch_synthetic_touch_phase(
                active.cocos_dispatch_target,
                "up",
                active.touch_object,
                active.touch_set,
                active.event_object,
                "host-touch-cocos",
            );
            if let Some(dispatched) = cocos_selector.clone() {
                if selector_name.is_empty() {
                    selector_name = dispatched;
                }
                invoked = true;
                self.runtime.host_input.cocos_dispatched = self.runtime.host_input.cocos_dispatched.saturating_add(1);
            }
        }
        let widget_selector = self.dispatch_scene_widget_activation(
            active.target,
            active.touch_object,
            "up",
            inside,
            "host-touch-scene-widget",
        );
        if let Some(widget_dispatched) = widget_selector.clone() {
            if selector_name.is_empty() {
                selector_name = widget_dispatched;
            }
            invoked = true;
            self.runtime.host_input.cocos_dispatched = self.runtime.host_input.cocos_dispatched.saturating_add(1);
        }

        if inside
            && self.should_post_dispatch_hit_callback(
                active.target,
                active.callback_target,
                active.callback_selector,
                &selector_name,
                active.ui_dispatch_target,
                active.cocos_dispatch_target,
                ui_selector.as_deref(),
                cocos_selector.as_deref(),
            )
        {
            let post_selector = self
                .synthetic_callback_selector_name(active.callback_selector)
                .unwrap_or_else(|| format!("0x{:08x}", active.callback_selector));
            if self.invoke_objc_selector_now(
                active.callback_target,
                &post_selector,
                active.target,
                0,
                120_000,
                "host-touch-post-activate",
            ) {
                self.diag.trace.push(format!(
                    "     ↳ host touch.post-callback target={} callbackTarget={} selector={} reason=scene-widget-activation uiDispatchTarget={} cocosDispatchTarget={} invoked=YES",
                    self.describe_ptr(active.target),
                    self.describe_ptr(active.callback_target),
                    post_selector,
                    self.describe_ptr(active.ui_dispatch_target),
                    self.describe_ptr(active.cocos_dispatch_target),
                ));
                post_dispatch_selector = Some(post_selector);
                invoked = true;
            }
        }

        if !invoked && inside && active.callback_selector != 0 {
            let callback_selector_name = self
                .synthetic_callback_selector_name(active.callback_selector)
                .unwrap_or_else(|| format!("0x{:08x}", active.callback_selector));
            if selector_name.is_empty() || !Self::selector_trace_mentions_selector(&selector_name, &callback_selector_name) {
                selector_name = callback_selector_name.clone();
            }
            invoked = self.invoke_objc_selector_now(
                active.callback_target,
                &callback_selector_name,
                active.target,
                0,
                120_000,
                "host-touch-callback",
            );
        }
        let mut dispatch_summary = match (ui_selector.as_deref(), cocos_selector.as_deref()) {
            (Some(ui), Some(cocos)) if ui != cocos => format!("{}+{}", ui, cocos),
            (Some(ui), _) => ui.to_string(),
            (_, Some(cocos)) => cocos.to_string(),
            _ if !selector_name.is_empty() => selector_name.clone(),
            _ => "<none>".to_string(),
        };
        if let Some(widget_selector) = widget_selector.as_deref() {
            if dispatch_summary == "<none>" || dispatch_summary == "idle" {
                dispatch_summary = format!("widget:{}", widget_selector);
            } else if !dispatch_summary.contains(widget_selector) {
                dispatch_summary = format!("{}+widget:{}", dispatch_summary, widget_selector);
            }
        }
        if let Some(post_selector) = post_dispatch_selector.as_deref() {
            if dispatch_summary == "<none>" || dispatch_summary == "idle" {
                dispatch_summary = format!("post:{}", post_selector);
            } else if !dispatch_summary.contains(post_selector) {
                dispatch_summary = format!("{}+post:{}", dispatch_summary, post_selector);
            }
        }
        self.runtime.host_input.last_target = Some(active.target);
        self.runtime.host_input.last_dispatch = Some(format!(
            "{}-touch-end:{}:{}:{}:{:.1}:{:.1}",
            active.dispatch_kind.as_str(),
            self.describe_ptr(active.target),
            if inside { "inside" } else { "outside" },
            if invoked { dispatch_summary.as_str() } else { "idle" },
            x,
            y,
        ));
        self.diag.trace.push(format!(
            "     ↳ host touch.up target={} selector={} callbackTarget={} dispatchKind={} uiDispatchTarget={} cocosDispatchTarget={} touch={} set={} event={} inside={} invoked={} x={:.1} y={:.1} source={} origin={} injections={} beganSource={}",
            self.describe_ptr(active.target),
            dispatch_summary,
            self.describe_ptr(active.callback_target),
            active.dispatch_kind.as_str(),
            self.describe_ptr(active.ui_dispatch_target),
            self.describe_ptr(active.cocos_dispatch_target),
            self.describe_ptr(active.touch_object),
            self.describe_ptr(active.touch_set),
            self.describe_ptr(active.event_object),
            if inside { "YES" } else { "NO" },
            if invoked { "YES" } else { "NO" },
            x,
            y,
            source,
            origin,
            self.runtime.scene.synthetic_touch_injections,
            active.source,
        ));
    }



}
