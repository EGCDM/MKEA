impl MemoryArm32Backend {
// UIKit-ish synthetic object helpers, scene-tree plumbing, UIGraphics, networking, and runloop.

    fn alloc_synthetic_ui_object(&mut self, label: impl Into<String>) -> u32 {
        let obj = self.runtime.graphics.synthetic_ui_object_cursor;
        self.runtime.graphics.synthetic_ui_object_cursor = self.runtime.graphics.synthetic_ui_object_cursor.saturating_add(0x20);
        self.diag.object_labels.insert(obj, label.into());
        obj
    }


    fn synthetic_notification_name_desc(&self, name_ptr: u32) -> String {
        if name_ptr == 0 {
            return "<any>".to_string();
        }
        self.diag
            .object_labels
            .get(&name_ptr)
            .cloned()
            .or_else(|| self.guest_string_value(name_ptr))
            .unwrap_or_else(|| self.describe_ptr(name_ptr))
    }

    fn synthetic_notification_name_matches(&self, lhs: u32, rhs: u32) -> bool {
        if lhs == 0 || rhs == 0 {
            return lhs == rhs;
        }
        if lhs == rhs {
            return true;
        }
        self.synthetic_notification_name_desc(lhs) == self.synthetic_notification_name_desc(rhs)
    }

    fn ensure_notification_center_default(&mut self) -> u32 {
        if self.runtime.ui_runtime.notification_center_default != 0 {
            return self.runtime.ui_runtime.notification_center_default;
        }
        let center = self.alloc_synthetic_ui_object("NSNotificationCenter.default(synth)");
        self.runtime.ui_runtime.notification_center_default = center;
        center
    }

    fn register_synthetic_notification_observer(
        &mut self,
        center: u32,
        observer: u32,
        selector_ptr: u32,
        name_ptr: u32,
        object: u32,
        origin: &str,
    ) {
        let selector_name = self
            .objc_read_selector_name(selector_ptr)
            .unwrap_or_else(|| format!("0x{selector_ptr:08x}"));
        let center = if center != 0 { center } else { self.ensure_notification_center_default() };
        let total = {
            let entries = self.runtime.ui_runtime.notification_observers.entry(center).or_default();
            if let Some(existing) = entries.iter_mut().find(|entry| {
            entry.observer == observer
                && entry.selector_ptr == selector_ptr
                && entry.name_ptr == name_ptr
                && entry.object == object
        }) {
                existing.registrations = existing.registrations.saturating_add(1);
            } else {
                entries.push(SyntheticNotificationObserverState {
                    observer,
                    selector_ptr,
                    selector_name: selector_name.clone(),
                    name_ptr,
                    object,
                    registrations: 1,
                });
            }
            entries.len()
        };
        self.push_callback_trace(format!(
            "notification.observe origin={} center={} observer={} selector={} name={} object={} total={}",
            origin,
            self.describe_ptr(center),
            self.describe_ptr(observer),
            selector_name,
            self.synthetic_notification_name_desc(name_ptr),
            self.describe_ptr(object),
            total,
        ));
    }

    fn remove_synthetic_notification_observer(
        &mut self,
        center: u32,
        observer: u32,
        name_ptr: Option<u32>,
        object: Option<u32>,
        origin: &str,
    ) -> usize {
        let center = if center != 0 { center } else { self.runtime.ui_runtime.notification_center_default };
        let requested_name_desc = name_ptr
            .map(|value| self.synthetic_notification_name_desc(value))
            .unwrap_or_else(|| "<any>".to_string());
        let requested_object_desc = object
            .map(|value| self.describe_ptr(value))
            .unwrap_or_else(|| "<any>".to_string());
        let Some(mut entries) = self.runtime.ui_runtime.notification_observers.remove(&center) else {
            return 0;
        };
        let before = entries.len();
        entries.retain(|entry| {
            if entry.observer != observer {
                return true;
            }
            if let Some(name) = name_ptr {
                let matches = if entry.name_ptr == 0 || name == 0 {
                    entry.name_ptr == name
                } else if entry.name_ptr == name {
                    true
                } else {
                    self.synthetic_notification_name_desc(entry.name_ptr) == requested_name_desc
                };
                if !matches {
                    return true;
                }
            }
            if let Some(obj) = object {
                if entry.object != 0 && obj != 0 && entry.object != obj {
                    return true;
                }
            }
            false
        });
        let remaining = entries.len();
        if !entries.is_empty() {
            self.runtime.ui_runtime.notification_observers.insert(center, entries);
        }
        let removed = before.saturating_sub(remaining);
        self.push_callback_trace(format!(
            "notification.remove origin={} center={} observer={} name={} object={} removed={} remaining={}",
            origin,
            self.describe_ptr(center),
            self.describe_ptr(observer),
            requested_name_desc,
            requested_object_desc,
            removed,
            remaining,
        ));
        removed
    }

    fn ensure_synthetic_movie_player_view(&mut self, player: u32) -> u32 {
        if let Some(existing) = self
            .runtime
            .ui_runtime
            .movie_players
            .get(&player)
            .map(|state| state.synthetic_view)
            .filter(|value| *value != 0)
        {
            return existing;
        }
        let view = self.alloc_synthetic_ui_object(format!("MPMoviePlayerView.synthetic<{}>", self.describe_ptr(player)));
        self.runtime
            .ui_runtime
            .movie_players
            .entry(player)
            .or_default()
            .synthetic_view = view;
        self.runtime.ui_objects.view_superviews.entry(view).or_insert(self.runtime.ui_objects.window);
        let children = self.runtime.ui_objects.view_subviews.entry(self.runtime.ui_objects.window).or_default();
        if !children.contains(&view) {
            children.push(view);
        }
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1) as f32;
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
        self.ui_set_frame_bits(view, [0.0f32.to_bits(), 0.0f32.to_bits(), surface_w.to_bits(), surface_h.to_bits()]);
        self.ui_set_bounds_bits(view, [0.0f32.to_bits(), 0.0f32.to_bits(), surface_w.to_bits(), surface_h.to_bits()]);
        view
    }

    fn synthetic_notification_fields(&self, object: u32) -> Option<(u32, u32, u32)> {
        self.runtime
            .ui_runtime
            .synthetic_notifications
            .get(&object)
            .map(|state| (state.name_ptr, state.object, state.user_info))
    }

    fn create_synthetic_notification(&mut self, name_ptr: u32, object: u32, user_info: u32, origin: &str) -> u32 {
        let mut label = format!(
            "NSNotification.synthetic<name={}, object={}>",
            self.synthetic_notification_name_desc(name_ptr),
            self.describe_ptr(object),
        );
        if label.len() > 160 {
            label.truncate(160);
        }
        let notification = self.alloc_synthetic_ui_object(label);
        if let Some(class_ptr) = self.objc_lookup_class_by_name("NSNotification") {
            self.objc_attach_receiver_class(notification, class_ptr, "NSNotification");
        }
        self.runtime.ui_runtime.synthetic_notifications.insert(
            notification,
            SyntheticNotificationState {
                name_ptr,
                object,
                user_info,
            },
        );
        self.push_callback_trace(format!(
            "notification.synthetic origin={} notification={} name={} object={} userInfo={}",
            origin,
            self.describe_ptr(notification),
            self.synthetic_notification_name_desc(name_ptr),
            self.describe_ptr(object),
            self.describe_ptr(user_info),
        ));
        notification
    }

    fn resolve_movie_playback_finished_notification_name(&mut self) -> u32 {
        self.materialize_host_string_object(
            "NSString.MPMoviePlayerPlaybackDidFinishNotification",
            "MPMoviePlayerPlaybackDidFinishNotification",
        )
    }

    fn read_mp4_duration_seconds(path: &std::path::Path) -> Option<f64> {
        use std::io::Read;

        fn read_box_header(buf: &[u8], offset: usize) -> Option<(u64, [u8; 4], usize)> {
            if offset.checked_add(8)? > buf.len() {
                return None;
            }
            let size32 = u32::from_be_bytes(buf[offset..offset + 4].try_into().ok()?);
            let kind: [u8; 4] = buf[offset + 4..offset + 8].try_into().ok()?;
            if size32 == 0 {
                return Some(((buf.len() - offset) as u64, kind, 8));
            }
            if size32 == 1 {
                if offset.checked_add(16)? > buf.len() {
                    return None;
                }
                let size64 = u64::from_be_bytes(buf[offset + 8..offset + 16].try_into().ok()?);
                return Some((size64, kind, 16));
            }
            Some((size32 as u64, kind, 8))
        }

        fn parse_range(buf: &[u8], start: usize, end: usize) -> Option<f64> {
            let mut offset = start;
            while offset.checked_add(8)? <= end && offset.checked_add(8)? <= buf.len() {
                let (size, kind, header_len) = read_box_header(buf, offset)?;
                if size < header_len as u64 {
                    return None;
                }
                let box_end = offset.checked_add(size as usize)?;
                if box_end > end || box_end > buf.len() {
                    return None;
                }
                if &kind == b"moov" {
                    if let Some(secs) = parse_range(buf, offset + header_len, box_end) {
                        return Some(secs);
                    }
                } else if &kind == b"mvhd" {
                    let body = offset + header_len;
                    if body >= box_end {
                        return None;
                    }
                    let version = buf[body];
                    if version == 0 {
                        if body.checked_add(20)? > box_end {
                            return None;
                        }
                        let timescale = u32::from_be_bytes(buf[body + 12..body + 16].try_into().ok()?);
                        let duration = u32::from_be_bytes(buf[body + 16..body + 20].try_into().ok()?);
                        if timescale != 0 {
                            return Some(duration as f64 / timescale as f64);
                        }
                    } else if version == 1 {
                        if body.checked_add(32)? > box_end {
                            return None;
                        }
                        let timescale = u32::from_be_bytes(buf[body + 20..body + 24].try_into().ok()?);
                        let duration = u64::from_be_bytes(buf[body + 24..body + 32].try_into().ok()?);
                        if timescale != 0 {
                            return Some(duration as f64 / timescale as f64);
                        }
                    }
                }
                if size == 0 {
                    break;
                }
                offset = box_end;
            }
            None
        }

        let mut file = std::fs::File::open(path).ok()?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).ok()?;
        parse_range(&buf, 0, buf.len())
    }

    fn resolve_movie_duration_ticks(&mut self, player: u32, origin: &str) -> u32 {
        let Some(content_url) = self.runtime.ui_runtime.movie_players.get(&player).map(|state| state.content_url) else {
            return 1;
        };
        let path = self.resolve_path_from_url_like_value(content_url, false);
        if let Some(path) = path.as_ref() {
            if let Some(seconds) = Self::read_mp4_duration_seconds(path) {
                let ticks = (seconds * 60.0).ceil().max(1.0) as u32;
                self.push_callback_trace(format!(
                    "movie.duration origin={} player={} path={} seconds={:.3} ticks={}",
                    origin,
                    self.describe_ptr(player),
                    path.display(),
                    seconds,
                    ticks,
                ));
                return ticks.max(1);
            }
            self.push_callback_trace(format!(
                "movie.duration origin={} player={} path={} seconds=<unavailable> fallbackTicks=1",
                origin,
                self.describe_ptr(player),
                path.display(),
            ));
        } else {
            self.push_callback_trace(format!(
                "movie.duration origin={} player={} path=<unresolved> fallbackTicks=1",
                origin,
                self.describe_ptr(player),
            ));
        }
        1
    }

    fn start_synthetic_movie_playback(&mut self, player: u32, origin: &str) -> bool {
        if player == 0 || !self.runtime.ui_runtime.movie_players.contains_key(&player) {
            return false;
        }
        let now_tick = self.runtime.ui_runtime.runloop_ticks;
        let (content_url, autoplay, remaining_before, finish_tick_before, duration_before) = {
            let state = self.runtime.ui_runtime.movie_players.get(&player).unwrap();
            (
                state.content_url,
                state.should_autoplay,
                state.playback_remaining_ticks,
                state.playback_finish_tick,
                state.playback_duration_ticks,
            )
        };
        let remaining = if remaining_before != 0 {
            remaining_before
        } else {
            let duration = self.resolve_movie_duration_ticks(player, origin).max(1);
            if let Some(state) = self.runtime.ui_runtime.movie_players.get_mut(&player) {
                state.playback_duration_ticks = duration;
            }
            duration
        };
        let finish_tick = now_tick.saturating_add(remaining.max(1));
        if let Some(state) = self.runtime.ui_runtime.movie_players.get_mut(&player) {
            state.is_playing = true;
            state.prepared = true;
            state.playback_started_tick = now_tick;
            state.playback_remaining_ticks = remaining.max(1);
            state.playback_finish_tick = finish_tick;
        }
        let url_desc = if content_url != 0 {
            self.resolve_path_from_url_like_value(content_url, false)
                .map(|path| path.display().to_string())
                .or_else(|| self.guest_string_value(content_url))
                .unwrap_or_else(|| self.describe_ptr(content_url))
        } else {
            "<none>".to_string()
        };
        self.push_callback_trace(format!(
            "movie.lifecycle.start origin={} player={} autoplay={} contentURL={} startTick={} finishTick={} durationTicks={} previousFinishTick={}",
            origin,
            self.describe_ptr(player),
            if autoplay { "YES" } else { "NO" },
            url_desc,
            now_tick,
            finish_tick,
            remaining.max(duration_before).max(1),
            finish_tick_before,
        ));
        true
    }

    fn maybe_autostart_synthetic_movie_player(&mut self, player: u32, origin: &str) -> bool {
        let Some(state) = self.runtime.ui_runtime.movie_players.get(&player).cloned() else {
            return false;
        };
        if !state.should_autoplay || state.is_playing {
            return false;
        }
        if state.synthetic_view == 0 {
            return false;
        }
        let is_attached = self.runtime.ui_objects.view_superviews.contains_key(&state.synthetic_view)
            || self.runtime.ui_objects.window == state.synthetic_view;
        if !is_attached || !self.runtime.ui_runtime.window_visible || !self.runtime.ui_runtime.app_active {
            return false;
        }
        self.start_synthetic_movie_playback(player, origin)
    }

    fn post_synthetic_movie_playback_finished(&mut self, player: u32, origin: &str) -> usize {
        let center = self.ensure_notification_center_default();
        let entries = self
            .runtime
            .ui_runtime
            .notification_observers
            .get(&center)
            .cloned()
            .unwrap_or_default();
        let fallback_name = self.resolve_movie_playback_finished_notification_name();
        let mut delivered = 0usize;
        for entry in entries {
            let object_matches = entry.object == 0 || entry.object == player;
            if !object_matches {
                continue;
            }
            let notification_name = if entry.name_ptr != 0 { entry.name_ptr } else { fallback_name };
            let deliveries = entry.registrations.max(1);
            for _ in 0..deliveries {
                let notification = self.create_synthetic_notification(notification_name, player, 0, origin);
                self.schedule_delayed_selector(entry.observer, &entry.selector_name, notification, 0, origin);
                delivered = delivered.saturating_add(1);
            }
            self.push_callback_trace(format!(
                "notification.movie-finished origin={} center={} observer={} selector={} name={} object={} deliveries={}",
                origin,
                self.describe_ptr(center),
                self.describe_ptr(entry.observer),
                entry.selector_name,
                self.synthetic_notification_name_desc(notification_name),
                self.describe_ptr(player),
                deliveries,
            ));
        }
        delivered
    }

    fn drive_synthetic_movie_players(&mut self, origin: &str) {
        let now_tick = self.runtime.ui_runtime.runloop_ticks;
        let players: Vec<u32> = self.runtime.ui_runtime.movie_players.keys().copied().collect();
        for player in players {
            let _ = self.maybe_autostart_synthetic_movie_player(player, &format!("{}:autoplay", origin));
            let Some(state) = self.runtime.ui_runtime.movie_players.get(&player).cloned() else {
                continue;
            };
            if !state.is_playing || state.playback_finish_tick == 0 || now_tick < state.playback_finish_tick {
                continue;
            }
            let delivered = self.post_synthetic_movie_playback_finished(player, &format!("{}:playback-finished", origin));
            if let Some(state_mut) = self.runtime.ui_runtime.movie_players.get_mut(&player) {
                state_mut.is_playing = false;
                state_mut.playback_remaining_ticks = 0;
                state_mut.playback_finish_tick = 0;
                state_mut.finish_notifications_posted = state_mut.finish_notifications_posted.saturating_add(delivered as u32);
            }
            self.push_callback_trace(format!(
                "movie.lifecycle.finish origin={} player={} tick={} callbacksQueued={} view={} running={} next={}",
                origin,
                self.describe_ptr(player),
                now_tick,
                delivered,
                self.describe_ptr(state.synthetic_view),
                self.describe_ptr(self.runtime.ui_cocos.running_scene),
                self.describe_ptr(self.runtime.ui_cocos.next_scene),
            ));
        }
    }

    fn f32_from_bits(bits: u32) -> f32 {
        f32::from_bits(bits)
    }

    fn nstimeinterval_secs_from_regs(&self, low_bits: u32, high_bits: u32) -> Option<f64> {
        let packed = ((high_bits as u64) << 32) | (low_bits as u64);
        let secs64 = f64::from_bits(packed);
        if secs64.is_finite() && secs64 > 0.0 {
            return Some(secs64);
        }
        let secs32 = Self::f32_from_bits(low_bits);
        if secs32.is_finite() && secs32 > 0.0 {
            return Some(secs32 as f64);
        }
        None
    }

    fn nstimeinterval_f32_bits_from_regs(&self, low_bits: u32, high_bits: u32) -> u32 {
        self
            .nstimeinterval_secs_from_regs(low_bits, high_bits)
            .map(|secs| (secs as f32).to_bits())
            .unwrap_or_else(|| (1.0f32 / 60.0f32).to_bits())
    }

    fn nstimeinterval_f32_bits_from_stack_words(&self, low_word_offset: u32) -> u32 {
        let low_bits = self.peek_stack_u32(low_word_offset).unwrap_or(0);
        let high_bits = self.peek_stack_u32(low_word_offset.saturating_add(1)).unwrap_or(0);
        self.nstimeinterval_f32_bits_from_regs(low_bits, high_bits)
    }

    fn plausible_ui_scalar(value: f32) -> bool {
        value.is_finite() && value.abs() <= 16384.0
    }

    fn plausible_ui_size(value: f32) -> bool {
        value.is_finite() && value > 0.0 && value <= 16384.0
    }

    fn peek_stack_u32(&self, word_offset: u32) -> Option<u32> {
        self.read_u32_le(self.cpu.regs[13].wrapping_add(word_offset.saturating_mul(4))).ok()
    }

    fn collect_objc_variadic_object_list(&self, first: u32, second: u32, max_stack_words: u32) -> Vec<u32> {
        let mut out = Vec::new();
        if first == 0 {
            return out;
        }
        out.push(first);
        if second == 0 {
            return out;
        }
        out.push(second);
        for word in 0..max_stack_words {
            let value = self.peek_stack_u32(word).unwrap_or(0);
            if value == 0 {
                break;
            }
            out.push(value);
        }
        out
    }

    fn read_u32_list(&self, base: u32, count: usize, max_count: usize) -> Vec<u32> {
        if base == 0 || count == 0 {
            return Vec::new();
        }
        let capped = count.min(max_count);
        let mut out = Vec::with_capacity(capped);
        for i in 0..capped {
            let addr = base.wrapping_add((i as u32).saturating_mul(4));
            let value = self.read_u32_le(addr).unwrap_or(0);
            if value == 0 {
                break;
            }
            out.push(value);
        }
        out
    }

    fn read_cg_size_from_regs(&self) -> Option<(u32, u32)> {
        let w = Self::f32_from_bits(self.cpu.regs[0]);
        let h = Self::f32_from_bits(self.cpu.regs[1]);
        if Self::plausible_ui_size(w) && Self::plausible_ui_size(h) {
            return Some((w.round().max(1.0) as u32, h.round().max(1.0) as u32));
        }
        if self.cpu.regs[0] != 0 {
            if let (Ok(w_bits), Ok(h_bits)) = (self.read_u32_le(self.cpu.regs[0]), self.read_u32_le(self.cpu.regs[0].wrapping_add(4))) {
                let w = Self::f32_from_bits(w_bits);
                let h = Self::f32_from_bits(h_bits);
                if Self::plausible_ui_size(w) && Self::plausible_ui_size(h) {
                    return Some((w.round().max(1.0) as u32, h.round().max(1.0) as u32));
                }
            }
        }
        None
    }

    fn read_cg_rect_after_ctx(&self) -> Option<(i32, i32, u32, u32)> {
        let x = Self::f32_from_bits(self.cpu.regs[1]);
        let y = Self::f32_from_bits(self.cpu.regs[2]);
        let w = Self::f32_from_bits(self.cpu.regs[3]);
        let h = Self::f32_from_bits(self.cpu.regs[4]);
        if Self::plausible_ui_scalar(x) && Self::plausible_ui_scalar(y) && Self::plausible_ui_size(w) && Self::plausible_ui_size(h) {
            return Some((x.round() as i32, y.round() as i32, w.round().max(1.0) as u32, h.round().max(1.0) as u32));
        }
        let ptr = self.cpu.regs[1];
        if ptr != 0 {
            let values = [
                self.read_u32_le(ptr).ok(),
                self.read_u32_le(ptr.wrapping_add(4)).ok(),
                self.read_u32_le(ptr.wrapping_add(8)).ok(),
                self.read_u32_le(ptr.wrapping_add(12)).ok(),
            ];
            if let [Some(a), Some(b), Some(c), Some(d)] = values {
                let x = Self::f32_from_bits(a);
                let y = Self::f32_from_bits(b);
                let w = Self::f32_from_bits(c);
                let h = Self::f32_from_bits(d);
                if Self::plausible_ui_scalar(x) && Self::plausible_ui_scalar(y) && Self::plausible_ui_size(w) && Self::plausible_ui_size(h) {
                    return Some((x.round() as i32, y.round() as i32, w.round().max(1.0) as u32, h.round().max(1.0) as u32));
                }
            }
        }
        None
    }

    fn score_ui_pair(values: (f32, f32), positive_only: bool) -> i32 {
        let (a, b) = values;
        if !a.is_finite() || !b.is_finite() {
            return -999_999;
        }
        if positive_only {
            if a <= 0.0 || b <= 0.0 || a > 16384.0 || b > 16384.0 {
                return -999_999;
            }
            let mut score = 0;
            if a <= 8192.0 { score += 3; }
            if b <= 8192.0 { score += 3; }
            if a > 0.0 { score += 1; }
            if b > 0.0 { score += 1; }
            return score;
        }
        if a.abs() > 16384.0 || b.abs() > 16384.0 {
            return -999_999;
        }
        let mut score = 0;
        if a.abs() <= 8192.0 { score += 2; }
        if b.abs() <= 8192.0 { score += 2; }
        if a != 0.0 || b != 0.0 { score += 1; }
        score
    }

    fn score_ui_rect(values: (f32, f32, f32, f32)) -> i32 {
        let (x, y, w, h) = values;
        if !x.is_finite() || !y.is_finite() || !w.is_finite() || !h.is_finite() {
            return -999_999;
        }
        if w < 0.0 || h < 0.0 || x.abs() > 1_000_000.0 || y.abs() > 1_000_000.0 || w > 1_000_000.0 || h > 1_000_000.0 {
            return -999_999;
        }
        let mut score = 0;
        if w <= 8192.0 { score += 2; }
        if h <= 8192.0 { score += 2; }
        if x.abs() <= 8192.0 { score += 1; }
        if y.abs() <= 8192.0 { score += 1; }
        if w > 0.0 { score += 1; }
        if h > 0.0 { score += 1; }
        score
    }


    fn score_ui_point(values: (f32, f32)) -> i32 {
        let (x, y) = values;
        if !x.is_finite() || !y.is_finite() {
            return -999_999;
        }
        if x.abs() > 1_000_000.0 || y.abs() > 1_000_000.0 {
            return -999_999;
        }
        let mut score = 0;
        if x.abs() <= 16_384.0 { score += 2; }
        if y.abs() <= 16_384.0 { score += 2; }
        if x.abs() <= 8_192.0 { score += 1; }
        if y.abs() <= 8_192.0 { score += 1; }
        score
    }

    fn is_trampoline_addr(&self, addr: u32) -> bool {
        let start = self.address_space.trampoline_addr;
        let end = start.saturating_add(self.address_space.trampoline_size);
        addr >= start && addr < end
    }

    fn looks_like_trampoline_stub_word(word: u32) -> bool {
        word == u32::from_le_bytes(ARM_BX_LR)
    }

    fn contains_trampoline_stub_words(words: &[u32]) -> bool {
        words.iter().copied().any(Self::looks_like_trampoline_stub_word)
    }

    fn read_words_at(&self, addr: u32, count: usize) -> Option<Vec<u32>> {
        if addr == 0 || count == 0 || self.is_trampoline_addr(addr) {
            return None;
        }
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let off = (i as u32).saturating_mul(4);
            out.push(self.read_u32_le(addr.wrapping_add(off)).ok()?);
        }
        if Self::contains_trampoline_stub_words(&out) {
            return None;
        }
        Some(out)
    }

    fn looks_like_single_scalar_tail(bits: u32) -> bool {
        if bits == 0 {
            return false;
        }
        let value = Self::f32_from_bits(bits);
        if !value.is_finite() {
            return true;
        }
        value.abs() > 0.0 && value.abs() <= 16_384.0
    }

    fn read_msgsend_point_arg(&self, arg2_bits: u32, arg3_bits: u32) -> Option<([u32; 2], String)> {
        let mut best: Option<([u32; 2], String, i32)> = None;
        let mut consider = |bits: [u32; 2], source: &str| {
            if Self::contains_trampoline_stub_words(&bits) {
                return;
            }
            let score = Self::score_ui_point((Self::f32_from_bits(bits[0]), Self::f32_from_bits(bits[1])));
            if score <= -999_999 {
                return;
            }
            let replace = best.as_ref().map(|(_, _, prev)| score > *prev).unwrap_or(true);
            if replace {
                best = Some((bits, source.to_string(), score));
            }
        };

        consider([arg2_bits, arg3_bits], "R2/R3");
        if let Some(words) = self.read_words_at(arg2_bits, 2) {
            consider([words[0], words[1]], "PTR@R2");
        }
        consider([self.vfp_get_s(0), self.vfp_get_s(1)], "S0/S1");

        best.map(|(bits, source, _)| (bits, source))
    }

    fn read_msgsend_float_arg(&self) -> Option<(u32, String)> {
        let mut best: Option<(u32, String, i32)> = None;
        let mut consider = |bits: u32, source: &str| {
            if Self::contains_trampoline_stub_words(&[bits]) {
                return;
            }
            let value = Self::f32_from_bits(bits);
            if !value.is_finite() {
                return;
            }
            let abs = value.abs();
            let score = if abs <= 16.0 {
                400
            } else if abs <= 4096.0 {
                250
            } else if abs <= 65536.0 {
                100
            } else {
                -999_999
            };
            if score <= -999_999 {
                return;
            }
            let replace = best.as_ref().map(|(_, _, prev)| score > *prev).unwrap_or(true);
            if replace {
                best = Some((bits, source.to_string(), score));
            }
        };

        consider(self.cpu.regs[2], "R2(f32)");
        if let Some(words) = self.read_words_at(self.cpu.regs[2], 1) {
            consider(words[0], "PTR@R2(f32)");
        }
        if let Some(word) = self.peek_stack_u32(0) {
            consider(word, "STACK[0](f32)");
        }
        consider(self.vfp_get_s(0), "S0(f32)");
        best.map(|(bits, src, _)| (bits, src))
    }

    fn read_msgsend_float_triplet_arg(&self) -> Option<([u32; 3], String)> {
        let mut best: Option<([u32; 3], String, i32)> = None;
        let mut consider = |bits: [u32; 3], source: &str| {
            if Self::contains_trampoline_stub_words(&bits) {
                return;
            }
            let duration = Self::f32_from_bits(bits[0]);
            let scale_x = Self::f32_from_bits(bits[1]);
            let scale_y = Self::f32_from_bits(bits[2]);
            if !duration.is_finite() || !scale_x.is_finite() || !scale_y.is_finite() {
                return;
            }
            if duration <= 0.0 || duration > 120.0 {
                return;
            }
            let ax = scale_x.abs();
            let ay = scale_y.abs();
            if ax > 65536.0 || ay > 65536.0 {
                return;
            }
            let mut score = 0i32;
            if duration <= 10.0 { score += 300; } else { score += 120; }
            if ax >= 1.0e-4 { score += 120; } else { score -= 80; }
            if ay >= 1.0e-4 { score += 120; } else { score -= 80; }
            if ax <= 4096.0 { score += 80; }
            if ay <= 4096.0 { score += 80; }
            if (0.125..=8192.0).contains(&ax) { score += 40; }
            if (0.125..=8192.0).contains(&ay) { score += 40; }
            let replace = best.as_ref().map(|(_, _, prev)| score > *prev).unwrap_or(true);
            if replace {
                best = Some((bits, source.to_string(), score));
            }
        };

        if let Some(stack0) = self.peek_stack_u32(0) {
            consider([self.cpu.regs[2], self.cpu.regs[3], stack0], "R2/R3+STACK[0]");
        }
        if let (Some(stack0), Some(stack1)) = (self.peek_stack_u32(0), self.peek_stack_u32(1)) {
            consider([self.cpu.regs[2], stack0, stack1], "R2+STACK[0..1]");
        }
        if let (Some(stack0), Some(stack1), Some(stack2)) = (
            self.peek_stack_u32(0),
            self.peek_stack_u32(1),
            self.peek_stack_u32(2),
        ) {
            consider([stack0, stack1, stack2], "STACK[0..2]");
        }
        consider([self.vfp_get_s(0), self.vfp_get_s(1), self.vfp_get_s(2)], "S0..S2");
        if let Some(words) = self.read_words_at(self.cpu.regs[2], 3) {
            consider([words[0], words[1], words[2]], "PTR@R2[0..2]");
        }

        best.map(|(bits, source, _)| (bits, source))
    }

    fn read_msgsend_bool_arg(&self) -> Option<(bool, String)> {
        let reg = self.cpu.regs[2];
        if reg == 0 || reg == 1 {
            return Some((reg != 0, "R2(bool)".to_string()));
        }
        if (reg & !0xff) == 0 && (reg & 0xff) <= 1 {
            return Some((((reg & 0xff) != 0), "R2(u8-bool)".to_string()));
        }
        if let Some(words) = self.read_words_at(reg, 1) {
            let raw = words[0];
            if raw == 0 || raw == 1 {
                return Some((raw != 0, "PTR@R2(bool)".to_string()));
            }
            if (raw & !0xff) == 0 && (raw & 0xff) <= 1 {
                return Some((((raw & 0xff) != 0), "PTR@R2(u8-bool)".to_string()));
            }
        }
        if let Some(word) = self.peek_stack_u32(0) {
            if word == 0 || word == 1 {
                return Some((word != 0, "STACK[0](bool)".to_string()));
            }
        }
        let s0 = self.vfp_get_s(0);
        if s0 == 0.0f32.to_bits() || s0 == 1.0f32.to_bits() {
            return Some((s0 != 0, "S0(f32-bool)".to_string()));
        }
        if self.cpu.regs[2] == 0 && self.cpu.regs[3] != 0 {
            return Some((
                false,
                format!("R2(zero-bool, tail-ignored=0x{:08x})", self.cpu.regs[3]),
            ));
        }
        None
    }

    fn classify_escaped_fastpath(&self, selector: &str, receiver: u32, arg2: u32, arg3: u32) -> Option<String> {
        let receiver_label = self.diag.object_labels.get(&receiver).cloned().unwrap_or_default();
        let class_name = self.objc_class_name_for_receiver(receiver).unwrap_or_default();
        let looks_menu_layer = self.active_profile().is_menu_layer_label(&class_name) || self.active_profile().is_menu_layer_label(&receiver_label);
        let selector_implies_cocos = matches!(
            selector,
            "spriteWithFile:"
                | "initWithFile:"
                | "spriteWithFile:rect:"
                | "initWithFile:rect:"
                | "itemFromNormalSprite:selectedSprite:disabledSprite:target:selector:"
                | "initFromNormalSprite:selectedSprite:disabledSprite:target:selector:"
                | "initWithTarget:selector:"
                | "menuWithItems:"
                | "initWithItems:"
                | "menuWithArray:"
                | "initWithArray:"
                | "alignItemsVertically"
                | "alignItemsVerticallyWithPadding:"
                | "setVisible:"
                | "setIsTouchEnabled:"
                | "visit"
                | "draw"
        );
        let looks_cocos_like = selector_implies_cocos
            || class_name.contains("CC")
            || receiver_label.contains("CC")
            || class_name.contains("Scene")
            || receiver_label.contains("Scene")
            || class_name.contains("Layer")
            || receiver_label.contains("Layer")
            || self.active_profile().is_first_scene_label(&receiver_label)
            || looks_menu_layer;
        match selector {
            "setRelativeAnchorPoint:" => {
                if looks_menu_layer {
                    return Some(format!(
                        "escaped-fastpath: reason=receiver-class-not-whitelisted class={} label={} | late MenuLayer bool-path candidate",
                        if class_name.is_empty() { "?" } else { &class_name },
                        if receiver_label.is_empty() { "?" } else { &receiver_label },
                    ));
                }
                if arg2 <= 1 && arg3 != 0 {
                    return Some(format!(
                        "escaped-fastpath: reason=noncanonical-bool-shape arg2=0x{arg2:08x} arg3=0x{arg3:08x}",
                    ));
                }
                if looks_cocos_like {
                    return Some(format!(
                        "escaped-fastpath: reason=receiver-class-not-whitelisted class={} label={}",
                        if class_name.is_empty() { "?" } else { &class_name },
                        if receiver_label.is_empty() { "?" } else { &receiver_label },
                    ));
                }
                None
            }
            "setAnchorPoint:" | "setPosition:" => {
                if looks_cocos_like {
                    return Some(format!(
                        "escaped-fastpath: reason=receiver-class-not-whitelisted class={} label={}",
                        if class_name.is_empty() { "?" } else { &class_name },
                        if receiver_label.is_empty() { "?" } else { &receiver_label },
                    ));
                }
                None
            }
            "spriteWithFile:"
            | "initWithFile:"
            | "spriteWithFile:rect:"
            | "initWithFile:rect:"
            | "itemFromNormalSprite:selectedSprite:disabledSprite:target:selector:"
            | "initFromNormalSprite:selectedSprite:disabledSprite:target:selector:"
            | "initWithTarget:selector:"
            | "menuWithItems:"
            | "initWithItems:"
            | "menuWithArray:"
            | "initWithArray:"
            | "alignItemsVertically"
            | "alignItemsVerticallyWithPadding:"
            | "setVisible:"
            | "setIsTouchEnabled:"
            | "visit"
            | "draw" => {
                if looks_cocos_like {
                    return Some(format!(
                        "escaped-fastpath: reason=scene-graph-selector class={} label={} selector={}",
                        if class_name.is_empty() { "?" } else { &class_name },
                        if receiver_label.is_empty() { "?" } else { &receiver_label },
                        selector,
                    ));
                }
                None
            }
            _ => None,
        }
    }

    fn read_msgsend_pair_arg(
        &self,
        positive_only: bool,
        allow_stack: bool,
        prefer_zero_regs: bool,
    ) -> Option<([u32; 2], String)> {
        let reg_bits = [self.cpu.regs[2], self.cpu.regs[3]];
        if prefer_zero_regs && reg_bits == [0, 0] {
            return Some((reg_bits, "R2/R3(zero-pref)".to_string()));
        }

        let mut best: Option<([u32; 2], String, i32)> = None;
        let mut consider = |bits: [u32; 2], source: &str| {
            if Self::contains_trampoline_stub_words(&bits) {
                return;
            }
            let score = Self::score_ui_pair((Self::f32_from_bits(bits[0]), Self::f32_from_bits(bits[1])), positive_only);
            if score <= -999_999 {
                return;
            }
            let replace = best.as_ref().map(|(_, _, prev)| score > *prev).unwrap_or(true);
            if replace {
                best = Some((bits, source.to_string(), score));
            }
        };

        consider(reg_bits, "R2/R3");
        if let Some(words) = self.read_words_at(self.cpu.regs[2], 2) {
            consider([words[0], words[1]], "PTR@R2");
        }
        consider([self.vfp_get_s(0), self.vfp_get_s(1)], "S0/S1");
        if allow_stack {
            if let (Some(a), Some(b)) = (self.peek_stack_u32(0), self.peek_stack_u32(1)) {
                consider([a, b], "STACK[0..1]");
            }
        }

        best.map(|(bits, source, _)| (bits, source))
    }

    fn read_msgsend_rect_arg(&self) -> Option<([u32; 4], String)> {
        let mut best: Option<([u32; 4], String, i32)> = None;
        let mut consider = |bits: [u32; 4], source: &str| {
            if Self::contains_trampoline_stub_words(&bits) {
                return;
            }
            let score = Self::score_ui_rect((
                Self::f32_from_bits(bits[0]),
                Self::f32_from_bits(bits[1]),
                Self::f32_from_bits(bits[2]),
                Self::f32_from_bits(bits[3]),
            ));
            if score <= -999_999 {
                return;
            }
            let replace = best.as_ref().map(|(_, _, prev)| score > *prev).unwrap_or(true);
            if replace {
                best = Some((bits, source.to_string(), score));
            }
        };

        if let (Some(c), Some(d)) = (self.peek_stack_u32(0), self.peek_stack_u32(1)) {
            consider([self.cpu.regs[2], self.cpu.regs[3], c, d], "R2/R3+STACK");
        }
        if let (Some(a), Some(b), Some(c), Some(d)) = (
            self.peek_stack_u32(0),
            self.peek_stack_u32(1),
            self.peek_stack_u32(2),
            self.peek_stack_u32(3),
        ) {
            consider([a, b, c, d], "STACK[0..3]");
        }
        if let Some(words) = self.read_words_at(self.cpu.regs[2], 4) {
            consider([words[0], words[1], words[2], words[3]], "PTR@R2");
        }
        consider([self.vfp_get_s(0), self.vfp_get_s(1), self.vfp_get_s(2), self.vfp_get_s(3)], "S0..S3");

        best.map(|(bits, source, _)| (bits, source))
    }

    fn read_msgsend_rect_after_object_arg(&self) -> Option<([u32; 4], String)> {
        let mut best: Option<([u32; 4], String, i32)> = None;
        let mut consider = |bits: [u32; 4], source: &str| {
            if Self::contains_trampoline_stub_words(&bits) {
                return;
            }
            let score = Self::score_ui_rect((
                Self::f32_from_bits(bits[0]),
                Self::f32_from_bits(bits[1]),
                Self::f32_from_bits(bits[2]),
                Self::f32_from_bits(bits[3]),
            ));
            if score <= -999_999 {
                return;
            }
            let replace = best.as_ref().map(|(_, _, prev)| score > *prev).unwrap_or(true);
            if replace {
                best = Some((bits, source.to_string(), score));
            }
        };

        if let (Some(b), Some(c), Some(d)) = (self.peek_stack_u32(0), self.peek_stack_u32(1), self.peek_stack_u32(2)) {
            consider([self.cpu.regs[3], b, c, d], "R3+STACK");
        }
        if let (Some(a), Some(b), Some(c), Some(d)) = (
            self.peek_stack_u32(0),
            self.peek_stack_u32(1),
            self.peek_stack_u32(2),
            self.peek_stack_u32(3),
        ) {
            consider([a, b, c, d], "STACK[0..3]");
        }
        if let Some(words) = self.read_words_at(self.cpu.regs[3], 4) {
            consider([words[0], words[1], words[2], words[3]], "PTR@R3");
        }
        consider([self.vfp_get_s(0), self.vfp_get_s(1), self.vfp_get_s(2), self.vfp_get_s(3)], "S0..S3");

        best.map(|(bits, source, _)| (bits, source))
    }


    fn ui_authoritative_surface_size(&self) -> (u32, u32, &'static str) {
        let viewport_w = self.runtime.ui_graphics.graphics_viewport_width;
        let viewport_h = self.runtime.ui_graphics.graphics_viewport_height;
        if self.runtime.ui_graphics.graphics_viewport_ready && viewport_w != 0 && viewport_h != 0 {
            return (viewport_w.max(1), viewport_h.max(1), "viewport");
        }
        let surface_w = self.runtime.ui_graphics.graphics_surface_width;
        let surface_h = self.runtime.ui_graphics.graphics_surface_height;
        if surface_w != 0 && surface_h != 0 {
            return (surface_w.max(1), surface_h.max(1), "surface");
        }
        (320, 480, "default")
    }

    fn ui_normalize_surface_size_candidate(&self, width: u32, height: u32) -> (u32, u32, String) {
        let width = width.max(1);
        let height = height.max(1);
        let viewport_w = self.runtime.ui_graphics.graphics_viewport_width.max(1);
        let viewport_h = self.runtime.ui_graphics.graphics_viewport_height.max(1);
        if self.runtime.ui_graphics.graphics_viewport_ready
            && viewport_w != 0
            && viewport_h != 0
            && (width != viewport_w || height != viewport_h)
        {
            let swapped = width == viewport_h && height == viewport_w;
            let same_area = width.saturating_mul(height) == viewport_w.saturating_mul(viewport_h);
            if swapped || same_area {
                return (
                    viewport_w,
                    viewport_h,
                    format!(
                        "viewport-authority candidate={}x{} viewport={}x{}{}",
                        width,
                        height,
                        viewport_w,
                        viewport_h,
                        if swapped { " swapped" } else { " same-area" },
                    ),
                );
            }
        }
        (width, height, format!("drawable candidate={}x{}", width, height))
    }

    fn ui_surface_rect_bits(&self) -> [u32; 4] {
        let (width, height, _) = self.ui_authoritative_surface_size();
        [
            0.0f32.to_bits(),
            0.0f32.to_bits(),
            (width as f32).to_bits(),
            (height as f32).to_bits(),
        ]
    }

    fn ui_rect_bits_to_string(bits: [u32; 4]) -> String {
        format!(
            "CGRect({:.3},{:.3} {:.3}x{:.3})",
            Self::f32_from_bits(bits[0]),
            Self::f32_from_bits(bits[1]),
            Self::f32_from_bits(bits[2]),
            Self::f32_from_bits(bits[3]),
        )
    }

    fn ui_content_scale_value_from_bits(bits: u32) -> f32 {
        let value = Self::f32_from_bits(bits);
        if value.is_finite() && value > 0.0 && value <= 16.0 {
            value
        } else {
            1.0
        }
    }

    fn ui_rect_size_bits(bits: [u32; 4]) -> [u32; 4] {
        [0.0f32.to_bits(), 0.0f32.to_bits(), bits[2], bits[3]]
    }

    fn ui_object_is_view_like(&self, object: u32) -> bool {
        object != 0 && (
            object == self.runtime.ui_objects.window
                || object == self.runtime.ui_objects.screen
                || object == self.runtime.ui_cocos.opengl_view
                || self.objc_receiver_inherits_named(object, "UIView")
                || self.objc_receiver_inherits_named(object, "UIWindow")
                || self.objc_receiver_inherits_named(object, "UIScreen")
        )
    }

    fn ui_object_looks_like_gl_view(&self, object: u32) -> bool {
        if object == 0 {
            return false;
        }
        let class_hint = self
            .objc_receiver_class_name_hint(object)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let label_hint = self
            .diag
            .object_labels
            .get(&object)
            .cloned()
            .unwrap_or_default()
            .to_ascii_lowercase();
        object == self.runtime.ui_cocos.opengl_view
            || class_hint.contains("eagl")
            || class_hint.contains("glview")
            || label_hint.contains("eaglview")
            || label_hint.contains("glview")
    }

    fn ui_adopt_cocos_opengl_view(&mut self, view: u32, reason: &str) {
        if view == 0 || !self.ui_object_looks_like_gl_view(view) {
            return;
        }

        let previous = self.runtime.ui_cocos.opengl_view;
        if previous != 0 && previous != view {
            if let Some(parent) = self.runtime.ui_objects.view_superviews.remove(&previous) {
                self.runtime.ui_objects.view_superviews.insert(view, parent);
                let children = self.runtime.ui_objects.view_subviews.entry(parent).or_default();
                let mut replaced = false;
                for child in children.iter_mut() {
                    if *child == previous {
                        *child = view;
                        replaced = true;
                    }
                }
                if !replaced && !children.contains(&view) {
                    children.push(view);
                }
            }

            if let Some(children) = self.runtime.ui_objects.view_subviews.remove(&previous) {
                for child in children.iter().copied() {
                    self.runtime.ui_objects.view_superviews.insert(child, view);
                }
                self.runtime.ui_objects.view_subviews.insert(view, children);
            }

            if let Some(frame_bits) = self.runtime.ui_objects.view_frames_bits.remove(&previous) {
                self.runtime.ui_objects.view_frames_bits.entry(view).or_insert(frame_bits);
            }
            if let Some(bounds_bits) = self.runtime.ui_objects.view_bounds_bits.remove(&previous) {
                self.runtime.ui_objects.view_bounds_bits.entry(view).or_insert(bounds_bits);
            }
            if let Some(scale_bits) = self.runtime.ui_objects.view_content_scale_bits.remove(&previous) {
                self.runtime.ui_objects.view_content_scale_bits.entry(view).or_insert(scale_bits);
            }
            if let Some(layer) = self.runtime.ui_objects.view_layers.remove(&previous) {
                self.runtime.ui_objects.view_layers.insert(view, layer);
                self.runtime.ui_objects.layer_host_views.insert(layer, view);
            }
        }

        self.runtime.ui_cocos.opengl_view = view;
        self.diag
            .object_labels
            .entry(view)
            .or_insert_with(|| "EAGLView.instance(guest)".to_string());

        if self.runtime.ui_objects.window != 0
            && self.runtime.ui_objects.view_superviews.get(&view).copied().unwrap_or(0) == 0
        {
            self.runtime
                .ui_objects
                .view_superviews
                .insert(view, self.runtime.ui_objects.window);
            let children = self
                .runtime
                .ui_objects
                .view_subviews
                .entry(self.runtime.ui_objects.window)
                .or_default();
            if !children.contains(&view) {
                children.push(view);
            }
        }

        self.ui_set_frame_bits(view, self.ui_surface_rect_bits());
        self.ui_set_bounds_bits(view, Self::ui_rect_size_bits(self.ui_surface_rect_bits()));
        self.ui_set_content_scale_bits(view, 1.0f32.to_bits());
        self.ui_attach_layer_to_view(view, self.runtime.ui_graphics.eagl_layer);

        if self.runtime.ui_objects.first_responder == 0
            || self.runtime.ui_objects.first_responder == previous
            || self.runtime.ui_objects.first_responder == self.runtime.ui_objects.root_controller
        {
            self.runtime.ui_objects.first_responder = view;
        }

        self.diag.trace.push(format!(
            "     ↳ ui adopt openglView reason={} view={} previous={} window={} firstResponder={}",
            reason,
            self.describe_ptr(view),
            self.describe_ptr(previous),
            self.describe_ptr(self.runtime.ui_objects.window),
            self.describe_ptr(self.runtime.ui_objects.first_responder),
        ));
    }

    fn ui_view_contains_window_point(&self, view: u32, x: f32, y: f32) -> bool {
        if view == 0 || !self.ui_object_is_view_like(view) {
            return false;
        }
        let frame_bits = self.ui_frame_bits_for_object(view);
        let bounds_bits = self.ui_bounds_bits_for_object(view);
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
        if !left.is_finite() || !top.is_finite() || !width.is_finite() || !height.is_finite() {
            return false;
        }
        x >= left && y >= top && x <= (left + width) && y <= (top + height)
    }

    fn ui_frame_rect_f32(&self, object: u32) -> (f32, f32, f32, f32) {
        let frame_bits = self.ui_frame_bits_for_object(object);
        let bounds_bits = self.ui_bounds_bits_for_object(object);
        let x = Self::f32_from_bits(frame_bits[0]);
        let y = Self::f32_from_bits(frame_bits[1]);
        let mut w = Self::f32_from_bits(frame_bits[2]);
        let mut h = Self::f32_from_bits(frame_bits[3]);
        if !Self::plausible_ui_size(w) {
            w = Self::f32_from_bits(bounds_bits[2]);
        }
        if !Self::plausible_ui_size(h) {
            h = Self::f32_from_bits(bounds_bits[3]);
        }
        (
            if x.is_finite() { x } else { 0.0 },
            if y.is_finite() { y } else { 0.0 },
            if Self::plausible_ui_size(w) { w } else { 0.0 },
            if Self::plausible_ui_size(h) { h } else { 0.0 },
        )
    }

    fn ui_bounds_rect_f32(&self, object: u32) -> (f32, f32, f32, f32) {
        let frame_bits = self.ui_frame_bits_for_object(object);
        let bounds_bits = self.ui_bounds_bits_for_object(object);
        let x = Self::f32_from_bits(bounds_bits[0]);
        let y = Self::f32_from_bits(bounds_bits[1]);
        let mut w = Self::f32_from_bits(bounds_bits[2]);
        let mut h = Self::f32_from_bits(bounds_bits[3]);
        if !Self::plausible_ui_size(w) {
            w = Self::f32_from_bits(frame_bits[2]);
        }
        if !Self::plausible_ui_size(h) {
            h = Self::f32_from_bits(frame_bits[3]);
        }
        (
            if x.is_finite() { x } else { 0.0 },
            if y.is_finite() { y } else { 0.0 },
            if Self::plausible_ui_size(w) { w } else { 0.0 },
            if Self::plausible_ui_size(h) { h } else { 0.0 },
        )
    }

    fn ui_convert_window_point_to_view_local(&self, view: u32, x: f32, y: f32) -> (f32, f32) {
        let (frame_x, frame_y, _, _) = self.ui_frame_rect_f32(view);
        let (bounds_x, bounds_y, _, _) = self.ui_bounds_rect_f32(view);
        (x - frame_x + bounds_x, y - frame_y + bounds_y)
    }

    fn ui_convert_local_point_to_subview_local(&self, subview: u32, x: f32, y: f32) -> (f32, f32) {
        let (frame_x, frame_y, _, _) = self.ui_frame_rect_f32(subview);
        let (bounds_x, bounds_y, _, _) = self.ui_bounds_rect_f32(subview);
        (x - frame_x + bounds_x, y - frame_y + bounds_y)
    }

    fn ui_view_contains_local_point(&self, view: u32, x: f32, y: f32) -> bool {
        if view == 0 || !self.ui_object_is_view_like(view) {
            return false;
        }
        let (bounds_x, bounds_y, width, height) = self.ui_bounds_rect_f32(view);
        if !bounds_x.is_finite() || !bounds_y.is_finite() || !width.is_finite() || !height.is_finite() {
            return false;
        }
        if width <= 0.0 || height <= 0.0 {
            return false;
        }
        x >= bounds_x && y >= bounds_y && x <= (bounds_x + width) && y <= (bounds_y + height)
    }

    fn ui_hit_test_view_subtree_local(&self, view: u32, x: f32, y: f32) -> Option<u32> {
        if !self.ui_view_contains_local_point(view, x, y) {
            return None;
        }
        if let Some(children) = self.runtime.ui_objects.view_subviews.get(&view) {
            for child in children.iter().rev().copied() {
                let (child_x, child_y) = self.ui_convert_local_point_to_subview_local(child, x, y);
                if let Some(hit) = self.ui_hit_test_view_subtree_local(child, child_x, child_y) {
                    return Some(hit);
                }
            }
        }
        Some(view)
    }

    fn ui_hit_test_view_subtree(&self, view: u32, x: f32, y: f32) -> Option<u32> {
        self.ui_hit_test_view_subtree_local(view, x, y)
    }

    fn ui_hit_test_window_point(&self, window: u32, x: f32, y: f32) -> Option<u32> {
        if window == 0 {
            return None;
        }
        let (local_x, local_y) = self.ui_convert_window_point_to_view_local(window, x, y);
        self.ui_hit_test_view_subtree_local(window, local_x, local_y)
    }

    fn ui_next_responder(&self, object: u32) -> u32 {
        if object == 0 {
            return 0;
        }
        if object == self.runtime.ui_objects.app {
            return self.runtime.ui_objects.delegate;
        }
        if object == self.runtime.ui_objects.window {
            if self.runtime.ui_objects.root_controller != 0 {
                return self.runtime.ui_objects.root_controller;
            }
            return self.runtime.ui_objects.app;
        }
        if let Some(parent) = self.runtime.ui_objects.view_superviews.get(&object).copied() {
            if parent != 0 && parent != object {
                return parent;
            }
        }
        if self.ui_object_is_view_like(object) {
            if self.runtime.ui_objects.window != 0 && object != self.runtime.ui_objects.window {
                return self.runtime.ui_objects.window;
            }
            if self.runtime.ui_objects.root_controller != 0 && object != self.runtime.ui_objects.root_controller {
                return self.runtime.ui_objects.root_controller;
            }
            return self.runtime.ui_objects.app;
        }
        if object == self.runtime.ui_objects.root_controller {
            if self.runtime.ui_objects.window != 0 {
                return self.runtime.ui_objects.window;
            }
            return self.runtime.ui_objects.app;
        }
        0
    }

    fn ui_object_is_layer_like(&self, object: u32) -> bool {
        object != 0 && (
            object == self.runtime.ui_graphics.eagl_layer
                || self.objc_receiver_inherits_named(object, "CALayer")
                || self.objc_receiver_inherits_named(object, "CAEAGLLayer")
        )
    }

    fn ui_attach_layer_to_view(&mut self, view: u32, layer: u32) {
        if view == 0 || layer == 0 {
            return;
        }
        self.runtime.ui_objects.view_layers.insert(view, layer);
        self.runtime.ui_objects.layer_host_views.insert(layer, view);
        let frame_bits = self.ui_frame_bits_for_object(view);
        let bounds_bits = self.ui_bounds_bits_for_object(view);
        self.runtime.ui_objects.view_frames_bits.entry(layer).or_insert(frame_bits);
        self.runtime.ui_objects.view_bounds_bits.entry(layer).or_insert(bounds_bits);
    }

    fn ui_frame_bits_for_object(&self, object: u32) -> [u32; 4] {
        if let Some(bits) = self.runtime.ui_objects.view_frames_bits.get(&object).copied() {
            return bits;
        }
        if self.ui_object_is_layer_like(object) {
            if let Some(view) = self.runtime.ui_objects.layer_host_views.get(&object).copied() {
                if let Some(bits) = self.runtime.ui_objects.view_frames_bits.get(&view).copied() {
                    return bits;
                }
            }
        }
        self.ui_surface_rect_bits()
    }

    fn ui_bounds_bits_for_object(&self, object: u32) -> [u32; 4] {
        if let Some(bits) = self.runtime.ui_objects.view_bounds_bits.get(&object).copied() {
            return bits;
        }
        if self.ui_object_is_layer_like(object) {
            if let Some(view) = self.runtime.ui_objects.layer_host_views.get(&object).copied() {
                if let Some(bits) = self.runtime.ui_objects.view_bounds_bits.get(&view).copied() {
                    return bits;
                }
                if let Some(bits) = self.runtime.ui_objects.view_frames_bits.get(&view).copied() {
                    return Self::ui_rect_size_bits(bits);
                }
            }
        }
        if let Some(bits) = self.runtime.ui_objects.view_frames_bits.get(&object).copied() {
            return Self::ui_rect_size_bits(bits);
        }
        Self::ui_rect_size_bits(self.ui_surface_rect_bits())
    }

    fn ui_rect_bits_for_selector(&self, object: u32, selector: &str) -> [u32; 4] {
        match selector {
            "bounds" | "applicationFrame" => self.ui_bounds_bits_for_object(object),
            _ => self.ui_frame_bits_for_object(object),
        }
    }

    fn ui_set_frame_bits(&mut self, object: u32, bits: [u32; 4]) {
        if object == 0 {
            return;
        }
        self.runtime.ui_objects.view_frames_bits.insert(object, bits);
        self.runtime.ui_objects
            .view_bounds_bits
            .entry(object)
            .or_insert_with(|| Self::ui_rect_size_bits(bits));
        if self.ui_object_is_view_like(object) {
            if let Some(layer) = self.runtime.ui_objects.view_layers.get(&object).copied() {
                self.runtime.ui_objects.view_frames_bits.insert(layer, bits);
                self.runtime.ui_objects.view_bounds_bits.insert(layer, Self::ui_rect_size_bits(bits));
            }
        }
    }

    fn ui_set_bounds_bits(&mut self, object: u32, bits: [u32; 4]) {
        if object == 0 {
            return;
        }
        self.runtime.ui_objects.view_bounds_bits.insert(object, bits);
        if self.ui_object_is_view_like(object) {
            if let Some(layer) = self.runtime.ui_objects.view_layers.get(&object).copied() {
                self.runtime.ui_objects.view_bounds_bits.insert(layer, bits);
            }
        }
    }

    fn ui_content_scale_bits_for_object(&self, object: u32) -> u32 {
        if let Some(bits) = self.runtime.ui_objects.view_content_scale_bits.get(&object).copied() {
            return bits;
        }
        if self.ui_object_is_layer_like(object) {
            if let Some(view) = self.runtime.ui_objects.layer_host_views.get(&object).copied() {
                if let Some(bits) = self.runtime.ui_objects.view_content_scale_bits.get(&view).copied() {
                    return bits;
                }
            }
        }
        1.0f32.to_bits()
    }

    fn ui_set_content_scale_bits(&mut self, object: u32, bits: u32) {
        if object == 0 {
            return;
        }
        let normalized = Self::ui_content_scale_value_from_bits(bits).to_bits();
        self.runtime.ui_objects.view_content_scale_bits.insert(object, normalized);
        if self.ui_object_is_view_like(object) {
            if let Some(layer) = self.runtime.ui_objects.view_layers.get(&object).copied() {
                self.runtime.ui_objects.view_content_scale_bits.insert(layer, normalized);
            }
        }
    }

    fn ui_resolve_drawable_surface_size(&self, drawable: u32) -> (u32, u32, f32, String) {
        let rect_bits = self.ui_bounds_bits_for_object(drawable);
        let rect_w = Self::f32_from_bits(rect_bits[2]);
        let rect_h = Self::f32_from_bits(rect_bits[3]);
        let (authoritative_w, authoritative_h, _) = self.ui_authoritative_surface_size();
        let base_w = if Self::plausible_ui_size(rect_w) {
            rect_w.round().max(1.0) as u32
        } else {
            authoritative_w
        };
        let base_h = if Self::plausible_ui_size(rect_h) {
            rect_h.round().max(1.0) as u32
        } else {
            authoritative_h
        };
        let scale_bits = self.ui_content_scale_bits_for_object(drawable);
        let scale = Self::ui_content_scale_value_from_bits(scale_bits);
        let scaled_w = ((base_w as f32) * scale).round().max(1.0) as u32;
        let scaled_h = ((base_h as f32) * scale).round().max(1.0) as u32;
        (
            scaled_w,
            scaled_h,
            scale,
            format!(
                "drawable={} bounds={} scale={:.3}",
                self.describe_ptr(drawable),
                Self::ui_rect_bits_to_string(rect_bits),
                scale,
            ),
        )
    }

    fn ui_refresh_surface_from_drawable(&mut self, drawable: u32, origin: &str) -> Option<String> {
        if drawable == 0 {
            return None;
        }
        let (raw_width, raw_height, scale, source) = self.ui_resolve_drawable_surface_size(drawable);
        let (width, height, normalization) = self.ui_normalize_surface_size_candidate(raw_width, raw_height);
        let changed = self.runtime.ui_graphics.graphics_surface_width != width
            || self.runtime.ui_graphics.graphics_surface_height != height;
        self.runtime.ui_graphics.graphics_surface_width = width;
        self.runtime.ui_graphics.graphics_surface_height = height;
        if self.runtime.ui_graphics.graphics_viewport_width == 0 || self.runtime.ui_graphics.graphics_viewport_height == 0 {
            self.runtime.ui_graphics.graphics_viewport_x = 0;
            self.runtime.ui_graphics.graphics_viewport_y = 0;
            self.runtime.ui_graphics.graphics_viewport_width = width;
            self.runtime.ui_graphics.graphics_viewport_height = height;
        }
        Some(format!(
            "{} surface={}x{} scale={:.3} source=[{} | {}] changed={}",
            origin,
            width,
            height,
            scale,
            source,
            normalization,
            if changed { "YES" } else { "NO" },
        ))
    }

    fn write_cg_point_to_guest_bits(&mut self, addr: u32, bits: [u32; 2]) -> CoreResult<()> {
        self.write_u32_le(addr, bits[0])?;
        self.write_u32_le(addr.wrapping_add(4), bits[1])?;
        Ok(())
    }

    fn write_cg_size_to_guest(&mut self, addr: u32, width: u32, height: u32) -> CoreResult<()> {
        self.write_u32_le(addr, (width as f32).to_bits())?;
        self.write_u32_le(addr.wrapping_add(4), (height as f32).to_bits())?;
        Ok(())
    }

    fn write_cg_rect_to_guest(&mut self, addr: u32, x: i32, y: i32, width: u32, height: u32) -> CoreResult<()> {
        self.write_u32_le(addr, (x as f32).to_bits())?;
        self.write_u32_le(addr.wrapping_add(4), (y as f32).to_bits())?;
        self.write_u32_le(addr.wrapping_add(8), (width as f32).to_bits())?;
        self.write_u32_le(addr.wrapping_add(12), (height as f32).to_bits())?;
        Ok(())
    }


    fn write_cg_rect_bits_to_guest(&mut self, addr: u32, bits: [u32; 4]) -> CoreResult<()> {
        self.write_u32_le(addr, bits[0])?;
        self.write_u32_le(addr.wrapping_add(4), bits[1])?;
        self.write_u32_le(addr.wrapping_add(8), bits[2])?;
        self.write_u32_le(addr.wrapping_add(12), bits[3])?;
        Ok(())
    }


    fn push_recent_entry(list: &mut Vec<String>, value: String, limit: usize) {
        list.push(value);
        if list.len() > limit {
            let overflow = list.len() - limit;
            list.drain(0..overflow);
        }
    }

    fn bump_count(map: &mut HashMap<String, u32>, key: &str) {
        let entry = map.entry(key.to_string()).or_insert(0);
        *entry = entry.saturating_add(1);
    }

    fn top_count_entries(map: &HashMap<String, u32>, limit: usize) -> Vec<RuntimeCountEntry> {
        let mut items: Vec<RuntimeCountEntry> = map
            .iter()
            .map(|(name, count)| RuntimeCountEntry {
                name: name.clone(),
                count: *count,
            })
            .collect();
        items.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        items.truncate(limit);
        items
    }

    fn record_objc_selector(&mut self, selector: &str, entry: String) {
        self.runtime.objc.objc_msgsend_calls = self.runtime.objc.objc_msgsend_calls.saturating_add(1);
        Self::bump_count(&mut self.runtime.objc.objc_selector_counts, selector);
        Self::push_recent_entry(&mut self.runtime.objc.recent_objc_selectors, entry, 24);
    }

    fn record_gl_call(&mut self, label: &str, entry: String) -> u32 {
        Self::bump_count(&mut self.runtime.graphics.gl_call_counts, label);
        let count = self.runtime.graphics.gl_call_counts.get(label).copied().unwrap_or(0);
        Self::push_recent_entry(&mut self.runtime.graphics.recent_gl_calls, entry, 24);
        count
    }

    fn collect_reachable_synthetic_nodes(
        &self,
        node: u32,
        out: &mut Vec<u32>,
        seen: &mut HashSet<u32>,
        depth: usize,
    ) {
        if node == 0 || depth >= 64 || !seen.insert(node) {
            return;
        }
        out.push(node);
        let children = self.runtime.graphics.synthetic_sprites.get(&node).map(|state| state.children).unwrap_or(0);
        if children == 0 {
            return;
        }
        if let Some(arr) = self.runtime.graphics.synthetic_arrays.get(&children) {
            for child in &arr.items {
                if *child != 0 {
                    self.collect_reachable_synthetic_nodes(*child, out, seen, depth + 1);
                }
            }
        }
    }

    fn runtime_ui_tree_label_like(label: &str) -> bool {
        let lower = label.to_ascii_lowercase();
        lower.contains("label") || lower.contains("font") || lower.contains("text")
    }

    fn runtime_ui_tree_trim_text(text: &str, limit: usize) -> String {
        let mut out = String::new();
        let mut count = 0usize;
        for ch in text.chars() {
            if count >= limit {
                out.push('…');
                break;
            }
            out.push(ch);
            count += 1;
        }
        out
    }

    fn runtime_ui_tree_sample_score(&self, node: u32) -> u32 {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return 0;
        };
        let label = self.diag.object_labels.get(&node).map(String::as_str).unwrap_or("");
        let has_text = self.string_backing(node).is_some();
        let is_label = Self::runtime_ui_tree_label_like(label) || has_text;
        let is_menu_item = label.contains("CCMenuItem");
        let is_menu = label.contains("CCMenu") && !is_menu_item;
        let mut score = 0u32;
        if state.visible { score = score.saturating_add(200); }
        if state.touch_enabled { score = score.saturating_add(220); }
        if state.callback_selector != 0 { score = score.saturating_add(180); }
        let effective_texture = self.synthetic_node_effective_texture(node);
        if effective_texture != 0 { score = score.saturating_add(120); }
        if effective_texture == 0 && state.visible { score = score.saturating_add(260); }
        if is_menu_item { score = score.saturating_add(320); }
        if is_menu { score = score.saturating_add(140); }
        if is_label { score = score.saturating_add(360); }
        if state.entered { score = score.saturating_add(60); }
        if state.children != 0 { score = score.saturating_add(30); }
        score
    }

    fn runtime_ui_tree_node_sample(&self, node: u32) -> RuntimeUiTreeNodeSample {
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let state = self.runtime.graphics.synthetic_sprites.get(&node).cloned().unwrap_or_default();
        let effective_texture = self.synthetic_node_effective_texture(node);
        let raw_width = state.width;
        let raw_height = state.height;
        let (draw_eligible, draw_reason, _draw_texture, resolved_w, resolved_h) = self.synthetic_node_draw_debug(node);
        let (width, height) = if resolved_w != 0 || resolved_h != 0 {
            (resolved_w, resolved_h)
        } else if state.width != 0 || state.height != 0 {
            (state.width, state.height)
        } else if effective_texture != 0 {
            self.synthetic_texture_dimensions(effective_texture).unwrap_or((0, 0))
        } else {
            (0, 0)
        };
        let texture_key = if effective_texture != 0 {
            self.synthetic_texture_debug_key(effective_texture)
        } else {
            None
        };
        let callback_selector = if state.callback_selector != 0 {
            self.objc_read_selector_name(state.callback_selector)
        } else {
            None
        };
        let text_backing = self
            .string_backing(node)
            .map(|backing| Self::runtime_ui_tree_trim_text(&backing.text.replace('\n', "\\n"), 48));
        let (world_x, world_y) = self.compute_synthetic_node_world_position(node);
        let default_anchor = Self::synthetic_default_anchor(&label);
        RuntimeUiTreeNodeSample {
            ptr: node,
            label,
            parent: state.parent,
            parent_label: (state.parent != 0)
                .then(|| self.diag.object_labels.get(&state.parent).cloned().unwrap_or_default())
                .filter(|value| !value.is_empty()),
            child_count: if state.children != 0 {
                self.synthetic_array_len(state.children) as u32
            } else {
                0
            },
            raw_texture: state.texture,
            texture: effective_texture,
            texture_key,
            position_x: Self::f32_from_bits(state.position_x_bits),
            position_y: Self::f32_from_bits(state.position_y_bits),
            world_x,
            world_y,
            raw_width,
            raw_height,
            width,
            height,
            anchor_x: if state.anchor_explicit {
                Self::f32_from_bits(state.anchor_x_bits)
            } else {
                default_anchor
            },
            anchor_y: if state.anchor_explicit {
                Self::f32_from_bits(state.anchor_y_bits)
            } else {
                default_anchor
            },
            anchor_explicit: state.anchor_explicit,
            visible: state.visible,
            entered: state.entered,
            touch_enabled: state.touch_enabled,
            draw_eligible,
            draw_reason,
            texture_rect_explicit: state.texture_rect_explicit,
            texture_rect_x: Self::f32_from_bits(state.texture_rect_x_bits),
            texture_rect_y: Self::f32_from_bits(state.texture_rect_y_bits),
            texture_rect_w: Self::f32_from_bits(state.texture_rect_w_bits),
            texture_rect_h: Self::f32_from_bits(state.texture_rect_h_bits),
            callback_selector,
            text_backing,
        }
    }

    fn runtime_ui_tree_summary(&self) -> RuntimeUiTreeSummary {
        let running_scene = self.runtime.ui_cocos.running_scene;
        let mut reachable = Vec::new();
        let mut seen = HashSet::new();
        if running_scene != 0 {
            self.collect_reachable_synthetic_nodes(running_scene, &mut reachable, &mut seen, 0);
        }

        let mut summary = RuntimeUiTreeSummary {
            running_scene,
            total_nodes: self.runtime.graphics.synthetic_sprites.len() as u32,
            reachable_nodes: reachable.len() as u32,
            detached_nodes: 0,
            visible_nodes: 0,
            entered_nodes: 0,
            textured_nodes: 0,
            visible_textureless_nodes: 0,
            menu_nodes: 0,
            menu_item_nodes: 0,
            label_nodes: 0,
            touch_enabled_nodes: 0,
            callback_nodes: 0,
            sampled_nodes: Vec::new(),
            detached_sampled_nodes: Vec::new(),
        };

        for node in &reachable {
            let Some(state) = self.runtime.graphics.synthetic_sprites.get(node) else {
                continue;
            };
            let label = self.diag.object_labels.get(node).map(String::as_str).unwrap_or("");
            let is_menu_item = label.contains("CCMenuItem");
            let is_menu = label.contains("CCMenu") && !is_menu_item;
            let is_label = Self::runtime_ui_tree_label_like(label) || self.string_backing(*node).is_some();
            if state.visible {
                summary.visible_nodes = summary.visible_nodes.saturating_add(1);
            }
            if state.entered {
                summary.entered_nodes = summary.entered_nodes.saturating_add(1);
            }
            let effective_texture = self.synthetic_node_effective_texture(*node);
            if effective_texture != 0 {
                summary.textured_nodes = summary.textured_nodes.saturating_add(1);
            }
            if state.visible && effective_texture == 0 && !label.contains("CCScene") && !label.contains("CCLayer") {
                summary.visible_textureless_nodes = summary.visible_textureless_nodes.saturating_add(1);
            }
            if is_menu {
                summary.menu_nodes = summary.menu_nodes.saturating_add(1);
            }
            if is_menu_item {
                summary.menu_item_nodes = summary.menu_item_nodes.saturating_add(1);
            }
            if is_label {
                summary.label_nodes = summary.label_nodes.saturating_add(1);
            }
            if state.touch_enabled {
                summary.touch_enabled_nodes = summary.touch_enabled_nodes.saturating_add(1);
            }
            if state.callback_selector != 0 {
                summary.callback_nodes = summary.callback_nodes.saturating_add(1);
            }
        }

        let mut scored: Vec<(u32, u32)> = reachable
            .iter()
            .copied()
            .map(|node| (self.runtime_ui_tree_sample_score(node), node))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (_, node) in scored.into_iter().take(24) {
            summary.sampled_nodes.push(self.runtime_ui_tree_node_sample(node));
        }

        let mut detached: Vec<u32> = self.runtime.graphics.synthetic_sprites
            .keys()
            .copied()
            .filter(|node| !seen.contains(node))
            .collect();
        summary.detached_nodes = detached.len() as u32;
        detached.sort_by_key(|node| *node);
        let mut detached_scored: Vec<(u32, u32)> = detached
            .into_iter()
            .map(|node| (self.runtime_ui_tree_sample_score(node), node))
            .collect();
        detached_scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (_, node) in detached_scored.into_iter().take(24) {
            summary.detached_sampled_nodes.push(self.runtime_ui_tree_node_sample(node));
        }

        summary
    }

    pub(crate) fn runtime_state_snapshot(&self) -> RuntimeStateReport {
        let has_error = self.runtime.ui_network.network_cancelled || self.runtime.ui_network.network_faulted || self.runtime.ui_network.network_timeout_armed;
        RuntimeStateReport {
            synthetic: RuntimeSyntheticConfigReport {
                runtime_mode: self.tuning.runtime_mode.to_string(),
                execution_backend: self.tuning.execution_backend.to_string(),
                network_fault_probes: self.tuning.synthetic_network_fault_probes,
                runloop_tick_budget: self.tuning.synthetic_runloop_ticks,
                menu_probe_selector: self.tuning.synthetic_menu_probe_selector.clone(),
                menu_probe_after_ticks: self.tuning.synthetic_menu_probe_after_ticks,
                menu_probe_attempts: self.runtime.scene.synthetic_menu_probe_attempts,
                menu_probe_fired: self.runtime.scene.synthetic_menu_probe_fired,
            },
            ui: RuntimeUiSummary {
                launched: self.runtime.ui_runtime.launched,
                delegate_set: self.runtime.ui_runtime.delegate_set,
                window_visible: self.runtime.ui_runtime.window_visible,
                app_active: self.runtime.ui_runtime.app_active,
                runloop_live: self.runtime.ui_runtime.runloop_live,
                exit_suppressed: self.runtime.ui_runtime.exit_suppressed,
                launch_count: self.runtime.ui_runtime.launch_count,
                activation_count: self.runtime.ui_runtime.activation_count,
                cocos_director: (self.runtime.ui_cocos.cocos_director != 0).then_some(self.runtime.ui_cocos.cocos_director),
                opengl_view: (self.runtime.ui_cocos.opengl_view != 0).then_some(self.runtime.ui_cocos.opengl_view),
                running_scene: (self.runtime.ui_cocos.running_scene != 0).then_some(self.runtime.ui_cocos.running_scene),
                director_type: self.runtime.ui_cocos.director_type,
                animation_running: self.runtime.ui_cocos.animation_running,
                display_fps_enabled: self.runtime.ui_cocos.display_fps_enabled,
            },
            runloop: RuntimeRunloopSummary {
                ticks: self.runtime.ui_runtime.runloop_ticks,
                sources: self.current_runloop_source_count(),
                last_tick_sources_before: self.runtime.ui_runtime.last_tick_sources_before,
                last_tick_sources_after: self.runtime.ui_runtime.last_tick_sources_after,
                idle_ticks_after_completion: self.runtime.ui_runtime.idle_ticks_after_completion,
            },
            input: RuntimeInputSummary {
                queued: self.runtime.host_input.queue.len() as u32,
                consumed: self.runtime.host_input.events_consumed,
                ignored: self.runtime.host_input.events_ignored,
                active_pointer_id: self.runtime.host_input.active_touch.as_ref().map(|touch| touch.pointer_id),
                pointer_down: self.runtime.host_input.active_touch.is_some(),
                ui_attempts: self.runtime.host_input.ui_attempts,
                ui_dispatched: self.runtime.host_input.ui_dispatched,
                cocos_attempts: self.runtime.host_input.cocos_attempts,
                cocos_dispatched: self.runtime.host_input.cocos_dispatched,
                last_phase: self.runtime.host_input.last_phase.clone(),
                last_target: self.runtime.host_input.last_target,
                last_x: self.runtime.host_input.last_x,
                last_y: self.runtime.host_input.last_y,
                last_dispatch: self.runtime.host_input.last_dispatch.clone(),
                last_source: self.runtime.host_input.last_source.clone(),
            },
            network: RuntimeNetworkSummary {
                url: self.network_url_string().to_string(),
                host: self.network_host_string().to_string(),
                path: self.network_path_string().to_string(),
                method: self.network_http_method().to_string(),
                payload_len: self.network_payload_len(),
                state_code: self.network_connection_state_code(),
                state: self.network_connection_state_name().to_string(),
                armed: self.runtime.ui_network.network_armed,
                completed: self.runtime.ui_network.network_completed,
                faulted: self.runtime.ui_network.network_faulted,
                cancelled: self.runtime.ui_network.network_cancelled,
                timeout_armed: self.runtime.ui_network.network_timeout_armed,
                source_closed: self.runtime.ui_network.network_source_closed,
                stage: self.runtime.ui_network.network_stage,
                events: self.runtime.ui_network.network_events,
                delegate_callbacks: self.runtime.ui_network.delegate_callbacks,
                bytes_delivered: self.runtime.ui_network.network_bytes_delivered,
                retained_response: self.runtime.ui_network.network_response_retained,
                retained_data: self.runtime.ui_network.network_data_retained,
                retained_error: self.runtime.ui_network.network_error_retained,
                fault_events: self.runtime.ui_network.network_fault_events,
                fault_mode: self.runtime.ui_network.network_fault_mode,
                fault_modes: self.runtime.ui_network.network_fault_history.clone(),
                last_error_domain: has_error.then(|| self.network_error_domain().to_string()),
                last_error_code: has_error.then(|| self.network_error_code()),
                last_error_kind: has_error.then(|| self.network_error_kind().to_string()),
                last_error_description: has_error.then(|| self.network_error_description().to_string()),
                retry_recommended: self.network_should_retry(),
                foundation_string_backing: self.foundation_string_backing_ready(),
                foundation_data_backing: self.foundation_data_backing_ready(),
                data_bytes_ptr: self.blob_backing(self.runtime.ui_network.network_data).map(|blob| blob.ptr),
                payload_preview_ascii: self
                    .blob_backing(self.runtime.ui_network.network_data)
                    .map(|blob| blob.preview_ascii.clone())
                    .unwrap_or_else(|| self.synthetic_payload_preview()),
                delegate_binding_trace: self.runtime.ui_network.network_delegate_binding_trace.clone(),
                connection_birth_trace: self.runtime.ui_network.network_connection_birth_trace.clone(),
                slot_trace: self.runtime.ui_network.network_slot_trace.clone(),
                owner_candidate_trace: self.runtime.ui_network.network_owner_candidate_trace.clone(),
                last_delegate_binding: self.runtime.ui_network.network_last_delegate_binding.clone(),
                last_connection_birth: self.runtime.ui_network.network_last_connection_birth.clone(),
                last_slot_event: self.runtime.ui_network.network_last_slot_event.clone(),
                last_owner_candidate: self.runtime.ui_network.network_last_owner_candidate.clone(),
                first_app_delegate_binding: self.runtime.ui_network.first_app_delegate_binding.clone(),
            },
            reachability: RuntimeReachabilitySummary {
                scheduled: self.runtime.ui_network.reachability_scheduled,
                callback_set: self.runtime.ui_network.reachability_callback_set,
                flags: self.reachability_flags(),
                flags_label: self.reachability_flags_label(),
                state: if self.runtime.ui_network.reachability_scheduled { "reachable".to_string() } else { "idle".to_string() },
            },
            streams: RuntimeStreamSummary {
                read_status_code: self.runtime.ui_network.read_stream_status,
                read_status: Self::stream_status_name(self.runtime.ui_network.read_stream_status).to_string(),
                read_open: self.runtime.ui_network.read_stream_open,
                read_scheduled: self.runtime.ui_network.read_stream_scheduled,
                read_client_set: self.runtime.ui_network.read_stream_client_set,
                read_events: self.runtime.ui_network.read_stream_events,
                read_bytes_consumed: self.runtime.ui_network.read_stream_bytes_consumed,
                read_has_bytes_available: self.read_stream_has_bytes_available(),
                write_status_code: self.runtime.ui_network.write_stream_status,
                write_status: Self::stream_status_name(self.runtime.ui_network.write_stream_status).to_string(),
                write_open: self.runtime.ui_network.write_stream_open,
                write_scheduled: self.runtime.ui_network.write_stream_scheduled,
                write_client_set: self.runtime.ui_network.write_stream_client_set,
                write_events: self.runtime.ui_network.write_stream_events,
                write_bytes_written: self.runtime.ui_network.write_stream_bytes_written,
                write_can_accept_bytes: self.write_stream_can_accept_bytes(),
            },
            graphics: RuntimeGraphicsSummary {
                api: self.graphics_api_name().to_string(),
                context_current: self.runtime.ui_graphics.graphics_context_current,
                layer_attached: self.runtime.ui_graphics.graphics_layer_attached,
                surface_ready: self.runtime.ui_graphics.graphics_surface_ready,
                framebuffer_complete: self.runtime.ui_graphics.graphics_framebuffer_complete,
                viewport_ready: self.runtime.ui_graphics.graphics_viewport_ready,
                presented: self.runtime.ui_graphics.graphics_presented,
                readback_ready: self.runtime.ui_graphics.graphics_readback_ready,
                frame_index: self.runtime.ui_graphics.graphics_frame_index,
                present_calls: self.runtime.ui_graphics.graphics_present_calls,
                draw_calls: self.runtime.ui_graphics.graphics_draw_calls,
                clear_calls: self.runtime.ui_graphics.graphics_clear_calls,
                readback_calls: self.runtime.ui_graphics.graphics_readback_calls,
                gl_calls: self.runtime.ui_graphics.graphics_gl_calls,
                last_error: self.runtime.ui_graphics.graphics_last_error,
                surface_width: self.runtime.ui_graphics.graphics_surface_width,
                surface_height: self.runtime.ui_graphics.graphics_surface_height,
                viewport_x: self.runtime.ui_graphics.graphics_viewport_x,
                viewport_y: self.runtime.ui_graphics.graphics_viewport_y,
                viewport_width: self.runtime.ui_graphics.graphics_viewport_width,
                viewport_height: self.runtime.ui_graphics.graphics_viewport_height,
                scissor_enabled: self.runtime.ui_graphics.graphics_scissor_enabled,
                scissor_x: self.runtime.ui_graphics.graphics_scissor_x,
                scissor_y: self.runtime.ui_graphics.graphics_scissor_y,
                scissor_width: self.runtime.ui_graphics.graphics_scissor_width,
                scissor_height: self.runtime.ui_graphics.graphics_scissor_height,
                framebuffer_bytes: self.runtime.ui_graphics.graphics_framebuffer_bytes,
                last_readback_bytes: self.runtime.ui_graphics.graphics_last_readback_bytes,
                last_readback_x: self.runtime.ui_graphics.graphics_last_readback_x,
                last_readback_y: self.runtime.ui_graphics.graphics_last_readback_y,
                last_readback_width: self.runtime.ui_graphics.graphics_last_readback_width,
                last_readback_height: self.runtime.ui_graphics.graphics_last_readback_height,
                last_readback_checksum: self.runtime.ui_graphics.graphics_last_readback_checksum,
                last_readback_origin: self.runtime.ui_graphics.graphics_last_readback_origin.clone(),
                last_present_source: self.runtime.ui_graphics.graphics_last_present_source.clone(),
                last_present_decision: self.runtime.ui_graphics.graphics_last_present_decision.clone(),
                last_visible_bbox_x: self.runtime.ui_graphics.graphics_last_visible_bbox_x,
                last_visible_bbox_y: self.runtime.ui_graphics.graphics_last_visible_bbox_y,
                last_visible_bbox_width: self.runtime.ui_graphics.graphics_last_visible_bbox_width,
                last_visible_bbox_height: self.runtime.ui_graphics.graphics_last_visible_bbox_height,
                last_visible_pixels: self.runtime.ui_graphics.graphics_last_visible_pixels,
                last_nonzero_pixels: self.runtime.ui_graphics.graphics_last_nonzero_pixels,
                diagnosis_hint: self.runtime.ui_graphics.graphics_diagnosis_hint.clone(),
                recent_events: self.runtime.ui_graphics.graphics_recent_events.clone(),
                readback_changed: self.runtime.ui_graphics.graphics_readback_changed,
                readback_stable_streak: self.runtime.ui_graphics.graphics_readback_stable_streak,
                dominant_rgba: self.runtime.ui_graphics.graphics_last_dominant_rgba.clone(),
                dominant_pct_milli: self.runtime.ui_graphics.graphics_last_dominant_pct_milli,
                unique_frames_saved: self.runtime.ui_graphics.graphics_unique_frames_saved,
                last_unique_dump_path: self.runtime.ui_graphics.graphics_last_unique_dump_path.clone(),
                retained_present_calls: self.runtime.ui_graphics.graphics_retained_present_calls,
                synthetic_fallback_present_calls: self.runtime.ui_graphics.graphics_synthetic_fallback_present_calls,
                auto_scene_present_calls: self.runtime.ui_graphics.graphics_auto_scene_present_calls,
                guest_draw_calls: self.runtime.ui_graphics.graphics_guest_draw_calls,
                guest_vertex_fetches: self.runtime.ui_graphics.graphics_guest_vertex_fetches,
                last_draw_mode: self.runtime.ui_graphics.graphics_last_draw_mode,
                last_draw_mode_label: self.runtime.ui_graphics.graphics_last_draw_mode_label.clone(),
                last_guest_draw_checksum: self.runtime.ui_graphics.graphics_last_guest_draw_checksum,
                uikit_context_current: self.runtime.graphics.current_uigraphics_context != 0 || !self.runtime.graphics.uigraphics_stack.is_empty(),
                uikit_contexts_created: self.runtime.ui_graphics.graphics_uikit_contexts_created,
                uikit_images_created: self.runtime.ui_graphics.graphics_uikit_images_created,
                uikit_draw_ops: self.runtime.ui_graphics.graphics_uikit_draw_ops,
                uikit_present_ops: self.runtime.ui_graphics.graphics_uikit_present_ops,
                last_ui_source: self.runtime.ui_graphics.graphics_last_ui_source.clone(),
                dump_frames_enabled: self.tuning.dump_frames,
                dump_every: self.tuning.dump_every,
                dump_limit: self.tuning.dump_limit,
                dumps_saved: self.runtime.ui_graphics.graphics_dump_saved,
                last_dump_path: self.runtime.ui_graphics.graphics_last_dump_path.clone(),
                last_raw_dump_path: self.runtime.ui_graphics.graphics_last_raw_dump_path.clone(),
                last_bgra_dump_path: self.runtime.ui_graphics.graphics_last_bgra_dump_path.clone(),
                last_viewport_tl_dump_path: self.runtime.ui_graphics.graphics_last_viewport_tl_dump_path.clone(),
                last_viewport_bl_dump_path: self.runtime.ui_graphics.graphics_last_viewport_bl_dump_path.clone(),
                last_bbox_dump_path: self.runtime.ui_graphics.graphics_last_bbox_dump_path.clone(),
            },
            scene: RuntimeSceneSummary {
                transition_calls: self.runtime.ui_cocos.scene_transition_calls.max(self.runtime.scene.synthetic_scene_transitions),
                run_with_scene_calls: self.runtime.ui_cocos.scene_run_with_scene_calls,
                replace_scene_calls: self.runtime.ui_cocos.scene_replace_scene_calls,
                push_scene_calls: self.runtime.ui_cocos.scene_push_scene_calls,
                on_enter_events: self.runtime.ui_cocos.scene_on_enter_events,
                on_exit_events: self.runtime.ui_cocos.scene_on_exit_events,
                on_enter_transition_finish_events: self.runtime.ui_cocos.scene_on_enter_transition_finish_events,
                running_scene_ticks: self.runtime.scene.synthetic_running_scene_ticks,
                menu_probe_attempts: self.runtime.scene.synthetic_menu_probe_attempts,
                menu_probe_fired: self.runtime.scene.synthetic_menu_probe_fired,
                recent_events: self.runtime.ui_cocos.scene_recent_events.clone(),
            },
            scheduler: RuntimeSchedulerSummary {
                mainloop_calls: self.runtime.ui_cocos.scheduler_mainloop_calls,
                draw_scene_calls: self.runtime.ui_cocos.scheduler_draw_scene_calls,
                draw_frame_calls: self.runtime.ui_cocos.scheduler_draw_frame_calls,
                schedule_calls: self.runtime.ui_cocos.scheduler_schedule_calls,
                update_calls: self.runtime.ui_cocos.scheduler_update_calls,
                invalidate_calls: self.runtime.ui_cocos.scheduler_invalidate_calls,
                render_callback_calls: self.runtime.ui_cocos.scheduler_render_callback_calls,
                recent_events: self.runtime.ui_cocos.scheduler_recent_events.clone(),
            },
            ui_tree: self.runtime_ui_tree_summary(),
            filesystem: RuntimeFilesystemSummary {
                bundle_available: self.runtime.fs.bundle_root.is_some(),
                bundle_root: self.bundle_root_string(),
                indexed_files: self.runtime.fs.bundle_resource_index.len() as u32,
                cached_images: self.runtime.fs.resource_image_cache.len() as u32,
                bundle_objects_created: self.runtime.fs.bundle_objects_created,
                bundle_scoped_hits: self.runtime.fs.bundle_scoped_hits,
                bundle_scoped_misses: self.runtime.fs.bundle_scoped_misses,
                png_cgbi_detected: self.runtime.fs.png_cgbi_detected,
                png_cgbi_decoded: self.runtime.fs.png_cgbi_decoded,
                png_decode_failures: self.runtime.fs.png_decode_failures,
                image_named_hits: self.runtime.fs.image_named_hits,
                image_named_misses: self.runtime.fs.image_named_misses,
                file_open_hits: self.runtime.fs.file_open_hits,
                file_open_misses: self.runtime.fs.file_open_misses,
                file_read_ops: self.runtime.fs.file_read_ops,
                file_bytes_read: self.runtime.fs.file_bytes_read,
                open_file_handles: self.runtime.fs.host_files.len() as u32,
                last_resource_name: self.runtime.fs.last_resource_name.clone(),
                last_resource_path: self.runtime.fs.last_resource_path.clone(),
                last_file_path: self.runtime.fs.last_file_path.clone(),
                last_file_mode: self.runtime.fs.last_file_mode.clone(),
            },
            heap: RuntimeHeapSummary {
                base: self.runtime.heap.synthetic_heap_allocations
                    .values()
                    .map(|alloc| alloc.ptr)
                    .min()
                    .unwrap_or(self.runtime.heap.synthetic_heap_cursor.min(self.runtime.heap.synthetic_heap_end)),
                end: self.runtime.heap.synthetic_heap_end,
                cursor: self.runtime.heap.synthetic_heap_cursor,
                allocations_total: self.runtime.heap.synthetic_heap_allocations_total,
                allocations_active: self.active_synthetic_heap_allocations(),
                frees: self.runtime.heap.synthetic_heap_frees,
                reallocs: self.runtime.heap.synthetic_heap_reallocs,
                bytes_active: self.runtime.heap.synthetic_heap_bytes_active,
                bytes_peak: self.runtime.heap.synthetic_heap_bytes_peak,
                bytes_reserved: self.synthetic_heap_reserved_bytes(),
                last_alloc_ptr: self.runtime.heap.synthetic_heap_last_alloc_ptr,
                last_alloc_size: self.runtime.heap.synthetic_heap_last_alloc_size,
                last_freed_ptr: self.runtime.heap.synthetic_heap_last_freed_ptr,
                last_realloc_old_ptr: self.runtime.heap.synthetic_heap_last_realloc_old_ptr,
                last_realloc_new_ptr: self.runtime.heap.synthetic_heap_last_realloc_new_ptr,
                last_realloc_size: self.runtime.heap.synthetic_heap_last_realloc_size,
                last_error: self.runtime.heap.synthetic_heap_last_error.clone(),
            },
            vfp: RuntimeVfpSummary {
                multi_ops: self.exec.vfp_multi_ops,
                load_multi_ops: self.exec.vfp_load_multi_ops,
                store_multi_ops: self.exec.vfp_store_multi_ops,
                pc_base_ops: self.exec.vfp_pc_base_ops,
                pc_base_load_ops: self.exec.vfp_pc_base_load_ops,
                pc_base_store_ops: self.exec.vfp_pc_base_store_ops,
                single_reg_capacity: (self.exec.vfp_d_regs.len() as u32).saturating_mul(2),
                single_range_ops: self.exec.vfp_single_range_ops,
                exact_opcode_hits: self.exec.vfp_exact_opcode_hits,
                exact_override_hits: self.exec.vfp_exact_override_hits,
                single_transfer_ops: self.exec.vfp_single_transfer_ops,
                double_transfer_ops: self.exec.vfp_double_transfer_ops,
                last_op: self.exec.vfp_last_op.clone(),
                last_start_addr: self.exec.vfp_last_start_addr,
                last_end_addr: self.exec.vfp_last_end_addr,
                last_pc_base_addr: self.exec.vfp_last_pc_base_addr,
                last_pc_base_word: self.exec.vfp_last_pc_base_word,
                last_single_range: self.exec.vfp_last_single_range.clone(),
                last_exact_opcode: self.exec.vfp_last_exact_opcode.clone(),
                last_exact_decoder_branch: self.exec.vfp_last_exact_decoder_branch.clone(),
                last_transfer_mode: self.exec.vfp_last_transfer_mode.clone(),
                last_transfer_start_reg: self.exec.vfp_last_transfer_start_reg,
                last_transfer_end_reg: self.exec.vfp_last_transfer_end_reg,
                last_transfer_count: self.exec.vfp_last_transfer_count,
                last_transfer_precision: self.exec.vfp_last_transfer_precision.clone(),
                last_transfer_addr: self.exec.vfp_last_transfer_addr,
                last_exact_reason: self.exec.vfp_last_exact_reason.clone(),
            },
            arm: RuntimeArmSummary {
                reg_shift_operand2_ops: self.exec.arm_reg_shift_operand2_ops,
                extra_load_store_ops: self.exec.arm_extra_load_store_ops,
                extra_load_store_loads: self.exec.arm_extra_load_store_loads,
                extra_load_store_stores: self.exec.arm_extra_load_store_stores,
                last_reg_shift: self.exec.arm_last_reg_shift.clone(),
                last_extra_load_store: self.exec.arm_last_extra_load_store.clone(),
                exact_epilogue_site_hits: self.exec.arm_exact_epilogue_site_hits,
                exact_epilogue_repairs: self.exec.arm_exact_epilogue_repairs,
                exact_epilogue_last_pc: self.exec.arm_exact_epilogue_last_pc,
                exact_epilogue_last_before_sp: self.exec.arm_exact_epilogue_last_before_sp,
                exact_epilogue_last_after_sp: self.exec.arm_exact_epilogue_last_after_sp,
                exact_epilogue_last_r0: self.exec.arm_exact_epilogue_last_r0,
                exact_epilogue_last_r7: self.exec.arm_exact_epilogue_last_r7,
                exact_epilogue_last_r8: self.exec.arm_exact_epilogue_last_r8,
                exact_epilogue_last_lr: self.exec.arm_exact_epilogue_last_lr,
                exact_epilogue_last_repair: self.exec.arm_exact_epilogue_last_repair.clone(),
            },
            objc_bridge: RuntimeObjcBridgeSummary {
                metadata_available: self.runtime.objc.objc_section_classlist.is_some() || self.runtime.objc.objc_section_const.is_some(),
                classlist_present: self.runtime.objc.objc_section_classlist.is_some(),
                cfstring_present: self.runtime.objc.objc_section_cfstring.is_some(),
                parsed_classes: self.runtime.objc.objc_classes_by_ptr.len() as u32,
                delegate_name: self.runtime.objc.objc_bridge_delegate_name.clone(),
                delegate_class_name: self.runtime.objc.objc_bridge_delegate_class_name.clone(),
                inferred_class_name: self.runtime.objc.objc_bridge_inferred_class_name.clone(),
                inferred_selector_hits: self.runtime.objc.objc_bridge_inferred_selector_hits,
                launch_selector: self.runtime.objc.objc_bridge_launch_selector.clone(),
                launch_imp: self.runtime.objc.objc_bridge_launch_imp,
                bridge_attempted: self.runtime.objc.objc_bridge_attempted,
                bridge_succeeded: self.runtime.objc.objc_bridge_succeeded,
                failure_reason: self.runtime.objc.objc_bridge_failure_reason.clone(),
                real_msgsend_dispatches: self.runtime.objc.objc_real_msgsend_dispatches,
                last_real_selector: self.runtime.objc.objc_last_real_selector.clone(),
                super_msgsend_dispatches: self.runtime.objc.objc_super_msgsend_dispatches,
                super_msgsend_fallback_returns: self.runtime.objc.objc_super_msgsend_fallback_returns,
                last_super_selector: self.runtime.objc.objc_last_super_selector.clone(),
                last_super_receiver: self.runtime.objc.objc_last_super_receiver,
                last_super_class: self.runtime.objc.objc_last_super_class,
                last_super_imp: self.runtime.objc.objc_last_super_imp,
                alloc_calls: self.runtime.objc.objc_alloc_calls,
                alloc_with_zone_calls: self.runtime.objc.objc_alloc_with_zone_calls,
                class_create_instance_calls: self.runtime.objc.objc_class_create_instance_calls,
                init_calls: self.runtime.objc.objc_init_calls,
                instances_materialized: self.runtime.objc.objc_instances_materialized,
                last_alloc_class: self.runtime.objc.objc_last_alloc_class.clone(),
                last_alloc_receiver: self.runtime.objc.objc_last_alloc_receiver,
                last_alloc_result: self.runtime.objc.objc_last_alloc_result,
                last_init_receiver: self.runtime.objc.objc_last_init_receiver,
                last_init_result: self.runtime.objc.objc_last_init_result,
            },
            hot_path: RuntimeHotPathSummary {
                objc_msgsend_calls: self.runtime.objc.objc_msgsend_calls,
                objc_unique_selectors: self.runtime.objc.objc_selector_counts.len() as u32,
                recent_objc_selectors: self.runtime.objc.recent_objc_selectors.clone(),
                top_objc_selectors: Self::top_count_entries(&self.runtime.objc.objc_selector_counts, 12),
                saw_draw_rect: self.runtime.objc.objc_selector_counts.contains_key("drawRect:"),
                saw_set_needs_display: self.runtime.objc.objc_selector_counts.contains_key("setNeedsDisplay"),
                saw_layout_subviews: self.runtime.objc.objc_selector_counts.contains_key("layoutSubviews"),
                saw_image_named: self.runtime.objc.objc_selector_counts.contains_key("imageNamed:"),
                saw_present_renderbuffer: self.runtime.objc.objc_selector_counts.contains_key("presentRenderbuffer:"),
                gl_calls_seen: self.runtime.graphics.gl_call_counts.values().copied().sum(),
                recent_gl_calls: self.runtime.graphics.recent_gl_calls.clone(),
                top_gl_calls: Self::top_count_entries(&self.runtime.graphics.gl_call_counts, 12),
                saw_gl_bind_texture: self.runtime.graphics.gl_call_counts.contains_key("glBindTexture"),
                saw_gl_teximage2d: self.runtime.graphics.gl_call_counts.contains_key("glTexImage2D"),
                saw_gl_draw_arrays: self.runtime.graphics.gl_call_counts.contains_key("glDrawArrays"),
                saw_gl_draw_elements: self.runtime.graphics.gl_call_counts.contains_key("glDrawElements"),
            },
            observability: RuntimeObservabilitySummary {
                trace_build_id: self.trace_build_id().to_string(),
                trace_banner_emitted: self.runtime.scheduler.trace.build_banner_emitted,
                scene_progress_trace: self.runtime.scene.scene_progress_trace.clone(),
                sprite_watch_trace: self.runtime.scene.sprite_watch_trace.clone(),
                graph_trace: self.runtime.scene.graph_trace.clone(),
                scheduler_trace: self.runtime.scheduler.trace.events.clone(),
                scheduler_trace_live_snapshot: format!(
                    "trace.verify.live build={} runloopLive={} appActive={} timerArmed={} sources={} recordedSources={}",
                    self.trace_build_id(),
                    if self.runtime.ui_runtime.runloop_live { "YES" } else { "NO" },
                    if self.runtime.ui_runtime.app_active { "YES" } else { "NO" },
                    if self.runtime.ui_runtime.timer_armed { "YES" } else { "NO" },
                    self.current_runloop_source_count(),
                    self.runtime.ui_runtime.runloop_sources,
                ),
                callback_trace: self.runtime.scheduler.trace.callbacks.clone(),
            },
            audio: crate::runtime::diagnostics::RuntimeAudioSummary {
                openal_device_open_calls: self.runtime.audio_trace.openal_device_open_calls,
                openal_context_create_calls: self.runtime.audio_trace.openal_context_create_calls,
                openal_make_current_calls: self.runtime.audio_trace.openal_make_current_calls,
                openal_buffers_generated: self.runtime.audio_trace.openal_buffers_generated,
                openal_sources_generated: self.runtime.audio_trace.openal_sources_generated,
                openal_buffer_upload_calls: self.runtime.audio_trace.openal_buffer_upload_calls,
                openal_bytes_uploaded: self.runtime.audio_trace.openal_bytes_uploaded,
                openal_queue_calls: self.runtime.audio_trace.openal_queue_calls,
                openal_unqueue_calls: self.runtime.audio_trace.openal_unqueue_calls,
                openal_play_calls: self.runtime.audio_trace.openal_play_calls,
                openal_stop_calls: self.runtime.audio_trace.openal_stop_calls,
                openal_last_buffer_format: self.runtime.audio_trace.openal_last_buffer_format.clone(),
                openal_last_source_state: self.runtime.audio_trace.openal_last_source_state.clone(),
                audioqueue_create_calls: self.runtime.audio_trace.audioqueue_create_calls,
                audioqueue_allocate_calls: self.runtime.audio_trace.audioqueue_allocate_calls,
                audioqueue_enqueue_calls: self.runtime.audio_trace.audioqueue_enqueue_calls,
                audioqueue_enqueued_bytes: self.runtime.audio_trace.audioqueue_enqueued_bytes,
                audioqueue_prime_calls: self.runtime.audio_trace.audioqueue_prime_calls,
                audioqueue_start_calls: self.runtime.audio_trace.audioqueue_start_calls,
                audioqueue_stop_calls: self.runtime.audio_trace.audioqueue_stop_calls,
                audioqueue_dispose_calls: self.runtime.audio_trace.audioqueue_dispose_calls,
                audioqueue_output_callback_dispatches: self.runtime.audio_trace.audioqueue_output_callback_dispatches,
                audioqueue_property_callback_dispatches: self.runtime.audio_trace.audioqueue_property_callback_dispatches,
                audioqueue_last_format: self.runtime.audio_trace.audioqueue_last_format.clone(),
                audioqueue_last_queue: self.runtime.audio_trace.audioqueue_last_queue,
                audioqueue_last_buffer: self.runtime.audio_trace.audioqueue_last_buffer,
                audioqueue_last_buffer_preview_hex: self.runtime.audio_trace.audioqueue_last_buffer_preview_hex.clone(),
                audioqueue_last_buffer_preview_ascii: self.runtime.audio_trace.audioqueue_last_buffer_preview_ascii.clone(),
                audiofile_open_calls: self.runtime.audio_trace.audiofile_open_calls,
                audiofile_read_bytes_calls: self.runtime.audio_trace.audiofile_read_bytes_calls,
                audiofile_read_packets_calls: self.runtime.audio_trace.audiofile_read_packets_calls,
                audiofile_bytes_served: self.runtime.audio_trace.audiofile_bytes_served,
                systemsound_create_calls: self.runtime.audio_trace.systemsound_create_calls,
                systemsound_play_calls: self.runtime.audio_trace.systemsound_play_calls,
                systemsound_dispose_calls: self.runtime.audio_trace.systemsound_dispose_calls,
                objc_audio_player_alloc_calls: self.runtime.audio_trace.objc_audio_player_alloc_calls,
                objc_audio_player_init_url_calls: self.runtime.audio_trace.objc_audio_player_init_url_calls,
                objc_audio_player_init_data_calls: self.runtime.audio_trace.objc_audio_player_init_data_calls,
                objc_audio_player_prepare_calls: self.runtime.audio_trace.objc_audio_player_prepare_calls,
                objc_audio_player_play_calls: self.runtime.audio_trace.objc_audio_player_play_calls,
                objc_audio_player_pause_calls: self.runtime.audio_trace.objc_audio_player_pause_calls,
                objc_audio_player_stop_calls: self.runtime.audio_trace.objc_audio_player_stop_calls,
                objc_audio_player_set_volume_calls: self.runtime.audio_trace.objc_audio_player_set_volume_calls,
                objc_audio_player_set_loops_calls: self.runtime.audio_trace.objc_audio_player_set_loops_calls,
                objc_audio_engine_shared_calls: self.runtime.audio_trace.objc_audio_engine_shared_calls,
                objc_audio_manager_shared_calls: self.runtime.audio_trace.objc_audio_manager_shared_calls,
                objc_audio_manager_soundengine_calls: self.runtime.audio_trace.objc_audio_manager_soundengine_calls,
                objc_audio_manager_soundengine_nil_results: self.runtime.audio_trace.objc_audio_manager_soundengine_nil_results,
                objc_audio_engine_preload_calls: self.runtime.audio_trace.objc_audio_engine_preload_calls,
                objc_audio_bgm_preload_calls: self.runtime.audio_trace.objc_audio_bgm_preload_calls,
                objc_audio_engine_play_calls: self.runtime.audio_trace.objc_audio_engine_play_calls,
                objc_audio_bgm_play_calls: self.runtime.audio_trace.objc_audio_bgm_play_calls,
                objc_audio_engine_stop_calls: self.runtime.audio_trace.objc_audio_engine_stop_calls,
                objc_audio_engine_effect_calls: self.runtime.audio_trace.objc_audio_engine_effect_calls,
                objc_audio_engine_async_load_progress_calls: self.runtime.audio_trace.objc_audio_engine_async_load_progress_calls,
                objc_audio_engine_async_load_progress_nil_receivers: self.runtime.audio_trace.objc_audio_engine_async_load_progress_nil_receivers,
                objc_audio_engine_playsound_calls: self.runtime.audio_trace.objc_audio_engine_playsound_calls,
                objc_audio_engine_playsound_nil_receivers: self.runtime.audio_trace.objc_audio_engine_playsound_nil_receivers,
                objc_audio_fallback_dispatches: self.runtime.audio_trace.objc_audio_fallback_dispatches,
                objc_audio_last_class: self.runtime.audio_trace.objc_audio_last_class.clone(),
                objc_audio_last_selector: self.runtime.audio_trace.objc_audio_last_selector.clone(),
                objc_audio_last_resource: self.runtime.audio_trace.objc_audio_last_resource.clone(),
                objc_audio_last_result: self.runtime.audio_trace.objc_audio_last_result.clone(),
                objc_audio_last_call_args: self.runtime.audio_trace.objc_audio_last_call_args.clone(),
                objc_audio_last_scalar_probe: self.runtime.audio_trace.objc_audio_last_scalar_probe.clone(),
                unsupported_events: self.runtime.audio_trace.unsupported_events,
                recent_events: self.runtime.audio_trace.recent_events.clone(),
            },
        }
    }


}
