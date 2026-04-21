impl MemoryArm32Backend {
// UIKit/CoreGraphics software surface helpers and synthetic present tick orchestration.

    fn ui_blend_pixel(buffer: &mut [u8], width: u32, height: u32, x: i32, y: i32, rgba: [u8; 4]) {
        if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 { return; }
        let idx = ((y as usize * width as usize) + x as usize) * 4;
        if idx + 3 >= buffer.len() { return; }
        let alpha = rgba[3] as u32;
        let inv = 255u32.saturating_sub(alpha);
        for c in 0..3 {
            let dst = buffer[idx + c] as u32;
            let src = rgba[c] as u32;
            buffer[idx + c] = (((src * alpha) + (dst * inv)) / 255) as u8;
        }
        buffer[idx + 3] = 255;
    }

    fn ui_fill_rect_rgba(buffer: &mut [u8], width: u32, height: u32, x: i32, y: i32, rect_w: u32, rect_h: u32, rgba: [u8; 4]) {
        let end_y = y.saturating_add(rect_h as i32);
        let end_x = x.saturating_add(rect_w as i32);
        for yy in y..end_y {
            for xx in x..end_x {
                Self::ui_blend_pixel(buffer, width, height, xx, yy, rgba);
            }
        }
    }

    fn ui_clear_rect_rgba(buffer: &mut [u8], width: u32, height: u32, x: i32, y: i32, rect_w: u32, rect_h: u32) {
        let end_y = y.saturating_add(rect_h as i32);
        let end_x = x.saturating_add(rect_w as i32);
        for yy in y..end_y {
            for xx in x..end_x {
                if xx < 0 || yy < 0 || xx >= width as i32 || yy >= height as i32 { continue; }
                let idx = ((yy as usize * width as usize) + xx as usize) * 4;
                if idx + 3 < buffer.len() {
                    buffer[idx..idx + 4].copy_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
    }

    fn composite_rgba_scaled_into(buffer: &mut [u8], dst_w: u32, dst_h: u32, src: &[u8], src_w: u32, src_h: u32, x: i32, y: i32, out_w: u32, out_h: u32) {
        Self::composite_rgba_scaled_tinted_into(buffer, dst_w, dst_h, src, src_w, src_h, x, y, out_w, out_h, [255, 255, 255, 255]);
    }

    fn composite_rgba_scaled_tinted_into(
        buffer: &mut [u8],
        dst_w: u32,
        dst_h: u32,
        src: &[u8],
        src_w: u32,
        src_h: u32,
        x: i32,
        y: i32,
        out_w: u32,
        out_h: u32,
        tint_rgba: [u8; 4],
    ) {
        if src_w == 0 || src_h == 0 || out_w == 0 || out_h == 0 { return; }
        for row in 0..out_h {
            let sy = ((row as u64 * src_h as u64) / out_h as u64).min(src_h.saturating_sub(1) as u64) as u32;
            for col in 0..out_w {
                let sx = ((col as u64 * src_w as u64) / out_w as u64).min(src_w.saturating_sub(1) as u64) as u32;
                let src_idx = ((sy * src_w + sx) * 4) as usize;
                if src_idx + 3 >= src.len() { continue; }
                let rgba = [
                    ((src[src_idx] as u32 * tint_rgba[0] as u32) / 255) as u8,
                    ((src[src_idx + 1] as u32 * tint_rgba[1] as u32) / 255) as u8,
                    ((src[src_idx + 2] as u32 * tint_rgba[2] as u32) / 255) as u8,
                    ((src[src_idx + 3] as u32 * tint_rgba[3] as u32) / 255) as u8,
                ];
                Self::ui_blend_pixel(buffer, dst_w, dst_h, x.saturating_add(col as i32), y.saturating_add(row as i32), rgba);
            }
        }
    }


    fn crop_rgba_region(src: &[u8], src_w: u32, src_h: u32, rect_x: u32, rect_y: u32, rect_w: u32, rect_h: u32) -> Option<Vec<u8>> {
        if src_w == 0 || src_h == 0 || rect_w == 0 || rect_h == 0 {
            return None;
        }
        let end_x = rect_x.checked_add(rect_w)?;
        let end_y = rect_y.checked_add(rect_h)?;
        if end_x > src_w || end_y > src_h {
            return None;
        }
        let mut out = vec![0u8; rect_w as usize * rect_h as usize * 4];
        for row in 0..rect_h {
            let src_idx = (((rect_y + row) * src_w + rect_x) * 4) as usize;
            let dst_idx = (row * rect_w * 4) as usize;
            let span = (rect_w * 4) as usize;
            out[dst_idx..dst_idx + span].copy_from_slice(&src[src_idx..src_idx + span]);
        }
        Some(out)
    }

    fn score_rgba_region(src: &[u8]) -> u64 {
        if src.is_empty() {
            return 0;
        }
        let mut opaque = 0u64;
        let mut energy = 0u64;
        for px in src.chunks_exact(4) {
            let a = px[3] as u64;
            if a > 8 {
                opaque += 1;
            }
            energy += (px[0] as i32 - px[1] as i32).unsigned_abs() as u64;
            energy += (px[1] as i32 - px[2] as i32).unsigned_abs() as u64;
            energy += (px[0] as i32 - px[2] as i32).unsigned_abs() as u64;
            energy += a / 8;
        }
        opaque.saturating_mul(1024).saturating_add(energy)
    }

    fn synthetic_sprite_visual_score(&self, node: u32) -> u64 {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return 0;
        };
        let effective_texture = self.synthetic_node_effective_texture(node);
        if effective_texture == 0 {
            return 0;
        }
        let Some(texture) = self.runtime.graphics.synthetic_textures.get(&effective_texture) else {
            return 0;
        };
        if texture.image == 0 {
            return 0;
        }
        let Some(image) = self.runtime.graphics.synthetic_images.get(&texture.image) else {
            return 0;
        };
        let (rgba, _, _) = self.resolve_sprite_texture_region(
            node,
            state,
            &image.rgba,
            image.width.max(1),
            image.height.max(1),
        );
        Self::score_rgba_region(&rgba)
    }

    fn maybe_force_menu_item_default_variant(
        &mut self,
        _item: u32,
        normal: u32,
        selected: u32,
        disabled: u32,
        selector_name: &str,
    ) -> Option<String> {
        if !self.active_profile().is_achievements_selector(selector_name) {
            return None;
        }
        let normal_score = self.synthetic_sprite_visual_score(normal);
        let selected_score = self.synthetic_sprite_visual_score(selected);
        let disabled_score = self.synthetic_sprite_visual_score(disabled);
        let preferred = if selected != 0 && selected_score >= normal_score.saturating_add(256) && selected_score >= disabled_score.saturating_add(256) && selected_score >= 1024 {
            Some(("selected", selected))
        } else if disabled != 0 && disabled_score >= normal_score.saturating_add(256) && disabled_score >= selected_score.saturating_add(256) && disabled_score >= 1024 {
            Some(("disabled", disabled))
        } else {
            None
        };
        let Some((name, chosen)) = preferred else {
            return None;
        };
        for ptr in [normal, selected, disabled] {
            if ptr != 0 {
                self.ensure_synthetic_sprite_state(ptr).visible = ptr == chosen;
            }
        }
        Some(format!(
            "default-variant {} normalScore={} selectedScore={} disabledScore={}",
            name,
            normal_score,
            selected_score,
            disabled_score,
        ))
    }

    fn maybe_promote_menu_item_variant(
        &mut self,
        _item: u32,
        normal: u32,
        selected: u32,
        disabled: u32,
    ) -> Option<String> {
        let variants = [("normal", normal), ("selected", selected), ("disabled", disabled)];
        let mut scored: Vec<(&'static str, u32, u64)> = variants
            .into_iter()
            .filter(|(_, ptr)| *ptr != 0)
            .map(|(name, ptr)| (name, ptr, self.synthetic_sprite_visual_score(ptr)))
            .collect();
        if scored.is_empty() {
            return None;
        }
        let normal_score = scored
            .iter()
            .find(|(name, ptr, _)| *name == "normal" && *ptr == normal)
            .map(|(_, _, score)| *score)
            .unwrap_or(0);
        scored.sort_by(|a, b| b.2.cmp(&a.2));
        let (best_name, best_ptr, best_score) = scored[0];
        let should_promote = best_ptr != 0
            && best_ptr != normal
            && best_score > normal_score.saturating_add(4_096)
            && best_score >= 8_192
            && normal_score <= 2_048;
        if !should_promote {
            return None;
        }
        for (_, ptr, _) in &scored {
            if *ptr != 0 {
                self.ensure_synthetic_sprite_state(*ptr).visible = *ptr == best_ptr;
            }
        }
        Some(format!(
            "variant-promote {} score={} normalScore={} selectedScore={} disabledScore={}",
            best_name,
            best_score,
            normal_score,
            self.synthetic_sprite_visual_score(selected),
            self.synthetic_sprite_visual_score(disabled),
        ))
    }

    fn ab_font5x7_rows(ch: char) -> [u8; 7] {
        match ch.to_ascii_uppercase() {
            'A' => [0b01110,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001],
            'B' => [0b11110,0b10001,0b10001,0b11110,0b10001,0b10001,0b11110],
            'C' => [0b01110,0b10001,0b10000,0b10000,0b10000,0b10001,0b01110],
            'D' => [0b11110,0b10001,0b10001,0b10001,0b10001,0b10001,0b11110],
            'E' => [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b11111],
            'F' => [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b10000],
            'G' => [0b01110,0b10001,0b10000,0b10111,0b10001,0b10001,0b01110],
            'H' => [0b10001,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001],
            'I' => [0b11111,0b00100,0b00100,0b00100,0b00100,0b00100,0b11111],
            'J' => [0b00111,0b00010,0b00010,0b00010,0b10010,0b10010,0b01100],
            'K' => [0b10001,0b10010,0b10100,0b11000,0b10100,0b10010,0b10001],
            'L' => [0b10000,0b10000,0b10000,0b10000,0b10000,0b10000,0b11111],
            'M' => [0b10001,0b11011,0b10101,0b10101,0b10001,0b10001,0b10001],
            'N' => [0b10001,0b11001,0b10101,0b10011,0b10001,0b10001,0b10001],
            'O' => [0b01110,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110],
            'P' => [0b11110,0b10001,0b10001,0b11110,0b10000,0b10000,0b10000],
            'Q' => [0b01110,0b10001,0b10001,0b10001,0b10101,0b10010,0b01101],
            'R' => [0b11110,0b10001,0b10001,0b11110,0b10100,0b10010,0b10001],
            'S' => [0b01111,0b10000,0b10000,0b01110,0b00001,0b00001,0b11110],
            'T' => [0b11111,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100],
            'U' => [0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110],
            'V' => [0b10001,0b10001,0b10001,0b10001,0b10001,0b01010,0b00100],
            'W' => [0b10001,0b10001,0b10001,0b10101,0b10101,0b10101,0b01010],
            'X' => [0b10001,0b10001,0b01010,0b00100,0b01010,0b10001,0b10001],
            'Y' => [0b10001,0b10001,0b01010,0b00100,0b00100,0b00100,0b00100],
            'Z' => [0b11111,0b00001,0b00010,0b00100,0b01000,0b10000,0b11111],
            '0' => [0b01110,0b10001,0b10011,0b10101,0b11001,0b10001,0b01110],
            '1' => [0b00100,0b01100,0b00100,0b00100,0b00100,0b00100,0b01110],
            '2' => [0b01110,0b10001,0b00001,0b00010,0b00100,0b01000,0b11111],
            '3' => [0b11110,0b00001,0b00001,0b01110,0b00001,0b00001,0b11110],
            '4' => [0b00010,0b00110,0b01010,0b10010,0b11111,0b00010,0b00010],
            '5' => [0b11111,0b10000,0b10000,0b11110,0b00001,0b00001,0b11110],
            '6' => [0b00110,0b01000,0b10000,0b11110,0b10001,0b10001,0b01110],
            '7' => [0b11111,0b00001,0b00010,0b00100,0b01000,0b01000,0b01000],
            '8' => [0b01110,0b10001,0b10001,0b01110,0b10001,0b10001,0b01110],
            '9' => [0b01110,0b10001,0b10001,0b01111,0b00001,0b00010,0b11100],
            '.' => [0,0,0,0,0,0b01100,0b01100],
            ':' => [0,0b01100,0b01100,0,0b01100,0b01100,0],
            '-' => [0,0,0,0b11111,0,0,0],
            '_' => [0,0,0,0,0,0,0b11111],
            '/' => [0b00001,0b00010,0b00100,0b01000,0b10000,0,0],
            '%' => [0b11001,0b11010,0b00100,0b01000,0b10110,0b00110,0],
            '(' => [0b00010,0b00100,0b01000,0b01000,0b01000,0b00100,0b00010],
            ')' => [0b01000,0b00100,0b00010,0b00010,0b00010,0b00100,0b01000],
            '!' => [0b00100,0b00100,0b00100,0b00100,0b00100,0,0b00100],
            ' ' => [0,0,0,0,0,0,0],
            _ => [0,0,0,0,0,0,0],
        }
    }

    fn blend_rgba_pixel_local(buffer: &mut [u8], width: u32, height: u32, x: i32, y: i32, rgba: [u8; 4]) {
        Self::ui_blend_pixel(buffer, width, height, x, y, rgba);
    }

    fn draw_ab_text_5x7(buffer: &mut [u8], width: u32, height: u32, text: &str, scale: u32, origin_x: i32, origin_y: i32, rgba: [u8; 4]) {
        let mut pen_x = origin_x;
        let scale = scale.max(1) as i32;
        for ch in text.chars() {
            let rows = Self::ab_font5x7_rows(ch);
            for (row_idx, row_bits) in rows.iter().copied().enumerate() {
                for col in 0..5 {
                    if (row_bits & (1 << (4 - col))) == 0 {
                        continue;
                    }
                    let px = pen_x + col as i32 * scale;
                    let py = origin_y + row_idx as i32 * scale;
                    for dy in 0..scale {
                        for dx in 0..scale {
                            Self::blend_rgba_pixel_local(buffer, width, height, px + dx, py + dy, rgba);
                        }
                    }
                }
            }
            pen_x += 6 * scale;
        }
    }

    fn synthesize_ab_achievements_button_region(&self, node: u32, state: &SyntheticSpriteState, src: &[u8], src_w: u32, src_h: u32) -> Option<(Vec<u8>, u32, u32)> {
        let item = state.parent;
        if item == 0 {
            return None;
        }
        let item_state = self.runtime.graphics.synthetic_sprites.get(&item)?;
        let menu = item_state.parent;
        if menu == 0 {
            return None;
        }
        let item_children = item_state.children;
        if item_children != 0 && self.synthetic_array_get(item_children, 0) != node {
            return None;
        }
        let menu_children = self.runtime.graphics.synthetic_sprites.get(&menu)?.children;
        if menu_children == 0 {
            return None;
        }
        let mut best_region: Option<(Vec<u8>, u32, u32, u64)> = None;
        for idx in 0..self.synthetic_array_len(menu_children) {
            let sibling_item = self.synthetic_array_get(menu_children, idx);
            if sibling_item == 0 || sibling_item == item {
                continue;
            }
            let Some(sibling_state) = self.runtime.graphics.synthetic_sprites.get(&sibling_item) else {
                continue;
            };
            let sibling_children = sibling_state.children;
            if sibling_children == 0 {
                continue;
            }
            let sibling_normal = self.synthetic_array_get(sibling_children, 0);
            if sibling_normal == 0 {
                continue;
            }
            let Some(sibling_normal_state) = self.runtime.graphics.synthetic_sprites.get(&sibling_normal) else {
                continue;
            };
            if sibling_normal_state.texture != state.texture {
                continue;
            }
            let (rgba, w, h) = self.resolve_sprite_texture_region(sibling_normal, sibling_normal_state, src, src_w, src_h);
            let score = Self::score_rgba_region(&rgba);
            if score < 8_192 {
                continue;
            }
            let replace = best_region.as_ref().map(|(_, _, _, best_score)| score > *best_score).unwrap_or(true);
            if replace {
                best_region = Some((rgba, w, h, score));
            }
        }
        let (mut rgba, w, h, _) = best_region?;
        let text = "ACHIEVEMENTS";
        let scale = 2u32;
        let text_w = text.chars().count() as i32 * 6 * scale as i32 - scale as i32;
        let text_h = 7 * scale as i32;
        let origin_x = ((w as i32 - text_w) / 2).max(4);
        let origin_y = ((h as i32 - text_h) / 2).max(4) - 1;
        Self::draw_ab_text_5x7(&mut rgba, w, h, text, scale, origin_x + 1, origin_y + 1, [255, 244, 210, 90]);
        Self::draw_ab_text_5x7(&mut rgba, w, h, text, scale, origin_x, origin_y, [38, 22, 8, 230]);
        Some((rgba, w, h))
    }

    fn resolve_sprite_texture_region(&self, node: u32, state: &SyntheticSpriteState, src: &[u8], src_w: u32, src_h: u32) -> (Vec<u8>, u32, u32) {
        if !state.texture_rect_explicit {
            return (src.to_vec(), src_w, src_h);
        }
        if state.width == 0 || state.height == 0 {
            return (Vec::new(), 0, 0);
        }
        let rect_x = Self::f32_from_bits(state.texture_rect_x_bits).round().max(0.0) as u32;
        let rect_y = Self::f32_from_bits(state.texture_rect_y_bits).round().max(0.0) as u32;
        let rect_w = Self::f32_from_bits(state.texture_rect_w_bits).round().max(0.0) as u32;
        let rect_h = Self::f32_from_bits(state.texture_rect_h_bits).round().max(0.0) as u32;
        if rect_w == 0 || rect_h == 0 {
            return (Vec::new(), 0, 0);
        }
        let effective_texture = self.synthetic_node_effective_texture(node);
        let texture_key = self.runtime.graphics.synthetic_textures.get(&effective_texture)
            .map(|texture| {
                format!(
                    "{}|{}|{}",
                    texture.cache_key,
                    texture.source_key,
                    texture.source_path,
                )
                .to_ascii_lowercase()
            })
            .unwrap_or_default();
        let top_left = Self::crop_rgba_region(src, src_w, src_h, rect_x, rect_y, rect_w, rect_h);
        let flipped_y = src_h.saturating_sub(rect_y.saturating_add(rect_h));
        let bottom_left = Self::crop_rgba_region(src, src_w, src_h, rect_x, flipped_y, rect_w, rect_h);

        let parent_selector_name = if state.parent == 0 {
            String::new()
        } else {
            self.runtime.graphics.synthetic_sprites
                .get(&state.parent)
                .and_then(|parent_state| {
                    if parent_state.callback_selector == 0 {
                        None
                    } else {
                        self.objc_read_selector_name(parent_state.callback_selector)
                    }
                })
                .unwrap_or_default()
        };
        let prefer_top_left_ab_achievements = self.active_profile().should_prefer_top_left_achievements_strip(
            &texture_key,
            rect_x,
            rect_y,
            rect_h,
            &parent_selector_name,
        );
        if prefer_top_left_ab_achievements {
            // After fixing menuWithItems:/initWithItems: varargs parsing, Achievements now lands in
            // the real CCMenu children list and its original atlas crop renders correctly.
            // Do not synthesize replacement text here anymore: it causes double-drawn/misaligned
            // lettering on top of the valid guest art.
            if let Some(region) = top_left.clone() {
                let score = Self::score_rgba_region(&region);
                if score > 2_048 {
                    return (region, rect_w, rect_h);
                }
            }
            if let Some(region) = bottom_left.clone() {
                let score = Self::score_rgba_region(&region);
                if score > 2_048 {
                    return (region, rect_w, rect_h);
                }
            }
        }

        let force_top_left_low_strip = self.active_profile().should_force_top_left_low_strip(
            &texture_key,
            rect_x,
            rect_y,
            rect_h,
        );
        if force_top_left_low_strip {
            if let Some(region) = top_left.clone() {
                let score = Self::score_rgba_region(&region);
                if !(self.active_profile().is_achievements_selector(&parent_selector_name) && score <= 2_048) {
                    return (region, rect_w, rect_h);
                }
            }
        }
        let prefer_bottom_left = self.synthetic_node_prefers_bottom_left_texture_rect(node);
        match (top_left, bottom_left) {
            (Some(a), Some(b)) => {
                if prefer_bottom_left {
                    let score_a = Self::score_rgba_region(&a);
                    let score_b = Self::score_rgba_region(&b);
                    if score_a > score_b.saturating_add(4096) {
                        (a, rect_w, rect_h)
                    } else {
                        (b, rect_w, rect_h)
                    }
                } else {
                    let score_a = Self::score_rgba_region(&a);
                    let score_b = Self::score_rgba_region(&b);
                    if score_b > score_a.saturating_add(512) {
                        (b, rect_w, rect_h)
                    } else {
                        (a, rect_w, rect_h)
                    }
                }
            }
            (Some(a), None) => (a, rect_w, rect_h),
            (None, Some(b)) => (b, rect_w, rect_h),
            (None, None) => (src.to_vec(), src_w, src_h),
        }
    }

    fn synthetic_matches_surface_dims(node_w: u32, node_h: u32, surface_w: u32, surface_h: u32) -> bool {
        (node_w.abs_diff(surface_w) <= 4 && node_h.abs_diff(surface_h) <= 4)
            || (node_w.abs_diff(surface_h) <= 4 && node_h.abs_diff(surface_w) <= 4)
    }

    fn synthetic_node_or_descendant_has_fullscreen_texture(
        &self,
        node: u32,
        surface_w: u32,
        surface_h: u32,
        depth: usize,
        exclude_subtree_root: u32,
    ) -> bool {
        if node == 0 || depth >= 32 {
            return false;
        }
        if exclude_subtree_root != 0 && node == exclude_subtree_root {
            return false;
        }
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return false;
        };
        if !state.visible {
            return false;
        }
        let effective_texture = self.synthetic_node_effective_texture(node);
        if effective_texture != 0 {
            let (node_w, node_h) = if state.width != 0 && state.height != 0 {
                (state.width, state.height)
            } else {
                self.synthetic_texture_dimensions(effective_texture).unwrap_or((0, 0))
            };
            if node_w != 0
                && node_h != 0
                && Self::synthetic_matches_surface_dims(node_w, node_h, surface_w, surface_h)
            {
                return true;
            }
        }
        if state.children == 0 {
            return false;
        }
        let children = self.runtime.graphics
            .synthetic_arrays
            .get(&state.children)
            .map(|arr| arr.items.clone())
            .unwrap_or_default();
        for child in children {
            if self.synthetic_node_or_descendant_has_fullscreen_texture(
                child,
                surface_w,
                surface_h,
                depth.saturating_add(1),
                exclude_subtree_root,
            ) {
                return true;
            }
        }
        false
    }

    fn synthetic_should_skip_fullscreen_color_layer_fill(
        &self,
        node: u32,
        state: &SyntheticSpriteState,
        draw_w: u32,
        draw_h: u32,
        surface_w: u32,
        surface_h: u32,
    ) -> bool {
        if node == 0 || state.parent == 0 {
            return false;
        }
        if !Self::synthetic_matches_surface_dims(draw_w, draw_h, surface_w, surface_h) {
            return false;
        }
        if !state.fill_rgba_explicit || state.fill_rgba[3] == 0 {
            return false;
        }
        let parent_label = self.diag.object_labels.get(&state.parent).cloned().unwrap_or_default();

        let parent_has_textured_subtree = self.synthetic_node_or_descendant_has_fullscreen_texture(
            state.parent,
            surface_w,
            surface_h,
            0,
            node,
        );
        if self.active_profile().should_skip_fullscreen_bootstrap_fill(
            &parent_label,
            parent_has_textured_subtree,
        ) {
            return true;
        }

        let siblings = self.runtime.graphics
            .synthetic_sprites
            .get(&state.parent)
            .and_then(|parent_state| {
                if parent_state.children == 0 {
                    None
                } else {
                    self.runtime.graphics.synthetic_arrays.get(&parent_state.children).map(|arr| arr.items.clone())
                }
            })
            .unwrap_or_default();
        let mut sibling_has_textured_subtree = false;
        for sibling in siblings {
            if sibling == 0 || sibling == node {
                continue;
            }
            let Some(sibling_state) = self.runtime.graphics.synthetic_sprites.get(&sibling) else {
                continue;
            };
            if !sibling_state.visible {
                continue;
            }
            if self.synthetic_node_or_descendant_has_fullscreen_texture(
                sibling,
                surface_w,
                surface_h,
                0,
                node,
            ) {
                sibling_has_textured_subtree = true;
                break;
            }
        }
        self.active_profile().should_skip_fullscreen_bootstrap_fill(
            &parent_label,
            sibling_has_textured_subtree,
        )
    }

    fn begin_uigraphics_context(&mut self, width: u32, height: u32) -> u32 {
        let ctx = self.alloc_synthetic_ui_object(format!("UIGraphicsContext.synthetic#{}", self.runtime.ui_graphics.graphics_uikit_contexts_created));
        let size = width.max(1).saturating_mul(height.max(1)).saturating_mul(4) as usize;
        self.runtime.graphics.synthetic_bitmap_contexts.insert(ctx, SyntheticBitmapContext {
            width: width.max(1),
            height: height.max(1),
            rgba: vec![0u8; size],
            fill_rgba: [255, 255, 255, 255],
        });
        self.runtime.graphics.current_uigraphics_context = ctx;
        self.runtime.graphics.uigraphics_stack.push(ctx);
        self.runtime.ui_graphics.graphics_uikit_contexts_created = self.runtime.ui_graphics.graphics_uikit_contexts_created.saturating_add(1);
        self.runtime.ui_graphics.graphics_last_ui_source = Some("UIGraphicsBeginImageContext".to_string());
        ctx
    }

    fn push_uigraphics_context(&mut self, ctx: u32) {
        if ctx != 0 && self.runtime.graphics.synthetic_bitmap_contexts.contains_key(&ctx) {
            self.runtime.graphics.current_uigraphics_context = ctx;
            self.runtime.graphics.uigraphics_stack.push(ctx);
        }
    }

    fn pop_uigraphics_context(&mut self) -> u32 {
        let popped = self.runtime.graphics.uigraphics_stack.pop().unwrap_or(0);
        self.runtime.graphics.current_uigraphics_context = *self.runtime.graphics.uigraphics_stack.last().unwrap_or(&0);
        popped
    }

    fn set_bitmap_context_fill_rgba(&mut self, ctx: u32, rgba: [u8; 4]) -> bool {
        if let Some(bitmap) = self.runtime.graphics.synthetic_bitmap_contexts.get_mut(&ctx) {
            bitmap.fill_rgba = rgba;
            true
        } else {
            false
        }
    }

    fn fill_bitmap_context_rect(&mut self, ctx: u32, rect: (i32, i32, u32, u32)) -> bool {
        if let Some(bitmap) = self.runtime.graphics.synthetic_bitmap_contexts.get_mut(&ctx) {
            Self::ui_fill_rect_rgba(&mut bitmap.rgba, bitmap.width, bitmap.height, rect.0, rect.1, rect.2, rect.3, bitmap.fill_rgba);
            self.runtime.ui_graphics.graphics_uikit_draw_ops = self.runtime.ui_graphics.graphics_uikit_draw_ops.saturating_add(1);
            self.runtime.ui_graphics.graphics_last_ui_source = Some("CGContextFillRect".to_string());
            true
        } else {
            false
        }
    }

    fn clear_bitmap_context_rect(&mut self, ctx: u32, rect: (i32, i32, u32, u32)) -> bool {
        if let Some(bitmap) = self.runtime.graphics.synthetic_bitmap_contexts.get_mut(&ctx) {
            Self::ui_clear_rect_rgba(&mut bitmap.rgba, bitmap.width, bitmap.height, rect.0, rect.1, rect.2, rect.3);
            self.runtime.ui_graphics.graphics_uikit_draw_ops = self.runtime.ui_graphics.graphics_uikit_draw_ops.saturating_add(1);
            self.runtime.ui_graphics.graphics_last_ui_source = Some("CGContextClearRect".to_string());
            true
        } else {
            false
        }
    }

    fn create_image_from_context(&mut self, ctx: u32) -> Option<u32> {
        let bitmap = self.runtime.graphics.synthetic_bitmap_contexts.get(&ctx)?.clone();
        let image = self.alloc_synthetic_ui_object(format!("UIImage.synthetic#{}", self.runtime.ui_graphics.graphics_uikit_images_created));
        self.runtime.graphics.synthetic_images.insert(image, SyntheticImage { width: bitmap.width, height: bitmap.height, rgba: bitmap.rgba });
        self.runtime.graphics.last_uikit_image_object = image;
        self.runtime.ui_graphics.graphics_uikit_images_created = self.runtime.ui_graphics.graphics_uikit_images_created.saturating_add(1);
        self.runtime.ui_graphics.graphics_last_ui_source = Some("UIGraphicsGetImageFromCurrentImageContext".to_string());
        Some(image)
    }

    fn encode_synthetic_image_png(&self, image: u32) -> Option<Vec<u8>> {
        let image = self.runtime.graphics.synthetic_images.get(&image)?;
        Self::encode_rgba_png(&image.rgba, image.width, image.height).ok()
    }

    fn composite_image_into_context(&mut self, ctx: u32, image: u32, rect: (i32, i32, u32, u32)) -> bool {
        let image = match self.runtime.graphics.synthetic_images.get(&image).cloned() { Some(v) => v, None => return false };
        if let Some(bitmap) = self.runtime.graphics.synthetic_bitmap_contexts.get_mut(&ctx) {
            Self::composite_rgba_scaled_into(&mut bitmap.rgba, bitmap.width, bitmap.height, &image.rgba, image.width, image.height, rect.0, rect.1, rect.2, rect.3);
            self.runtime.ui_graphics.graphics_uikit_draw_ops = self.runtime.ui_graphics.graphics_uikit_draw_ops.saturating_add(1);
            self.runtime.ui_graphics.graphics_last_ui_source = Some("CGContextDrawImage".to_string());
            true
        } else {
            false
        }
    }

    fn composite_current_uikit_surface_to_framebuffer(&mut self, reason: &str) -> bool {
        self.bootstrap_synthetic_graphics();
        let maybe_ctx = if self.runtime.graphics.current_uigraphics_context != 0 && self.runtime.graphics.synthetic_bitmap_contexts.contains_key(&self.runtime.graphics.current_uigraphics_context) {
            Some((self.runtime.graphics.current_uigraphics_context, None))
        } else if self.runtime.graphics.last_uikit_image_object != 0 && self.runtime.graphics.synthetic_images.contains_key(&self.runtime.graphics.last_uikit_image_object) {
            Some((0, Some(self.runtime.graphics.last_uikit_image_object)))
        } else {
            None
        };
        let Some((ctx_obj, image_obj)) = maybe_ctx else { return false; };
        self.ensure_framebuffer_backing();
        if ctx_obj != 0 {
            let bitmap = match self.runtime.graphics.synthetic_bitmap_contexts.get(&ctx_obj).cloned() { Some(v) => v, None => return false };
            Self::composite_rgba_scaled_into(&mut self.runtime.graphics.synthetic_framebuffer, self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1), &bitmap.rgba, bitmap.width, bitmap.height, 0, 0, self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1));
        } else if let Some(image_obj) = image_obj {
            let image = match self.runtime.graphics.synthetic_images.get(&image_obj).cloned() { Some(v) => v, None => return false };
            Self::composite_rgba_scaled_into(&mut self.runtime.graphics.synthetic_framebuffer, self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1), &image.rgba, image.width, image.height, 0, 0, self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1));
        }
        self.runtime.ui_graphics.graphics_framebuffer_bytes = self.runtime.graphics.synthetic_framebuffer.len() as u32;
        self.runtime.ui_graphics.graphics_uikit_present_ops = self.runtime.ui_graphics.graphics_uikit_present_ops.saturating_add(1);
        self.runtime.ui_graphics.graphics_last_ui_source = Some(reason.to_string());
        self.runtime.graphics.uikit_framebuffer_dirty = true;
        true
    }

    fn read_fill_components_rgba(&self, ptr: u32) -> Option<[u8; 4]> {
        if ptr == 0 { return None; }
        let comps = [
            self.read_u32_le(ptr).ok()?,
            self.read_u32_le(ptr.wrapping_add(4)).ok()?,
            self.read_u32_le(ptr.wrapping_add(8)).ok()?,
            self.read_u32_le(ptr.wrapping_add(12)).ok().unwrap_or(0x3f800000),
        ];
        Some([
            Self::gl_float_to_u8(comps[0]),
            Self::gl_float_to_u8(comps[1]),
            Self::gl_float_to_u8(comps[2]),
            Self::gl_float_to_u8(comps[3]),
        ])
    }

    fn gl_float_to_u8(bits: u32) -> u8 {
        let f = f32::from_bits(bits);
        if !f.is_finite() {
            return 0;
        }
        let clamped = f.clamp(0.0, 1.0);
        (clamped * 255.0).round() as u8
    }

    fn maybe_sync_logical_surface_from_scene_root(&mut self, root: u32, source_note: &str) -> Option<String> {
        if root == 0 || !self.runtime.graphics.synthetic_sprites.contains_key(&root) {
            return None;
        }

        let old_w = self.runtime.ui_graphics.graphics_surface_width.max(1);
        let old_h = self.runtime.ui_graphics.graphics_surface_height.max(1);

        #[derive(Clone)]
        struct SurfaceCandidate {
            width: u32,
            height: u32,
            source: String,
            priority: u32,
        }

        let mut candidates: Vec<SurfaceCandidate> = Vec::new();

        if let Some((probe, probe_w, probe_h)) = self.synthetic_find_scene_projection_probe(root) {
            let width = probe_w.round().max(1.0) as u32;
            let height = probe_h.round().max(1.0) as u32;
            candidates.push(SurfaceCandidate {
                width,
                height,
                source: format!("scene-probe:{}:{}", self.describe_ptr(probe), source_note),
                priority: 300,
            });
        }

        let (layout_w, layout_h) = self.synthetic_node_layout_size(root);

        if let Some((child_w, child_h)) = self.synthetic_node_scene_canvas_from_children(root) {
            let child_w_u32 = child_w.round().max(1.0) as u32;
            let child_h_u32 = child_h.round().max(1.0) as u32;
            let layout_w_u32 = layout_w.round().max(1.0) as u32;
            let layout_h_u32 = layout_h.round().max(1.0) as u32;
            let root_looks_inflated = layout_w_u32 > child_w_u32.saturating_mul(5) / 4
                && (layout_h_u32 as i64 - child_h_u32 as i64).abs() <= ((child_h_u32.max(1) as i64) / 6).max(8)
                && layout_w_u32 >= old_w.saturating_mul(3) / 4;
            candidates.push(SurfaceCandidate {
                width: child_w_u32,
                height: child_h_u32,
                source: format!(
                    "scene-children:{}{}",
                    source_note,
                    if root_looks_inflated { ":inflated-root-canvas" } else { "" },
                ),
                priority: if root_looks_inflated { 360 } else { 240 },
            });
        }

        if layout_w > 0.0 && layout_h > 0.0 {
            candidates.push(SurfaceCandidate {
                width: layout_w.round().max(1.0) as u32,
                height: layout_h.round().max(1.0) as u32,
                source: format!("scene-layout:{}", source_note),
                priority: 180,
            });
        }

        let mut best: Option<(SurfaceCandidate, i64)> = None;
        for candidate in candidates.into_iter() {
            if candidate.width < 32 || candidate.height < 32 {
                continue;
            }
            if candidate.width > 4096 || candidate.height > 4096 {
                continue;
            }
            let area = candidate.width.saturating_mul(candidate.height);
            if area < 32 * 32 {
                continue;
            }
            let aspect_delta_direct = (candidate.width as i64 - old_w as i64).abs()
                + (candidate.height as i64 - old_h as i64).abs();
            let aspect_delta_swapped = (candidate.width as i64 - old_h as i64).abs()
                + (candidate.height as i64 - old_w as i64).abs();
            let orientation_bonus = if aspect_delta_swapped + 4 < aspect_delta_direct {
                48
            } else if aspect_delta_direct + 4 < aspect_delta_swapped {
                16
            } else {
                0
            };
            let fullscreen_bonus = if (candidate.width >= old_w.saturating_mul(3) / 4
                && candidate.height >= old_h.saturating_mul(3) / 4)
                || (candidate.width >= old_h.saturating_mul(3) / 4
                    && candidate.height >= old_w.saturating_mul(3) / 4)
            {
                32
            } else {
                0
            };
            let score = candidate.priority as i64
                + orientation_bonus
                + fullscreen_bonus
                - ((aspect_delta_direct.min(aspect_delta_swapped)) / 16);
            let replace = best.as_ref().map(|(_, best_score)| score > *best_score).unwrap_or(true);
            if replace {
                best = Some((candidate, score));
            }
        }

        let Some((chosen, _)) = best else {
            return None;
        };

        if chosen.width == old_w && chosen.height == old_h {
            return None;
        }

        self.runtime.ui_graphics.graphics_surface_width = chosen.width;
        self.runtime.ui_graphics.graphics_surface_height = chosen.height;

        let viewport_matches_previous_surface = self.runtime.ui_graphics.graphics_viewport_x == 0
            && self.runtime.ui_graphics.graphics_viewport_y == 0
            && self.runtime.ui_graphics.graphics_viewport_width == old_w
            && self.runtime.ui_graphics.graphics_viewport_height == old_h;
        if self.runtime.ui_graphics.graphics_viewport_width == 0
            || self.runtime.ui_graphics.graphics_viewport_height == 0
            || viewport_matches_previous_surface
        {
            self.runtime.ui_graphics.graphics_viewport_x = 0;
            self.runtime.ui_graphics.graphics_viewport_y = 0;
            self.runtime.ui_graphics.graphics_viewport_width = chosen.width;
            self.runtime.ui_graphics.graphics_viewport_height = chosen.height;
        }

        self.ensure_framebuffer_backing();
        let summary = format!(
            "logical-surface-sync root={} source={} {}x{} -> {}x{} ({})",
            self.describe_ptr(root),
            source_note,
            old_w,
            old_h,
            chosen.width,
            chosen.height,
            chosen.source,
        );
        self.diag.trace.push(format!("     ↳ {}", summary));
        Some(summary)
    }

    fn simulate_graphics_tick(&mut self) {
        self.bootstrap_synthetic_graphics();
        self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
        if !self.runtime.ui_graphics.graphics_layer_attached {
            self.runtime.ui_graphics.graphics_layer_attached = true;
            self.diag.trace.push(format!(
                "     ↳ hle CAEAGLLayer.attach layer={} window={} size={}x{} retainedBacking=NO color=RGBA8",
                self.describe_ptr(self.runtime.ui_graphics.eagl_layer),
                self.describe_ptr(self.runtime.ui_objects.window),
                self.runtime.ui_graphics.graphics_surface_width,
                self.runtime.ui_graphics.graphics_surface_height,
            ));
        } else if !self.runtime.ui_graphics.graphics_context_current {
            self.runtime.ui_graphics.graphics_context_current = true;
            self.diag.trace.push(format!(
                "     ↳ hle EAGLContext.setCurrentContext(ctx={}) -> YES api={}",
                self.describe_ptr(self.runtime.ui_graphics.eagl_context),
                self.graphics_api_name(),
            ));
        } else if !self.runtime.ui_graphics.graphics_surface_ready {
            self.runtime.ui_graphics.graphics_surface_ready = true;
            self.runtime.ui_graphics.graphics_framebuffer_complete = true;
            self.runtime.ui_graphics.graphics_viewport_ready = true;
            self.runtime.ui_graphics.graphics_viewport_x = 0;
            self.runtime.ui_graphics.graphics_viewport_y = 0;
            self.runtime.ui_graphics.graphics_viewport_width = self.runtime.ui_graphics.graphics_surface_width;
            self.runtime.ui_graphics.graphics_viewport_height = self.runtime.ui_graphics.graphics_surface_height;
            self.ensure_framebuffer_backing();
            self.diag.trace.push(format!(
                "     ↳ hle EAGLContext.renderbufferStorage(ctx={} drawable={} rb={} fb={}) -> allocated {}x{} complete=YES",
                self.describe_ptr(self.runtime.ui_graphics.eagl_context),
                self.describe_ptr(self.runtime.ui_graphics.eagl_layer),
                self.describe_ptr(self.runtime.ui_graphics.gl_renderbuffer),
                self.describe_ptr(self.runtime.ui_graphics.gl_framebuffer),
                self.runtime.ui_graphics.graphics_surface_width,
                self.runtime.ui_graphics.graphics_surface_height,
            ));
        } else {
            let scene_root = self.resolve_auto_scene_root();
            if let Some((root, source_note)) = scene_root.as_ref() {
                let _ = self.maybe_sync_logical_surface_from_scene_root(*root, source_note);
            }
            let guest_clear_only = self.runtime.graphics.guest_framebuffer_dirty && self.runtime.graphics.guest_draws_since_present == 0;
            let allow_auto_scene_visit = scene_root
                .as_ref()
                .map(|_| !self.runtime.graphics.guest_framebuffer_dirty || guest_clear_only || self.runtime.graphics.guest_draws_since_present <= 1)
                .unwrap_or(false);
            let mut auto_scene_draws = 0usize;
            let mut auto_scene_signature = None;
            let auto_scene_changed;
            let trace_frame = self.runtime.ui_graphics.graphics_frame_index.saturating_add(1);
            match scene_root.as_ref() {
                Some((root, source_note)) => {
                    let mut stats = AutoSceneVisitStats::default();
                    self.collect_auto_scene_visit_stats(*root, 0, &mut stats);
                    let signature = self.compute_auto_scene_signature(*root, 0);
                    auto_scene_signature = Some(signature);
                    auto_scene_changed = self
                        .runtime.scene.auto_scene_last_present_signature
                        .map(|previous| previous != signature)
                        .unwrap_or(true);
                    let bbox_area = self
                        .runtime.ui_graphics
                        .graphics_last_visible_bbox_width
                        .saturating_mul(self.runtime.ui_graphics.graphics_last_visible_bbox_height);
                    let surface_area = self
                        .runtime.ui_graphics
                        .graphics_surface_width
                        .max(1)
                        .saturating_mul(self.runtime.ui_graphics.graphics_surface_height.max(1));
                    let partial_retained_frame = bbox_area > 0
                        && (self.runtime.ui_graphics.graphics_last_visible_bbox_x > 8
                            || self.runtime.ui_graphics.graphics_last_visible_bbox_y > 8
                            || bbox_area.saturating_mul(100) < surface_area.saturating_mul(70));
                    let should_auto_visit_scene = allow_auto_scene_visit
                        && (auto_scene_changed
                            || partial_retained_frame
                            || guest_clear_only
                            || self.runtime.ui_graphics.graphics_present_calls == 0
                            || self.runtime.ui_graphics.graphics_frame_index == 0);
                    self.begin_scene_visit_observability_frame(
                        trace_frame,
                        *root,
                        source_note,
                        allow_auto_scene_visit,
                        should_auto_visit_scene,
                        partial_retained_frame,
                        guest_clear_only,
                        signature,
                    );
                    self.diag.trace.push(format!(
                        "     ↳ auto-scene probe root={} source={} allow={} sceneChanged={} partialRetained={} guestClearOnly={} sig=0x{:016x} guestDirty={} guestFrameDraws={} guestDraws={} nodes={} drawn={} enteredNO={} visibleNO={} containerSkip={} noTexture={} zeroSize={} missing={} maxDepth={}",
                        self.describe_ptr(*root),
                        source_note,
                        if should_auto_visit_scene { "YES" } else { "NO" },
                        if auto_scene_changed { "YES" } else { "NO" },
                        if partial_retained_frame { "YES" } else { "NO" },
                        if guest_clear_only { "YES" } else { "NO" },
                        signature,
                        if self.runtime.graphics.guest_framebuffer_dirty { "YES" } else { "NO" },
                        self.runtime.graphics.guest_draws_since_present,
                        self.runtime.ui_graphics.graphics_guest_draw_calls,
                        stats.nodes_seen,
                        stats.nodes_drawn,
                        stats.entered_no,
                        stats.visible_no,
                        stats.container_skip,
                        stats.no_texture,
                        stats.zero_size,
                        stats.missing,
                        stats.max_depth,
                    ));
                    let mut trace_budget = SceneVisitTraceBudget::new(SCENE_VISIT_TRACE_EVENT_LIMIT);
                    if should_auto_visit_scene {
                        self.ensure_framebuffer_backing();
                        self.runtime.graphics.synthetic_framebuffer.fill(0);
                        auto_scene_draws = self.visit_synthetic_node_recursive(*root, 0, &mut trace_budget);
                        self.push_graph_trace(format!(
                            "scene.visit.summary rev={} frame={} root={} draws={} traceEvents={} sceneChanged={} partialRetained={} source={} decision=visit sig=0x{:016x}",
                            SCENE_VISIT_OBSERVABILITY_REV,
                            trace_frame,
                            self.describe_ptr(*root),
                            auto_scene_draws,
                            trace_budget.emitted(),
                            if auto_scene_changed { "YES" } else { "NO" },
                            if partial_retained_frame { "YES" } else { "NO" },
                            source_note,
                            signature,
                        ));
                    } else {
                        if allow_auto_scene_visit && !auto_scene_changed {
                            self.diag.trace.push(format!(
                                "     ↳ auto-scene retained existing framebuffer root={} sig=0x{:016x}",
                                self.describe_ptr(*root),
                                signature,
                            ));
                        }
                        self.push_graph_trace(format!(
                            "scene.visit.summary rev={} frame={} root={} draws=0 traceEvents={} sceneChanged={} partialRetained={} source={} decision=skip sig=0x{:016x}",
                            SCENE_VISIT_OBSERVABILITY_REV,
                            trace_frame,
                            self.describe_ptr(*root),
                            trace_budget.emitted(),
                            if auto_scene_changed { "YES" } else { "NO" },
                            if partial_retained_frame { "YES" } else { "NO" },
                            source_note,
                            signature,
                        ));
                    }
                }
                None => {
                    auto_scene_changed = false;
                    let root_candidates = self
                        .runtime.graphics.synthetic_sprites
                        .iter()
                        .filter(|(_, state)| state.parent == 0)
                        .count();
                    self.begin_scene_visit_observability_frame(
                        trace_frame,
                        0,
                        "none",
                        allow_auto_scene_visit,
                        false,
                        false,
                        guest_clear_only,
                        0,
                    );
                    self.push_graph_trace(format!(
                        "scene.visit.summary rev={} frame={} root=nil draws=0 traceEvents=0 rootCandidates={} decision=no-root",
                        SCENE_VISIT_OBSERVABILITY_REV,
                        trace_frame,
                        root_candidates,
                    ));
                    self.diag.trace.push(format!(
                        "     ↳ auto-scene probe root=nil source=none allow={} sceneChanged=NO guestDirty={} guestFrameDraws={} guestDraws={} rootCandidates={}",
                        if allow_auto_scene_visit { "YES" } else { "NO" },
                        if self.runtime.graphics.guest_framebuffer_dirty { "YES" } else { "NO" },
                        self.runtime.graphics.guest_draws_since_present,
                        self.runtime.ui_graphics.graphics_guest_draw_calls,
                        root_candidates,
                    ));
                }
            }
            let dump_path = self.render_presented_frame("displaylink", auto_scene_draws);
            if auto_scene_draws > 0 {
                self.runtime.scene.auto_scene_last_present_signature = auto_scene_signature;
            }
            self.diag.trace.push(format!(
                "     ↳ hle GLES frame#{} source={} fb={} rb={} viewport={}x{} guestDraws={} guestVerts={} autoSceneDraws={}",
                self.runtime.ui_graphics.graphics_frame_index,
                self.runtime.ui_graphics.graphics_last_present_source.clone().unwrap_or_else(|| "unknown".to_string()),
                self.describe_ptr(self.runtime.ui_graphics.gl_framebuffer),
                self.describe_ptr(self.runtime.ui_graphics.gl_renderbuffer),
                self.runtime.ui_graphics.graphics_viewport_width,
                self.runtime.ui_graphics.graphics_viewport_height,
                self.runtime.ui_graphics.graphics_guest_draw_calls,
                self.runtime.ui_graphics.graphics_guest_vertex_fetches,
                auto_scene_draws,
            ));
            if let Some(decision) = self.runtime.ui_graphics.graphics_last_present_decision.clone() {
                self.diag.trace.push(format!("     ↳ hle present decision {}", decision));
            }
            self.diag.trace.push(format!(
                "     ↳ hle EAGLContext.presentRenderbuffer(ctx={} rb={}) -> YES frame#{} readbackReady=YES rbBytes={} checksum=0x{:08x}{}",
                self.describe_ptr(self.runtime.ui_graphics.eagl_context),
                self.describe_ptr(self.runtime.ui_graphics.gl_renderbuffer),
                self.runtime.ui_graphics.graphics_frame_index,
                self.runtime.ui_graphics.graphics_last_readback_bytes,
                self.runtime.ui_graphics.graphics_last_readback_checksum,
                dump_path.as_ref().map(|p| format!(" dump={}", p)).unwrap_or_default(),
            ));
        }
        self.refresh_graphics_object_labels();
    }

}
