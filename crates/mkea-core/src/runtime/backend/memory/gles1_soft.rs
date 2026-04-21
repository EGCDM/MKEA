const SCENE_VISIT_OBSERVABILITY_REV: &str = "scene-visit-sprite-quad-v2";
const SCENE_VISIT_TRACE_EVENT_LIMIT: u32 = 40;

#[derive(Debug, Clone, Copy)]
struct SceneVisitTraceBudget {
    emitted: u32,
    limit: u32,
}

impl SceneVisitTraceBudget {
    fn new(limit: u32) -> Self {
        Self { emitted: 0, limit }
    }

    fn try_take(&mut self) -> bool {
        if self.emitted >= self.limit {
            return false;
        }
        self.emitted = self.emitted.saturating_add(1);
        true
    }

    fn emitted(&self) -> u32 {
        self.emitted
    }
}

impl MemoryArm32Backend {
// Software GLES1 rasterization, presented-frame diagnostics, and readback helpers.

    fn decode_cccolor4b(arg: u32) -> [u8; 4] {
        [
            (arg & 0xff) as u8,
            ((arg >> 8) & 0xff) as u8,
            ((arg >> 16) & 0xff) as u8,
            ((arg >> 24) & 0xff) as u8,
        ]
    }


    fn gl_matrix_stack_ref(&self, mode: GraphicsMatrixMode) -> &Vec<[f32; 16]> {
        match mode {
            GraphicsMatrixMode::ModelView => &self.runtime.ui_graphics.graphics_matrices.modelview_stack,
            GraphicsMatrixMode::Projection => &self.runtime.ui_graphics.graphics_matrices.projection_stack,
            GraphicsMatrixMode::Texture => &self.runtime.ui_graphics.graphics_matrices.texture_stack,
        }
    }

    fn gl_matrix_stack_mut(&mut self, mode: GraphicsMatrixMode) -> &mut Vec<[f32; 16]> {
        match mode {
            GraphicsMatrixMode::ModelView => &mut self.runtime.ui_graphics.graphics_matrices.modelview_stack,
            GraphicsMatrixMode::Projection => &mut self.runtime.ui_graphics.graphics_matrices.projection_stack,
            GraphicsMatrixMode::Texture => &mut self.runtime.ui_graphics.graphics_matrices.texture_stack,
        }
    }

    fn gl_current_matrix(&self, mode: GraphicsMatrixMode) -> [f32; 16] {
        self.gl_matrix_stack_ref(mode)
            .last()
            .copied()
            .unwrap_or_else(gl_identity_mat4)
    }

    fn gl_set_current_matrix(&mut self, mode: GraphicsMatrixMode, matrix: [f32; 16]) {
        let stack = self.gl_matrix_stack_mut(mode);
        if stack.is_empty() {
            stack.push(gl_identity_mat4());
        }
        if let Some(top) = stack.last_mut() {
            *top = matrix;
        }
        self.mark_gl_matrix_touched(mode);
    }

    fn mark_gl_matrix_touched(&mut self, mode: GraphicsMatrixMode) {
        let matrices = &mut self.runtime.ui_graphics.graphics_matrices;
        matrices.op_count = matrices.op_count.saturating_add(1);
        match mode {
            GraphicsMatrixMode::ModelView => matrices.modelview_touched = true,
            GraphicsMatrixMode::Projection => matrices.projection_touched = true,
            GraphicsMatrixMode::Texture => matrices.texture_touched = true,
        }
    }

    fn gl_mat4_mul(lhs: [f32; 16], rhs: [f32; 16]) -> [f32; 16] {
        let mut out = [0.0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                out[col * 4 + row] = lhs[0 * 4 + row] * rhs[col * 4 + 0]
                    + lhs[1 * 4 + row] * rhs[col * 4 + 1]
                    + lhs[2 * 4 + row] * rhs[col * 4 + 2]
                    + lhs[3 * 4 + row] * rhs[col * 4 + 3];
            }
        }
        out
    }

    fn gl_mat4_transform(mat: [f32; 16], v: [f32; 4]) -> [f32; 4] {
        [
            mat[0] * v[0] + mat[4] * v[1] + mat[8] * v[2] + mat[12] * v[3],
            mat[1] * v[0] + mat[5] * v[1] + mat[9] * v[2] + mat[13] * v[3],
            mat[2] * v[0] + mat[6] * v[1] + mat[10] * v[2] + mat[14] * v[3],
            mat[3] * v[0] + mat[7] * v[1] + mat[11] * v[2] + mat[15] * v[3],
        ]
    }

    fn gl_mat4_translate(tx: f32, ty: f32, tz: f32) -> [f32; 16] {
        let mut out = gl_identity_mat4();
        out[12] = tx;
        out[13] = ty;
        out[14] = tz;
        out
    }

    fn gl_mat4_scale(sx: f32, sy: f32, sz: f32) -> [f32; 16] {
        let mut out = gl_identity_mat4();
        out[0] = sx;
        out[5] = sy;
        out[10] = sz;
        out
    }

    fn gl_mat4_rotate(angle_deg: f32, x: f32, y: f32, z: f32) -> [f32; 16] {
        let len = (x * x + y * y + z * z).sqrt();
        if !len.is_finite() || len <= 1.0e-6 || !angle_deg.is_finite() {
            return gl_identity_mat4();
        }
        let x = x / len;
        let y = y / len;
        let z = z / len;
        let radians = angle_deg.to_radians();
        let s = radians.sin();
        let c = radians.cos();
        let one_minus_c = 1.0 - c;
        [
            x * x * one_minus_c + c,
            y * x * one_minus_c + z * s,
            z * x * one_minus_c - y * s,
            0.0,
            x * y * one_minus_c - z * s,
            y * y * one_minus_c + c,
            z * y * one_minus_c + x * s,
            0.0,
            x * z * one_minus_c + y * s,
            y * z * one_minus_c - x * s,
            z * z * one_minus_c + c,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ]
    }

    fn gl_mat4_ortho(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Option<[f32; 16]> {
        let rl = right - left;
        let tb = top - bottom;
        let fn_span = far - near;
        if rl.abs() <= 1.0e-6 || tb.abs() <= 1.0e-6 || fn_span.abs() <= 1.0e-6 {
            return None;
        }
        Some([
            2.0 / rl,
            0.0,
            0.0,
            0.0,
            0.0,
            2.0 / tb,
            0.0,
            0.0,
            0.0,
            0.0,
            -2.0 / fn_span,
            0.0,
            -(right + left) / rl,
            -(top + bottom) / tb,
            -(far + near) / fn_span,
            1.0,
        ])
    }

    fn gl_mat4_frustum(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Option<[f32; 16]> {
        let rl = right - left;
        let tb = top - bottom;
        let fn_span = far - near;
        if rl.abs() <= 1.0e-6 || tb.abs() <= 1.0e-6 || fn_span.abs() <= 1.0e-6 || near.abs() <= 1.0e-6 {
            return None;
        }
        Some([
            (2.0 * near) / rl,
            0.0,
            0.0,
            0.0,
            0.0,
            (2.0 * near) / tb,
            0.0,
            0.0,
            (right + left) / rl,
            (top + bottom) / tb,
            -(far + near) / fn_span,
            -1.0,
            0.0,
            0.0,
            -(2.0 * far * near) / fn_span,
            0.0,
        ])
    }

    fn gl_read_call_arg_f32(&self, index: u32) -> f32 {
        let bits = match index {
            0 => self.cpu.regs[0],
            1 => self.cpu.regs[1],
            2 => self.cpu.regs[2],
            3 => self.cpu.regs[3],
            other => self.peek_stack_u32(other.saturating_sub(4)).unwrap_or(0),
        };
        Self::f32_from_bits(bits)
    }

    fn gl_read_guest_matrix_f32(&self, ptr: u32) -> Option<[f32; 16]> {
        if ptr == 0 {
            return None;
        }
        let mut out = [0.0f32; 16];
        for (idx, slot) in out.iter_mut().enumerate() {
            let bits = self.read_u32_le(ptr.wrapping_add((idx as u32).saturating_mul(4))).ok()?;
            *slot = Self::f32_from_bits(bits);
        }
        Some(out)
    }

    fn gl_has_active_transform_pipeline(&self) -> bool {
        let matrices = &self.runtime.ui_graphics.graphics_matrices;
        matrices.modelview_touched || matrices.projection_touched
    }

    fn synthetic_affine_identity() -> [f32; 6] {
        [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
    }

    fn synthetic_affine_mul(lhs: [f32; 6], rhs: [f32; 6]) -> [f32; 6] {
        [
            lhs[0] * rhs[0] + lhs[2] * rhs[1],
            lhs[1] * rhs[0] + lhs[3] * rhs[1],
            lhs[0] * rhs[2] + lhs[2] * rhs[3],
            lhs[1] * rhs[2] + lhs[3] * rhs[3],
            lhs[0] * rhs[4] + lhs[2] * rhs[5] + lhs[4],
            lhs[1] * rhs[4] + lhs[3] * rhs[5] + lhs[5],
        ]
    }

    fn synthetic_affine_translate(tx: f32, ty: f32) -> [f32; 6] {
        [1.0, 0.0, 0.0, 1.0, tx, ty]
    }

    fn synthetic_affine_scale(sx: f32, sy: f32) -> [f32; 6] {
        [sx, 0.0, 0.0, sy, 0.0, 0.0]
    }

    fn synthetic_affine_rotate(angle_deg: f32) -> [f32; 6] {
        if !angle_deg.is_finite() || angle_deg.abs() <= 1.0e-6 {
            return Self::synthetic_affine_identity();
        }
        let radians = angle_deg.to_radians();
        let s = radians.sin();
        let c = radians.cos();
        [c, s, -s, c, 0.0, 0.0]
    }

    fn synthetic_affine_skew(skew_x_deg: f32, skew_y_deg: f32) -> [f32; 6] {
        if (!skew_x_deg.is_finite() || skew_x_deg.abs() <= 1.0e-6)
            && (!skew_y_deg.is_finite() || skew_y_deg.abs() <= 1.0e-6)
        {
            return Self::synthetic_affine_identity();
        }
        let skew_x = if skew_x_deg.is_finite() {
            skew_x_deg.to_radians().tan()
        } else {
            0.0
        };
        let skew_y = if skew_y_deg.is_finite() {
            skew_y_deg.to_radians().tan()
        } else {
            0.0
        };
        [1.0, skew_y, skew_x, 1.0, 0.0, 0.0]
    }

    fn synthetic_affine_transform_point(transform: [f32; 6], x: f32, y: f32) -> (f32, f32) {
        (
            transform[0] * x + transform[2] * y + transform[4],
            transform[1] * x + transform[3] * y + transform[5],
        )
    }

    fn synthetic_local_scale(state: &SyntheticSpriteState) -> (f32, f32) {
        let mut scale_x = if state.scale_explicit && state.scale_x_bits != 0 {
            Self::f32_from_bits(state.scale_x_bits)
        } else {
            1.0
        };
        let mut scale_y = if state.scale_explicit && state.scale_y_bits != 0 {
            Self::f32_from_bits(state.scale_y_bits)
        } else {
            1.0
        };
        if !scale_x.is_finite() || scale_x.abs() < 1.0e-6 {
            scale_x = 1.0;
        }
        if !scale_y.is_finite() || scale_y.abs() < 1.0e-6 {
            scale_y = 1.0;
        }
        (scale_x, scale_y)
    }

    fn synthetic_node_debug_rgba(node: u32, label: &str, reason: &str) -> [u8; 4] {
        let mut seed = node ^ 0x5a17_9b3d;
        for byte in label.as_bytes().iter().chain(reason.as_bytes()) {
            seed = seed.rotate_left(5) ^ u32::from(*byte);
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        }
        [
            64u8.saturating_add(((seed >> 16) & 0x7f) as u8),
            64u8.saturating_add(((seed >> 8) & 0x7f) as u8),
            64u8.saturating_add((seed & 0x7f) as u8),
            208,
        ]
    }

    fn synthetic_label_is_scene_container(label: &str) -> bool {
        label.contains("CCScene")
            || label.contains("Scene")
            || label.contains("MenuLayer")
            || label.contains("CCLayer")
            || label.contains("FirstScene")
            || label.contains("CCMultiplexLayer")
            || Self::is_transition_like_label(label)
    }

    fn synthetic_node_scene_canvas_candidate(
        &self,
        child: u32,
        surface_w: f32,
        surface_h: f32,
    ) -> Option<(f32, f32, f32)> {
        let child_state = self.runtime.graphics.synthetic_sprites.get(&child)?;
        let child_label = self.diag.object_labels.get(&child).cloned().unwrap_or_default();
        let mut child_w = child_state.width as f32;
        let mut child_h = child_state.height as f32;
        let child_texture = self.synthetic_node_effective_texture(child);
        if child_texture != 0 {
            if let Some(texture) = self.runtime.graphics.synthetic_textures.get(&child_texture) {
                if child_w <= 0.0 {
                    child_w = texture.width as f32;
                }
                if child_h <= 0.0 {
                    child_h = texture.height as f32;
                }
            }
        }
        if child_w <= 0.0 || child_h <= 0.0 {
            if Self::synthetic_label_is_scene_container(&child_label) {
                if child_w <= 0.0 {
                    child_w = surface_w;
                }
                if child_h <= 0.0 {
                    child_h = surface_h;
                }
            }
        }
        if child_w <= 0.0 || child_h <= 0.0 {
            return None;
        }
        let (child_pos_x, child_pos_y, _) = self.synthetic_node_effective_position_for_content(child, child_w, child_h);
        let child_anchor_x = if child_state.anchor_explicit {
            Self::f32_from_bits(child_state.anchor_x_bits)
        } else if child_label.contains("CCSprite") || child_label.contains("CCMenuItem") {
            0.5
        } else {
            0.0
        };
        let child_anchor_y = if child_state.anchor_explicit {
            Self::f32_from_bits(child_state.anchor_y_bits)
        } else if child_label.contains("CCSprite") || child_label.contains("CCMenuItem") {
            0.5
        } else {
            0.0
        };
        let (child_scale_x, child_scale_y) = Self::synthetic_local_scale(child_state);
        child_w *= child_scale_x.abs();
        child_h *= child_scale_y.abs();
        let child_zeroish_pos = child_pos_x.abs() <= 1.0 && child_pos_y.abs() <= 1.0;
        let child_centered_anchor = (child_anchor_x - 0.5).abs() <= 0.02
            && (child_anchor_y - 0.5).abs() <= 0.02;
        let child_centered_pos = (child_pos_x - surface_w * 0.5).abs() <= 2.0
            && (child_pos_y - surface_h * 0.5).abs() <= 2.0;
        let child_origin_canvas_like = child_centered_anchor
            && (child_pos_x - child_w * 0.5).abs() <= 2.0
            && (child_pos_y - child_h * 0.5).abs() <= 2.0
            && (child_w >= surface_w * 0.45 || child_h >= surface_h * 0.75);
        if child_origin_canvas_like {
            let area = child_w * child_h;
            return Some((child_w, child_h, area + surface_w * surface_h));
        }
        let child_fullscreenish = (child_w >= surface_w * 0.75 && child_h >= surface_h * 0.75)
            || (child_w >= surface_h * 0.75 && child_h >= surface_w * 0.75);
        if child_fullscreenish
            && (child_zeroish_pos
                || child_centered_pos
                || (child_centered_anchor && child_centered_pos))
        {
            let area = child_w * child_h;
            return Some((child_w, child_h, area));
        }
        None
    }

    fn synthetic_node_scene_canvas_from_children(&self, node: u32) -> Option<(f32, f32)> {
        let state = self.runtime.graphics.synthetic_sprites.get(&node)?;
        if state.children == 0 {
            return None;
        }
        let children = self.runtime.graphics.synthetic_arrays.get(&state.children)?;
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1) as f32;
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
        let mut best: Option<(f32, f32, f32)> = None;
        for child in children.items.iter().copied() {
            if child == 0 {
                continue;
            }
            let Some((w, h, area)) = self.synthetic_node_scene_canvas_candidate(child, surface_w, surface_h) else {
                continue;
            };
            let replace = best.map(|(_, _, best_area)| area > best_area).unwrap_or(true);
            if replace {
                best = Some((w, h, area));
            }
        }
        best.map(|(w, h, _)| (w, h))
    }

    fn synthetic_node_has_fullscreenish_child(&self, node: u32) -> bool {
        self.synthetic_node_scene_canvas_from_children(node).is_some()
    }


    fn synthetic_effective_relative_anchor_point(
        &self,
        state: &SyntheticSpriteState,
        label: &str,
    ) -> bool {
        if state.relative_anchor_point_explicit {
            return state.relative_anchor_point;
        }
        if Self::is_label_class_name(label)
            || label.contains("CCSprite")
            || Self::is_menu_item_class_name(label)
        {
            return true;
        }
        state.relative_anchor_point
    }

    fn synthetic_node_has_neutral_container_transform(&self, node: u32) -> bool {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return false;
        };
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let parent = state.parent;
        let parent_label = if parent != 0 {
            self.diag.object_labels.get(&parent).cloned().unwrap_or_default()
        } else {
            String::new()
        };
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1) as f32;
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
        let (local_scale_x, local_scale_y) = Self::synthetic_local_scale(state);
        let width = state.width.max(1) as f32 * local_scale_x.abs();
        let height = state.height.max(1) as f32 * local_scale_y.abs();
        let (pos_x, pos_y, _) = self.synthetic_node_effective_position_for_content(node, state.width.max(1) as f32, state.height.max(1) as f32);
        let anchor_x = if state.anchor_explicit {
            Self::f32_from_bits(state.anchor_x_bits)
        } else if label.contains("CCSprite") || label.contains("CCMenuItem") {
            0.5
        } else {
            0.0
        };
        let anchor_y = if state.anchor_explicit {
            Self::f32_from_bits(state.anchor_y_bits)
        } else if label.contains("CCSprite") || label.contains("CCMenuItem") {
            0.5
        } else {
            0.0
        };
        let zeroish_pos = pos_x.abs() <= 1.0 && pos_y.abs() <= 1.0;
        let centered_anchor = (anchor_x - 0.5).abs() <= 0.02 && (anchor_y - 0.5).abs() <= 0.02;
        let centered_pos = (pos_x - surface_w * 0.5).abs() <= 2.0 && (pos_y - surface_h * 0.5).abs() <= 2.0;
        let fullscreenish = (width >= surface_w * 0.75 && height >= surface_h * 0.75)
            || (width >= surface_h * 0.75 && height >= surface_w * 0.75);
        let scene_like = label.contains("CCScene")
            || label.contains("Scene")
            || label.contains("MenuLayer")
            || label.contains("CCLayer")
            || label.contains("CCColorLayer")
            || label.contains("CCMultiplexLayer")
            || label.contains("SplashScreens")
            || Self::is_transition_like_label(&label);
        if scene_like && fullscreenish && (zeroish_pos || (centered_anchor && centered_pos)) {
            return true;
        }

        let parent_scene_like = parent_label.contains("CCScene")
            || parent_label.contains("Scene")
            || parent_label.contains("FirstScene")
            || parent_label.contains("MenuLayer");
        let menu_container = label.contains("CCMenu") && !label.contains("CCMenuItem");
        let menu_wrapper = menu_container
            && parent_scene_like
            && self.synthetic_node_has_fullscreenish_child(node)
            && (zeroish_pos || centered_pos || (centered_anchor && centered_pos));
        let hard_wrapper = label.contains("FirstScene")
            || label.contains("MenuLayer")
            || Self::is_transition_like_label(&label)
            || (label.contains("CCMultiplexLayer")
                || label.contains("CCColorLayer")
                || label.contains("CCLayer"))
                && parent_scene_like;
        if menu_wrapper || (hard_wrapper && fullscreenish && (zeroish_pos || centered_pos)) {
            return true;
        }

        if Self::is_transition_like_label(&label) {
            let destination = self.runtime.graphics.synthetic_splash_destinations.get(&node).copied().unwrap_or(0);
            if destination != 0 {
                return true;
            }
            if state.children != 0 {
                if let Some(children) = self.runtime.graphics.synthetic_arrays.get(&state.children) {
                    if children.items.iter().copied().any(|child| {
                        child != 0 && (child == self.runtime.ui_cocos.running_scene
                            || self.diag.object_labels
                                .get(&child)
                                .map(|child_label| {
                                    child_label.contains("CCScene")
                                        || child_label.contains("FirstScene")
                                        || child_label.contains("Scene.instance")
                                        || child_label.contains("Scene.synthetic")
                                })
                                .unwrap_or(false))
                    }) {
                        return true;
                    }
                }
            }
        }

        false
    }

    fn synthetic_node_layout_size(&self, node: u32) -> (f32, f32) {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return (0.0, 0.0);
        };
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let mut width = state.width as f32;
        let mut height = state.height as f32;
        let effective_texture = self.synthetic_node_effective_texture(node);
        if effective_texture != 0 {
            if let Some(texture) = self.runtime.graphics.synthetic_textures.get(&effective_texture) {
                if width <= 0.0 {
                    width = texture.width as f32;
                }
                if height <= 0.0 {
                    height = texture.height as f32;
                }
            }
        }
        if (width <= 0.0 || height <= 0.0) && self.string_backing(node).is_some() {
            let text = self.string_backing(node).map(|entry| entry.text.clone()).unwrap_or_default();
            let scale = Self::synthetic_text_scale_for_height(height.max(14.0) as u32);
            let (text_w, text_h) = Self::synthetic_text_dimensions_5x7(&text, scale);
            if width <= 0.0 {
                width = text_w.max(1) as f32;
            }
            if height <= 0.0 {
                height = text_h.max(1) as f32;
            }
        }
        let scene_like = Self::synthetic_label_is_scene_container(&label);
        if scene_like && effective_texture == 0 {
            if let Some((child_w, child_h)) = self.synthetic_node_scene_canvas_from_children(node) {
                let parent_portraitish = width > 0.0 && height > 0.0 && height > width * 1.25;
                let child_landscapeish = child_w > child_h * 1.10;
                if width <= 0.0
                    || height <= 0.0
                    || (parent_portraitish && child_landscapeish)
                {
                    width = child_w;
                    height = child_h;
                }
            }
        }
        if width <= 0.0 || height <= 0.0 {
            if scene_like {
                if width <= 0.0 {
                    width = self.runtime.ui_graphics.graphics_surface_width.max(1) as f32;
                }
                if height <= 0.0 {
                    height = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
                }
            }
        }
        (width.max(0.0), height.max(0.0))
    }

    fn synthetic_node_content_size_for_layout(&self, node: u32, layout_w: u32, layout_h: u32) -> (f32, f32, &'static str) {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return (layout_w as f32, layout_h as f32, "layout");
        };
        if state.untrimmed_explicit {
            let w = Self::f32_from_bits(state.untrimmed_w_bits).max(0.0);
            let h = Self::f32_from_bits(state.untrimmed_h_bits).max(0.0);
            if w > 0.0 || h > 0.0 {
                return (
                    if w > 0.0 { w } else { layout_w as f32 },
                    if h > 0.0 { h } else { layout_h as f32 },
                    "untrimmed",
                );
            }
        }
        (layout_w as f32, layout_h as f32, "layout")
    }

    fn synthetic_node_anchor_pixels_for_content(
        &self,
        node: u32,
        content_w: f32,
        content_h: f32,
    ) -> (f32, f32, &'static str) {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return (0.0, 0.0, "none");
        };
        if state.anchor_pixels_explicit {
            return (
                Self::f32_from_bits(state.anchor_pixels_x_bits),
                Self::f32_from_bits(state.anchor_pixels_y_bits),
                "anchorPointInPixels",
            );
        }
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let default_anchor = Self::synthetic_default_anchor(&label);
        let anchor_x = if state.anchor_explicit { Self::f32_from_bits(state.anchor_x_bits) } else { default_anchor };
        let anchor_y = if state.anchor_explicit { Self::f32_from_bits(state.anchor_y_bits) } else { default_anchor };
        (anchor_x * content_w, anchor_y * content_h, "anchor*content")
    }

    fn synthetic_node_effective_position_for_content(
        &self,
        node: u32,
        content_w: f32,
        content_h: f32,
    ) -> (f32, f32, &'static str) {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return (0.0, 0.0, "none");
        };
        if state.position_bl_explicit {
            let bl_x = Self::f32_from_bits(state.position_bl_x_bits);
            let bl_y = Self::f32_from_bits(state.position_bl_y_bits);
            let (anchor_px_x, anchor_px_y, _) = self.synthetic_node_anchor_pixels_for_content(node, content_w, content_h);
            return (bl_x + anchor_px_x, bl_y + anchor_px_y, "positionBL+anchor");
        }
        (
            Self::f32_from_bits(state.position_x_bits),
            Self::f32_from_bits(state.position_y_bits),
            "position",
        )
    }

    fn synthetic_node_content_local_quad(
        &self,
        node: u32,
        layout_w: u32,
        layout_h: u32,
    ) -> ([(f32, f32); 4], &'static str) {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return (
                [
                    (0.0, 0.0),
                    (layout_w as f32, 0.0),
                    (0.0, layout_h as f32),
                    (layout_w as f32, layout_h as f32),
                ],
                "layout-rect",
            );
        };
        let (content_w, content_h, _) = self.synthetic_node_content_size_for_layout(node, layout_w, layout_h);
        let sprite_w = if state.texture_rect_explicit {
            Self::f32_from_bits(state.texture_rect_w_bits).max(0.0)
        } else {
            layout_w as f32
        };
        let sprite_h = if state.texture_rect_explicit {
            Self::f32_from_bits(state.texture_rect_h_bits).max(0.0)
        } else {
            layout_h as f32
        };
        // CCSpriteFrame offsets are authored relative to the center of the
        // untrimmed/original content box, not as an absolute bottom-left
        // origin. So when a trimmed atlas rect is rendered into a larger
        // logical content size, we first re-center the rect inside that box
        // and only then apply the explicit offset.
        let centered_left = (content_w - sprite_w) * 0.5;
        let centered_bottom = (content_h - sprite_h) * 0.5;
        let left = centered_left + if state.offset_explicit {
            Self::f32_from_bits(state.offset_x_bits)
        } else {
            0.0
        };
        let bottom = centered_bottom + if state.offset_explicit {
            Self::f32_from_bits(state.offset_y_bits)
        } else {
            0.0
        };
        let right = left + sprite_w;
        let top = bottom + sprite_h;
        (
            [(left, bottom), (right, bottom), (left, top), (right, top)],
            if (content_w - sprite_w).abs() > 0.001 || (content_h - sprite_h).abs() > 0.001 {
                if state.offset_explicit {
                    "sprite-local-quad(center+offset)"
                } else {
                    "sprite-local-quad(centered)"
                }
            } else if state.offset_explicit {
                "sprite-local-quad(offset)"
            } else if state.texture_rect_explicit {
                "sprite-local-quad(rect)"
            } else {
                "layout-rect"
            },
        )
    }

    fn synthetic_node_local_affine(&self, node: u32) -> [f32; 6] {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return Self::synthetic_affine_identity();
        };
        let (layout_w, layout_h) = self.synthetic_node_layout_size(node);
        let layout_w_u32 = layout_w.max(0.0).round() as u32;
        let layout_h_u32 = layout_h.max(0.0).round() as u32;
        let (content_w, content_h, _) = self.synthetic_node_content_size_for_layout(node, layout_w_u32, layout_h_u32);
        let (anchor_px_x, anchor_px_y, _) = self.synthetic_node_anchor_pixels_for_content(node, content_w, content_h);
        let (mut pos_x, mut pos_y, _) = self.synthetic_node_effective_position_for_content(node, content_w, content_h);
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let relative_anchor_point = self.synthetic_effective_relative_anchor_point(state, &label);
        if !relative_anchor_point {
            pos_x += anchor_px_x;
            pos_y += anchor_px_y;
        }
        let (scale_x, scale_y) = Self::synthetic_local_scale(state);
        let rotation_deg = 0.0f32;
        let skew_x_deg = 0.0f32;
        let skew_y_deg = 0.0f32;
        Self::synthetic_affine_mul(
            Self::synthetic_affine_translate(pos_x, pos_y),
            Self::synthetic_affine_mul(
                Self::synthetic_affine_rotate(rotation_deg),
                Self::synthetic_affine_mul(
                    Self::synthetic_affine_skew(skew_x_deg, skew_y_deg),
                    Self::synthetic_affine_mul(
                        Self::synthetic_affine_scale(scale_x, scale_y),
                        Self::synthetic_affine_translate(-anchor_px_x, -anchor_px_y),
                    ),
                ),
            ),
        )
    }

    fn compute_synthetic_node_world_affine(&self, node: u32) -> [f32; 6] {
        let mut chain: Vec<u32> = Vec::new();
        let mut current = node;
        let mut depth = 0u32;
        while current != 0 && depth < 32 {
            chain.push(current);
            current = self
                .runtime
                .graphics
                .synthetic_sprites
                .get(&current)
                .map(|state| state.parent)
                .unwrap_or(0);
            depth = depth.saturating_add(1);
        }
        chain.reverse();

        let mut world = Self::synthetic_affine_identity();
        for (idx, current) in chain.iter().copied().enumerate() {
            let is_leaf = idx + 1 == chain.len();
            let local = if !is_leaf && self.synthetic_node_has_neutral_container_transform(current) {
                Self::synthetic_affine_identity()
            } else {
                self.synthetic_node_local_affine(current)
            };
            world = Self::synthetic_affine_mul(world, local);
        }
        world
    }

    fn compute_synthetic_node_world_transform(&self, node: u32) -> (f32, f32, f32, f32) {
        let world = self.compute_synthetic_node_world_affine(node);
        let origin = Self::synthetic_affine_transform_point(world, 0.0, 0.0);
        let x_axis = Self::synthetic_affine_transform_point(world, 1.0, 0.0);
        let y_axis = Self::synthetic_affine_transform_point(world, 0.0, 1.0);
        let scale_x = ((x_axis.0 - origin.0).powi(2) + (x_axis.1 - origin.1).powi(2)).sqrt();
        let scale_y = ((y_axis.0 - origin.0).powi(2) + (y_axis.1 - origin.1).powi(2)).sqrt();
        (origin.0, origin.1, scale_x, scale_y)
    }

    fn compute_synthetic_node_world_position(&self, node: u32) -> (f32, f32) {
        let (world_x, world_y, _, _) = self.compute_synthetic_node_world_transform(node);
        (world_x, world_y)
    }

    fn synthetic_project_world_point_via_gl(
        &self,
        world_x: f32,
        world_y: f32,
        world_z: f32,
    ) -> Option<(f32, f32)> {
        if !self.gl_has_active_transform_pipeline() {
            return None;
        }
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
        let viewport_x = self.runtime.ui_graphics.graphics_viewport_x as f32;
        let viewport_y = self.runtime.ui_graphics.graphics_viewport_y as f32;
        let viewport_w = self
            .runtime
            .ui_graphics
            .graphics_viewport_width
            .max(self.runtime.ui_graphics.graphics_surface_width)
            .max(1) as f32;
        let viewport_h = self
            .runtime
            .ui_graphics
            .graphics_viewport_height
            .max(self.runtime.ui_graphics.graphics_surface_height)
            .max(1) as f32;

        let model = self.gl_current_matrix(GraphicsMatrixMode::ModelView);
        let proj = self.gl_current_matrix(GraphicsMatrixMode::Projection);
        let mv = Self::gl_mat4_transform(model, [world_x, world_y, world_z, 1.0]);
        let clip = Self::gl_mat4_transform(proj, mv);
        if !clip[3].is_finite() || clip[3].abs() <= 1.0e-6 {
            return None;
        }
        let inv_w = 1.0 / clip[3];
        let ndc_x = clip[0] * inv_w;
        let ndc_y = clip[1] * inv_w;
        if !ndc_x.is_finite() || !ndc_y.is_finite() {
            return None;
        }
        let gl_x = viewport_x + ((ndc_x + 1.0) * 0.5) * viewport_w;
        let gl_y = viewport_y + ((ndc_y + 1.0) * 0.5) * viewport_h;
        let sx = gl_x;
        let sy = surface_h - gl_y;
        if !sx.is_finite() || !sy.is_finite() {
            return None;
        }
        Some((sx, sy))
    }

    fn synthetic_world_to_surface_point_direct(&self, world_x: f32, world_y: f32) -> (f32, f32) {
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
        let vp_x = self.runtime.ui_graphics.graphics_viewport_x as f32;
        let vp_y = self.runtime.ui_graphics.graphics_viewport_y as f32;
        (vp_x + world_x, surface_h - (vp_y + world_y))
    }

    fn synthetic_world_to_surface_point(&self, world_x: f32, world_y: f32) -> (f32, f32) {
        if let Some(projected) = self.synthetic_project_world_point_via_gl(world_x, world_y, 0.0) {
            return projected;
        }
        self.synthetic_world_to_surface_point_direct(world_x, world_y)
    }

    fn synthetic_top_ancestor(&self, node: u32) -> u32 {
        let mut current = node;
        let mut guard = 0usize;
        while current != 0 && guard < 64 {
            let parent = self
                .runtime
                .graphics
                .synthetic_sprites
                .get(&current)
                .map(|state| state.parent)
                .unwrap_or(0);
            if parent == 0 || parent == current {
                break;
            }
            current = parent;
            guard = guard.saturating_add(1);
        }
        current
    }

    fn synthetic_find_scene_projection_probe_recursive(
        &self,
        node: u32,
        depth: usize,
        best: &mut Option<(u32, f32, f32, f32)>,
    ) {
        if node == 0 || depth >= 8 {
            return;
        }
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1) as f32;
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
        if let Some((w, h, area)) = self.synthetic_node_scene_canvas_candidate(node, surface_w, surface_h) {
            let replace = best
                .as_ref()
                .map(|(_, _, _, best_area)| area > *best_area)
                .unwrap_or(true);
            if replace {
                *best = Some((node, w, h, area));
            }
        }
        let children = self
            .runtime
            .graphics
            .synthetic_sprites
            .get(&node)
            .map(|state| state.children)
            .unwrap_or(0);
        if children == 0 {
            return;
        }
        let items = self
            .runtime
            .graphics
            .synthetic_arrays
            .get(&children)
            .map(|arr| arr.items.clone())
            .unwrap_or_default();
        for child in items {
            self.synthetic_find_scene_projection_probe_recursive(child, depth.saturating_add(1), best);
        }
    }

    fn synthetic_find_scene_projection_probe(&self, node: u32) -> Option<(u32, f32, f32)> {
        let root = self.synthetic_top_ancestor(node);
        if root == 0 {
            return None;
        }
        let mut best: Option<(u32, f32, f32, f32)> = None;
        self.synthetic_find_scene_projection_probe_recursive(root, 0, &mut best);
        best.map(|(probe, w, h, _)| (probe, w, h))
    }

    fn synthetic_projection_rect_score(
        &self,
        rect: (i32, i32, u32, u32),
        expected_w: f32,
        expected_h: f32,
    ) -> f32 {
        let (x, y, w, h) = rect;
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1) as f32;
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
        let rect_w = w.max(1) as f32;
        let rect_h = h.max(1) as f32;
        let size_error = (rect_w - expected_w.max(1.0)).abs() + (rect_h - expected_h.max(1.0)).abs();
        let origin_error = (x.max(0) as f32).min(surface_w) + (y.max(0) as f32).min(surface_h);
        size_error + origin_error * 0.5
    }

    fn synthetic_scene_prefers_direct_surface_projection(&self, node: u32) -> bool {
        let Some((probe, probe_w, probe_h)) = self.synthetic_find_scene_projection_probe(node) else {
            return false;
        };
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1) as f32;
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
        let direct_like_delta = (probe_w - surface_w).abs() + (probe_h - surface_h).abs();
        let swapped_like_delta = (probe_w - surface_h).abs() + (probe_h - surface_w).abs();
        if !(direct_like_delta + 2.0 < swapped_like_delta) {
            return false;
        }
        let probe_layout_w = probe_w.round().max(1.0) as u32;
        let probe_layout_h = probe_h.round().max(1.0) as u32;
        let (local_quad, _) = self.synthetic_node_content_local_quad(probe, probe_layout_w, probe_layout_h);
        let Some((_, direct_rect)) = self.synthetic_surface_quad_and_rect_from_local_quad_direct(probe, &local_quad) else {
            return false;
        };
        let direct_score = self.synthetic_projection_rect_score(direct_rect, probe_w, probe_h);
        let gl_score = self
            .synthetic_surface_quad_and_rect_from_local_quad_gl(probe, &local_quad)
            .map(|(_, rect)| self.synthetic_projection_rect_score(rect, probe_w, probe_h))
            .unwrap_or(f32::INFINITY);
        direct_score + 4.0 < gl_score
    }

    fn synthetic_surface_quad_and_rect_from_local_quad_impl(
        &self,
        node: u32,
        local_corners: &[(f32, f32); 4],
        use_gl_projection: bool,
    ) -> Option<([(f32, f32); 4], (i32, i32, u32, u32))> {
        let world = self.compute_synthetic_node_world_affine(node);
        let mut projected = [(0.0f32, 0.0f32); 4];
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for (idx, (lx, ly)) in local_corners.iter().copied().enumerate() {
            let (wx, wy) = Self::synthetic_affine_transform_point(world, lx, ly);
            let (sx, sy) = if use_gl_projection {
                self.synthetic_world_to_surface_point(wx, wy)
            } else {
                self.synthetic_world_to_surface_point_direct(wx, wy)
            };
            projected[idx] = (sx, sy);
            min_x = min_x.min(sx);
            min_y = min_y.min(sy);
            max_x = max_x.max(sx);
            max_y = max_y.max(sy);
        }
        if !min_x.is_finite() || !min_y.is_finite() || !max_x.is_finite() || !max_y.is_finite() {
            return None;
        }
        let left = min_x.floor() as i32;
        let top = min_y.floor() as i32;
        let width = (max_x - min_x).ceil().max(1.0) as u32;
        let height = (max_y - min_y).ceil().max(1.0) as u32;
        Some((projected, (left, top, width, height)))
    }

    fn synthetic_surface_quad_and_rect_from_local_quad_gl(
        &self,
        node: u32,
        local_corners: &[(f32, f32); 4],
    ) -> Option<([(f32, f32); 4], (i32, i32, u32, u32))> {
        self.synthetic_surface_quad_and_rect_from_local_quad_impl(node, local_corners, true)
    }

    fn synthetic_surface_quad_and_rect_from_local_quad_direct(
        &self,
        node: u32,
        local_corners: &[(f32, f32); 4],
    ) -> Option<([(f32, f32); 4], (i32, i32, u32, u32))> {
        self.synthetic_surface_quad_and_rect_from_local_quad_impl(node, local_corners, false)
    }

    fn synthetic_surface_quad_and_rect_from_local_quad(
        &self,
        node: u32,
        local_corners: &[(f32, f32); 4],
    ) -> Option<([(f32, f32); 4], (i32, i32, u32, u32))> {
        if self.synthetic_scene_prefers_direct_surface_projection(node) {
            if let Some(projected) = self.synthetic_surface_quad_and_rect_from_local_quad_direct(node, local_corners) {
                return Some(projected);
            }
        }
        self.synthetic_surface_quad_and_rect_from_local_quad_gl(node, local_corners)
            .or_else(|| self.synthetic_surface_quad_and_rect_from_local_quad_direct(node, local_corners))
    }

    fn synthetic_surface_quad_and_rect_for_node(
        &self,
        node: u32,
        layout_w: u32,
        layout_h: u32,
    ) -> Option<([(f32, f32); 4], (i32, i32, u32, u32), &'static str)> {
        if layout_w == 0 || layout_h == 0 {
            return None;
        }
        let (local_corners, quad_source) = self.synthetic_node_content_local_quad(node, layout_w, layout_h);
        self.synthetic_surface_quad_and_rect_from_local_quad(node, &local_corners)
            .map(|(projected, rect)| (projected, rect, quad_source))
    }

    fn synthetic_surface_rect_for_node(&self, node: u32, draw_w: u32, draw_h: u32) -> Option<(i32, i32, u32, u32)> {
        self.synthetic_surface_quad_and_rect_for_node(node, draw_w, draw_h)
            .map(|(_, rect, _)| rect)
    }

    fn trace_synthetic_visit_semantics(
        &mut self,
        node: u32,
        reason: &str,
        layout_w: u32,
        layout_h: u32,
        projected_quad: &[(f32, f32); 4],
        final_rect: (i32, i32, u32, u32),
    ) {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node).cloned() else {
            return;
        };
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let local = self.synthetic_node_local_affine(node);
        let world = self.compute_synthetic_node_world_affine(node);
        let default_anchor = Self::synthetic_default_anchor(&label);
        let anchor_x = if state.anchor_explicit {
            Self::f32_from_bits(state.anchor_x_bits)
        } else {
            default_anchor
        };
        let anchor_y = if state.anchor_explicit {
            Self::f32_from_bits(state.anchor_y_bits)
        } else {
            default_anchor
        };
        let (scale_x, scale_y) = Self::synthetic_local_scale(&state);
        let neutral = self.synthetic_node_has_neutral_container_transform(node);
        let (pos_x, pos_y, _) = self.synthetic_node_effective_position_for_content(node, layout_w as f32, layout_h as f32);
        let (rect_x, rect_y, rect_w, rect_h) = final_rect;
        self.diag.trace.push(format!(
            "     ↳ scene-visit node={} label={} z={} reason={} parent={} neutral={} layout={}x{} pos=({:.2},{:.2}) anchor=({:.3},{:.3}) scale=({:.3},{:.3}) local=[{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}] world=[{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}] quad=[({:.2},{:.2}),({:.2},{:.2}),({:.2},{:.2}),({:.2},{:.2})] rect=({},{} {}x{})",
            self.describe_ptr(node),
            label,
            state.z_order,
            reason,
            self.describe_ptr(state.parent),
            if neutral { "YES" } else { "NO" },
            layout_w,
            layout_h,
            pos_x,
            pos_y,
            anchor_x,
            anchor_y,
            scale_x,
            scale_y,
            local[0], local[1], local[2], local[3], local[4], local[5],
            world[0], world[1], world[2], world[3], world[4], world[5],
            projected_quad[0].0, projected_quad[0].1,
            projected_quad[1].0, projected_quad[1].1,
            projected_quad[2].0, projected_quad[2].1,
            projected_quad[3].0, projected_quad[3].1,
            rect_x, rect_y, rect_w, rect_h,
        ));
    }

    fn begin_scene_visit_observability_frame(
        &mut self,
        frame_index: u32,
        root: u32,
        source_note: &str,
        allow_auto_visit: bool,
        will_visit: bool,
        partial_retained_frame: bool,
        guest_clear_only: bool,
        signature: u64,
    ) {
        self.push_graph_trace(format!(
            "scene.visit.frame rev={} pkg={}/{} frame={} present={} root={} source={} allow={} visit={} partialRetained={} guestClearOnly={} guestDirty={} guestFrameDraws={} guestDraws={} runningScene={} cachedRoot={} sig=0x{:016x}",
            SCENE_VISIT_OBSERVABILITY_REV,
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            frame_index,
            self.runtime.ui_graphics.graphics_present_calls.saturating_add(1),
            self.describe_ptr(root),
            source_note,
            if allow_auto_visit { "YES" } else { "NO" },
            if will_visit { "YES" } else { "NO" },
            if partial_retained_frame { "YES" } else { "NO" },
            if guest_clear_only { "YES" } else { "NO" },
            if self.runtime.graphics.guest_framebuffer_dirty { "YES" } else { "NO" },
            self.runtime.graphics.guest_draws_since_present,
            self.runtime.ui_graphics.graphics_guest_draw_calls,
            self.describe_ptr(self.runtime.ui_cocos.running_scene),
            self.describe_ptr(self.runtime.scene.auto_scene_cached_root),
            signature,
        ));
    }

    fn push_scene_visit_order_trace(
        &mut self,
        budget: &mut SceneVisitTraceBudget,
        depth: u32,
        phase: &str,
        parent: u32,
        node: u32,
        sibling_index: Option<usize>,
        z_order: i32,
    ) {
        if !budget.try_take() {
            return;
        }
        self.push_graph_trace(format!(
            "scene.visit.order depth={} phase={} parent={} node={} siblingIndex={} z={}",
            depth,
            phase,
            self.describe_ptr(parent),
            self.describe_ptr(node),
            sibling_index.map(|v| v.to_string()).unwrap_or_else(|| "<self>".to_string()),
            z_order,
        ));
    }

    fn push_scene_visit_node_trace(
        &mut self,
        budget: &mut SceneVisitTraceBudget,
        node: u32,
        depth: u32,
        phase: &str,
        reason: &str,
        layout_w: u32,
        layout_h: u32,
        projected_quad: &[(f32, f32); 4],
        final_rect: (i32, i32, u32, u32),
        rect_source: &str,
        render_outcome: &str,
    ) {
        if !budget.try_take() {
            return;
        }
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node).cloned() else {
            return;
        };
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let local = self.synthetic_node_local_affine(node);
        let world = self.compute_synthetic_node_world_affine(node);
        let default_anchor = Self::synthetic_default_anchor(&label);
        let anchor_x = if state.anchor_explicit { Self::f32_from_bits(state.anchor_x_bits) } else { default_anchor };
        let anchor_y = if state.anchor_explicit { Self::f32_from_bits(state.anchor_y_bits) } else { default_anchor };
        let neutral = self.synthetic_node_has_neutral_container_transform(node);
        let child_count = if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 };
        let (pos_x, pos_y, _) = self.synthetic_node_effective_position_for_content(node, layout_w as f32, layout_h as f32);
        let (rect_x, rect_y, rect_w, rect_h) = final_rect;
        self.push_graph_trace(format!(
            "scene.visit.node depth={} phase={} node={} label={} parent={} z={} reason={} content={}x{} layout={}x{} children={} anchor=({:.3},{:.3}) pos=({:.2},{:.2}) local=[{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}] world=[{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}] quad=[({:.2},{:.2}),({:.2},{:.2}),({:.2},{:.2}),({:.2},{:.2})] rectSrc={} rect=({},{} {}x{}) neutral={} entered={} visible={} render={}",
            depth,
            phase,
            self.describe_ptr(node),
            if label.is_empty() { "<unknown>" } else { &label },
            self.describe_ptr(state.parent),
            state.z_order,
            reason,
            state.width,
            state.height,
            layout_w,
            layout_h,
            child_count,
            anchor_x,
            anchor_y,
            pos_x,
            pos_y,
            local[0], local[1], local[2], local[3], local[4], local[5],
            world[0], world[1], world[2], world[3], world[4], world[5],
            projected_quad[0].0, projected_quad[0].1,
            projected_quad[1].0, projected_quad[1].1,
            projected_quad[2].0, projected_quad[2].1,
            projected_quad[3].0, projected_quad[3].1,
            rect_source,
            rect_x, rect_y, rect_w, rect_h,
            if neutral { "YES" } else { "NO" },
            if state.entered { "YES" } else { "NO" },
            if state.visible { "YES" } else { "NO" },
            render_outcome,
        ));
    }

    fn push_scene_visit_sprite_trace(
        &mut self,
        budget: &mut SceneVisitTraceBudget,
        node: u32,
        depth: u32,
        layout_w: u32,
        layout_h: u32,
        quad_source: &str,
    ) {
        if !budget.try_take() {
            return;
        }
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node).cloned() else {
            return;
        };
        let (content_w, content_h, content_src) = self.synthetic_node_content_size_for_layout(node, layout_w, layout_h);
        let (anchor_px_x, anchor_px_y, anchor_px_src) = self.synthetic_node_anchor_pixels_for_content(node, content_w, content_h);
        let (local_quad, _) = self.synthetic_node_content_local_quad(node, layout_w, layout_h);
        let world = self.compute_synthetic_node_world_affine(node);
        let mut world_quad = [(0.0f32, 0.0f32); 4];
        for (idx, (lx, ly)) in local_quad.iter().copied().enumerate() {
            world_quad[idx] = Self::synthetic_affine_transform_point(world, lx, ly);
        }
        self.push_graph_trace(format!(
            "scene.visit.sprite depth={} node={} label={} contentSrc={} content=({:.2}x{:.2}) rect=({:.2},{:.2} {:.2}x{:.2}) untrimmed={}({:.2},{:.2}) anchorPx={}({:.2},{:.2}) offset={}({:.2},{:.2}) flip=({},{}) quadSrc={} localQuad=[({:.2},{:.2}),({:.2},{:.2}),({:.2},{:.2}),({:.2},{:.2})] worldQuad=[({:.2},{:.2}),({:.2},{:.2}),({:.2},{:.2}),({:.2},{:.2})]",
            depth,
            self.describe_ptr(node),
            self.diag.object_labels.get(&node).cloned().unwrap_or_default(),
            content_src,
            content_w,
            content_h,
            Self::f32_from_bits(state.texture_rect_x_bits),
            Self::f32_from_bits(state.texture_rect_y_bits),
            Self::f32_from_bits(state.texture_rect_w_bits),
            Self::f32_from_bits(state.texture_rect_h_bits),
            if state.untrimmed_explicit { "YES" } else { "NO" },
            Self::f32_from_bits(state.untrimmed_w_bits),
            Self::f32_from_bits(state.untrimmed_h_bits),
            anchor_px_src,
            anchor_px_x,
            anchor_px_y,
            if state.offset_explicit { "YES" } else { "NO" },
            Self::f32_from_bits(state.offset_x_bits),
            Self::f32_from_bits(state.offset_y_bits),
            if state.flip_x { "YES" } else { "NO" },
            if state.flip_y { "YES" } else { "NO" },
            quad_source,
            local_quad[0].0, local_quad[0].1,
            local_quad[1].0, local_quad[1].1,
            local_quad[2].0, local_quad[2].1,
            local_quad[3].0, local_quad[3].1,
            world_quad[0].0, world_quad[0].1,
            world_quad[1].0, world_quad[1].1,
            world_quad[2].0, world_quad[2].1,
            world_quad[3].0, world_quad[3].1,
        ));
    }

    fn render_synthetic_node_into_framebuffer(
        &mut self,
        node: u32,
        reason: &str,
        depth: u32,
        trace_budget: &mut SceneVisitTraceBudget,
    ) -> bool {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node).cloned() else {
            return false;
        };
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
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
        let self_selector_name = if state.callback_selector == 0 {
            String::new()
        } else {
            self.objc_read_selector_name(state.callback_selector).unwrap_or_default()
        };
        let ach_related = self_selector_name.contains("achievementsCallback:")
            || parent_selector_name.contains("achievementsCallback:");
        let effective_texture = self.synthetic_node_effective_texture(node);
        if ach_related {
            self.diag.trace.push(format!(
                "     ↳ ab-ach draw-enter node={} label={} visible={} parent={} selfSel={} parentSel={} size={}x{} children={} tex={} reason={}",
                self.describe_ptr(node),
                label,
                if state.visible { "YES" } else { "NO" },
                self.describe_ptr(state.parent),
                if self_selector_name.is_empty() { "<none>" } else { &self_selector_name },
                if parent_selector_name.is_empty() { "<none>" } else { &parent_selector_name },
                state.width,
                state.height,
                if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 },
                self.describe_ptr(effective_texture),
                reason,
            ));
        }
        if !state.visible {
            if ach_related {
                self.diag.trace.push(format!(
                    "     ↳ ab-ach skip-invisible node={} label={}",
                    self.describe_ptr(node),
                    label,
                ));
            }
            return false;
        }
        let mut layout_w = state.width;
        let mut layout_h = state.height;
        let explicit_zero_rect = state.texture_rect_explicit && (state.width == 0 || state.height == 0);
        let mut image: Option<(Vec<u8>, u32, u32)> = None;
        let child_count = if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 };
        if effective_texture != 0 {
            if let Some(texture) = self.runtime.graphics.synthetic_textures.get(&effective_texture) {
                if !explicit_zero_rect {
                    if layout_w == 0 {
                        layout_w = texture.width;
                    }
                    if layout_h == 0 {
                        layout_h = texture.height;
                    }
                }
                if texture.image != 0 {
                    if let Some(img) = self.runtime.graphics.synthetic_images.get(&texture.image) {
                        image = Some((img.rgba.clone(), img.width, img.height));
                    }
                }
            }
        }
        let has_resolved_text_texture = image.is_some();
        let should_fallback_to_builtin_text = self.string_backing(node).is_some()
            && (Self::is_label_class_name(&label) || label.contains("FontAtlas"))
            && !has_resolved_text_texture;
        if layout_w == 0 || layout_h == 0 {
            if let Some(text_backing) = self.string_backing(node) {
                let scale = Self::synthetic_text_scale_for_height(layout_h.max(14));
                let (text_w, text_h) = Self::synthetic_text_dimensions_5x7(&text_backing.text, scale);
                if layout_w == 0 {
                    layout_w = text_w.max(1);
                }
                if layout_h == 0 {
                    layout_h = text_h.max(1);
                }
            }
            if label.contains("CCColorLayer") || label.contains("CCScene") || label.contains("MenuLayer") || label.contains("FirstScene") || Self::is_transition_like_label(&label) {
                if layout_w == 0 {
                    layout_w = self.runtime.ui_graphics.graphics_surface_width.max(1);
                }
                if layout_h == 0 {
                    layout_h = self.runtime.ui_graphics.graphics_surface_height.max(1);
                }
            }
        }
        if layout_w == 0 || layout_h == 0 {
            return false;
        }

        self.ensure_framebuffer_backing();
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1);
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1);
        let (world_x, world_y, world_scale_x, world_scale_y) = self.compute_synthetic_node_world_transform(node);
        let default_anchor = Self::synthetic_default_anchor(&label);
        let anchor_x = if state.anchor_explicit {
            Self::f32_from_bits(state.anchor_x_bits)
        } else {
            default_anchor
        };
        let anchor_y = if state.anchor_explicit {
            Self::f32_from_bits(state.anchor_y_bits)
        } else {
            default_anchor
        };
        let Some((projected_quad, (draw_x, draw_y, mapped_draw_w, mapped_draw_h), quad_source)) =
            self.synthetic_surface_quad_and_rect_for_node(node, layout_w.max(1), layout_h.max(1))
        else {
            return false;
        };
        self.trace_synthetic_visit_semantics(
            node,
            reason,
            layout_w.max(1),
            layout_h.max(1),
            &projected_quad,
            (draw_x, draw_y, mapped_draw_w, mapped_draw_h),
        );
        let rect_source = if self.synthetic_node_has_neutral_container_transform(node) {
            format!("{}+neutral-parent", quad_source)
        } else {
            quad_source.to_string()
        };
        if label.contains("CCSprite") {
            self.push_scene_visit_sprite_trace(
                trace_budget,
                node,
                depth,
                layout_w.max(1),
                layout_h.max(1),
                quad_source,
            );
        }
        let draw_w = mapped_draw_w.max(1);
        let draw_h = mapped_draw_h.max(1);
        let is_sprite_sheet_like = label.contains("CCSpriteSheet")
            || label.contains("CCSpriteBatchNode")
            || label.contains("TextureAtlas");
        let is_scene_container = Self::synthetic_label_is_scene_container(&label)
            || label.contains("CCNode");
        let is_container_only = child_count > 0
            && (((image.is_none() || is_sprite_sheet_like)
                && (label.contains("CCMenu")
                    || is_scene_container
                    || is_sprite_sheet_like))
                || Self::is_menu_item_class_name(&label));
        if is_container_only {
            self.push_scene_visit_node_trace(
                trace_budget,
                node,
                depth,
                "self",
                reason,
                layout_w.max(1),
                layout_h.max(1),
                &projected_quad,
                (draw_x, draw_y, mapped_draw_w, mapped_draw_h),
                &rect_source,
                "skip-container-only",
            );
            if ach_related {
                self.diag.trace.push(format!(
                    "     ↳ ab-ach skip-container-only node={} label={} childCount={} imagePresent={} draw={}x{} world=({:.1},{:.1}) anchor=({:.2},{:.2})",
                    self.describe_ptr(node),
                    label,
                    child_count,
                    if image.is_some() { "YES" } else { "NO" },
                    draw_w,
                    draw_h,
                    world_x,
                    world_y,
                    anchor_x,
                    anchor_y,
                ));
            }
            return false;
        }

        let render_outcome = if let Some((rgba, src_w, src_h)) = image {
            let bg_texture = self.runtime.graphics
                .synthetic_textures
                .get(&effective_texture)
                .map(|tex| tex.source_key.eq_ignore_ascii_case("menu_background.png"))
                .unwrap_or(false);
            let texture_image_ptr = self.runtime.graphics
                .synthetic_textures
                .get(&effective_texture)
                .map(|tex| tex.image)
                .unwrap_or(0);
            let texture_source_key = self.runtime.graphics
                .synthetic_textures
                .get(&effective_texture)
                .map(|tex| tex.source_key.clone())
                .unwrap_or_default();
            let texture_source_path = self.runtime.graphics
                .synthetic_textures
                .get(&effective_texture)
                .map(|tex| tex.source_path.clone())
                .unwrap_or_default();
            let texture_pma = self.runtime.graphics
                .synthetic_textures
                .get(&effective_texture)
                .map(|tex| tex.has_premultiplied_alpha)
                .unwrap_or(true);
            let (resolved_rgba, resolved_w, resolved_h) = self.resolve_sprite_texture_region(node, &state, &rgba, src_w.max(1), src_h.max(1));
            if resolved_rgba.is_empty() || resolved_w == 0 || resolved_h == 0 {
                self.push_scene_visit_node_trace(
                    trace_budget,
                    node,
                    depth,
                    "self",
                    reason,
                    layout_w.max(1),
                    layout_h.max(1),
                    &projected_quad,
                    (draw_x, draw_y, mapped_draw_w, mapped_draw_h),
                    &rect_source,
                    "skip-empty-image",
                );
                return false;
            }
            if ach_related {
                let mut alpha_nonzero = 0usize;
                let mut alpha_sum = 0u64;
                for px in resolved_rgba.chunks_exact(4) {
                    if px[3] != 0 {
                        alpha_nonzero += 1;
                    }
                    alpha_sum = alpha_sum.saturating_add(px[3] as u64);
                }
                let total_px = (resolved_w.max(1) as usize).saturating_mul(resolved_h.max(1) as usize).max(1);
                let avg_alpha = alpha_sum as f64 / total_px as f64;
                self.diag.trace.push(format!(
                    "     ↳ ab-ach draw-image node={} drawRect=({},{} {}x{}) src={}x{} resolved={}x{} alphaNZ={}/{} avgAlpha={:.1} texRect=({:.1},{:.1} {:.1}x{:.1})",
                    self.describe_ptr(node),
                    draw_x,
                    draw_y,
                    draw_w,
                    draw_h,
                    src_w,
                    src_h,
                    resolved_w,
                    resolved_h,
                    alpha_nonzero,
                    total_px,
                    avg_alpha,
                    Self::f32_from_bits(state.texture_rect_x_bits),
                    Self::f32_from_bits(state.texture_rect_y_bits),
                    Self::f32_from_bits(state.texture_rect_w_bits),
                    Self::f32_from_bits(state.texture_rect_h_bits),
                ));
            }
            if bg_texture {
                let decode_fp = sample_rgba_fingerprint(&rgba, src_w.max(1), src_h.max(1));
                let resolved_fp = sample_rgba_fingerprint(&resolved_rgba, resolved_w.max(1), resolved_h.max(1));
                let texture_image_fp = self.runtime.graphics
                    .synthetic_images
                    .get(&texture_image_ptr)
                    .map(|img| sample_rgba_fingerprint(&img.rgba, img.width.max(1), img.height.max(1)))
                    .unwrap_or_else(|| "missing-image".to_string());
                self.diag.trace.push(format!(
                    "     ↳ ab-bgtex-draw-pre node={} label={} tex={} image={} key={} pma={} world=({:.1},{:.1}) drawRect=({},{} {}x{}) src={}x{} resolved={}x{} path={} decodeFp={} texImgFp={} resolvedFp={} texRect=({:.1},{:.1} {:.1}x{:.1})",
                    self.describe_ptr(node),
                    label,
                    self.describe_ptr(state.texture),
                    self.describe_ptr(texture_image_ptr),
                    if texture_source_key.is_empty() { "<none>" } else { &texture_source_key },
                    if texture_pma { "YES" } else { "NO" },
                    world_x,
                    world_y,
                    draw_x,
                    draw_y,
                    draw_w,
                    draw_h,
                    src_w,
                    src_h,
                    resolved_w,
                    resolved_h,
                    if texture_source_path.is_empty() { "<none>" } else { &texture_source_path },
                    decode_fp,
                    texture_image_fp,
                    resolved_fp,
                    Self::f32_from_bits(state.texture_rect_x_bits),
                    Self::f32_from_bits(state.texture_rect_y_bits),
                    Self::f32_from_bits(state.texture_rect_w_bits),
                    Self::f32_from_bits(state.texture_rect_h_bits),
                ));
            }
            let tint_rgba = if state.fill_rgba_explicit {
                state.fill_rgba
            } else {
                [255, 255, 255, 255]
            };
            Self::composite_rgba_scaled_tinted_into(
                &mut self.runtime.graphics.synthetic_framebuffer,
                surface_w,
                surface_h,
                &resolved_rgba,
                resolved_w.max(1),
                resolved_h.max(1),
                draw_x,
                draw_y,
                draw_w.max(1),
                draw_h.max(1),
                tint_rgba,
            );
            if bg_texture {
                let fb_fp = sample_framebuffer_region_fingerprint(
                    &self.runtime.graphics.synthetic_framebuffer,
                    surface_w,
                    surface_h,
                    draw_x,
                    draw_y,
                    draw_w.max(1),
                    draw_h.max(1),
                );
                self.diag.trace.push(format!(
                    "     ↳ ab-bgtex-draw-post node={} fbRect=({},{} {}x{}) fbFp={}",
                    self.describe_ptr(node),
                    draw_x,
                    draw_y,
                    draw_w,
                    draw_h,
                    fb_fp,
                ));
            }
            if bg_texture { "draw-image:bg" } else { "draw-image" }
        } else if should_fallback_to_builtin_text {
            let Some(text_backing) = self.string_backing(node) else {
                return false;
            };
            let text = text_backing.text.replace('\0', "");
            let scale = Self::synthetic_text_scale_for_height(draw_h);
            let (text_w, text_h) = Self::synthetic_text_dimensions_5x7(&text, scale);
            let text_origin_x = if draw_w > text_w {
                draw_x.saturating_add(((draw_w - text_w) / 2) as i32)
            } else {
                draw_x
            };
            let text_origin_y = if draw_h > text_h {
                draw_y.saturating_add(((draw_h - text_h) / 2) as i32)
            } else {
                draw_y
            };
            for (line_idx, line) in text.split('\n').enumerate() {
                let line_y = text_origin_y.saturating_add((line_idx as u32).saturating_mul(8 * scale) as i32);
                Self::draw_ab_text_5x7(
                    &mut self.runtime.graphics.synthetic_framebuffer,
                    surface_w,
                    surface_h,
                    line,
                    scale,
                    text_origin_x.saturating_add(1),
                    line_y.saturating_add(1),
                    [255, 244, 210, 96],
                );
                Self::draw_ab_text_5x7(
                    &mut self.runtime.graphics.synthetic_framebuffer,
                    surface_w,
                    surface_h,
                    line,
                    scale,
                    text_origin_x,
                    line_y,
                    [38, 22, 8, 230],
                );
            }
            self.diag.trace.push(format!(
                "     ↳ synth-text draw node={} label={} text='{}' rect=({},{} {}x{}) scale={}",
                self.describe_ptr(node),
                label,
                text.replace('\n', "\\n"),
                draw_x,
                draw_y,
                draw_w,
                draw_h,
                scale,
            ));
            "draw-text"
        } else {
            let fill = if state.fill_rgba_explicit {
                Some(state.fill_rgba)
            } else if label.contains("CCColorLayer") {
                Some([0, 0, 0, 0])
            } else {
                None
            };
            if let Some(fill_rgba) = fill {
                let skip_fill = label.contains("CCColorLayer")
                    && self.synthetic_should_skip_fullscreen_color_layer_fill(node, &state, draw_w, draw_h, surface_w, surface_h);
                if !skip_fill && fill_rgba[3] != 0 {
                    Self::ui_fill_rect_rgba(
                        &mut self.runtime.graphics.synthetic_framebuffer,
                        surface_w,
                        surface_h,
                        draw_x,
                        draw_y,
                        draw_w.max(1),
                        draw_h.max(1),
                        fill_rgba,
                    );
                    "draw-fill"
                } else if skip_fill {
                    "skip-fill-neutralized"
                } else {
                    "skip-fill-alpha0"
                }
            } else {
                let tint = Self::synthetic_node_debug_rgba(node, &label, reason);
                Self::ui_fill_rect_rgba(
                    &mut self.runtime.graphics.synthetic_framebuffer,
                    surface_w,
                    surface_h,
                    draw_x,
                    draw_y,
                    draw_w.max(1),
                    draw_h.max(1),
                    tint,
                );
                "draw-debug-fill"
            }
        };
        self.push_scene_visit_node_trace(
            trace_budget,
            node,
            depth,
            "self",
            reason,
            layout_w.max(1),
            layout_h.max(1),
            &projected_quad,
            (draw_x, draw_y, mapped_draw_w, mapped_draw_h),
            &rect_source,
            render_outcome,
        );
        self.runtime.graphics.guest_framebuffer_dirty = true;
        self.runtime.ui_graphics.graphics_guest_draw_calls = self.runtime.ui_graphics.graphics_guest_draw_calls.saturating_add(1);
        self.runtime.ui_graphics.graphics_guest_vertex_fetches = self.runtime.ui_graphics.graphics_guest_vertex_fetches.saturating_add(4);
        self.runtime.ui_graphics.graphics_last_draw_mode = GL_TRIANGLE_STRIP;
        self.runtime.ui_graphics.graphics_last_draw_mode_label = Some(format!("synthetic-{}", reason));
        self.runtime.ui_graphics.graphics_last_guest_draw_checksum = Self::checksum_bytes(&self.runtime.graphics.synthetic_framebuffer);
        true
    }

    fn visit_synthetic_node_recursive(
        &mut self,
        node: u32,
        depth: u32,
        trace_budget: &mut SceneVisitTraceBudget,
    ) -> usize {
        let mut visited = HashSet::new();
        let mut path = Vec::new();
        self.visit_synthetic_node_recursive_guarded(node, depth, trace_budget, &mut visited, &mut path)
    }

    fn visit_synthetic_node_recursive_guarded(
        &mut self,
        node: u32,
        depth: u32,
        trace_budget: &mut SceneVisitTraceBudget,
        visited: &mut HashSet<u32>,
        path: &mut Vec<u32>,
    ) -> usize {
        if node == 0 {
            return 0;
        }
        if depth > 64 {
            self.push_graph_trace_critical(format!(
                "traversal.depth-limit walk=visit_synthetic depth={} node={}",
                depth,
                self.describe_ptr(node),
            ));
            return 0;
        }
        if let Some(pos) = path.iter().position(|value| *value == node) {
            let loop_nodes = path[pos..].to_vec();
            self.record_scene_graph_cycle("visit_synthetic", &loop_nodes, node, depth);
            return 0;
        }
        if !visited.insert(node) {
            return 0;
        }
        if !self.runtime.graphics.synthetic_sprites.get(&node).map(|state| state.visible).unwrap_or(true) {
            return 0;
        }
        path.push(node);
        let node_parent = self.runtime.graphics.synthetic_sprites.get(&node).map(|state| state.parent).unwrap_or(0);
        let node_z = self.runtime.graphics.synthetic_sprites.get(&node).map(|state| state.z_order).unwrap_or(0);
        let children = self
            .runtime
            .graphics
            .synthetic_sprites
            .get(&node)
            .map(|state| state.children)
            .unwrap_or(0);
        let ordered_children = self
            .runtime
            .graphics
            .synthetic_arrays
            .get(&children)
            .map(|arr| arr.items.clone())
            .unwrap_or_default();

        let mut draws = 0usize;
        for (index, child) in ordered_children.iter().copied().enumerate() {
            if child == 0 { continue; }
            let child_z = self.runtime.graphics.synthetic_sprites.get(&child).map(|state| state.z_order).unwrap_or(0);
            if child_z >= 0 { break; }
            self.push_scene_visit_order_trace(trace_budget, depth.saturating_add(1), "child-neg", node, child, Some(index), child_z);
            draws = draws.saturating_add(self.visit_synthetic_node_recursive_guarded(
                child,
                depth.saturating_add(1),
                trace_budget,
                visited,
                path,
            ));
        }
        self.push_scene_visit_order_trace(trace_budget, depth, "self", node_parent, node, None, node_z);
        if self.render_synthetic_node_into_framebuffer(node, "visit", depth, trace_budget) {
            draws = draws.saturating_add(1);
        }
        for (index, child) in ordered_children.iter().copied().enumerate() {
            if child == 0 { continue; }
            let child_z = self.runtime.graphics.synthetic_sprites.get(&child).map(|state| state.z_order).unwrap_or(0);
            if child_z < 0 { continue; }
            self.push_scene_visit_order_trace(trace_budget, depth.saturating_add(1), "child-pos", node, child, Some(index), child_z);
            draws = draws.saturating_add(self.visit_synthetic_node_recursive_guarded(
                child,
                depth.saturating_add(1),
                trace_budget,
                visited,
                path,
            ));
        }
        path.pop();
        draws
    }

    fn collect_auto_scene_visit_stats(&mut self, node: u32, depth: u32, stats: &mut AutoSceneVisitStats) {
        let mut visited = HashSet::new();
        let mut path = Vec::new();
        self.collect_auto_scene_visit_stats_guarded(node, depth, stats, &mut visited, &mut path)
    }

    fn collect_auto_scene_visit_stats_guarded(
        &mut self,
        node: u32,
        depth: u32,
        stats: &mut AutoSceneVisitStats,
        visited: &mut HashSet<u32>,
        path: &mut Vec<u32>,
    ) {
        if node == 0 {
            return;
        }
        if depth > 64 {
            stats.max_depth = stats.max_depth.max(64);
            self.push_graph_trace_critical(format!(
                "traversal.depth-limit walk=collect_auto_scene_visit_stats depth={} node={}",
                depth,
                self.describe_ptr(node),
            ));
            return;
        }
        if let Some(pos) = path.iter().position(|value| *value == node) {
            let loop_nodes = path[pos..].to_vec();
            self.record_scene_graph_cycle("collect_auto_scene_visit_stats", &loop_nodes, node, depth);
            return;
        }
        if !visited.insert(node) {
            return;
        }
        path.push(node);
        stats.nodes_seen = stats.nodes_seen.saturating_add(1);
        stats.max_depth = stats.max_depth.max(depth);
        let effective_texture = self.synthetic_node_effective_texture(node);
        let (state_entered, state_visible, state_children, state_width, state_height, state_texture, state_texture_rect_explicit) =
            match self.runtime.graphics.synthetic_sprites.get(&node) {
                Some(state) => (
                    state.entered,
                    state.visible,
                    state.children,
                    state.width,
                    state.height,
                    effective_texture,
                    state.texture_rect_explicit,
                ),
                None => {
                    stats.missing = stats.missing.saturating_add(1);
                    path.pop();
                    return;
                }
            };
        if !state_entered {
            stats.entered_no = stats.entered_no.saturating_add(1);
        }
        if !state_visible {
            stats.visible_no = stats.visible_no.saturating_add(1);
            path.pop();
            return;
        }
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let child_count = if state_children != 0 { self.synthetic_array_len(state_children) } else { 0 };
        let mut draw_w = state_width;
        let mut draw_h = state_height;
        let explicit_zero_rect = state_texture_rect_explicit && (state_width == 0 || state_height == 0);
        let mut has_image = false;
        if state_texture != 0 {
            if let Some(texture) = self.runtime.graphics.synthetic_textures.get(&state_texture) {
                if !explicit_zero_rect {
                    if draw_w == 0 {
                        draw_w = texture.width;
                    }
                    if draw_h == 0 {
                        draw_h = texture.height;
                    }
                }
                has_image = texture.image != 0;
            }
        }
        if draw_w == 0 || draw_h == 0 {
            if label.contains("CCColorLayer") || label.contains("CCScene") || label.contains("MenuLayer") || label.contains("FirstScene") || Self::is_transition_like_label(&label) {
                draw_w = draw_w.max(self.runtime.ui_graphics.graphics_surface_width.max(1));
                draw_h = draw_h.max(self.runtime.ui_graphics.graphics_surface_height.max(1));
            }
        }
        let container_only = child_count > 0
            && ((!has_image
                && (label.contains("CCMenu")
                    || label.contains("MenuLayer")
                    || label.contains("CCScene")
                    || label.contains("CCLayer")
                    || label.contains("FirstScene")
                    || Self::is_transition_like_label(&label)
                    || label.contains("CCNode")))
                || Self::is_menu_item_class_name(&label));
        if draw_w == 0 || draw_h == 0 {
            if state_texture == 0 {
                stats.no_texture = stats.no_texture.saturating_add(1);
            } else {
                stats.zero_size = stats.zero_size.saturating_add(1);
            }
        } else if container_only {
            stats.container_skip = stats.container_skip.saturating_add(1);
        } else {
            stats.nodes_drawn = stats.nodes_drawn.saturating_add(1);
        }
        if state_children != 0 {
            let items = self.runtime.graphics.synthetic_arrays.get(&state_children).map(|arr| arr.items.clone()).unwrap_or_default();
            for child in items {
                self.collect_auto_scene_visit_stats_guarded(
                    child,
                    depth.saturating_add(1),
                    stats,
                    visited,
                    path,
                );
            }
        }
        path.pop();
    }


    fn mix_auto_scene_signature(seed: u64, value: u64) -> u64 {
        let mixed = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
        seed.rotate_left(13) ^ mixed.rotate_left(7).wrapping_mul(0xff51_afd7_ed55_8ccd)
    }

    fn compute_auto_scene_signature(&mut self, node: u32, depth: u32) -> u64 {
        let mut visited = HashSet::new();
        let mut path = Vec::new();
        self.compute_auto_scene_signature_guarded(node, depth, &mut visited, &mut path)
    }

    fn compute_auto_scene_signature_guarded(
        &mut self,
        node: u32,
        depth: u32,
        visited: &mut HashSet<u32>,
        path: &mut Vec<u32>,
    ) -> u64 {
        let mut sig = 0x6a09_e667_f3bc_c909u64 ^ ((node as u64) << 1) ^ u64::from(depth);
        if node == 0 {
            return Self::mix_auto_scene_signature(sig, 0xdeaf_beef_dead_beefu64);
        }
        if depth > 64 {
            self.push_graph_trace_critical(format!(
                "traversal.depth-limit walk=compute_auto_scene_signature depth={} node={}",
                depth,
                self.describe_ptr(node),
            ));
            return Self::mix_auto_scene_signature(sig, 0xd3ad_f00d_dead_beefu64);
        }
        if let Some(pos) = path.iter().position(|value| *value == node) {
            let loop_nodes = path[pos..].to_vec();
            self.record_scene_graph_cycle("compute_auto_scene_signature", &loop_nodes, node, depth);
            return Self::mix_auto_scene_signature(sig, 0xc1c1_c1c1_c1c1_c1c1u64 ^ u64::from(node));
        }
        if !visited.insert(node) {
            return Self::mix_auto_scene_signature(sig, 0xb4b4_b4b4_b4b4_b4b4u64 ^ u64::from(node));
        }
        path.push(node);
        let effective_texture = self.synthetic_node_effective_texture(node);
        let (
            state_texture,
            state_width,
            state_height,
            state_anchor_x_bits,
            state_anchor_y_bits,
            state_anchor_pixels_x_bits,
            state_anchor_pixels_y_bits,
            state_position_x_bits,
            state_position_y_bits,
            state_scale_x_bits,
            state_scale_y_bits,
            state_texture_rect_x_bits,
            state_texture_rect_y_bits,
            state_texture_rect_w_bits,
            state_texture_rect_h_bits,
            state_untrimmed_w_bits,
            state_untrimmed_h_bits,
            state_offset_x_bits,
            state_offset_y_bits,
            state_fill_rgba,
            state_last_display_frame_key,
            state_last_display_frame_index,
            state_content_revision,
            state_z_order,
            state_tag,
            state_visible,
            state_entered,
            state_touch_enabled,
            state_anchor_explicit,
            state_anchor_pixels_explicit,
            state_texture_rect_explicit,
            state_scale_explicit,
            state_untrimmed_explicit,
            state_offset_explicit,
            state_flip_x,
            state_flip_y,
            state_fill_rgba_explicit,
            state_relative_anchor_point,
            state_guest_graph_observed,
            state_children,
        ) = match self.runtime.graphics.synthetic_sprites.get(&node) {
            Some(state) => (
                effective_texture,
                state.width,
                state.height,
                state.anchor_x_bits,
                state.anchor_y_bits,
                state.anchor_pixels_x_bits,
                state.anchor_pixels_y_bits,
                state.position_x_bits,
                state.position_y_bits,
                state.scale_x_bits,
                state.scale_y_bits,
                state.texture_rect_x_bits,
                state.texture_rect_y_bits,
                state.texture_rect_w_bits,
                state.texture_rect_h_bits,
                state.untrimmed_w_bits,
                state.untrimmed_h_bits,
                state.offset_x_bits,
                state.offset_y_bits,
                state.fill_rgba,
                state.last_display_frame_key,
                state.last_display_frame_index,
                state.content_revision,
                state.z_order,
                state.tag,
                state.visible,
                state.entered,
                state.touch_enabled,
                state.anchor_explicit,
                state.anchor_pixels_explicit,
                state.texture_rect_explicit,
                state.scale_explicit,
                state.untrimmed_explicit,
                state.offset_explicit,
                state.flip_x,
                state.flip_y,
                state.fill_rgba_explicit,
                state.relative_anchor_point,
                state.guest_graph_observed,
                state.children,
            ),
            None => {
                path.pop();
                return Self::mix_auto_scene_signature(sig, 0xdeaf_beef_dead_beefu64);
            }
        };

        sig = Self::mix_auto_scene_signature(sig, u64::from(node));
        let raw_state_texture = self.runtime.graphics.synthetic_sprites.get(&node).map(|state| state.texture).unwrap_or(0);
        sig = Self::mix_auto_scene_signature(sig, u64::from(raw_state_texture));
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_texture));
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_width) | (u64::from(state_height) << 32));
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_anchor_x_bits) | (u64::from(state_anchor_y_bits) << 32));
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_anchor_pixels_x_bits) | (u64::from(state_anchor_pixels_y_bits) << 32));
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_position_x_bits) | (u64::from(state_position_y_bits) << 32));
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_scale_x_bits) | (u64::from(state_scale_y_bits) << 32));
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_texture_rect_x_bits) | (u64::from(state_texture_rect_y_bits) << 32));
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_texture_rect_w_bits) | (u64::from(state_texture_rect_h_bits) << 32));
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_untrimmed_w_bits) | (u64::from(state_untrimmed_h_bits) << 32));
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_offset_x_bits) | (u64::from(state_offset_y_bits) << 32));
        sig = Self::mix_auto_scene_signature(sig, u64::from(u32::from_le_bytes(state_fill_rgba)));
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_last_display_frame_key) | (u64::from(state_last_display_frame_index) << 32));
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_content_revision));
        sig = Self::mix_auto_scene_signature(sig, (state_z_order as i64) as u64);
        sig = Self::mix_auto_scene_signature(sig, u64::from(state_tag));
        let flags = (state_visible as u64)
            | ((state_entered as u64) << 1)
            | ((state_touch_enabled as u64) << 2)
            | ((state_anchor_explicit as u64) << 3)
            | ((state_anchor_pixels_explicit as u64) << 4)
            | ((state_texture_rect_explicit as u64) << 5)
            | ((state_scale_explicit as u64) << 6)
            | ((state_untrimmed_explicit as u64) << 7)
            | ((state_offset_explicit as u64) << 8)
            | ((state_flip_x as u64) << 9)
            | ((state_flip_y as u64) << 10)
            | ((state_fill_rgba_explicit as u64) << 11)
            | ((state_relative_anchor_point as u64) << 12)
            | ((state_guest_graph_observed as u64) << 13);
        sig = Self::mix_auto_scene_signature(sig, flags);

        if let Some(texture) = self.runtime.graphics.synthetic_textures.get(&state_texture) {
            sig = Self::mix_auto_scene_signature(sig, u64::from(texture.width) | (u64::from(texture.height) << 32));
            sig = Self::mix_auto_scene_signature(sig, u64::from(texture.gl_name));
            sig = Self::mix_auto_scene_signature(sig, texture.source_key.len() as u64);
            for byte in texture.source_key.as_bytes() {
                sig = Self::mix_auto_scene_signature(sig, u64::from(*byte));
            }
        }

        if let Some(label) = self.diag.object_labels.get(&node) {
            sig = Self::mix_auto_scene_signature(sig, label.len() as u64);
            for byte in label.as_bytes() {
                sig = Self::mix_auto_scene_signature(sig, u64::from(*byte));
            }
        }

        if depth >= 32 || state_children == 0 {
            path.pop();
            return sig;
        }

        let child_count = self.synthetic_array_len(state_children);
        sig = Self::mix_auto_scene_signature(sig, child_count as u64);
        for index in 0..child_count {
            let child = self.synthetic_array_get(state_children, index);
            sig = Self::mix_auto_scene_signature(sig, ((index as u64) << 32) | u64::from(child));
            sig = Self::mix_auto_scene_signature(sig, self.compute_auto_scene_signature_guarded(
                child,
                depth.saturating_add(1),
                visited,
                path,
            ));
        }
        path.pop();
        sig
    }


    fn resolve_transition_render_destination(&self, scene: u32) -> Option<u32> {
        if scene == 0 {
            return None;
        }
        let label = self.diag.object_labels.get(&scene).cloned().unwrap_or_default();
        if !Self::is_transition_like_label(&label) {
            return None;
        }
        let destination = self.runtime.graphics.synthetic_splash_destinations.get(&scene).copied().unwrap_or(0);
        if destination == 0 || !self.runtime.graphics.synthetic_sprites.contains_key(&destination) {
            return None;
        }
        Some(destination)
    }

    fn resolve_auto_scene_root(&mut self) -> Option<(u32, String)> {
        let effect = self.active_effect_scene();
        if effect != 0 {
            if let Some(destination) = self.resolve_transition_render_destination(effect) {
                self.remember_auto_scene_root(destination, format!("effect_scene-destination:{}", self.describe_ptr(effect)));
                return Some((destination, format!("effect_scene-destination:{}", self.describe_ptr(effect))));
            }
            let destination = self.runtime.graphics.synthetic_splash_destinations.get(&effect).copied().unwrap_or(0);
            let effect_label = self.diag.object_labels.get(&effect).cloned().unwrap_or_default();
            let effect_children = self.runtime.graphics.synthetic_sprites.get(&effect).map(|state| {
                if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 }
            }).unwrap_or(0);
            let effect_renderable = self.runtime.graphics.synthetic_sprites.get(&effect).map(|state| state.visible).unwrap_or(false)
                && (effect_children != 0 || destination == 0 || !Self::is_transition_like_label(&effect_label));
            if effect_renderable {
                self.remember_auto_scene_root(effect, "effect_scene-live");
                return Some((effect, "effect_scene".to_string()));
            }
            if destination != 0 && self.runtime.graphics.synthetic_sprites.contains_key(&destination) {
                self.remember_auto_scene_root(destination, format!("effect_scene-destination:{}", self.describe_ptr(effect)));
                return Some((destination, format!("effect_scene-destination:{}", self.describe_ptr(effect))));
            }
        }
        if self.runtime.ui_cocos.running_scene != 0 {
            let running = self.runtime.ui_cocos.running_scene;
            if let Some(destination) = self.resolve_transition_render_destination(running) {
                self.remember_auto_scene_root(destination, format!("transition-destination:{}", self.describe_ptr(running)));
                return Some((destination, format!("running_scene-transition-destination:{}", self.describe_ptr(running))));
            }
            let running_label = self.diag.object_labels.get(&running).cloned().unwrap_or_default();
            if Self::is_transition_like_label(&running_label) {
                let running_children = self.runtime.graphics.synthetic_sprites.get(&running).map(|state| {
                    if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 }
                }).unwrap_or(0);
                let running_renderable = self.runtime.graphics.synthetic_sprites.get(&running).map(|state| state.visible).unwrap_or(false)
                    && running_children != 0;
                if running_renderable {
                    self.remember_auto_scene_root(running, "running_scene-transition-live");
                    return Some((running, format!("running_scene-transition:{}", self.describe_ptr(running))));
                }
                if let Some(destination) = self.runtime.graphics.synthetic_splash_destinations.get(&running).copied() {
                    if destination != 0 && self.runtime.graphics.synthetic_sprites.contains_key(&destination) {
                        self.remember_auto_scene_root(destination, format!("transition-destination:{}", self.describe_ptr(running)));
                        return Some((destination, format!("running_scene-transition-destination:{}", self.describe_ptr(running))));
                    }
                }
            }
            self.remember_auto_scene_root(running, "running_scene-live");
            return Some((running, "running_scene".to_string()));
        }
        if self.runtime.scene.auto_scene_inferred_root != 0 && self.runtime.graphics.synthetic_sprites.contains_key(&self.runtime.scene.auto_scene_inferred_root) {
            let source = self.runtime.scene
                .auto_scene_inferred_source
                .clone()
                .unwrap_or_else(|| "inferred".to_string());
            return Some((self.runtime.scene.auto_scene_inferred_root, format!("cached-inferred:{}", source)));
        }
        if self.runtime.scene.auto_scene_cached_root != 0 && self.runtime.graphics.synthetic_sprites.contains_key(&self.runtime.scene.auto_scene_cached_root) {
            let source = self.runtime.scene.auto_scene_cached_source.clone().unwrap_or_else(|| "cached".to_string());
            return Some((self.runtime.scene.auto_scene_cached_root, format!("cached:{}", source)));
        }
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1);
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1);
        let mut best: Option<(u32, String, i32)> = None;
        for (&node, state) in &self.runtime.graphics.synthetic_sprites {
            if node == 0 || state.parent != 0 || !state.visible {
                continue;
            }
            let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
            let child_count = if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 };
            if child_count == 0 {
                continue;
            }
            let scene_like = label.contains("Scene")
                || label.contains("MenuLayer")
                || label.contains("FirstScene")
                || label.contains("CCLayer")
                || label.contains("CCMenu");
            if !scene_like {
                continue;
            }
            let mut score = (child_count as i32).saturating_mul(10);
            if label.contains("FirstScene") {
                score += 140;
            }
            if Self::is_transition_like_label(&label) {
                score -= 200;
            }
            if label.contains("CCScene") || label.contains("Scene.instance") {
                score += 120;
            }
            if label.contains("MenuLayer") {
                score += 90;
            }
            if state.width >= surface_w / 2 {
                score += 8;
            }
            if state.height >= surface_h / 2 {
                score += 8;
            }
            if state.entered {
                score += 6;
            }
            if self.synthetic_node_effective_texture(node) == 0 {
                score += 4;
            }
            let replace = best.as_ref().map(|(_, _, prev)| score > *prev).unwrap_or(true);
            if replace {
                best = Some((node, label, score));
            }
        }
        let resolved = best.map(|(node, label, _)| {
            let child_count = self.runtime.graphics
                .synthetic_sprites
                .get(&node)
                .map(|state| if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 })
                .unwrap_or(0);
            let entered = self.runtime.graphics.synthetic_sprites.get(&node).map(|state| state.entered).unwrap_or(false);
            (
                node,
                format!(
                    "fallback:{} children={} entered={}",
                    if label.is_empty() { format!("0x{node:08x}") } else { label },
                    child_count,
                    if entered { "YES" } else { "NO" },
                ),
            )
        });
        if let Some((node, source)) = resolved.clone() {
            self.remember_auto_scene_root(node, source.clone());
            Some((node, source))
        } else {
            None
        }
    }

    fn bundle_lookup_candidates(name: &str) -> Vec<String> {
        let trimmed = name.trim().trim_matches('\"').trim_matches('\'');
        if trimmed.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let normalized = trimmed.replace('\\', "/").to_ascii_lowercase();
        out.push(normalized.clone());
        let file_name = Path::new(trimmed)
            .file_name()
            .and_then(|v| v.to_str())
            .map(|v| v.to_ascii_lowercase())
            .unwrap_or_else(|| normalized.clone());
        if !out.contains(&file_name) {
            out.push(file_name.clone());
        }
        let stem = Path::new(&file_name)
            .file_stem()
            .and_then(|v| v.to_str())
            .map(|v| v.to_ascii_lowercase())
            .unwrap_or_else(|| file_name.clone());
        if !out.contains(&stem) {
            out.push(stem.clone());
        }
        let stem_png = format!("{stem}.png");
        if !out.contains(&stem_png) {
            out.push(stem_png);
        }
        out
    }

    fn decode_png_rgba(bytes: &[u8]) -> CoreResult<(SyntheticImage, bool, bool)> {
        let saw_cgbi = Self::png_has_cgbi(bytes);
        if saw_cgbi {
            match Self::decode_png_rgba_cgbi_via_pngcrate(bytes) {
                Ok((image, _inflate_mode)) => return Ok((image, true, true)),
                Err(pngcrate_err) => match Self::decode_png_rgba_cgbi(bytes) {
                    Ok(image) => return Ok((image, true, true)),
                    Err(cgbi_err) => match Self::decode_png_rgba_standard(bytes) {
                        Ok(image) => return Ok((image, true, false)),
                        Err(std_err) => {
                            return Err(CoreError::Backend(format!(
                                "png decode failed: cgbi_via_pngcrate={pngcrate_err}; cgbi_manual={cgbi_err}; standard={std_err}"
                            )));
                        }
                    },
                },
            }
        }

        match Self::decode_png_rgba_standard(bytes) {
            Ok(image) => Ok((image, false, false)),
            Err(primary_err) => match Self::decode_png_rgba_cgbi_via_pngcrate(bytes) {
                Ok((image, _inflate_mode)) => Ok((image, true, true)),
                Err(cgbi_pngcrate_err) => match Self::decode_png_rgba_cgbi(bytes) {
                    Ok(image) => Ok((image, true, true)),
                    Err(cgbi_manual_err) => Err(CoreError::Backend(format!(
                        "png decode failed: standard={primary_err}; cgbi_via_pngcrate={cgbi_pngcrate_err}; cgbi_manual={cgbi_manual_err}"
                    ))),
                },
            },
        }
    }

    fn decode_png_rgba_standard(bytes: &[u8]) -> CoreResult<SyntheticImage> {
        let mut decoder = png::Decoder::new(Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder
            .read_info()
            .map_err(|err| CoreError::Backend(format!("png read_info failed: {err}")))?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader
            .next_frame(&mut buf)
            .map_err(|err| CoreError::Backend(format!("png next_frame failed: {err}")))?;
        let src = &buf[..info.buffer_size()];
        let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
        match info.color_type {
            ColorType::Rgba => rgba.extend_from_slice(src),
            ColorType::Rgb => {
                for px in src.chunks_exact(3) {
                    rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
            ColorType::Grayscale => {
                for &g in src {
                    rgba.extend_from_slice(&[g, g, g, 255]);
                }
            }
            ColorType::GrayscaleAlpha => {
                for px in src.chunks_exact(2) {
                    rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
                }
            }
            ColorType::Indexed => {
                return Err(CoreError::Backend("indexed png survived EXPAND transformation".to_string()));
            }
        }
        Ok(SyntheticImage {
            width: info.width,
            height: info.height,
            rgba,
        })
    }

    fn png_has_cgbi(bytes: &[u8]) -> bool {
        Self::parse_png_for_cgbi(bytes)
            .map(|(_, _, _, _, _, has_cgbi, _)| has_cgbi)
            .unwrap_or(false)
    }

    fn parse_png_for_cgbi(bytes: &[u8]) -> CoreResult<(u32, u32, u8, u8, u8, bool, Vec<u8>)> {
        let (_ihdr, width, height, bit_depth, color_type, interlace, has_cgbi, idat) = Self::parse_png_core_chunks_for_cgbi(bytes)?;
        Ok((width, height, bit_depth, color_type, interlace, has_cgbi, idat))
    }

    fn parse_png_core_chunks_for_cgbi(bytes: &[u8]) -> CoreResult<(Vec<u8>, u32, u32, u8, u8, u8, bool, Vec<u8>)> {
        if bytes.len() < 16 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
            return Err(CoreError::Backend("not a PNG stream".to_string()));
        }
        let mut off = 8usize;
        let mut ihdr = Vec::new();
        let mut width = 0u32;
        let mut height = 0u32;
        let mut bit_depth = 0u8;
        let mut color_type = 0u8;
        let mut interlace = 0u8;
        let mut has_cgbi = false;
        let mut idat = Vec::new();
        while off + 8 <= bytes.len() {
            let len = u32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]) as usize;
            let chunk_type = &bytes[off + 4..off + 8];
            off += 8;
            if off + len + 4 > bytes.len() {
                return Err(CoreError::Backend("png chunk exceeds input".to_string()));
            }
            let chunk = &bytes[off..off + len];
            off += len;
            off += 4; // crc
            match chunk_type {
                b"CgBI" => has_cgbi = true,
                b"IHDR" => {
                    if chunk.len() < 13 {
                        return Err(CoreError::Backend("IHDR too short".to_string()));
                    }
                    ihdr = chunk.to_vec();
                    width = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    height = u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                    bit_depth = chunk[8];
                    color_type = chunk[9];
                    interlace = chunk[12];
                }
                b"IDAT" => idat.extend_from_slice(chunk),
                b"IEND" => break,
                _ => {}
            }
        }
        if width == 0 || height == 0 || ihdr.is_empty() {
            return Err(CoreError::Backend("PNG IHDR missing or invalid".to_string()));
        }
        Ok((ihdr, width, height, bit_depth, color_type, interlace, has_cgbi, idat))
    }

    fn inflate_png_payload_with_mode(bytes: &[u8]) -> CoreResult<(Vec<u8>, &'static str)> {
        let mut z = ZlibDecoder::new(bytes);
        let mut out = Vec::new();
        match z.read_to_end(&mut out) {
            Ok(_) => return Ok((out, "zlib")),
            Err(zerr) => {
                let mut def = DeflateDecoder::new(bytes);
                let mut raw = Vec::new();
                match def.read_to_end(&mut raw) {
                    Ok(_) => return Ok((raw, "deflate")),
                    Err(derr) => {
                        return Err(CoreError::Backend(format!(
                            "inflate failed: zlib={zerr}; deflate={derr}"
                        )));
                    }
                }
            }
        }
    }

    fn inflate_png_payload(bytes: &[u8]) -> CoreResult<Vec<u8>> {
        Self::inflate_png_payload_with_mode(bytes).map(|(raw, _)| raw)
    }

    fn png_paeth(a: u8, b: u8, c: u8) -> u8 {
        let a_i = a as i32;
        let b_i = b as i32;
        let c_i = c as i32;
        let p = a_i + b_i - c_i;
        let pa = (p - a_i).abs();
        let pb = (p - b_i).abs();
        let pc = (p - c_i).abs();
        if pa <= pb && pa <= pc {
            a
        } else if pb <= pc {
            b
        } else {
            c
        }
    }

    fn png_unfilter_row(filter: u8, row: &mut [u8], prev: &[u8], bpp: usize) -> CoreResult<()> {
        match filter {
            0 => Ok(()),
            1 => {
                for i in bpp..row.len() {
                    row[i] = row[i].wrapping_add(row[i - bpp]);
                }
                Ok(())
            }
            2 => {
                for (i, byte) in row.iter_mut().enumerate() {
                    *byte = byte.wrapping_add(prev.get(i).copied().unwrap_or(0));
                }
                Ok(())
            }
            3 => {
                for i in 0..row.len() {
                    let left = if i >= bpp { row[i - bpp] } else { 0 };
                    let up = prev.get(i).copied().unwrap_or(0);
                    row[i] = row[i].wrapping_add(((left as u16 + up as u16) / 2) as u8);
                }
                Ok(())
            }
            4 => {
                for i in 0..row.len() {
                    let left = if i >= bpp { row[i - bpp] } else { 0 };
                    let up = prev.get(i).copied().unwrap_or(0);
                    let up_left = if i >= bpp { prev.get(i - bpp).copied().unwrap_or(0) } else { 0 };
                    row[i] = row[i].wrapping_add(Self::png_paeth(left, up, up_left));
                }
                Ok(())
            }
            other => Err(CoreError::Backend(format!("unsupported png filter type {other}"))),
        }
    }

    fn decode_png_rgba_cgbi(bytes: &[u8]) -> CoreResult<SyntheticImage> {
        let (width, height, bit_depth, color_type, interlace, has_cgbi, idat) = Self::parse_png_for_cgbi(bytes)?;
        if !has_cgbi {
            return Err(CoreError::Backend("not a CgBI PNG".to_string()));
        }
        if interlace != 0 {
            return Err(CoreError::Backend("interlaced CgBI PNG is not supported yet".to_string()));
        }
        if bit_depth != 8 {
            return Err(CoreError::Backend(format!("unsupported CgBI bit depth {bit_depth}")));
        }
        let src_bpp = match color_type {
            2 => 3usize,
            6 => 4usize,
            other => {
                return Err(CoreError::Backend(format!(
                    "unsupported CgBI color type {other}"
                )));
            }
        };
        let raw = Self::inflate_png_payload(&idat)?;
        let stride = width as usize * src_bpp;
        let expected = (stride + 1) * height as usize;
        if raw.len() < expected {
            return Err(CoreError::Backend(format!(
                "CgBI payload too short: got {} expected at least {}",
                raw.len(),
                expected
            )));
        }
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        let mut prev = vec![0u8; stride];
        let mut rp = 0usize;

        #[inline]
        fn unpremul(c: u8, a: u8) -> u8 {
            if a == 0 {
                0
            } else if a == 255 {
                c
            } else {
                (((u32::from(c) * 255) + (u32::from(a) / 2)) / u32::from(a)).min(255) as u8
            }
        }

        for y in 0..height as usize {
            let filter = raw[rp];
            rp += 1;
            let mut row = raw[rp..rp + stride].to_vec();
            rp += stride;
            Self::png_unfilter_row(filter, &mut row, &prev, src_bpp)?;
            prev.copy_from_slice(&row);
            if src_bpp == 3 {
                for x in 0..width as usize {
                    let src_idx = x * 3;
                    let dst_idx = (y * width as usize + x) * 4;
                    rgba[dst_idx] = row[src_idx + 2];
                    rgba[dst_idx + 1] = row[src_idx + 1];
                    rgba[dst_idx + 2] = row[src_idx];
                    rgba[dst_idx + 3] = 255;
                }
            } else {
                for x in 0..width as usize {
                    let src_idx = x * 4;
                    let dst_idx = (y * width as usize + x) * 4;
                    let b = row[src_idx];
                    let g = row[src_idx + 1];
                    let r = row[src_idx + 2];
                    let a = row[src_idx + 3];
                    rgba[dst_idx] = unpremul(r, a);
                    rgba[dst_idx + 1] = unpremul(g, a);
                    rgba[dst_idx + 2] = unpremul(b, a);
                    rgba[dst_idx + 3] = a;
                }
            }
        }
        Ok(SyntheticImage { width, height, rgba })
    }

    fn png_crc32_bytes(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg() & 0xedb8_8320;
                crc = (crc >> 1) ^ mask;
            }
        }
        !crc
    }

    fn png_append_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_in = Vec::with_capacity(4 + data.len());
        crc_in.extend_from_slice(kind);
        crc_in.extend_from_slice(data);
        out.extend_from_slice(&Self::png_crc32_bytes(&crc_in).to_be_bytes());
    }

    fn build_standard_png_stream(ihdr: &[u8], compressed_idat: &[u8]) -> CoreResult<Vec<u8>> {
        if ihdr.len() != 13 {
            return Err(CoreError::Backend(format!("unexpected IHDR length {}", ihdr.len())));
        }
        let mut out = Vec::with_capacity(8 + 12 + ihdr.len() + 12 + compressed_idat.len() + 12);
        out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        Self::png_append_chunk(&mut out, b"IHDR", ihdr);
        Self::png_append_chunk(&mut out, b"IDAT", compressed_idat);
        Self::png_append_chunk(&mut out, b"IEND", &[]);
        Ok(out)
    }

    fn standardize_cgbi_png_stream(bytes: &[u8]) -> CoreResult<(Vec<u8>, &'static str, u8, u8)> {
        let (ihdr, _width, _height, _bit_depth, color_type, interlace, has_cgbi, idat) =
            Self::parse_png_core_chunks_for_cgbi(bytes)?;
        if !has_cgbi {
            return Err(CoreError::Backend("not a CgBI PNG".to_string()));
        }
        match color_type {
            2 | 6 => {}
            other => {
                return Err(CoreError::Backend(format!(
                    "unsupported CgBI color type {other}"
                )));
            }
        }
        // Apple-optimized CgBI assets may be Adam7 interlaced. In that case the inflated
        // payload does not match a simple (stride + filter) * height layout, but it is still
        // a valid PNG scanline stream once we wrap the raw-deflate payload back into a normal
        // zlib stream and keep the original IHDR/interlace metadata intact.
        let (raw, inflate_mode) = Self::inflate_png_payload_with_mode(&idat)?;
        let mut enc = FlateZlibEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut enc, &raw)
            .map_err(|err| CoreError::Backend(format!("zlib rewrap write failed: {err}")))?;
        let compressed = enc
            .finish()
            .map_err(|err| CoreError::Backend(format!("zlib rewrap finish failed: {err}")))?;
        let stream = Self::build_standard_png_stream(&ihdr, &compressed)?;
        Ok((stream, inflate_mode, color_type, interlace))
    }

    fn decode_png_rgba_cgbi_via_pngcrate(bytes: &[u8]) -> CoreResult<(SyntheticImage, &'static str)> {
        let (standard_png, inflate_mode, color_type, interlace) = Self::standardize_cgbi_png_stream(bytes)?;
        let mut decoded = Self::decode_png_rgba_standard(&standard_png)?;

        // Important: there are two real CgBI populations in the wild.
        //
        // * Non-interlaced assets (Alive4ever-style) still arrive from the rewrapped
        //   png crate path with B/G/R ordering and premultiplied alpha semantics, so we
        //   must normalize them here.
        // * Adam7-interlaced assets (AgeOfWar-style) already come back from the rewrapped
        //   standard PNG decode in canonical RGBA order. Applying the swap/unpremul logic
        //   again double-corrects them and turns the scene blue.
        //
        // Keep the correction tied to the actual PNG encoding mode instead of a title-
        // specific heuristic so the fix scales across apps.
        if interlace == 0 {
            match color_type {
                2 => {
                    decoded.rgba = Self::swap_red_blue_rgba(&decoded.rgba);
                }
                6 => {
                    for px in decoded.rgba.chunks_exact_mut(4) {
                        let b = px[0];
                        let g = px[1];
                        let r = px[2];
                        let a = px[3];
                        let unpremul = |c: u8, a: u8| -> u8 {
                            if a == 0 {
                                0
                            } else {
                                (((c as u32) * 255 + (a as u32 / 2)) / a as u32).min(255) as u8
                            }
                        };
                        px[0] = unpremul(r, a);
                        px[1] = unpremul(g, a);
                        px[2] = unpremul(b, a);
                        px[3] = a;
                    }
                }
                _ => {}
            }
        }
        Ok((decoded, inflate_mode))
    }

    fn png_filter_histogram(raw: &[u8], height: u32, stride: usize) -> [u32; 5] {
        let mut hist = [0u32; 5];
        let mut rp = 0usize;
        for _ in 0..height as usize {
            if rp >= raw.len() { break; }
            let filter = raw[rp] as usize;
            if filter < hist.len() {
                hist[filter] = hist[filter].saturating_add(1);
            }
            rp = rp.saturating_add(stride + 1);
        }
        hist
    }

    fn image_diff_summary(a: &[u8], b: &[u8]) -> (usize, u8, [f64; 4]) {
        let px_count = a.len().min(b.len()) / 4;
        if px_count == 0 { return (0, 0, [0.0; 4]); }
        let mut diff_px = 0usize;
        let mut max_abs = 0u8;
        let mut sum = [0u64; 4];
        for i in 0..px_count {
            let mut any = false;
            for c in 0..4 {
                let av = a[i * 4 + c];
                let bv = b[i * 4 + c];
                let d = av.abs_diff(bv);
                if d != 0 { any = true; }
                if d > max_abs { max_abs = d; }
                sum[c] = sum[c].saturating_add(d as u64);
            }
            if any { diff_px += 1; }
        }
        let denom = px_count as f64;
        (diff_px, max_abs, [sum[0] as f64 / denom, sum[1] as f64 / denom, sum[2] as f64 / denom, sum[3] as f64 / denom])
    }

    fn load_png_image_from_path(&mut self, key: &str, path: &Path) -> Option<u32> {
        let bytes = fs::read(path).ok()?;
        let bg_probe = key.eq_ignore_ascii_case("menu_background.png")
            || path.file_name().and_then(|v| v.to_str()).map(|v| v.eq_ignore_ascii_case("menu_background.png")).unwrap_or(false);
        let (mut image, saw_cgbi, used_cgbi) = match Self::decode_png_rgba(&bytes) {
            Ok(v) => v,
            Err(_err) => {
                self.runtime.fs.png_decode_failures = self.runtime.fs.png_decode_failures.saturating_add(1);
                return None;
            }
        };
        let image_xform = self
            .active_profile()
            .bundle_image_channel_transform(key, path, saw_cgbi, used_cgbi);
        match image_xform {
            crate::runtime::profiles::SyntheticImageChannelTransform::None => {}
            crate::runtime::profiles::SyntheticImageChannelTransform::SwapRedBlue => {
                image.rgba = Self::swap_red_blue_rgba(&image.rgba);
                self.diag.trace.push(format!(
                    "     ↳ bundle-image-xform key={} path={} xform=swap-rb sawCgbi={} usedCgbi={} fp={}",
                    key,
                    path.display(),
                    if saw_cgbi { "YES" } else { "NO" },
                    if used_cgbi { "YES" } else { "NO" },
                    sample_rgba_fingerprint(&image.rgba, image.width.max(1), image.height.max(1)),
                ));
            }
        }
        if bg_probe && saw_cgbi {
            if let Ok((ihdr, width, height, _bit_depth, color_type, _interlace, _has_cgbi, idat)) = Self::parse_png_core_chunks_for_cgbi(&bytes) {
                if let Ok((raw, inflate_mode)) = Self::inflate_png_payload_with_mode(&idat) {
                    let src_bpp = match color_type { 2 => 3usize, 6 => 4usize, _ => 0usize };
                    if src_bpp != 0 {
                        let stride = width as usize * src_bpp;
                        let hist = Self::png_filter_histogram(&raw, height, stride);
                        let expected = (stride + 1) * height as usize;
                        let ref_probe = Self::decode_png_rgba_cgbi_via_pngcrate(&bytes);
                        match ref_probe {
                            Ok((mut ref_img, ref_inflate_mode)) => {
                                match image_xform {
                                    crate::runtime::profiles::SyntheticImageChannelTransform::None => {}
                                    crate::runtime::profiles::SyntheticImageChannelTransform::SwapRedBlue => {
                                        ref_img.rgba = Self::swap_red_blue_rgba(&ref_img.rgba);
                                    }
                                }
                                let (diff_px, max_abs, avg_abs) = Self::image_diff_summary(&image.rgba, &ref_img.rgba);
                                self.diag.trace.push(format!(
                                    "     ↳ ab-bgscan file={} key={} cgbi=YES colorType={} inflate={} refInflate={} raw={} expected={} ihdr={} filters=[{},{},{},{},{}] curFp={} refFp={} refMatch={} diffPx={} maxAbs={} avgAbs={:.3}/{:.3}/{:.3}/{:.3}",
                                    path.display(),
                                    key,
                                    color_type,
                                    inflate_mode,
                                    ref_inflate_mode,
                                    raw.len(),
                                    expected,
                                    ihdr.len(),
                                    hist[0], hist[1], hist[2], hist[3], hist[4],
                                    sample_rgba_fingerprint(&image.rgba, image.width.max(1), image.height.max(1)),
                                    sample_rgba_fingerprint(&ref_img.rgba, ref_img.width.max(1), ref_img.height.max(1)),
                                    if diff_px == 0 { "YES" } else { "NO" },
                                    diff_px,
                                    max_abs,
                                    avg_abs[0], avg_abs[1], avg_abs[2], avg_abs[3],
                                ));
                            }
                            Err(err) => {
                                self.diag.trace.push(format!(
                                    "     ↳ ab-bgscan file={} key={} cgbi=YES colorType={} inflate={} raw={} expected={} ihdr={} refDecode=ERR({})",
                                    path.display(), key, color_type, inflate_mode, raw.len(), expected, ihdr.len(), err
                                ));
                            }
                        }
                    }
                }
            }
        }
        if saw_cgbi {
            self.runtime.fs.png_cgbi_detected = self.runtime.fs.png_cgbi_detected.saturating_add(1);
        }
        if used_cgbi {
            self.runtime.fs.png_cgbi_decoded = self.runtime.fs.png_cgbi_decoded.saturating_add(1);
        }
        let label = format!("UIImage.bundle<'{}'>", path.file_name().and_then(|v| v.to_str()).unwrap_or(key));
        let obj = self.alloc_synthetic_ui_object(label.clone());
        let image_width = image.width;
        let image_height = image.height;
        let image_fp = sample_rgba_fingerprint(&image.rgba, image.width.max(1), image.height.max(1));
        self.runtime.graphics.synthetic_images.insert(obj, image);
        self.runtime.fs.resource_image_cache.insert(key.to_string(), obj);
        self.runtime.graphics.last_uikit_image_object = obj;
        self.runtime.ui_graphics.graphics_uikit_images_created = self.runtime.ui_graphics.graphics_uikit_images_created.saturating_add(1);
        self.runtime.ui_graphics.graphics_last_ui_source = Some("UIImage.imageNamed".to_string());
        if bg_probe {
            self.diag.trace.push(format!(
                "     ↳ ab-bgimg-create image={} label={} key={} path={} size={}x{} sawCgbi={} usedCgbi={} fp={}",
                self.describe_ptr(obj),
                label,
                key,
                path.display(),
                image_width,
                image_height,
                if saw_cgbi { "YES" } else { "NO" },
                if used_cgbi { "YES" } else { "NO" },
                image_fp,
            ));
        }
        Some(obj)
    }

    fn load_bundle_image_named(&mut self, name: &str) -> Option<u32> {
        let candidates = Self::bundle_lookup_candidates(name);
        self.runtime.fs.last_resource_name = Some(name.to_string());
        if candidates.is_empty() {
            self.runtime.fs.image_named_misses = self.runtime.fs.image_named_misses.saturating_add(1);
            self.runtime.fs.last_resource_path = None;
            return None;
        }
        for key in &candidates {
            if let Some(obj) = self.runtime.fs.resource_image_cache.get(key).copied() {
                self.runtime.fs.image_named_hits = self.runtime.fs.image_named_hits.saturating_add(1);
                if let Some(path) = self.runtime.fs.bundle_resource_index.get(key) {
                    self.runtime.fs.last_resource_path = Some(path.display().to_string());
                }
                return Some(obj);
            }
        }
        let matched = candidates
            .iter()
            .find_map(|key| self.runtime.fs.bundle_resource_index.get(key).cloned().map(|path| (key.clone(), path)));
        let Some((key, path)) = matched else {
            self.runtime.fs.image_named_misses = self.runtime.fs.image_named_misses.saturating_add(1);
            self.runtime.fs.last_resource_path = None;
            return None;
        };
        if !path.extension().and_then(|v| v.to_str()).map(|v| v.eq_ignore_ascii_case("png")).unwrap_or(false) {
            self.runtime.fs.image_named_misses = self.runtime.fs.image_named_misses.saturating_add(1);
            self.runtime.fs.last_resource_path = Some(path.display().to_string());
            return None;
        }
        self.runtime.fs.last_resource_path = Some(path.display().to_string());
        match self.load_png_image_from_path(&key, &path) {
            Some(obj) => {
                self.runtime.fs.image_named_hits = self.runtime.fs.image_named_hits.saturating_add(1);
                Some(obj)
            }
            None => {
                self.runtime.fs.image_named_misses = self.runtime.fs.image_named_misses.saturating_add(1);
                None
            }
        }
    }

    fn resolve_bundle_resource_path(&self, name: &str, ext: Option<&str>) -> Option<PathBuf> {
        let mut query = name.to_string();
        if let Some(ext) = ext {
            let ext = ext.trim().trim_matches('"').trim_matches('\'');
            let ext_suffix = format!(".{}", ext.to_ascii_lowercase());
            if !ext.is_empty() && !name.to_ascii_lowercase().ends_with(&ext_suffix) {
                query = format!("{name}.{ext}");
            }
        }
        let candidates = Self::bundle_lookup_candidates(&query);
        candidates
            .into_iter()
            .find_map(|key| self.runtime.fs.bundle_resource_index.get(&key).cloned())
    }

    fn resolve_bundle_lookup_hit(&self, name: &str) -> Option<(String, PathBuf)> {
        let candidates = Self::bundle_lookup_candidates(name);
        candidates
            .into_iter()
            .find_map(|key| self.runtime.fs.bundle_resource_index.get(&key).cloned().map(|path| (key, path)))
    }

    fn bundle_root_for_receiver(&self, receiver: u32) -> Option<PathBuf> {
        self.runtime.fs.bundle_roots
            .get(&receiver)
            .cloned()
            .or_else(|| {
                if receiver == HLE_FAKE_MAIN_BUNDLE {
                    self.runtime.fs.bundle_root.clone()
                } else {
                    None
                }
            })
    }

    fn bundle_root_string_for_receiver(&self, receiver: u32) -> Option<String> {
        self.bundle_root_for_receiver(receiver)
            .map(|path| path.display().to_string())
    }

    fn resolve_bundle_directory_path(&self, request: &str) -> Option<PathBuf> {
        let trimmed = request.trim().trim_matches('"').trim_matches('\'');
        if trimmed.is_empty() {
            return None;
        }
        let direct = PathBuf::from(trimmed);
        if direct.is_dir() {
            return Some(direct);
        }
        if let Some(root) = &self.runtime.fs.bundle_root {
            let normalized = trimmed.replace('\\', "/");
            let joined = root.join(normalized.trim_start_matches('/'));
            if joined.is_dir() {
                return Some(joined);
            }
            if let Some((_, suffix)) = normalized.rsplit_once(".app/") {
                let app_joined = root.join(suffix);
                if app_joined.is_dir() {
                    return Some(app_joined);
                }
            }
        }
        None
    }

    fn materialize_bundle_object(&mut self, label: &str, root: PathBuf) -> u32 {
        let obj = self.alloc_synthetic_ui_object(label.to_string());
        self.runtime.fs.bundle_roots.insert(obj, root);
        self.runtime.fs.bundle_objects_created = self.runtime.fs.bundle_objects_created.saturating_add(1);
        obj
    }

    fn resolve_bundle_resource_path_for_receiver(&mut self, receiver: u32, name: &str, ext: Option<&str>) -> Option<PathBuf> {
        let mut query = name.to_string();
        if let Some(ext) = ext {
            let ext = ext.trim().trim_matches('"').trim_matches('\'');
            let ext_suffix = format!(".{}", ext.to_ascii_lowercase());
            if !ext.is_empty() && !name.to_ascii_lowercase().ends_with(&ext_suffix) {
                query = format!("{name}.{ext}");
            }
        }
        let root = match self.bundle_root_for_receiver(receiver) {
            Some(root) => root,
            None => {
                self.runtime.fs.bundle_scoped_misses = self.runtime.fs.bundle_scoped_misses.saturating_add(1);
                return None;
            }
        };
        let normalized = query.trim().trim_matches('"').trim_matches('\'').replace('\\', "/");
        if !normalized.is_empty() {
            let direct = root.join(normalized.trim_start_matches('/'));
            if direct.is_file() {
                self.runtime.fs.bundle_scoped_hits = self.runtime.fs.bundle_scoped_hits.saturating_add(1);
                return Some(direct);
            }
        }
        let candidates = Self::bundle_lookup_candidates(&query);
        if let Some(main_root) = &self.runtime.fs.bundle_root {
            if let Ok(rel) = root.strip_prefix(main_root) {
                let prefix = rel.to_string_lossy().replace('\\', "/").trim_matches('/').to_ascii_lowercase();
                if !prefix.is_empty() {
                    for key in &candidates {
                        let scoped = format!("{}/{}", prefix, key.trim_start_matches('/'));
                        if let Some(path) = self.runtime.fs.bundle_resource_index.get(&scoped).cloned() {
                            self.runtime.fs.bundle_scoped_hits = self.runtime.fs.bundle_scoped_hits.saturating_add(1);
                            return Some(path);
                        }
                    }
                }
            }
        }
        for key in candidates {
            if let Some(path) = self.runtime.fs.bundle_resource_index.get(&key).cloned() {
                self.runtime.fs.bundle_scoped_hits = self.runtime.fs.bundle_scoped_hits.saturating_add(1);
                return Some(path);
            }
        }
        self.runtime.fs.bundle_scoped_misses = self.runtime.fs.bundle_scoped_misses.saturating_add(1);
        None
    }

    fn resolve_bundle_resource_path_for_receiver_in_directory(
        &mut self,
        receiver: u32,
        name: Option<&str>,
        ext: Option<&str>,
        directory: Option<&str>,
    ) -> Option<PathBuf> {
        let root = match self.bundle_root_for_receiver(receiver) {
            Some(root) => root,
            None => {
                self.runtime.fs.bundle_scoped_misses = self.runtime.fs.bundle_scoped_misses.saturating_add(1);
                return None;
            }
        };

        let directory = directory
            .map(|value| value.trim().trim_matches('"').trim_matches('\''))
            .filter(|value| !value.is_empty());
        let scoped_root = if let Some(directory) = directory {
            let normalized = directory.replace('\\', "/");
            if let Some((_, suffix)) = normalized.rsplit_once(".app/") {
                root.join(suffix.trim_start_matches('/'))
            } else {
                root.join(normalized.trim_start_matches('/'))
            }
        } else {
            root.clone()
        };

        let name = name
            .map(|value| value.trim().trim_matches('"').trim_matches('\''))
            .filter(|value| !value.is_empty());
        let ext = ext
            .map(|value| value.trim().trim_matches('"').trim_matches('\''))
            .filter(|value| !value.is_empty());

        if name.is_none() && ext.is_none() {
            if scoped_root.exists() {
                self.runtime.fs.bundle_scoped_hits = self.runtime.fs.bundle_scoped_hits.saturating_add(1);
                return Some(scoped_root);
            }
            self.runtime.fs.bundle_scoped_misses = self.runtime.fs.bundle_scoped_misses.saturating_add(1);
            return None;
        }

        let mut query = name.unwrap_or_default().to_string();
        if let Some(ext) = ext {
            let ext_suffix = format!(".{}", ext.to_ascii_lowercase());
            if !query.is_empty() && !query.to_ascii_lowercase().ends_with(&ext_suffix) {
                query.push('.');
                query.push_str(ext);
            } else if query.is_empty() {
                query = ext.to_string();
            }
        }

        let normalized = query.trim().trim_matches('"').trim_matches('\'').replace('\\', "/");
        if !normalized.is_empty() {
            let direct = scoped_root.join(normalized.trim_start_matches('/'));
            if direct.is_file() || direct.is_dir() {
                self.runtime.fs.bundle_scoped_hits = self.runtime.fs.bundle_scoped_hits.saturating_add(1);
                return Some(direct);
            }
        }

        let candidates = Self::bundle_lookup_candidates(&query);
        if let Ok(rel) = scoped_root.strip_prefix(&root) {
            let prefix = rel.to_string_lossy().replace('\\', "/").trim_matches('/').to_ascii_lowercase();
            if !prefix.is_empty() {
                for key in &candidates {
                    let scoped = format!("{}/{}", prefix, key.trim_start_matches('/'));
                    if let Some(path) = self.runtime.fs.bundle_resource_index.get(&scoped).cloned() {
                        self.runtime.fs.bundle_scoped_hits = self.runtime.fs.bundle_scoped_hits.saturating_add(1);
                        return Some(path);
                    }
                }
            }
        }
        for key in candidates {
            if let Some(path) = self.runtime.fs.bundle_resource_index.get(&key).cloned() {
                self.runtime.fs.bundle_scoped_hits = self.runtime.fs.bundle_scoped_hits.saturating_add(1);
                return Some(path);
            }
        }

        self.runtime.fs.bundle_scoped_misses = self.runtime.fs.bundle_scoped_misses.saturating_add(1);
        None
    }

    fn bundle_root_string(&self) -> Option<String> {
        self.runtime.fs.bundle_root.as_ref().map(|path| path.display().to_string())
    }

    fn resolve_bundle_file_path(&self, request: &str) -> Option<PathBuf> {
        let trimmed = request.trim().trim_matches('"').trim_matches('\'');
        if trimmed.is_empty() {
            return None;
        }
        let direct = PathBuf::from(trimmed);
        if direct.is_file() {
            return Some(direct);
        }
        if let Some(root) = &self.runtime.fs.bundle_root {
            let normalized_trimmed = trimmed.replace('\\', "/");
            let joined = root.join(normalized_trimmed.trim_start_matches('/'));
            if joined.is_file() {
                return Some(joined);
            }
            if let Some((_, suffix)) = normalized_trimmed.rsplit_once(".app/") {
                let joined = root.join(suffix);
                if joined.is_file() {
                    return Some(joined);
                }
            }
        }
        let candidates = Self::bundle_lookup_candidates(trimmed);
        candidates
            .into_iter()
            .find_map(|key| self.runtime.fs.bundle_resource_index.get(&key).cloned())
    }

    fn resolve_bundle_file_path_for_receiver(&self, receiver: u32, request: &str) -> Option<PathBuf> {
        let trimmed = request.trim().trim_matches('"').trim_matches('\'');
        if trimmed.is_empty() {
            return None;
        }
        let direct = PathBuf::from(trimmed);
        if direct.is_file() {
            return Some(direct);
        }
        if let Some(root) = self.bundle_root_for_receiver(receiver) {
            let normalized_trimmed = trimmed.replace('\\', "/");
            let joined = root.join(normalized_trimmed.trim_start_matches('/'));
            if joined.is_file() {
                return Some(joined);
            }
            if let Some(main_root) = &self.runtime.fs.bundle_root {
                if let Ok(rel) = root.strip_prefix(main_root) {
                    let prefix = rel.to_string_lossy().replace('\\', "/").trim_matches('/').to_ascii_lowercase();
                    let candidates = Self::bundle_lookup_candidates(trimmed);
                    if !prefix.is_empty() {
                        for key in &candidates {
                            let scoped = format!("{}/{}", prefix, key.trim_start_matches('/'));
                            if let Some(path) = self.runtime.fs.bundle_resource_index.get(&scoped).cloned() {
                                return Some(path);
                            }
                        }
                    }
                    for key in candidates {
                        if let Some(path) = self.runtime.fs.bundle_resource_index.get(&key).cloned() {
                            return Some(path);
                        }
                    }
                }
            }
        }
        self.resolve_bundle_file_path(trimmed)
    }

    fn synthetic_file_url_absolute_string(path: &str, is_directory: bool) -> String {
        fn percent_encode_path_component(text: &str) -> String {
            let mut out = String::with_capacity(text.len());
            for &b in text.as_bytes() {
                let keep = matches!(b,
                    b'A'..=b'Z' |
                    b'a'..=b'z' |
                    b'0'..=b'9' |
                    b'-' | b'_' | b'.' | b'~' | b'/'
                );
                if keep {
                    out.push(char::from(b));
                } else {
                    let _ = std::fmt::Write::write_fmt(&mut out, format_args!("%{:02X}", b));
                }
            }
            out
        }

        let mut normalized = path.replace('\\', "/");
        if !normalized.starts_with('/') {
            normalized.insert(0, '/');
        }
        if is_directory && !normalized.ends_with('/') {
            normalized.push('/');
        }
        format!("file://{}", percent_encode_path_component(&normalized))
    }

    fn synthetic_file_url_path(&self, object: u32) -> Option<String> {
        self.runtime.fs.synthetic_file_urls.get(&object).map(|entry| entry.original_path.clone())
    }

    fn synthetic_file_url_host_path(&self, object: u32) -> Option<PathBuf> {
        self.runtime
            .fs
            .synthetic_file_urls
            .get(&object)
            .and_then(|entry| entry.host_path.as_ref().map(PathBuf::from))
    }

    fn synthetic_file_url_absolute_string_value(&self, object: u32) -> Option<String> {
        self.runtime.fs.synthetic_file_urls.get(&object).map(|entry| entry.absolute_string.clone())
    }

    fn synthetic_file_url_path_extension_value(&self, object: u32) -> Option<String> {
        self.runtime.fs.synthetic_file_urls.get(&object).map(|entry| entry.path_extension.clone())
    }

    fn synthetic_file_url_last_path_component_value(&self, object: u32) -> Option<String> {
        self.runtime.fs.synthetic_file_urls.get(&object).map(|entry| entry.last_path_component.clone())
    }

    fn synthetic_file_url_is_directory(&self, object: u32) -> bool {
        self.runtime
            .fs
            .synthetic_file_urls
            .get(&object)
            .map(|entry| entry.is_directory)
            .unwrap_or(false)
    }

    fn resolve_existing_path_for_request(&self, request: &str, is_directory: bool) -> Option<PathBuf> {
        let trimmed = request.trim().trim_matches('"').trim_matches('\'');
        if trimmed.is_empty() {
            return None;
        }
        if is_directory {
            self.resolve_bundle_directory_path(trimmed)
        } else {
            self.resolve_bundle_file_path(trimmed).or_else(|| self.resolve_bundle_directory_path(trimmed))
        }
    }

    fn url_like_scheme(request: &str) -> Option<String> {
        let trimmed = request.trim();
        let scheme_end = trimmed.find(':')?;
        if scheme_end == 0 {
            return None;
        }
        if scheme_end == 1 {
            let bytes = trimmed.as_bytes();
            if matches!(bytes.get(2).copied(), Some(b'\\') | Some(b'/')) {
                return None;
            }
        }
        let scheme = &trimmed[..scheme_end];
        if !scheme
            .bytes()
            .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'-' | b'.'))
        {
            return None;
        }
        Some(scheme.to_ascii_lowercase())
    }

    fn percent_decode_url_text(text: &str) -> String {
        fn hex_value(byte: u8) -> Option<u8> {
            match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(10 + (byte - b'a')),
                b'A'..=b'F' => Some(10 + (byte - b'A')),
                _ => None,
            }
        }

        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut idx = 0usize;
        while idx < bytes.len() {
            if bytes[idx] == b'%' && idx + 2 < bytes.len() {
                if let (Some(hi), Some(lo)) = (hex_value(bytes[idx + 1]), hex_value(bytes[idx + 2])) {
                    out.push((hi << 4) | lo);
                    idx += 3;
                    continue;
                }
            }
            out.push(bytes[idx]);
            idx += 1;
        }
        String::from_utf8_lossy(&out).to_string()
    }

    fn canonicalize_url_like_path_text(&self, request: &str, is_directory: bool) -> Option<String> {
        let trimmed = request.trim().trim_matches('"').trim_matches('\'');
        if trimmed.is_empty() {
            return None;
        }
        match Self::url_like_scheme(trimmed).as_deref() {
            Some("file") => {
                let suffix = trimmed
                    .get(trimmed.find(':')?.saturating_add(1)..)
                    .unwrap_or_default()
                    .trim_start_matches('/');
                let suffix = suffix.strip_prefix("localhost/").unwrap_or(suffix);
                let decoded = Self::percent_decode_url_text(&format!("/{}", suffix));
                let normalized = if decoded.starts_with('/') {
                    decoded
                } else {
                    format!("/{decoded}")
                };
                Some(normalized)
            }
            Some(_) => None,
            None => {
                let direct = Self::percent_decode_url_text(trimmed);
                if direct.is_empty() {
                    None
                } else if let Some(resolved) = self.resolve_existing_path_for_request(&direct, is_directory) {
                    Some(resolved.display().to_string())
                } else {
                    Some(direct)
                }
            }
        }
    }

    fn resolve_path_from_url_request(&self, request: &str, is_directory: bool) -> Option<PathBuf> {
        let trimmed = request.trim().trim_matches('"').trim_matches('\'');
        if trimmed.is_empty() {
            return None;
        }

        if let Some(canonical) = self.canonicalize_url_like_path_text(trimmed, is_directory) {
            if let Some(resolved) = self.resolve_existing_path_for_request(&canonical, is_directory) {
                return Some(resolved);
            }
            let direct = PathBuf::from(canonical.trim());
            let exists = if is_directory { direct.is_dir() } else { direct.is_file() || direct.is_dir() };
            if exists {
                return Some(direct);
            }
        }

        let direct = PathBuf::from(trimmed);
        let exists = if is_directory { direct.is_dir() } else { direct.is_file() || direct.is_dir() };
        if exists {
            return Some(direct);
        }
        None
    }

    fn create_synthetic_file_url_from_string_request(&mut self, request: &str, is_directory: bool) -> Option<u32> {
        let path_text = self.canonicalize_url_like_path_text(request, is_directory)?;
        let resolved = self.resolve_path_from_url_request(&path_text, is_directory);
        let host_path = resolved.as_ref().map(|path| path.display().to_string());
        let absolute_seed = host_path.clone().unwrap_or_else(|| path_text.clone());
        let absolute_string = Self::synthetic_file_url_absolute_string(&absolute_seed, is_directory);
        let path_for_parts = Path::new(host_path.as_deref().unwrap_or(&path_text));
        let path_extension = path_for_parts.extension().and_then(|v| v.to_str()).unwrap_or_default().to_string();
        let last_path_component = path_for_parts.file_name().and_then(|v| v.to_str()).unwrap_or_default().to_string();
        let label_name = if last_path_component.is_empty() {
            path_text.clone()
        } else {
            last_path_component.clone()
        };
        let obj = self.alloc_synthetic_ui_object(format!("CFURL.file<'{}'>", label_name));
        self.runtime.fs.synthetic_file_urls.insert(
            obj,
            SyntheticFileUrlState {
                original_path: path_text,
                host_path,
                is_directory,
                absolute_string,
                path_extension,
                last_path_component,
            },
        );
        self.diag
            .object_labels
            .entry(obj)
            .or_insert_with(|| format!("CFURL.file<'{}'>", label_name));
        Some(obj)
    }

    fn url_like_debug_summary(&self, value: u32, is_directory: bool) -> String {
        fn sanitize(text: &str) -> String {
            text.replace('\n', "\\n")
        }

        if value == 0 {
            return "value=nil".to_string();
        }

        let mut parts = vec![format!("value={}", self.describe_ptr(value))];
        if let Some(class_name) = self.objc_receiver_class_name_hint(value) {
            parts.push(format!("class={class_name}"));
        }
        if let Some(path) = self.synthetic_file_url_path(value) {
            parts.push(format!("path='{}'", sanitize(&path)));
        }
        if let Some(abs) = self.synthetic_file_url_absolute_string_value(value) {
            parts.push(format!("absolute='{}'", sanitize(&abs)));
        }
        if let Some(host) = self.synthetic_file_url_host_path(value) {
            let exists = if is_directory { host.is_dir() } else { host.is_file() || host.is_dir() };
            parts.push(format!("host='{}'", sanitize(&host.display().to_string())));
            parts.push(format!("hostExists={}", if exists { "YES" } else { "NO" }));
        }
        if let Some(text) = self.guest_string_value(value) {
            let scheme = Self::url_like_scheme(&text).unwrap_or_else(|| "<none>".to_string());
            parts.push(format!("text='{}'", sanitize(&text)));
            parts.push(format!("scheme={scheme}"));
            if let Some(path_text) = self.canonicalize_url_like_path_text(&text, is_directory) {
                parts.push(format!("pathText='{}'", sanitize(&path_text)));
            }
        }
        match self.resolve_path_from_url_like_value(value, is_directory) {
            Some(path) => {
                let exists = if is_directory { path.is_dir() } else { path.is_file() || path.is_dir() };
                parts.push(format!("resolved='{}'", sanitize(&path.display().to_string())));
                parts.push(format!("resolvedExists={}", if exists { "YES" } else { "NO" }));
            }
            None => parts.push("resolved=<none>".to_string()),
        }
        if let Some(root) = self.bundle_root_string() {
            parts.push(format!("bundleRoot='{}'", sanitize(&root)));
        }
        parts.join(" ")
    }

    fn resolve_path_from_url_like_value(&self, value: u32, is_directory: bool) -> Option<PathBuf> {
        if value == 0 {
            return None;
        }
        if let Some(path) = self.synthetic_file_url_host_path(value) {
            return Some(path);
        }
        if let Some(text) = self.synthetic_file_url_path(value) {
            return self.resolve_path_from_url_request(&text, is_directory);
        }
        if let Some(text) = self.guest_string_value(value) {
            return self.resolve_path_from_url_request(&text, is_directory);
        }
        None
    }

    fn create_synthetic_file_url_from_fs_representation(&mut self, buffer_ptr: u32, buf_len: u32, is_directory: bool) -> CoreResult<u32> {
        let mut raw = if buffer_ptr != 0 && buf_len != 0 {
            self.read_guest_bytes(buffer_ptr, buf_len)?
        } else {
            Vec::new()
        };
        if let Some(nul) = raw.iter().position(|b| *b == 0) {
            raw.truncate(nul);
        }
        let original_path = String::from_utf8_lossy(&raw).trim().trim_matches('"').trim_matches('\'').to_string();
        let resolved = self.resolve_existing_path_for_request(&original_path, is_directory).or_else(|| {
            let p = PathBuf::from(original_path.trim());
            if p.exists() { Some(p) } else { None }
        });
        let host_path = resolved.as_ref().map(|path| path.display().to_string());
        let absolute_seed = host_path.clone().unwrap_or_else(|| original_path.clone());
        let absolute_string = Self::synthetic_file_url_absolute_string(&absolute_seed, is_directory);
        let path_for_parts = Path::new(host_path.as_deref().unwrap_or(&original_path));
        let path_extension = path_for_parts.extension().and_then(|v| v.to_str()).unwrap_or_default().to_string();
        let last_path_component = path_for_parts.file_name().and_then(|v| v.to_str()).unwrap_or_default().to_string();
        let label_name = if last_path_component.is_empty() {
            original_path.clone()
        } else {
            last_path_component.clone()
        };
        let obj = self.alloc_synthetic_ui_object(format!("CFURL.file<'{}'>", label_name));
        self.runtime.fs.synthetic_file_urls.insert(
            obj,
            SyntheticFileUrlState {
                original_path,
                host_path,
                is_directory,
                absolute_string,
                path_extension,
                last_path_component,
            },
        );
        self.diag
            .object_labels
            .entry(obj)
            .or_insert_with(|| format!("CFURL.file<'{}'>", label_name));
        Ok(obj)
    }

    fn create_synthetic_data_provider_from_url(&mut self, url: u32) -> Option<u32> {
        let path = self.resolve_path_from_url_like_value(url, false)?;
        let data = fs::read(&path).ok()?;
        let obj = self.alloc_synthetic_ui_object(format!(
            "CGDataProvider.synthetic<'{}'>",
            path.file_name().and_then(|v| v.to_str()).unwrap_or("file")
        ));
        self.runtime.fs.synthetic_data_providers.insert(
            obj,
            SyntheticDataProviderState {
                url_object: url,
                path: path.display().to_string(),
                byte_len: data.len().min(u32::MAX as usize) as u32,
            },
        );
        self.diag.object_labels.entry(obj).or_insert_with(|| {
            format!(
                "CGDataProvider.synthetic<'{}'>",
                path.file_name().and_then(|v| v.to_str()).unwrap_or("file")
            )
        });
        Some(obj)
    }

    fn open_host_file_from_path(&mut self, path: &Path, mode: &str, label_prefix: &str) -> Option<u32> {
        let normalized_mode = mode.trim();
        if normalized_mode.is_empty() || !normalized_mode.starts_with('r') {
            return None;
        }
        let data = fs::read(path).ok()?;
        let display_path = path.display().to_string();
        let file_name = path.file_name().and_then(|v| v.to_str()).unwrap_or(label_prefix);
        let handle = self.alloc_synthetic_ui_object(format!("{}<'{}'>", label_prefix, file_name));
        self.runtime.fs.host_files.insert(
            handle,
            HostFileHandle {
                path: display_path,
                mode: normalized_mode.to_string(),
                data,
                pos: 0,
                eof: false,
                error: false,
            },
        );
        Some(handle)
    }

    fn open_audio_file_from_url(&mut self, url: u32) -> Option<u32> {
        let path = self.resolve_path_from_url_like_value(url, false)?;
        let handle = self.open_host_file_from_path(&path, "rb", "AudioFileID")?;
        let (byte_len, metadata, packet_table) = self
            .runtime
            .fs
            .host_files
            .get(&handle)
            .map(|entry| {
                (
                    entry.data.len().min(u32::MAX as usize) as u32,
                    Self::parse_synthetic_audio_metadata(&entry.data)
                        .map(|(metadata, packet_table)| (Some(metadata), packet_table))
                        .unwrap_or_else(|| (None, Vec::new())),
                )
            })
            .map(|(byte_len, (metadata, packet_table))| (byte_len, metadata, packet_table))
            .unwrap_or((0, None, Vec::new()));
        self.runtime.fs.synthetic_audio_files.insert(
            handle,
            SyntheticAudioFileState {
                url_object: url,
                path: path.display().to_string(),
                byte_len,
                metadata: metadata.unwrap_or_default(),
                packet_table,
            },
        );
        Some(handle)
    }

    fn parse_synthetic_audio_metadata(data: &[u8]) -> Option<(SyntheticAudioFileMetadata, Vec<SyntheticAudioPacketEntry>)> {
        Self::parse_synthetic_audio_metadata_wav(data)
            .or_else(|| Self::parse_synthetic_audio_metadata_mp3(data))
    }

    fn parse_synthetic_audio_metadata_wav(data: &[u8]) -> Option<(SyntheticAudioFileMetadata, Vec<SyntheticAudioPacketEntry>)> {
        if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
            return None;
        }
        let mut cursor = 12usize;
        let mut fmt_chunk: Option<&[u8]> = None;
        let mut data_offset: Option<usize> = None;
        let mut data_size: Option<usize> = None;
        while cursor.saturating_add(8) <= data.len() {
            let chunk_id = &data[cursor..cursor + 4];
            let chunk_size = u32::from_le_bytes([
                data[cursor + 4],
                data[cursor + 5],
                data[cursor + 6],
                data[cursor + 7],
            ]) as usize;
            let chunk_data_start = cursor.saturating_add(8);
            let chunk_data_end = chunk_data_start.saturating_add(chunk_size).min(data.len());
            if chunk_data_start > data.len() {
                break;
            }
            if chunk_id == b"fmt " {
                fmt_chunk = data.get(chunk_data_start..chunk_data_end);
            } else if chunk_id == b"data" {
                data_offset = Some(chunk_data_start);
                data_size = Some(chunk_data_end.saturating_sub(chunk_data_start));
            }
            let padded = chunk_size + (chunk_size & 1);
            cursor = chunk_data_start.saturating_add(padded);
        }
        let fmt = fmt_chunk?;
        if fmt.len() < 16 {
            return None;
        }
        let format_tag = u16::from_le_bytes([fmt[0], fmt[1]]);
        let channels = u16::from_le_bytes([fmt[2], fmt[3]]) as u32;
        let sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]) as f64;
        let avg_bytes_per_sec = u32::from_le_bytes([fmt[8], fmt[9], fmt[10], fmt[11]]);
        let block_align = u16::from_le_bytes([fmt[12], fmt[13]]) as u32;
        let bits_per_channel = u16::from_le_bytes([fmt[14], fmt[15]]) as u32;
        let (data_format_id, format_flags) = match format_tag {
            0x0001 => {
                let signed = bits_per_channel >= 16;
                let flags = 0x0008u32 | if signed { 0x0004 } else { 0 };
                (0x6c70_636d, flags)
            }
            0x0003 => (0x6c70_636d, 0x0009u32),
            _ => return None,
        };
        let data_offset = data_offset? as u64;
        let audio_data_byte_count = data_size.unwrap_or(0) as u64;
        let bytes_per_packet = block_align.max(1);
        let frames_per_packet = 1u32;
        let bytes_per_frame = block_align.max(1);
        let audio_data_packet_count = if bytes_per_packet == 0 {
            0
        } else {
            audio_data_byte_count / bytes_per_packet as u64
        };
        let estimated_duration_seconds = if sample_rate > 0.0 && channels > 0 && bytes_per_frame > 0 {
            audio_data_byte_count as f64 / bytes_per_frame as f64 / sample_rate
        } else if avg_bytes_per_sec > 0 {
            audio_data_byte_count as f64 / avg_bytes_per_sec as f64
        } else {
            0.0
        };
        Some((
            SyntheticAudioFileMetadata {
                container_type: 0x5741_5645,
                data_format_id,
                sample_rate,
                channels_per_frame: channels.max(1),
                bits_per_channel,
                bytes_per_packet,
                frames_per_packet,
                bytes_per_frame,
                format_flags,
                audio_data_offset: data_offset,
                audio_data_byte_count,
                audio_data_packet_count,
                packet_size_upper_bound: bytes_per_packet,
                maximum_packet_size: bytes_per_packet,
                estimated_duration_seconds,
            },
            Vec::new(),
        ))
    }

    fn parse_synthetic_audio_metadata_mp3(data: &[u8]) -> Option<(SyntheticAudioFileMetadata, Vec<SyntheticAudioPacketEntry>)> {
        let mut cursor = 0usize;
        if data.len() >= 10 && &data[0..3] == b"ID3" {
            let footer = (data[5] & 0x10) != 0;
            let tag_size = (((data[6] & 0x7f) as usize) << 21)
                | (((data[7] & 0x7f) as usize) << 14)
                | (((data[8] & 0x7f) as usize) << 7)
                | ((data[9] & 0x7f) as usize);
            cursor = 10usize
                .saturating_add(tag_size)
                .saturating_add(if footer { 10 } else { 0 })
                .min(data.len());
        }

        let mut first_frame_offset = None::<usize>;
        let mut packet_table = Vec::new();
        let mut sample_rate = 0f64;
        let mut channels = 0u32;
        let mut frames_per_packet = 0u32;
        let mut format_id = 0u32;
        let mut max_packet_size = 0u32;
        let mut constant_packet_size = None::<u32>;

        while cursor.saturating_add(4) <= data.len() {
            let Some((frame_size, frame_sample_rate, frame_channels, frame_samples_per_packet, frame_format_id)) =
                Self::parse_mp3_frame_header(&data[cursor..])
            else {
                if packet_table.is_empty() {
                    cursor = cursor.saturating_add(1);
                    continue;
                }
                break;
            };
            if frame_size == 0 || cursor.saturating_add(frame_size as usize) > data.len() {
                break;
            }
            first_frame_offset.get_or_insert(cursor);
            if sample_rate == 0.0 {
                sample_rate = frame_sample_rate;
            }
            if channels == 0 {
                channels = frame_channels;
            }
            if frames_per_packet == 0 {
                frames_per_packet = frame_samples_per_packet;
            }
            if format_id == 0 {
                format_id = frame_format_id;
            }
            max_packet_size = max_packet_size.max(frame_size);
            constant_packet_size = match constant_packet_size {
                None => Some(frame_size),
                Some(prev) if prev == frame_size => Some(prev),
                Some(_) => Some(0),
            };
            packet_table.push(SyntheticAudioPacketEntry {
                file_offset: cursor as u64,
                byte_count: frame_size,
            });
            cursor = cursor.saturating_add(frame_size as usize);
        }

        let first_frame_offset = first_frame_offset?;
        if packet_table.is_empty() || sample_rate <= 0.0 || frames_per_packet == 0 {
            return None;
        }
        let audio_data_byte_count = packet_table
            .iter()
            .fold(0u64, |acc, entry| acc.saturating_add(entry.byte_count as u64));
        let audio_data_packet_count = packet_table.len() as u64;
        let total_frames = audio_data_packet_count.saturating_mul(frames_per_packet as u64);
        let estimated_duration_seconds = total_frames as f64 / sample_rate;
        let bytes_per_packet = match constant_packet_size {
            Some(size) if size != 0 => size,
            _ => 0,
        };
        Some((
            SyntheticAudioFileMetadata {
                container_type: 0x4d50_4733,
                data_format_id: if format_id != 0 { format_id } else { 0x2e6d_7033 },
                sample_rate,
                channels_per_frame: channels.max(1),
                bits_per_channel: 0,
                bytes_per_packet,
                frames_per_packet,
                bytes_per_frame: 0,
                format_flags: 0,
                audio_data_offset: first_frame_offset as u64,
                audio_data_byte_count,
                audio_data_packet_count,
                packet_size_upper_bound: max_packet_size,
                maximum_packet_size: max_packet_size,
                estimated_duration_seconds,
            },
            packet_table,
        ))
    }

    fn parse_mp3_frame_header(data: &[u8]) -> Option<(u32, f64, u32, u32, u32)> {
        if data.len() < 4 {
            return None;
        }
        let header = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        if (header >> 21) & 0x7ff != 0x7ff {
            return None;
        }
        let version_id = ((header >> 19) & 0x3) as u8;
        let layer_bits = ((header >> 17) & 0x3) as u8;
        let bitrate_idx = ((header >> 12) & 0xf) as u8;
        let sample_rate_idx = ((header >> 10) & 0x3) as u8;
        let padding = ((header >> 9) & 0x1) as u32;
        let channel_mode = ((header >> 6) & 0x3) as u8;

        if version_id == 0x1 || layer_bits == 0x0 || bitrate_idx == 0x0 || bitrate_idx == 0xf || sample_rate_idx == 0x3 {
            return None;
        }

        let (mpeg1, sample_rate_base) = match version_id {
            0x3 => (true, [44_100u32, 48_000, 32_000][sample_rate_idx as usize]),
            0x2 => (false, [22_050u32, 24_000, 16_000][sample_rate_idx as usize]),
            0x0 => (false, [11_025u32, 12_000, 8_000][sample_rate_idx as usize]),
            _ => return None,
        };
        let layer = match layer_bits {
            0x3 => 1u8,
            0x2 => 2u8,
            0x1 => 3u8,
            _ => return None,
        };
        let bitrate_kbps = Self::mp3_bitrate_kbps(mpeg1, layer, bitrate_idx)? as u32;
        let sample_rate = sample_rate_base as f64;
        let frame_size = match layer {
            1 => (((12 * bitrate_kbps * 1000) / sample_rate_base) + padding) * 4,
            2 => ((144 * bitrate_kbps * 1000) / sample_rate_base) + padding,
            3 if mpeg1 => ((144 * bitrate_kbps * 1000) / sample_rate_base) + padding,
            3 => ((72 * bitrate_kbps * 1000) / sample_rate_base) + padding,
            _ => return None,
        };
        let frames_per_packet = match layer {
            1 => 384,
            2 => 1152,
            3 if mpeg1 => 1152,
            3 => 576,
            _ => return None,
        };
        let channels = if channel_mode == 0x3 { 1 } else { 2 };
        Some((frame_size, sample_rate, channels, frames_per_packet, 0x2e6d_7033))
    }

    fn mp3_bitrate_kbps(mpeg1: bool, layer: u8, idx: u8) -> Option<u16> {
        const MPEG1_LAYER1: [u16; 16] = [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 0];
        const MPEG1_LAYER2: [u16; 16] = [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0];
        const MPEG1_LAYER3: [u16; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
        const MPEG2_LAYER1: [u16; 16] = [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 0];
        const MPEG2_LAYER23: [u16; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];
        let table = match (mpeg1, layer) {
            (true, 1) => &MPEG1_LAYER1,
            (true, 2) => &MPEG1_LAYER2,
            (true, 3) => &MPEG1_LAYER3,
            (false, 1) => &MPEG2_LAYER1,
            (false, 2 | 3) => &MPEG2_LAYER23,
            _ => return None,
        };
        let bitrate = table.get(idx as usize).copied().unwrap_or(0);
        (bitrate != 0).then_some(bitrate)
    }

    fn synthetic_audio_file_property_name(property_id: u32) -> String {
        match property_id {
            0x6666_6d74 => "kAudioFilePropertyFileFormat".to_string(),
            0x6466_6d74 => "kAudioFilePropertyDataFormat".to_string(),
            0x6263_6e74 => "kAudioFilePropertyAudioDataByteCount".to_string(),
            0x7063_6e74 => "kAudioFilePropertyAudioDataPacketCount".to_string(),
            0x7073_7a65 => "kAudioFilePropertyMaximumPacketSize".to_string(),
            0x706b_7562 => "kAudioFilePropertyPacketSizeUpperBound".to_string(),
            0x646f_6666 => "kAudioFilePropertyDataOffset".to_string(),
            0x6564_7572 => "kAudioFilePropertyEstimatedDuration".to_string(),
            0x6d67_6963 => "kAudioFilePropertyMagicCookieData".to_string(),
            _ => {
                let tag = property_id.to_be_bytes();
                let printable = tag.iter().all(|b| b.is_ascii_graphic() || *b == b' ');
                if printable {
                    format!("property:'{}'", String::from_utf8_lossy(&tag))
                } else {
                    format!("property:0x{property_id:08x}")
                }
            }
        }
    }

    fn synthetic_audio_file_property_size(property_id: u32, meta: &SyntheticAudioFileMetadata) -> Option<u32> {
        match property_id {
            0x6666_6d74 => Some(4),
            0x6466_6d74 => Some(40),
            0x6263_6e74 | 0x7063_6e74 | 0x646f_6666 | 0x6564_7572 => Some(8),
            0x7073_7a65 | 0x706b_7562 => Some(4),
            0x6d67_6963 => Some(0),
            _ if meta.bytes_per_packet != 0 => None,
            _ => None,
        }
    }

    fn synthetic_audio_file_property_payload(
        &self,
        property_id: u32,
        meta: &SyntheticAudioFileMetadata,
        requested_size: u32,
    ) -> Option<(Vec<u8>, String)> {
        let payload = match property_id {
            0x6666_6d74 => meta.container_type.to_le_bytes().to_vec(),
            0x6466_6d74 => {
                let mut bytes = vec![0u8; 40];
                bytes[0..8].copy_from_slice(&meta.sample_rate.to_le_bytes());
                bytes[8..12].copy_from_slice(&meta.data_format_id.to_le_bytes());
                bytes[12..16].copy_from_slice(&meta.format_flags.to_le_bytes());
                bytes[16..20].copy_from_slice(&meta.bytes_per_packet.to_le_bytes());
                bytes[20..24].copy_from_slice(&meta.frames_per_packet.to_le_bytes());
                bytes[24..28].copy_from_slice(&meta.bytes_per_frame.to_le_bytes());
                bytes[28..32].copy_from_slice(&meta.channels_per_frame.to_le_bytes());
                bytes[32..36].copy_from_slice(&meta.bits_per_channel.to_le_bytes());
                bytes[36..40].copy_from_slice(&0u32.to_le_bytes());
                bytes
            }
            0x6263_6e74 => meta.audio_data_byte_count.to_le_bytes().to_vec(),
            0x7063_6e74 => meta.audio_data_packet_count.to_le_bytes().to_vec(),
            0x7073_7a65 => meta.maximum_packet_size.to_le_bytes().to_vec(),
            0x706b_7562 => meta.packet_size_upper_bound.to_le_bytes().to_vec(),
            0x646f_6666 => meta.audio_data_offset.to_le_bytes().to_vec(),
            0x6564_7572 => meta.estimated_duration_seconds.to_le_bytes().to_vec(),
            0x6d67_6963 => Vec::new(),
            _ if requested_size > 0 && requested_size <= 256 => vec![0u8; requested_size as usize],
            _ => return None,
        };
        Some((payload, Self::synthetic_audio_file_property_name(property_id)))
    }

    fn open_bundle_file(&mut self, request: &str, mode: &str) -> Option<u32> {
        self.runtime.fs.last_file_mode = Some(mode.to_string());
        self.runtime.fs.last_file_path = None;
        let normalized_mode = mode.trim();
        if normalized_mode.is_empty() || !normalized_mode.starts_with('r') {
            self.runtime.fs.file_open_misses = self.runtime.fs.file_open_misses.saturating_add(1);
            return None;
        }
        let path = match self.resolve_bundle_file_path(request) {
            Some(path) => path,
            None => {
                self.runtime.fs.file_open_misses = self.runtime.fs.file_open_misses.saturating_add(1);
                return None;
            }
        };
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(_) => {
                self.runtime.fs.file_open_misses = self.runtime.fs.file_open_misses.saturating_add(1);
                self.runtime.fs.last_file_path = Some(path.display().to_string());
                return None;
            }
        };
        let display_path = path.display().to_string();
        let handle = self.alloc_synthetic_ui_object(format!("FILE*<'{}'>", path.file_name().and_then(|v| v.to_str()).unwrap_or(request)));
        self.runtime.fs.host_files.insert(
            handle,
            HostFileHandle {
                path: display_path.clone(),
                mode: normalized_mode.to_string(),
                data,
                pos: 0,
                eof: false,
                error: false,
            },
        );
        self.runtime.fs.file_open_hits = self.runtime.fs.file_open_hits.saturating_add(1);
        self.runtime.fs.last_file_path = Some(display_path);
        Some(handle)
    }

    fn graphics_api_name(&self) -> &'static str {
        "OpenGLES1"
    }

    fn refresh_graphics_object_labels(&mut self) {
        self.diag.object_labels.insert(
            self.runtime.ui_graphics.eagl_context,
            format!(
                "EAGLContext.synthetic#0<api={} current={} surfaceReady={} framebufferComplete={}>",
                self.graphics_api_name(),
                if self.runtime.ui_graphics.graphics_context_current { "YES" } else { "NO" },
                if self.runtime.ui_graphics.graphics_surface_ready { "YES" } else { "NO" },
                if self.runtime.ui_graphics.graphics_framebuffer_complete { "YES" } else { "NO" },
            ),
        );
        self.diag.object_labels.insert(
            self.runtime.ui_graphics.eagl_layer,
            format!(
                "CAEAGLLayer.synthetic#0<attached={} presented={} size={}x{}>",
                if self.runtime.ui_graphics.graphics_layer_attached { "YES" } else { "NO" },
                if self.runtime.ui_graphics.graphics_presented { "YES" } else { "NO" },
                self.runtime.ui_graphics.graphics_surface_width,
                self.runtime.ui_graphics.graphics_surface_height,
            ),
        );
        self.diag.object_labels.insert(
            self.runtime.ui_graphics.gl_framebuffer,
            format!(
                "GLFramebuffer.synthetic#0<complete={} viewport={}x{} frames={}>",
                if self.runtime.ui_graphics.graphics_framebuffer_complete { "YES" } else { "NO" },
                self.runtime.ui_graphics.graphics_viewport_width,
                self.runtime.ui_graphics.graphics_viewport_height,
                self.runtime.ui_graphics.graphics_frame_index,
            ),
        );
        self.diag.object_labels.insert(
            self.runtime.ui_graphics.gl_renderbuffer,
            format!(
                "GLRenderbuffer.synthetic#0<surfaceReady={} presents={} readback={} rbCalls={}>",
                if self.runtime.ui_graphics.graphics_surface_ready { "YES" } else { "NO" },
                self.runtime.ui_graphics.graphics_present_calls,
                if self.runtime.ui_graphics.graphics_readback_ready { "YES" } else { "NO" },
                self.runtime.ui_graphics.graphics_readback_calls,
            ),
        );
    }


    fn graphics_draw_mode_name(mode: u32) -> &'static str {
        match mode {
            GL_POINTS => "points",
            GL_LINES => "lines",
            GL_LINE_LOOP => "line-loop",
            GL_LINE_STRIP => "line-strip",
            GL_TRIANGLES => "triangles",
            GL_TRIANGLE_STRIP => "triangle-strip",
            GL_TRIANGLE_FAN => "triangle-fan",
            _ => "unknown",
        }
    }

    fn graphics_client_array_name(array: u32) -> Option<&'static str> {
        match array {
            GL_VERTEX_ARRAY => Some("vertex"),
            GL_COLOR_ARRAY => Some("color"),
            GL_TEXTURE_COORD_ARRAY => Some("texcoord"),
            _ => None,
        }
    }

    pub(crate) fn gl_type_size(ty: u32) -> u32 {
        match ty {
            GL_BYTE | GL_UNSIGNED_BYTE => 1,
            GL_SHORT | GL_UNSIGNED_SHORT => 2,
            GL_FLOAT | GL_FIXED => 4,
            _ => 4,
        }
    }

    fn read_i16_le(&self, addr: u32) -> CoreResult<i16> {
        Ok(self.read_u16_le(addr)? as i16)
    }

    fn read_i8(&self, addr: u32) -> CoreResult<i8> {
        Ok(self.read_u8(addr)? as i8)
    }

    fn read_gl_scalar_f32(&self, ty: u32, addr: u32) -> CoreResult<f32> {
        match ty {
            GL_FLOAT => Ok(f32::from_bits(self.read_u32_le(addr)?)),
            GL_FIXED => Ok((self.read_u32_le(addr)? as i32 as f32) / 65536.0),
            GL_SHORT => Ok(self.read_i16_le(addr)? as f32),
            GL_UNSIGNED_SHORT => Ok(self.read_u16_le(addr)? as f32),
            GL_BYTE => Ok(self.read_i8(addr)? as f32),
            GL_UNSIGNED_BYTE => Ok(self.read_u8(addr)? as f32),
            _ => Ok(f32::from_bits(self.read_u32_le(addr)?)),
        }
    }

    fn read_gl_scalar_u8_normalized(&self, ty: u32, addr: u32) -> CoreResult<u8> {
        match ty {
            GL_UNSIGNED_BYTE => Ok(self.read_u8(addr)?),
            GL_BYTE => {
                let raw = self.read_i8(addr)? as i16;
                Ok(((raw.clamp(0, 127) as u16 * 255) / 127) as u8)
            }
            GL_FLOAT => Ok(Self::gl_float_to_u8(self.read_u32_le(addr)?)),
            GL_FIXED => {
                let f = (self.read_u32_le(addr)? as i32 as f32) / 65536.0;
                Ok((f.clamp(0.0, 1.0) * 255.0).round() as u8)
            }
            GL_SHORT => {
                let v = self.read_i16_le(addr)? as f32 / 32767.0;
                Ok((v.clamp(0.0, 1.0) * 255.0).round() as u8)
            }
            GL_UNSIGNED_SHORT => Ok(((self.read_u16_le(addr)? as u32 * 255) / 65535) as u8),
            _ => Ok(255),
        }
    }

    fn fetch_vertex_xyz(&self, index: u32) -> Option<(f32, f32, f32)> {
        let array = self.runtime.graphics.gl_vertex_array;
        if !array.enabled || !array.configured() || array.size < 2 {
            return None;
        }
        let addr = array.ptr.wrapping_add(index.wrapping_mul(array.element_stride_bytes()));
        let step = Self::gl_type_size(array.ty);
        let x = self.read_gl_scalar_f32(array.ty, addr).ok()?;
        let y = self.read_gl_scalar_f32(array.ty, addr.wrapping_add(step)).ok()?;
        let z = if array.size >= 3 {
            self.read_gl_scalar_f32(array.ty, addr.wrapping_add(step.saturating_mul(2))).ok().unwrap_or(0.0)
        } else {
            0.0
        };
        Some((x, y, z))
    }

    fn fetch_color_rgba(&self, index: u32) -> [u8; 4] {
        let array = self.runtime.graphics.gl_color_array;
        if array.enabled && array.configured() {
            let addr = array.ptr.wrapping_add(index.wrapping_mul(array.element_stride_bytes()));
            let step = Self::gl_type_size(array.ty);
            let r = self.read_gl_scalar_u8_normalized(array.ty, addr).unwrap_or(255);
            let g = if array.size >= 2 { self.read_gl_scalar_u8_normalized(array.ty, addr.wrapping_add(step)).unwrap_or(255) } else { r };
            let b = if array.size >= 3 { self.read_gl_scalar_u8_normalized(array.ty, addr.wrapping_add(step.saturating_mul(2))).unwrap_or(255) } else { g };
            let a = if array.size >= 4 { self.read_gl_scalar_u8_normalized(array.ty, addr.wrapping_add(step.saturating_mul(3))).unwrap_or(255) } else { 255 };
            [r, g, b, a]
        } else {
            self.runtime.graphics.gl_current_color
        }
    }

    fn fetch_texcoord_uv(&self, index: u32) -> Option<(f32, f32)> {
        let array = self.runtime.graphics.gl_texcoord_array;
        if !array.enabled || !array.configured() || array.size < 2 {
            return None;
        }
        let addr = array.ptr.wrapping_add(index.wrapping_mul(array.element_stride_bytes()));
        let step = Self::gl_type_size(array.ty);
        let u = self.read_gl_scalar_f32(array.ty, addr).ok()?;
        let v = self.read_gl_scalar_f32(array.ty, addr.wrapping_add(step)).ok()?;
        Some((u, v))
    }

    fn texture_row_stride_bytes(width: u32, format: u32, ty: u32) -> Option<u32> {
        let bpp = match (format, ty) {
            (GL_RGBA, GL_UNSIGNED_BYTE) => 4,
            (GL_RGB, GL_UNSIGNED_BYTE) => 3,
            (GL_ALPHA, GL_UNSIGNED_BYTE) => 1,
            (GL_LUMINANCE, GL_UNSIGNED_BYTE) => 1,
            (GL_LUMINANCE_ALPHA, GL_UNSIGNED_BYTE) => 2,
            (GL_RGBA, GL_UNSIGNED_SHORT_4_4_4_4) => 2,
            (GL_RGBA, GL_UNSIGNED_SHORT_5_5_5_1) => 2,
            (GL_RGB, GL_UNSIGNED_SHORT_5_6_5) => 2,
            _ => return None,
        };
        Some(width.saturating_mul(bpp))
    }

    fn decode_guest_texture_rgba(&self, width: u32, height: u32, format: u32, ty: u32, pixels_ptr: u32) -> Option<Vec<u8>> {
        if width == 0 || height == 0 {
            return Some(Vec::new());
        }
        if pixels_ptr == 0 {
            return Some(vec![0u8; width.saturating_mul(height).saturating_mul(4) as usize]);
        }
        let row_stride = Self::texture_row_stride_bytes(width, format, ty)?;
        let mut out = vec![0u8; width.saturating_mul(height).saturating_mul(4) as usize];
        for y in 0..height {
            let row_base = pixels_ptr.wrapping_add(y.wrapping_mul(row_stride));
            for x in 0..width {
                let out_idx = ((y as usize * width as usize) + x as usize) * 4;
                let rgba = match (format, ty) {
                    (GL_RGBA, GL_UNSIGNED_BYTE) => {
                        let addr = row_base.wrapping_add(x.wrapping_mul(4));
                        [
                            self.read_u8(addr).ok()?,
                            self.read_u8(addr.wrapping_add(1)).ok()?,
                            self.read_u8(addr.wrapping_add(2)).ok()?,
                            self.read_u8(addr.wrapping_add(3)).ok()?,
                        ]
                    }
                    (GL_RGB, GL_UNSIGNED_BYTE) => {
                        let addr = row_base.wrapping_add(x.wrapping_mul(3));
                        [
                            self.read_u8(addr).ok()?,
                            self.read_u8(addr.wrapping_add(1)).ok()?,
                            self.read_u8(addr.wrapping_add(2)).ok()?,
                            255,
                        ]
                    }
                    (GL_ALPHA, GL_UNSIGNED_BYTE) => {
                        let a = self.read_u8(row_base.wrapping_add(x)).ok()?;
                        [255, 255, 255, a]
                    }
                    (GL_LUMINANCE, GL_UNSIGNED_BYTE) => {
                        let l = self.read_u8(row_base.wrapping_add(x)).ok()?;
                        [l, l, l, 255]
                    }
                    (GL_LUMINANCE_ALPHA, GL_UNSIGNED_BYTE) => {
                        let addr = row_base.wrapping_add(x.wrapping_mul(2));
                        let l = self.read_u8(addr).ok()?;
                        let a = self.read_u8(addr.wrapping_add(1)).ok()?;
                        [l, l, l, a]
                    }
                    (GL_RGBA, GL_UNSIGNED_SHORT_4_4_4_4) => {
                        let raw = self.read_u16_le(row_base.wrapping_add(x.wrapping_mul(2))).ok()?;
                        [
                            (((raw >> 12) & 0x0f) * 17) as u8,
                            (((raw >> 8) & 0x0f) * 17) as u8,
                            (((raw >> 4) & 0x0f) * 17) as u8,
                            ((raw & 0x0f) * 17) as u8,
                        ]
                    }
                    (GL_RGBA, GL_UNSIGNED_SHORT_5_5_5_1) => {
                        let raw = self.read_u16_le(row_base.wrapping_add(x.wrapping_mul(2))).ok()?;
                        [
                            ((((raw >> 11) & 0x1f) * 255) / 31) as u8,
                            ((((raw >> 6) & 0x1f) * 255) / 31) as u8,
                            ((((raw >> 1) & 0x1f) * 255) / 31) as u8,
                            if (raw & 0x1) != 0 { 255 } else { 0 },
                        ]
                    }
                    (GL_RGB, GL_UNSIGNED_SHORT_5_6_5) => {
                        let raw = self.read_u16_le(row_base.wrapping_add(x.wrapping_mul(2))).ok()?;
                        [
                            ((((raw >> 11) & 0x1f) * 255) / 31) as u8,
                            ((((raw >> 5) & 0x3f) * 255) / 63) as u8,
                            (((raw & 0x1f) * 255) / 31) as u8,
                            255,
                        ]
                    }
                    _ => return None,
                };
                out[out_idx..out_idx + 4].copy_from_slice(&rgba);
            }
        }
        Some(out)
    }

    fn wrap_texcoord(coord: f32, wrap: u32) -> f32 {
        if !coord.is_finite() {
            return 0.0;
        }
        match wrap {
            GL_REPEAT => {
                let frac = coord - coord.floor();
                if frac < 0.0 { frac + 1.0 } else { frac }
            }
            GL_CLAMP | GL_CLAMP_TO_EDGE => coord.clamp(0.0, 1.0),
            _ => coord.clamp(0.0, 1.0),
        }
    }

    fn sample_guest_texture_rgba(texture: &GuestGlTextureObject, u: f32, v: f32) -> [u8; 4] {
        if texture.width == 0 || texture.height == 0 {
            return [255, 255, 255, 255];
        }
        if texture.pixels_rgba.len() < texture.width.saturating_mul(texture.height).saturating_mul(4) as usize {
            return [255, 255, 255, 255];
        }
        let uu = Self::wrap_texcoord(u, texture.wrap_s);
        let vv = Self::wrap_texcoord(v, texture.wrap_t);
        let max_x = texture.width.saturating_sub(1) as f32;
        let max_y = texture.height.saturating_sub(1) as f32;
        let tx = (uu * max_x).round().clamp(0.0, max_x) as u32;
        let ty = (vv * max_y).round().clamp(0.0, max_y) as u32;
        let idx = ((ty as usize * texture.width as usize) + tx as usize) * 4;
        [
            texture.pixels_rgba[idx],
            texture.pixels_rgba[idx + 1],
            texture.pixels_rgba[idx + 2],
            texture.pixels_rgba[idx + 3],
        ]
    }

    fn apply_texture_env(primary: [u8; 4], sampled: [u8; 4], mode: u32) -> [u8; 4] {
        match mode {
            GL_REPLACE => sampled,
            GL_MODULATE => [
                ((primary[0] as u32 * sampled[0] as u32) / 255) as u8,
                ((primary[1] as u32 * sampled[1] as u32) / 255) as u8,
                ((primary[2] as u32 * sampled[2] as u32) / 255) as u8,
                ((primary[3] as u32 * sampled[3] as u32) / 255) as u8,
            ],
            _ => [
                ((primary[0] as u32 * sampled[0] as u32) / 255) as u8,
                ((primary[1] as u32 * sampled[1] as u32) / 255) as u8,
                ((primary[2] as u32 * sampled[2] as u32) / 255) as u8,
                ((primary[3] as u32 * sampled[3] as u32) / 255) as u8,
            ],
        }
    }

    fn vertex_to_surface_xy(&self, x: f32, y: f32, z: f32) -> Option<(i32, i32)> {
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1) as f32;
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
        let viewport_x = self.runtime.ui_graphics.graphics_viewport_x as f32;
        let viewport_y = self.runtime.ui_graphics.graphics_viewport_y as f32;
        let viewport_w = self.runtime.ui_graphics.graphics_viewport_width.max(self.runtime.ui_graphics.graphics_surface_width).max(1) as f32;
        let viewport_h = self.runtime.ui_graphics.graphics_viewport_height.max(self.runtime.ui_graphics.graphics_surface_height).max(1) as f32;

        if self.gl_has_active_transform_pipeline() {
            let model = self.gl_current_matrix(GraphicsMatrixMode::ModelView);
            let proj = self.gl_current_matrix(GraphicsMatrixMode::Projection);
            let mv = Self::gl_mat4_transform(model, [x, y, z, 1.0]);
            let clip = Self::gl_mat4_transform(proj, mv);
            if clip[3].is_finite() && clip[3].abs() > 1.0e-6 {
                let inv_w = 1.0 / clip[3];
                let ndc_x = clip[0] * inv_w;
                let ndc_y = clip[1] * inv_w;
                if ndc_x.is_finite() && ndc_y.is_finite() {
                    let gl_x = viewport_x + ((ndc_x + 1.0) * 0.5) * viewport_w;
                    let gl_y = viewport_y + ((ndc_y + 1.0) * 0.5) * viewport_h;
                    let sx = gl_x;
                    let sy = surface_h - gl_y;
                    if sx.is_finite() && sy.is_finite() {
                        return Some((
                            sx.round().clamp(0.0, surface_w - 1.0) as i32,
                            sy.round().clamp(0.0, surface_h - 1.0) as i32,
                        ));
                    }
                }
            }
        }

        let (sx, sy) = if x.abs() <= 2.5 && y.abs() <= 2.5 {
            let nx = x.clamp(-1.25, 1.25);
            let ny = y.clamp(-1.25, 1.25);
            (((nx * 0.5) + 0.5) * (viewport_w - 1.0), (1.0 - ((ny * 0.5) + 0.5)) * (viewport_h - 1.0))
        } else {
            (x, y)
        };
        if !sx.is_finite() || !sy.is_finite() {
            return None;
        }
        Some((
            sx.round().clamp(0.0, surface_w - 1.0) as i32,
            sy.round().clamp(0.0, surface_h - 1.0) as i32,
        ))
    }

    fn scissor_allows_pixel(&self, x: i32, y: i32) -> bool {
        if !self.runtime.ui_graphics.graphics_scissor_enabled {
            return true;
        }
        let sx = self.runtime.ui_graphics.graphics_scissor_x as i32;
        let sy = self.runtime.ui_graphics.graphics_scissor_y as i32;
        let sw = self.runtime.ui_graphics.graphics_scissor_width.max(1) as i32;
        let sh = self.runtime.ui_graphics.graphics_scissor_height.max(1) as i32;
        x >= sx && y >= sy && x < sx.saturating_add(sw) && y < sy.saturating_add(sh)
    }

    fn blend_factor_rgba(factor: u32, src: [u8; 4], _dst: [u8; 4]) -> [u8; 4] {
        let alpha = src[3] as u32;
        match factor {
            GL_ZERO => [0, 0, 0, 0],
            GL_ONE => [255, 255, 255, 255],
            GL_SRC_ALPHA => [alpha as u8, alpha as u8, alpha as u8, alpha as u8],
            GL_ONE_MINUS_SRC_ALPHA => {
                let inv = 255u32.saturating_sub(alpha) as u8;
                [inv, inv, inv, inv]
            }
            _ => [255, 255, 255, 255],
        }
    }

    fn blend_pixel_rgba(&mut self, x: i32, y: i32, rgba: [u8; 4]) {
        self.ensure_framebuffer_backing();
        let width = self.runtime.ui_graphics.graphics_surface_width.max(1) as i32;
        let height = self.runtime.ui_graphics.graphics_surface_height.max(1) as i32;
        if x < 0 || y < 0 || x >= width || y >= height || !self.scissor_allows_pixel(x, y) {
            return;
        }
        let idx = ((y as usize * width as usize) + x as usize) * 4;
        if idx + 3 >= self.runtime.graphics.synthetic_framebuffer.len() {
            return;
        }
        let dst = [
            self.runtime.graphics.synthetic_framebuffer[idx],
            self.runtime.graphics.synthetic_framebuffer[idx + 1],
            self.runtime.graphics.synthetic_framebuffer[idx + 2],
            self.runtime.graphics.synthetic_framebuffer[idx + 3],
        ];
        let out = if self.runtime.graphics.gl_blend_enabled {
            let sf = Self::blend_factor_rgba(self.runtime.graphics.gl_blend_src_factor, rgba, dst);
            let df = Self::blend_factor_rgba(self.runtime.graphics.gl_blend_dst_factor, rgba, dst);
            let mut mixed = [0u8; 4];
            for c in 0..4 {
                let src_term = rgba[c] as u32 * sf[c] as u32;
                let dst_term = dst[c] as u32 * df[c] as u32;
                mixed[c] = ((src_term.saturating_add(dst_term)).min(255 * 255) / 255) as u8;
            }
            mixed
        } else {
            rgba
        };
        self.runtime.graphics.synthetic_framebuffer[idx..idx + 4].copy_from_slice(&out);
    }

    fn rasterize_triangle(
        &mut self,
        a: (i32, i32),
        b: (i32, i32),
        c: (i32, i32),
        ca: [u8; 4],
        cb: [u8; 4],
        cc: [u8; 4],
        ta: Option<(f32, f32)>,
        tb: Option<(f32, f32)>,
        tc: Option<(f32, f32)>,
        texture: Option<GuestGlTextureObject>,
    ) {
        let min_x = a.0.min(b.0).min(c.0);
        let max_x = a.0.max(b.0).max(c.0);
        let min_y = a.1.min(b.1).min(c.1);
        let max_y = a.1.max(b.1).max(c.1);
        let edge = |p0: (i32, i32), p1: (i32, i32), p: (i32, i32)| -> i64 {
            (p.0 - p0.0) as i64 * (p1.1 - p0.1) as i64 - (p.1 - p0.1) as i64 * (p1.0 - p0.0) as i64
        };
        let area = edge(a, b, c);
        if area == 0 {
            self.draw_line_rgba(a.0, a.1, b.0, b.1, ca);
            self.draw_line_rgba(b.0, b.1, c.0, c.1, cb);
            self.draw_line_rgba(c.0, c.1, a.0, a.1, cc);
            return;
        }
        let sign = if area < 0 { -1.0 } else { 1.0 };
        let area_f = (area.abs()) as f32;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let p = (x, y);
                let w0 = edge(b, c, p) as f32 * sign;
                let w1 = edge(c, a, p) as f32 * sign;
                let w2 = edge(a, b, p) as f32 * sign;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let l0 = w0 / area_f;
                let l1 = w1 / area_f;
                let l2 = w2 / area_f;
                let primary = [
                    (ca[0] as f32 * l0 + cb[0] as f32 * l1 + cc[0] as f32 * l2).round().clamp(0.0, 255.0) as u8,
                    (ca[1] as f32 * l0 + cb[1] as f32 * l1 + cc[1] as f32 * l2).round().clamp(0.0, 255.0) as u8,
                    (ca[2] as f32 * l0 + cb[2] as f32 * l1 + cc[2] as f32 * l2).round().clamp(0.0, 255.0) as u8,
                    (ca[3] as f32 * l0 + cb[3] as f32 * l1 + cc[3] as f32 * l2).round().clamp(0.0, 255.0) as u8,
                ];
                let final_rgba = if let (Some(tex), Some(ta), Some(tb), Some(tc)) = (texture.as_ref(), ta, tb, tc) {
                    let u = ta.0 * l0 + tb.0 * l1 + tc.0 * l2;
                    let v = ta.1 * l0 + tb.1 * l1 + tc.1 * l2;
                    let sampled = Self::sample_guest_texture_rgba(tex, u, v);
                    Self::apply_texture_env(primary, sampled, self.runtime.graphics.gl_tex_env_mode)
                } else {
                    primary
                };
                self.blend_pixel_rgba(x, y, final_rgba);
            }
        }
    }

    fn draw_point_rgba(&mut self, x: i32, y: i32, rgba: [u8; 4], radius: i32) {
        let r = radius.max(0);
        for yy in (y - r)..=(y + r) {
            for xx in (x - r)..=(x + r) {
                self.blend_pixel_rgba(xx, yy, rgba);
            }
        }
    }

    fn draw_line_rgba(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, rgba: [u8; 4]) {
        let mut x0 = x0;
        let mut y0 = y0;
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.draw_point_rgba(x0, y0, rgba, 1);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = err.saturating_mul(2);
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    fn fill_triangle_rgba(&mut self, a: (i32, i32), b: (i32, i32), c: (i32, i32), rgba: [u8; 4]) {
        let min_x = a.0.min(b.0).min(c.0);
        let max_x = a.0.max(b.0).max(c.0);
        let min_y = a.1.min(b.1).min(c.1);
        let max_y = a.1.max(b.1).max(c.1);
        let edge = |p0: (i32, i32), p1: (i32, i32), p: (i32, i32)| -> i64 {
            (p.0 - p0.0) as i64 * (p1.1 - p0.1) as i64 - (p.1 - p0.1) as i64 * (p1.0 - p0.0) as i64
        };
        let area = edge(a, b, c);
        if area == 0 {
            self.draw_line_rgba(a.0, a.1, b.0, b.1, rgba);
            self.draw_line_rgba(b.0, b.1, c.0, c.1, rgba);
            self.draw_line_rgba(c.0, c.1, a.0, a.1, rgba);
            return;
        }
        let sign = if area < 0 { -1 } else { 1 };
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let p = (x, y);
                let w0 = edge(b, c, p) * sign;
                let w1 = edge(c, a, p) * sign;
                let w2 = edge(a, b, p) * sign;
                if w0 >= 0 && w1 >= 0 && w2 >= 0 {
                    self.blend_pixel_rgba(x, y, rgba);
                }
            }
        }
        self.draw_line_rgba(a.0, a.1, b.0, b.1, rgba);
        self.draw_line_rgba(b.0, b.1, c.0, c.1, rgba);
        self.draw_line_rgba(c.0, c.1, a.0, a.1, rgba);
    }

    fn resolve_draw_indices(&self, mode: u32, first: u32, count: u32, index_type: Option<u32>, indices_ptr: Option<u32>) -> Vec<u32> {
        let mut indices = Vec::new();
        if let (Some(ty), Some(ptr)) = (index_type, indices_ptr) {
            for i in 0..count {
                let addr = match ty {
                    GL_UNSIGNED_BYTE => ptr.wrapping_add(i),
                    GL_UNSIGNED_SHORT => ptr.wrapping_add(i.wrapping_mul(2)),
                    _ => ptr.wrapping_add(i.wrapping_mul(2)),
                };
                let idx = match ty {
                    GL_UNSIGNED_BYTE => self.read_u8(addr).map(|v| v as u32).ok(),
                    GL_UNSIGNED_SHORT => self.read_u16_le(addr).map(|v| v as u32).ok(),
                    _ => self.read_u16_le(addr).map(|v| v as u32).ok(),
                };
                if let Some(idx) = idx { indices.push(idx); } else { break; }
            }
        } else {
            for i in 0..count {
                indices.push(first.saturating_add(i));
            }
        }
        if matches!(mode, GL_TRIANGLES | GL_TRIANGLE_STRIP | GL_TRIANGLE_FAN | GL_POINTS | GL_LINES | GL_LINE_STRIP | GL_LINE_LOOP) {
            indices
        } else {
            Vec::new()
        }
    }

    fn draw_guest_geometry(&mut self, mode: u32, first: u32, count: u32, index_type: Option<u32>, indices_ptr: Option<u32>) -> Option<(usize, u32)> {
        let indices = self.resolve_draw_indices(mode, first, count, index_type, indices_ptr);
        if indices.is_empty() || !self.runtime.graphics.gl_vertex_array.enabled || !self.runtime.graphics.gl_vertex_array.configured() {
            return None;
        }
        self.ensure_framebuffer_backing();
        let active_texture = if self.runtime.graphics.gl_texture_2d_enabled {
            self.runtime
                .graphics
                .guest_gl_textures
                .get(&self.runtime.graphics.current_bound_texture_name)
                .filter(|tex| tex.width > 0 && tex.height > 0 && !tex.pixels_rgba.is_empty())
                .cloned()
        } else {
            None
        };
        let mut vertices = Vec::new();
        for &idx in &indices {
            let xyz = self.fetch_vertex_xyz(idx)?;
            let pt = self.vertex_to_surface_xy(xyz.0, xyz.1, xyz.2)?;
            let color = self.fetch_color_rgba(idx);
            let uv = if active_texture.is_some() { self.fetch_texcoord_uv(idx) } else { None };
            vertices.push((pt, color, uv, idx));
        }
        if vertices.is_empty() {
            return None;
        }
        match mode {
            GL_POINTS => {
                for (pt, color, _, _) in &vertices {
                    self.draw_point_rgba(pt.0, pt.1, *color, 2);
                }
            }
            GL_LINES => {
                for chunk in vertices.chunks(2) {
                    if let [a, b] = chunk {
                        let color = [
                            ((a.1[0] as u16 + b.1[0] as u16) / 2) as u8,
                            ((a.1[1] as u16 + b.1[1] as u16) / 2) as u8,
                            ((a.1[2] as u16 + b.1[2] as u16) / 2) as u8,
                            ((a.1[3] as u16 + b.1[3] as u16) / 2) as u8,
                        ];
                        self.draw_line_rgba(a.0.0, a.0.1, b.0.0, b.0.1, color);
                    }
                }
            }
            GL_LINE_STRIP => {
                for pair in vertices.windows(2) {
                    let a = &pair[0];
                    let b = &pair[1];
                    let color = [
                        ((a.1[0] as u16 + b.1[0] as u16) / 2) as u8,
                        ((a.1[1] as u16 + b.1[1] as u16) / 2) as u8,
                        ((a.1[2] as u16 + b.1[2] as u16) / 2) as u8,
                        ((a.1[3] as u16 + b.1[3] as u16) / 2) as u8,
                    ];
                    self.draw_line_rgba(a.0.0, a.0.1, b.0.0, b.0.1, color);
                }
            }
            GL_LINE_LOOP => {
                for i in 0..vertices.len() {
                    let a = &vertices[i];
                    let b = &vertices[(i + 1) % vertices.len()];
                    let color = [
                        ((a.1[0] as u16 + b.1[0] as u16) / 2) as u8,
                        ((a.1[1] as u16 + b.1[1] as u16) / 2) as u8,
                        ((a.1[2] as u16 + b.1[2] as u16) / 2) as u8,
                        ((a.1[3] as u16 + b.1[3] as u16) / 2) as u8,
                    ];
                    self.draw_line_rgba(a.0.0, a.0.1, b.0.0, b.0.1, color);
                }
            }
            GL_TRIANGLES => {
                for chunk in vertices.chunks(3) {
                    if let [a, b, c] = chunk {
                        self.rasterize_triangle(a.0, b.0, c.0, a.1, b.1, c.1, a.2, b.2, c.2, active_texture.clone());
                    }
                }
            }
            GL_TRIANGLE_STRIP => {
                for i in 2..vertices.len() {
                    let a = &vertices[i - 2];
                    let b = &vertices[i - 1];
                    let c = &vertices[i];
                    if i % 2 == 0 {
                        self.rasterize_triangle(a.0, b.0, c.0, a.1, b.1, c.1, a.2, b.2, c.2, active_texture.clone());
                    } else {
                        self.rasterize_triangle(b.0, a.0, c.0, b.1, a.1, c.1, b.2, a.2, c.2, active_texture.clone());
                    }
                }
            }
            GL_TRIANGLE_FAN => {
                if vertices.len() >= 3 {
                    let first_v = vertices[0].clone();
                    for i in 2..vertices.len() {
                        let b = &vertices[i - 1];
                        let c = &vertices[i];
                        self.rasterize_triangle(first_v.0, b.0, c.0, first_v.1, b.1, c.1, first_v.2, b.2, c.2, active_texture.clone());
                    }
                }
            }
            _ => return None,
        }
        self.runtime.graphics.guest_framebuffer_dirty = true;
        self.runtime.ui_graphics.graphics_guest_draw_calls = self.runtime.ui_graphics.graphics_guest_draw_calls.saturating_add(1);
        self.runtime.ui_graphics.graphics_guest_vertex_fetches = self.runtime.ui_graphics.graphics_guest_vertex_fetches.saturating_add(vertices.len() as u32);
        self.runtime.ui_graphics.graphics_last_draw_mode = mode;
        self.runtime.ui_graphics.graphics_last_draw_mode_label = Some(Self::graphics_draw_mode_name(mode).to_string());
        self.runtime.ui_graphics.graphics_last_guest_draw_checksum = Self::checksum_bytes(&self.runtime.graphics.synthetic_framebuffer);
        Some((vertices.len(), self.runtime.ui_graphics.graphics_last_guest_draw_checksum))
    }

    fn bootstrap_synthetic_graphics(&mut self) {
        if self.runtime.ui_graphics.graphics_surface_width == 0 {
            self.runtime.ui_graphics.graphics_surface_width = 320;
        }
        if self.runtime.ui_graphics.graphics_surface_height == 0 {
            self.runtime.ui_graphics.graphics_surface_height = 480;
        }
        self.ensure_framebuffer_backing();
        self.refresh_graphics_object_labels();
    }

    fn ensure_framebuffer_backing(&mut self) {
        let width = self.runtime.ui_graphics.graphics_surface_width.max(1);
        let height = self.runtime.ui_graphics.graphics_surface_height.max(1);
        let size = width.saturating_mul(height).saturating_mul(4) as usize;
        if self.runtime.graphics.synthetic_framebuffer.len() != size {
            self.runtime.graphics.synthetic_framebuffer.resize(size, 0);
        }
        self.runtime.ui_graphics.graphics_framebuffer_bytes = self.runtime.graphics.synthetic_framebuffer.len() as u32;
        if self.runtime.ui_graphics.graphics_viewport_width == 0 {
            self.runtime.ui_graphics.graphics_viewport_width = width;
        }
        if self.runtime.ui_graphics.graphics_viewport_height == 0 {
            self.runtime.ui_graphics.graphics_viewport_height = height;
        }
    }

    fn fill_framebuffer_rgba(&mut self, rgba: [u8; 4]) {
        self.ensure_framebuffer_backing();
        for px in self.runtime.graphics.synthetic_framebuffer.chunks_exact_mut(4) {
            px.copy_from_slice(&rgba);
        }
        self.runtime.ui_graphics.graphics_framebuffer_bytes = self.runtime.graphics.synthetic_framebuffer.len() as u32;
    }

    fn rasterize_synthetic_frame(&mut self, frame_index: u32) {
        self.ensure_framebuffer_backing();
        let width = self.runtime.ui_graphics.graphics_surface_width.max(1);
        let height = self.runtime.ui_graphics.graphics_surface_height.max(1);
        let bg = self.runtime.graphics.current_clear_rgba;
        self.fill_framebuffer_rgba(bg);
        let w = width as usize;
        let h = height as usize;
        let pulse = (frame_index & 0xff) as u8;
        for y in 0..h {
            for x in 0..w {
                let idx = (y * w + x) * 4;
                let x8 = ((x as u32).saturating_mul(255) / width.max(1)) as u8;
                let y8 = ((y as u32).saturating_mul(255) / height.max(1)) as u8;
                self.runtime.graphics.synthetic_framebuffer[idx] = bg[0].saturating_div(2).saturating_add(x8 / 2).wrapping_add(pulse / 5);
                self.runtime.graphics.synthetic_framebuffer[idx + 1] = bg[1].saturating_div(2).saturating_add(y8 / 2).wrapping_add(pulse / 7);
                self.runtime.graphics.synthetic_framebuffer[idx + 2] = bg[2].saturating_div(2).saturating_add((x8 ^ y8) / 2).wrapping_add(pulse / 3);
                self.runtime.graphics.synthetic_framebuffer[idx + 3] = 0xff;
            }
        }
        let box_w = (width / 5).max(24) as usize;
        let box_h = (height / 7).max(24) as usize;
        let max_x = w.saturating_sub(box_w).max(1);
        let max_y = h.saturating_sub(box_h).max(1);
        let origin_x = ((frame_index as usize) * 11) % max_x;
        let origin_y = ((frame_index as usize) * 7) % max_y;
        let accent = [255u8.wrapping_sub(bg[0] / 2), 196u8.wrapping_sub(bg[1] / 3), 128u8.wrapping_add(pulse / 2), 0xff];
        for y in origin_y..(origin_y + box_h).min(h) {
            for x in origin_x..(origin_x + box_w).min(w) {
                let idx = (y * w + x) * 4;
                self.runtime.graphics.synthetic_framebuffer[idx] = accent[0];
                self.runtime.graphics.synthetic_framebuffer[idx + 1] = accent[1];
                self.runtime.graphics.synthetic_framebuffer[idx + 2] = accent[2];
                self.runtime.graphics.synthetic_framebuffer[idx + 3] = accent[3];
            }
        }
        self.runtime.ui_graphics.graphics_framebuffer_bytes = self.runtime.graphics.synthetic_framebuffer.len() as u32;
    }

    fn snapshot_framebuffer_rgba(&mut self) -> Vec<u8> {
        self.ensure_framebuffer_backing();
        self.runtime.graphics.synthetic_framebuffer.clone()
    }

    fn snapshot_framebuffer_region_rgba(&mut self, x: u32, y: u32, width: u32, height: u32) -> Vec<u8> {
        self.ensure_framebuffer_backing();
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1);
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1);
        let out_w = width.max(1).min(surface_w);
        let out_h = height.max(1).min(surface_h);
        let mut out = vec![0u8; out_w.saturating_mul(out_h).saturating_mul(4) as usize];
        let base_x = x.min(surface_w.saturating_sub(1));
        let base_y = y.min(surface_h.saturating_sub(1));
        for row in 0..out_h {
            let src_y = base_y.saturating_add(row).min(surface_h.saturating_sub(1));
            let dst_row = row as usize * out_w as usize * 4;
            let src_row = src_y as usize * surface_w as usize * 4;
            for col in 0..out_w {
                let src_x = base_x.saturating_add(col).min(surface_w.saturating_sub(1));
                let src_idx = src_row + src_x as usize * 4;
                let dst_idx = dst_row + col as usize * 4;
                out[dst_idx..dst_idx + 4].copy_from_slice(&self.runtime.graphics.synthetic_framebuffer[src_idx..src_idx + 4]);
            }
        }
        out
    }

    fn encode_rgba_png(bytes: &[u8], width: u32, height: u32) -> CoreResult<Vec<u8>> {
        let mut cursor = Cursor::new(Vec::new());
        let mut encoder = Encoder::new(&mut cursor, width.max(1), height.max(1));
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|err| CoreError::Backend(format!("png write_header failed: {err}")))?;
        writer
            .write_image_data(bytes)
            .map_err(|err| CoreError::Backend(format!("png write_image_data failed: {err}")))?;
        drop(writer);
        Ok(cursor.into_inner())
    }

    fn checksum_bytes(bytes: &[u8]) -> u32 {
        bytes.iter()
            .fold(0u32, |acc, &b| acc.wrapping_mul(16_777_619).wrapping_add(b as u32))
    }

    fn push_graphics_event(&mut self, event: impl Into<String>) {
        const MAX_EVENTS: usize = 16;
        let event = event.into();
        self.runtime.ui_graphics.graphics_recent_events.push(event);
        if self.runtime.ui_graphics.graphics_recent_events.len() > MAX_EVENTS {
            let overflow = self.runtime.ui_graphics.graphics_recent_events.len().saturating_sub(MAX_EVENTS);
            self.runtime.ui_graphics.graphics_recent_events.drain(0..overflow);
        }
    }

    fn push_scene_event(&mut self, event: impl Into<String>) {
        const MAX_EVENTS: usize = 16;
        let event = event.into();
        self.runtime.ui_cocos.scene_recent_events.push(event);
        if self.runtime.ui_cocos.scene_recent_events.len() > MAX_EVENTS {
            let overflow = self.runtime.ui_cocos.scene_recent_events.len().saturating_sub(MAX_EVENTS);
            self.runtime.ui_cocos.scene_recent_events.drain(0..overflow);
        }
    }

    fn push_scene_progress_trace(&mut self, event: impl Into<String>) {
        const MAX_EVENTS: usize = 16;
        let event = event.into();
        self.runtime.scene.scene_progress_trace.push(event);
        if self.runtime.scene.scene_progress_trace.len() > MAX_EVENTS {
            let overflow = self.runtime.scene.scene_progress_trace.len().saturating_sub(MAX_EVENTS);
            self.runtime.scene.scene_progress_trace.drain(0..overflow);
        }
    }

    fn push_sprite_watch_trace(&mut self, event: impl Into<String>) {
        const MAX_EVENTS: usize = 16;
        let event = event.into();
        self.runtime.scene.sprite_watch_trace.push(event);
        if self.runtime.scene.sprite_watch_trace.len() > MAX_EVENTS {
            let overflow = self.runtime.scene.sprite_watch_trace.len().saturating_sub(MAX_EVENTS);
            self.runtime.scene.sprite_watch_trace.drain(0..overflow);
        }
    }

    fn push_graph_trace(&mut self, event: impl Into<String>) {
        const MAX_EVENTS: usize = 64;
        let event = event.into();
        self.runtime.scene.graph_trace.push(event);
        if self.runtime.scene.graph_trace.len() > MAX_EVENTS {
            let overflow = self.runtime.scene.graph_trace.len().saturating_sub(MAX_EVENTS);
            self.runtime.scene.graph_trace.drain(0..overflow);
        }
    }

    fn push_scheduler_trace(&mut self, event: impl Into<String>) {
        const MAX_EVENTS: usize = 64;
        let event = event.into();
        self.runtime.scheduler.trace.events.push(event);
        if self.runtime.scheduler.trace.events.len() > MAX_EVENTS {
            let overflow = self.runtime.scheduler.trace.events.len().saturating_sub(MAX_EVENTS);
            self.runtime.scheduler.trace.events.drain(0..overflow);
        }
    }

    fn push_callback_trace(&mut self, event: impl Into<String>) {
        const MAX_EVENTS: usize = 160;
        let event = event.into();
        self.runtime.scheduler.trace.callbacks.push(event);
        if self.runtime.scheduler.trace.callbacks.len() > MAX_EVENTS {
            let overflow = self.runtime.scheduler.trace.callbacks.len().saturating_sub(MAX_EVENTS);
            self.runtime.scheduler.trace.callbacks.drain(0..overflow);
        }
    }

    fn is_scene_progress_selector(selector: &str) -> bool {
        selector.contains("DestinationScene")
            || matches!(
                selector,
                "replaceScene:"
                    | "replaceScene:byTarget:selector:"
                    | "runWithScene:"
                    | "pushScene:"
                    | "popScene"
                    | "switchTo:"
                    | "switchToAndReleaseMe:"
                    | "transitionWithDuration:scene:"
                    | "initWithDuration:scene:"
                    | "effectScene"
                    | "setEffectScene:"
                    | "setNextScene"
                    | "onEnter"
                    | "onExit"
                    | "onEnterTransitionDidFinish"
            )
    }

    fn push_scene_progress_selector_event(
        &mut self,
        label: &str,
        receiver: u32,
        class_desc: &str,
        selector: &str,
        arg2: u32,
        arg3: u32,
        result: Option<u32>,
        destination_updated: bool,
    ) {
        if !Self::is_scene_progress_selector(selector) {
            return;
        }
        self.push_scene_progress_trace(format!(
            "call label={} recv={} class={} sel={} arg2={} arg3={} result={} destinationUpdated={}",
            label,
            self.describe_ptr(receiver),
            if class_desc.is_empty() { "<unknown>" } else { class_desc },
            selector,
            self.describe_ptr(arg2),
            self.describe_ptr(arg3),
            result.map(|value| self.describe_ptr(value)).unwrap_or_else(|| "<deferred>".to_string()),
            if destination_updated { "YES" } else { "NO" },
        ));
    }

    fn is_scheduler_trace_selector(selector: &str) -> bool {
        let lower = selector.to_ascii_lowercase();
        [
            "schedule",
            "update",
            "tick",
            "performselector",
            "delay",
            "timer",
            "onenter",
            "onentertransitiondidfinish",
            "addchild",
            "runwithscene",
            "replacescene",
            "pushscene",
            "setnextscene",
            "applicationdidfinishlaunching",
            "didfinishlaunchingwithoptions",
            "applicationdidbecomeactive",
            "connectiondidfinishloading",
            "reachabilitychanged",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }

    fn scheduler_trace_watch_scene(&self) -> u32 {
        let armed_scene = self.runtime.scheduler.trace.window_scene;
        if armed_scene != 0 && self.runtime.graphics.synthetic_sprites.contains_key(&armed_scene) {
            armed_scene
        } else {
            self.resolve_synthetic_progress_watch_scene(self.runtime.ui_cocos.running_scene)
        }
    }

    fn is_scheduler_trace_window_active(&self) -> bool {
        self.runtime.scheduler.trace.window_scene != 0 && self.runtime.ui_runtime.runloop_ticks <= self.runtime.scheduler.trace.window_end_tick
    }

    fn arm_scheduler_trace_window(&mut self, scene: u32, origin: &str, label: &str) {
        const WINDOW_TICKS: u32 = 24;
        if scene == 0 {
            return;
        }
        self.runtime.scheduler.trace.window_scene = scene;
        self.runtime.scheduler.trace.window_start_tick = self.runtime.ui_runtime.runloop_ticks;
        self.runtime.scheduler.trace.window_end_tick = self.runtime.ui_runtime.runloop_ticks.saturating_add(WINDOW_TICKS);
        self.runtime.scheduler.trace.window_origin = Some(origin.to_string());
        self.push_scheduler_trace(format!(
            "window.arm tick={} until={} scene={} label={} origin={} sceneState=[{}] parentChain={}",
            self.runtime.scheduler.trace.window_start_tick,
            self.runtime.scheduler.trace.window_end_tick,
            self.describe_ptr(scene),
            if label.is_empty() { "<unknown>" } else { label },
            origin,
            self.describe_node_graph_state(scene),
            self.describe_parent_chain(scene),
        ));
    }

    fn describe_parent_chain(&self, ptr: u32) -> String {
        if ptr == 0 {
            return "nil".to_string();
        }
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut current = ptr;
        let mut depth = 0usize;
        while current != 0 && seen.insert(current) && depth < 12 {
            chain.push(self.describe_ptr(current));
            current = self.runtime.graphics.synthetic_sprites.get(&current).map(|state| state.parent).unwrap_or(0);
            depth += 1;
        }
        if current != 0 {
            chain.push("…".to_string());
        }
        chain.join(" <- ")
    }

    fn parent_chain_contains(&self, ptr: u32, ancestor: u32) -> bool {
        if ptr == 0 || ancestor == 0 {
            return false;
        }
        let mut seen = HashSet::new();
        let mut current = ptr;
        let mut depth = 0usize;
        while current != 0 && seen.insert(current) && depth < 12 {
            if current == ancestor {
                return true;
            }
            current = self.runtime.graphics.synthetic_sprites.get(&current).map(|state| state.parent).unwrap_or(0);
            depth += 1;
        }
        false
    }

    fn lifecycle_relation_to_scene(&self, target: u32, scene: u32) -> &'static str {
        if target == 0 || scene == 0 {
            return "unrelated";
        }
        if target == scene {
            return "scene-root";
        }
        let parent = self.runtime.graphics.synthetic_sprites.get(&target).map(|state| state.parent).unwrap_or(0);
        if parent == scene {
            return "direct-child";
        }
        if self.parent_chain_contains(target, scene) {
            return "descendant";
        }
        "unrelated"
    }

    fn describe_scheduler_trace_target(&self, role: &str, ptr: u32, watched_scene: u32) -> String {
        if ptr == 0 {
            return format!("{}=nil", role);
        }
        let class_name = self
            .objc_class_name_for_receiver(ptr)
            .or_else(|| self.objc_class_name_for_ptr(ptr))
            .unwrap_or_default();
        let label = self.diag.object_labels.get(&ptr).cloned().unwrap_or_default();
        let relation = if ptr == watched_scene {
            "watched-scene"
        } else if self.parent_chain_contains(ptr, watched_scene) {
            "descendant-of-watch"
        } else if ptr == self.runtime.ui_cocos.cocos_director {
            "director"
        } else {
            "unrelated"
        };
        format!(
            "{}={{ptr={}, class={}, label={}, relation={}, parentChain={}}}",
            role,
            self.describe_ptr(ptr),
            if class_name.is_empty() { "<unknown>" } else { &class_name },
            if label.is_empty() { "<none>" } else { &label },
            relation,
            self.describe_parent_chain(ptr),
        )
    }

    fn push_scheduler_trace_selector_event(
        &mut self,
        label: &str,
        receiver: u32,
        class_desc: &str,
        selector: &str,
        arg2: u32,
        arg3: u32,
        result: Option<u32>,
    ) {
        if !Self::is_scheduler_trace_selector(selector) {
            return;
        }
        let watched_scene = self.scheduler_trace_watch_scene();
        let within_window = self.is_scheduler_trace_window_active();
        let fallback_hit = watched_scene != 0
            && (receiver == watched_scene
                || self.parent_chain_contains(receiver, watched_scene)
                || arg2 == watched_scene
                || self.parent_chain_contains(arg2, watched_scene)
                || arg3 == watched_scene
                || self.parent_chain_contains(arg3, watched_scene)
                || result.map(|value| value == watched_scene || self.parent_chain_contains(value, watched_scene)).unwrap_or(false));
        if !within_window && !fallback_hit {
            return;
        }
        let mut targets = vec![
            self.describe_scheduler_trace_target("recv", receiver, watched_scene),
            self.describe_scheduler_trace_target("arg2", arg2, watched_scene),
            self.describe_scheduler_trace_target("arg3", arg3, watched_scene),
        ];
        if let Some(value) = result {
            targets.push(self.describe_scheduler_trace_target("result", value, watched_scene));
        } else {
            targets.push("result=<deferred>".to_string());
        }
        self.push_scheduler_trace(format!(
            "call tick={} window={} label={} sel={} recv={} class={} arg2={} arg3={} result={} watchedScene={} watchOrigin={} targets=[{}]",
            self.runtime.ui_runtime.runloop_ticks,
            if within_window { "ACTIVE" } else { "fallback" },
            label,
            selector,
            self.describe_ptr(receiver),
            if class_desc.is_empty() { "<unknown>" } else { class_desc },
            self.describe_ptr(arg2),
            self.describe_ptr(arg3),
            result.map(|value| self.describe_ptr(value)).unwrap_or_else(|| "<deferred>".to_string()),
            if watched_scene != 0 { self.describe_ptr(watched_scene) } else { "nil".to_string() },
            self.runtime.scheduler.trace.window_origin.clone().unwrap_or_else(|| "<none>".to_string()),
            targets.join("; "),
        ));
    }

    fn push_scheduler_event(&mut self, event: impl Into<String>) {
        const MAX_EVENTS: usize = 16;
        let event = event.into();
        self.runtime.ui_cocos.scheduler_recent_events.push(event);
        if self.runtime.ui_cocos.scheduler_recent_events.len() > MAX_EVENTS {
            let overflow = self.runtime.ui_cocos.scheduler_recent_events.len().saturating_sub(MAX_EVENTS);
            self.runtime.ui_cocos.scheduler_recent_events.drain(0..overflow);
        }
    }

    fn note_scheduler_selector(&mut self, selector: &str, receiver: u32) {
        let detail = format!("{} on {}", selector, self.describe_ptr(receiver));
        match selector {
            "mainLoop" => {
                self.runtime.ui_cocos.scheduler_mainloop_calls = self.runtime.ui_cocos.scheduler_mainloop_calls.saturating_add(1);
                self.push_scheduler_event(format!("mainLoop {}", self.describe_ptr(receiver)));
            }
            "drawScene" => {
                self.runtime.ui_cocos.scheduler_draw_scene_calls = self.runtime.ui_cocos.scheduler_draw_scene_calls.saturating_add(1);
                self.push_scheduler_event(format!("drawScene {}", self.describe_ptr(receiver)));
            }
            "drawFrame:" => {
                self.runtime.ui_cocos.scheduler_draw_frame_calls = self.runtime.ui_cocos.scheduler_draw_frame_calls.saturating_add(1);
                self.push_scheduler_event(format!("drawFrame {}", self.describe_ptr(receiver)));
            }
            "invalidate" => {
                self.runtime.ui_cocos.scheduler_invalidate_calls = self.runtime.ui_cocos.scheduler_invalidate_calls.saturating_add(1);
                self.runtime.scene.auto_scene_last_present_signature = None;
                self.push_scheduler_event(format!("invalidate {}", self.describe_ptr(receiver)));
            }
            "setNeedsDisplay" | "layoutIfNeeded" | "layoutSubviews" | "display" | "displayIfNeeded" | "swapBuffers" | "presentRenderbuffer:" => {
                self.runtime.ui_cocos.scheduler_render_callback_calls = self.runtime.ui_cocos.scheduler_render_callback_calls.saturating_add(1);
                self.push_scheduler_event(format!("render-cb {}", detail));
            }
            "update:" | "tick:" => {
                self.runtime.ui_cocos.scheduler_update_calls = self.runtime.ui_cocos.scheduler_update_calls.saturating_add(1);
                self.push_scheduler_event(format!("update {}", detail));
            }
            _ if selector.starts_with("schedule") || selector.starts_with("unschedule") => {
                self.runtime.ui_cocos.scheduler_schedule_calls = self.runtime.ui_cocos.scheduler_schedule_calls.saturating_add(1);
                self.push_scheduler_event(format!("schedule {}", detail));
            }
            _ => {}
        }
    }

    fn note_scheduler_selector_handoff(
        &mut self,
        origin: &str,
        receiver: u32,
        target: u32,
        selector: &str,
        arg2: u32,
        reason: &str,
    ) {
        let selector = selector.trim_matches('\0');
        if selector.is_empty() {
            return;
        }
        let class_desc = self
            .objc_receiver_class_name_hint(target)
            .or_else(|| self.objc_receiver_class_name_hint(receiver))
            .unwrap_or_else(|| "<unknown>".to_string());
        self.note_scheduler_selector(selector, target);
        self.push_scheduler_trace_selector_event(
            if origin.is_empty() { "scheduler-handoff" } else { origin },
            target,
            &class_desc,
            selector,
            arg2,
            0,
            None,
        );
        self.push_callback_trace(format!(
            "scheduler.handoff tick={} origin={} reason={} recv={} target={} class={} sel={} arg2={} watchedScene={}",
            self.runtime.ui_runtime.runloop_ticks,
            if origin.is_empty() { "<unknown>" } else { origin },
            if reason.is_empty() { "<none>" } else { reason },
            self.describe_ptr(receiver),
            self.describe_ptr(target),
            class_desc,
            selector,
            self.describe_ptr(arg2),
            self.describe_ptr(self.scheduler_trace_watch_scene()),
        ));
        self.diag.trace.push(format!(
            "     ↳ hle scheduler-handoff origin={} reason={} recv={} target={} class={} selector={} arg2={} tick={}",
            if origin.is_empty() { "<unknown>" } else { origin },
            if reason.is_empty() { "<none>" } else { reason },
            self.describe_ptr(receiver),
            self.describe_ptr(target),
            class_desc,
            selector,
            self.describe_ptr(arg2),
            self.runtime.ui_runtime.runloop_ticks,
        ));
    }

    fn color_to_hex_rgba(rgba: [u8; 4]) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", rgba[0], rgba[1], rgba[2], rgba[3])
    }

    fn analyze_dominant_color(bytes: &[u8], width: u32, height: u32) -> Option<([u8; 4], u32)> {
        if width == 0 || height == 0 {
            return None;
        }
        let expected = width.saturating_mul(height).saturating_mul(4) as usize;
        if bytes.len() < expected {
            return None;
        }
        let mut counts: HashMap<u32, u32> = HashMap::new();
        let mut best_key = 0u32;
        let mut best_count = 0u32;
        for px in bytes[..expected].chunks_exact(4) {
            let key = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
            let count = counts.entry(key).or_insert(0);
            *count = count.saturating_add(1);
            if *count > best_count {
                best_count = *count;
                best_key = key;
            }
        }
        Some((best_key.to_le_bytes(), best_count))
    }

    fn maybe_dump_unique_frame(&mut self, origin: &str, rgba: &[u8], width: u32, height: u32, checksum: u32) -> Option<String> {
        if !self.tuning.dump_frames {
            return None;
        }
        let max_unique = self.tuning.dump_limit.max(4);
        if max_unique > 0 && self.runtime.ui_graphics.graphics_unique_frames_saved >= max_unique {
            return None;
        }
        if self.runtime.graphics.synthetic_unique_frame_checksums.iter().any(|seen| *seen == checksum) {
            return None;
        }
        self.runtime.graphics.synthetic_unique_frame_checksums.push_back(checksum);
        while self.runtime.graphics.synthetic_unique_frame_checksums.len() > 16 {
            self.runtime.graphics.synthetic_unique_frame_checksums.pop_front();
        }
        let idx = self.runtime.ui_graphics.graphics_unique_frames_saved.saturating_add(1);
        let safe_origin: String = origin.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let file_name = format!("graphics_unique_{:03}_{}.png", idx, safe_origin);
        let path = self.write_named_png_dump(&file_name, rgba, width, height);
        if let Some(path_str) = path.clone() {
            self.runtime.ui_graphics.graphics_unique_frames_saved = self.runtime.ui_graphics.graphics_unique_frames_saved.saturating_add(1);
            self.runtime.ui_graphics.graphics_last_unique_dump_path = Some(path_str.clone());
            self.push_graphics_event(format!("unique-frame #{} origin={} checksum=0x{:08x} path={}", idx, origin, checksum, path_str));
        }
        path
    }

    fn swap_red_blue_rgba(bytes: &[u8]) -> Vec<u8> {
        let mut out = bytes.to_vec();
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        out
    }

    fn analyze_rgba_visible_bbox(bytes: &[u8], width: u32, height: u32) -> Option<(u32, u32, u32, u32, u32, u32)> {
        if width == 0 || height == 0 {
            return None;
        }
        let expected = width.saturating_mul(height).saturating_mul(4) as usize;
        if bytes.len() < expected {
            return None;
        }
        let mut min_x = width;
        let mut min_y = height;
        let mut max_x = 0u32;
        let mut max_y = 0u32;
        let mut visible = 0u32;
        let mut nonzero = 0u32;
        for y in 0..height {
            let row = y as usize * width as usize * 4;
            for x in 0..width {
                let idx = row + x as usize * 4;
                let px = &bytes[idx..idx + 4];
                let rgb_nonzero = px[0] != 0 || px[1] != 0 || px[2] != 0;
                let visible_px = px[3] != 0 || rgb_nonzero;
                if rgb_nonzero {
                    nonzero = nonzero.saturating_add(1);
                }
                if !visible_px {
                    continue;
                }
                visible = visible.saturating_add(1);
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
        if visible == 0 {
            None
        } else {
            Some((
                min_x,
                min_y,
                max_x.saturating_sub(min_x).saturating_add(1),
                max_y.saturating_sub(min_y).saturating_add(1),
                visible,
                nonzero,
            ))
        }
    }

    fn write_named_png_dump(&mut self, file_name: &str, rgba: &[u8], width: u32, height: u32) -> Option<String> {
        if !self.tuning.dump_frames {
            return None;
        }
        if let Err(err) = fs::create_dir_all(&self.tuning.frame_dump_dir) {
            self.diag.trace.push(format!("     ↳ hle graphics-dump mkdir failed dir={} err={}", self.tuning.frame_dump_dir.display(), err));
            return None;
        }
        let png = match Self::encode_rgba_png(rgba, width.max(1), height.max(1)) {
            Ok(png) => png,
            Err(err) => {
                self.diag.trace.push(format!("     ↳ hle graphics-dump encode failed file={} err={}", file_name, err));
                return None;
            }
        };
        let path = self.tuning.frame_dump_dir.join(file_name);
        if let Err(err) = fs::write(&path, &png) {
            self.diag.trace.push(format!("     ↳ hle graphics-dump write failed path={} err={}", path.display(), err));
            return None;
        }
        Some(path.display().to_string())
    }

    fn update_present_diagnostics(&mut self, origin: &str, rgba: &[u8], width: u32, height: u32) {
        self.runtime.ui_graphics.graphics_last_visible_bbox_x = 0;
        self.runtime.ui_graphics.graphics_last_visible_bbox_y = 0;
        self.runtime.ui_graphics.graphics_last_visible_bbox_width = 0;
        self.runtime.ui_graphics.graphics_last_visible_bbox_height = 0;
        self.runtime.ui_graphics.graphics_last_visible_pixels = 0;
        self.runtime.ui_graphics.graphics_last_nonzero_pixels = 0;
        self.runtime.ui_graphics.graphics_diagnosis_hint = None;
        self.runtime.ui_graphics.graphics_last_dominant_rgba = None;
        self.runtime.ui_graphics.graphics_last_dominant_pct_milli = 0;
        self.runtime.ui_graphics.graphics_last_raw_dump_path = None;
        self.runtime.ui_graphics.graphics_last_bgra_dump_path = None;
        self.runtime.ui_graphics.graphics_last_viewport_tl_dump_path = None;
        self.runtime.ui_graphics.graphics_last_viewport_bl_dump_path = None;
        self.runtime.ui_graphics.graphics_last_bbox_dump_path = None;

        let Some((bbox_x, bbox_y, bbox_w, bbox_h, visible_px, nonzero_px)) = Self::analyze_rgba_visible_bbox(rgba, width, height) else {
            let hint = format!(
                "{} readback is fully blank; inspect present path before pixel-format guesses",
                origin,
            );
            self.runtime.ui_graphics.graphics_diagnosis_hint = Some(hint.clone());
            self.push_graphics_event(format!(
                "present-diag origin={} surface={}x{} viewport=({},{} {}x{}) scissor={}({},{} {}x{}) bbox=<none>",
                origin,
                width,
                height,
                self.runtime.ui_graphics.graphics_viewport_x,
                self.runtime.ui_graphics.graphics_viewport_y,
                self.runtime.ui_graphics.graphics_viewport_width,
                self.runtime.ui_graphics.graphics_viewport_height,
                if self.runtime.ui_graphics.graphics_scissor_enabled { "on" } else { "off" },
                self.runtime.ui_graphics.graphics_scissor_x,
                self.runtime.ui_graphics.graphics_scissor_y,
                self.runtime.ui_graphics.graphics_scissor_width,
                self.runtime.ui_graphics.graphics_scissor_height,
            ));
            return;
        };

        self.runtime.ui_graphics.graphics_last_visible_bbox_x = bbox_x;
        self.runtime.ui_graphics.graphics_last_visible_bbox_y = bbox_y;
        self.runtime.ui_graphics.graphics_last_visible_bbox_width = bbox_w;
        self.runtime.ui_graphics.graphics_last_visible_bbox_height = bbox_h;
        self.runtime.ui_graphics.graphics_last_visible_pixels = visible_px;
        self.runtime.ui_graphics.graphics_last_nonzero_pixels = nonzero_px;

        let surface_area = width.max(1).saturating_mul(height.max(1));
        if let Some((dom_rgba, dom_count)) = Self::analyze_dominant_color(rgba, width, height) {
            self.runtime.ui_graphics.graphics_last_dominant_rgba = Some(Self::color_to_hex_rgba(dom_rgba));
            self.runtime.ui_graphics.graphics_last_dominant_pct_milli = if surface_area == 0 { 0 } else { dom_count.saturating_mul(1000) / surface_area.max(1) };
        }
        let bbox_area = bbox_w.saturating_mul(bbox_h);
        let bbox_pct = if surface_area == 0 { 0.0 } else { bbox_area as f32 * 100.0 / surface_area as f32 };
        let viewport_mismatch = self.runtime.ui_graphics.graphics_viewport_ready
            && (self.runtime.ui_graphics.graphics_viewport_x != 0
                || self.runtime.ui_graphics.graphics_viewport_y != 0
                || self.runtime.ui_graphics.graphics_viewport_width != 0 && self.runtime.ui_graphics.graphics_viewport_width != width
                || self.runtime.ui_graphics.graphics_viewport_height != 0 && self.runtime.ui_graphics.graphics_viewport_height != height);
        let bbox_suspiciously_small = bbox_area > 0 && bbox_area.saturating_mul(100) < surface_area.saturating_mul(70);
        let bbox_off_origin = bbox_x > 8 || bbox_y > 8;

        let hint = if viewport_mismatch && (bbox_suspiciously_small || bbox_off_origin) {
            format!(
                "viewport/scissor mismatch likely: surface={}x{}, viewport=({},{} {}x{}), bbox=({},{} {}x{}, {:.1}% of surface)",
                width,
                height,
                self.runtime.ui_graphics.graphics_viewport_x,
                self.runtime.ui_graphics.graphics_viewport_y,
                self.runtime.ui_graphics.graphics_viewport_width,
                self.runtime.ui_graphics.graphics_viewport_height,
                bbox_x,
                bbox_y,
                bbox_w,
                bbox_h,
                bbox_pct,
            )
        } else if bbox_suspiciously_small {
            format!(
                "readback contains only a partial image: bbox=({},{} {}x{}, {:.1}% of surface); inspect CGContext/viewport destination rects and backing scale",
                bbox_x,
                bbox_y,
                bbox_w,
                bbox_h,
                bbox_pct,
            )
        } else {
            format!(
                "full-surface content exists; inspect origin/pixel-format if colors still look wrong (bbox=({},{} {}x{}))",
                bbox_x,
                bbox_y,
                bbox_w,
                bbox_h,
            )
        };
        self.runtime.ui_graphics.graphics_diagnosis_hint = Some(hint.clone());
        self.push_graphics_event(format!(
            "present-diag origin={} surface={}x{} viewport=({},{} {}x{}) scissor={}({},{} {}x{}) bbox=({},{} {}x{}) visiblePx={} nonzeroPx={} hint={}",
            origin,
            width,
            height,
            self.runtime.ui_graphics.graphics_viewport_x,
            self.runtime.ui_graphics.graphics_viewport_y,
            self.runtime.ui_graphics.graphics_viewport_width,
            self.runtime.ui_graphics.graphics_viewport_height,
            if self.runtime.ui_graphics.graphics_scissor_enabled { "on" } else { "off" },
            self.runtime.ui_graphics.graphics_scissor_x,
            self.runtime.ui_graphics.graphics_scissor_y,
            self.runtime.ui_graphics.graphics_scissor_width,
            self.runtime.ui_graphics.graphics_scissor_height,
            bbox_x,
            bbox_y,
            bbox_w,
            bbox_h,
            visible_px,
            nonzero_px,
            hint,
        ));

        if self.tuning.dump_frames {
            self.runtime.ui_graphics.graphics_last_raw_dump_path = self.write_named_png_dump("graphics_latest_raw.png", rgba, width, height);
            let bgra = Self::swap_red_blue_rgba(rgba);
            self.runtime.graphics.synthetic_last_bgra_swizzle_rgba = bgra.clone();
            self.runtime.ui_graphics.graphics_last_bgra_dump_path = self.write_named_png_dump("graphics_latest_bgra_swizzle.png", &bgra, width, height);
            if bbox_w > 0 && bbox_h > 0 {
                if let Some(bbox_rgba) = Self::crop_rgba_region(rgba, width, height, bbox_x, bbox_y, bbox_w, bbox_h) {
                    self.runtime.ui_graphics.graphics_last_bbox_dump_path = self.write_named_png_dump("graphics_latest_bbox.png", &bbox_rgba, bbox_w, bbox_h);
                }
            }
            let vp_w = self.runtime.ui_graphics.graphics_viewport_width.min(width);
            let vp_h = self.runtime.ui_graphics.graphics_viewport_height.min(height);
            if vp_w > 0 && vp_h > 0 {
                let vp_x = self.runtime.ui_graphics.graphics_viewport_x.min(width.saturating_sub(1));
                let vp_y = self.runtime.ui_graphics.graphics_viewport_y.min(height.saturating_sub(1));
                if vp_x.saturating_add(vp_w) <= width && vp_y.saturating_add(vp_h) <= height {
                    if let Some(vp_rgba) = Self::crop_rgba_region(rgba, width, height, vp_x, vp_y, vp_w, vp_h) {
                        self.runtime.ui_graphics.graphics_last_viewport_tl_dump_path = self.write_named_png_dump("graphics_latest_viewport_tl.png", &vp_rgba, vp_w, vp_h);
                    }
                }
                let flipped_y = height.saturating_sub(vp_y.saturating_add(vp_h));
                if vp_x.saturating_add(vp_w) <= width && flipped_y.saturating_add(vp_h) <= height {
                    if let Some(vp_rgba) = Self::crop_rgba_region(rgba, width, height, vp_x, flipped_y, vp_w, vp_h) {
                        self.runtime.ui_graphics.graphics_last_viewport_bl_dump_path = self.write_named_png_dump("graphics_latest_viewport_bl.png", &vp_rgba, vp_w, vp_h);
                    }
                }
            }
        }
    }

    fn stage_readback_rgba(&mut self, origin: &str, rgba: Vec<u8>) {
        self.runtime.graphics.synthetic_last_readback_rgba = rgba;
        self.runtime.ui_graphics.graphics_readback_ready = !self.runtime.graphics.synthetic_last_readback_rgba.is_empty();
        self.runtime.ui_graphics.graphics_readback_calls = self.runtime.ui_graphics.graphics_readback_calls.saturating_add(1);
        self.runtime.ui_graphics.graphics_last_readback_bytes = self.runtime.graphics.synthetic_last_readback_rgba.len() as u32;
        let checksum = Self::checksum_bytes(&self.runtime.graphics.synthetic_last_readback_rgba);
        let changed = self.runtime.graphics
            .synthetic_previous_readback_checksum
            .map(|prev| prev != checksum)
            .unwrap_or(true);
        self.runtime.ui_graphics.graphics_readback_changed = changed;
        self.runtime.ui_graphics.graphics_readback_stable_streak = if changed {
            0
        } else {
            self.runtime.ui_graphics.graphics_readback_stable_streak.saturating_add(1)
        };
        self.runtime.graphics.synthetic_previous_readback_checksum = Some(checksum);
        self.runtime.ui_graphics.graphics_last_readback_checksum = checksum;
        self.runtime.ui_graphics.graphics_last_readback_origin = Some(origin.to_string());
        if self.runtime.ui_graphics.graphics_last_readback_width == 0 {
            self.runtime.ui_graphics.graphics_last_readback_width = self.runtime.ui_graphics.graphics_surface_width.max(1);
        }
        if self.runtime.ui_graphics.graphics_last_readback_height == 0 {
            self.runtime.ui_graphics.graphics_last_readback_height = self.runtime.ui_graphics.graphics_surface_height.max(1);
        }
        let width = self.runtime.ui_graphics.graphics_last_readback_width.max(1);
        let height = self.runtime.ui_graphics.graphics_last_readback_height.max(1);
        let readback_rgba = self.runtime.graphics.synthetic_last_readback_rgba.clone();
        let _ = self.maybe_dump_unique_frame(origin, &readback_rgba, width, height, checksum);
        self.push_graphics_event(format!(
            "readback origin={} checksum=0x{:08x} changed={} stableStreak={} rect=({},{} {}x{})",
            origin,
            checksum,
            if changed { "YES" } else { "NO" },
            self.runtime.ui_graphics.graphics_readback_stable_streak,
            self.runtime.ui_graphics.graphics_last_readback_x,
            self.runtime.ui_graphics.graphics_last_readback_y,
            self.runtime.ui_graphics.graphics_last_readback_width,
            self.runtime.ui_graphics.graphics_last_readback_height,
        ));
    }

    fn stage_full_surface_readback(&mut self, origin: &str) {
        let width = self.runtime.ui_graphics.graphics_surface_width.max(1);
        let height = self.runtime.ui_graphics.graphics_surface_height.max(1);
        self.runtime.ui_graphics.graphics_last_readback_x = 0;
        self.runtime.ui_graphics.graphics_last_readback_y = 0;
        self.runtime.ui_graphics.graphics_last_readback_width = width;
        self.runtime.ui_graphics.graphics_last_readback_height = height;
        let rgba = self.snapshot_framebuffer_rgba();
        self.stage_readback_rgba(origin, rgba);
        let latest = self.runtime.graphics.synthetic_last_readback_rgba.clone();
        self.update_present_diagnostics(origin, &latest, width, height);
    }

    fn framebuffer_has_visible_pixels(&self) -> bool {
        self.runtime.graphics.synthetic_framebuffer.chunks_exact(4).any(|px| px.iter().any(|&b| b != 0))
    }

    fn maybe_dump_current_frame(&mut self, origin: &str) -> Option<String> {
        if !self.tuning.dump_frames {
            return None;
        }
        let present = self.runtime.ui_graphics.graphics_present_calls.max(self.runtime.ui_graphics.graphics_frame_index);
        if present == 0 {
            return None;
        }
        if present % self.tuning.dump_every.max(1) != 0 {
            return None;
        }
        if self.tuning.dump_limit > 0 && self.runtime.ui_graphics.graphics_dump_saved >= self.tuning.dump_limit {
            return None;
        }
        let width = self.runtime.ui_graphics.graphics_surface_width.max(1);
        let height = self.runtime.ui_graphics.graphics_surface_height.max(1);
        let expected_len = width.saturating_mul(height).saturating_mul(4) as usize;
        let using_readback = !self.runtime.graphics.synthetic_last_readback_rgba.is_empty() && self.runtime.graphics.synthetic_last_readback_rgba.len() == expected_len;
        let rgba = if using_readback {
            self.runtime.graphics.synthetic_last_readback_rgba.clone()
        } else {
            self.snapshot_framebuffer_rgba()
        };
        let png = match Self::encode_rgba_png(&rgba, width, height) {
            Ok(png) => png,
            Err(err) => {
                self.diag.trace.push(format!("     ↳ hle frame-dump encode failed origin={} err={}", origin, err));
                return None;
            }
        };
        if let Err(err) = fs::create_dir_all(&self.tuning.frame_dump_dir) {
            self.diag.trace.push(format!("     ↳ hle frame-dump mkdir failed dir={} err={}", self.tuning.frame_dump_dir.display(), err));
            return None;
        }
        let path = self.tuning.frame_dump_dir.join(format!("frame_{present:06}.png"));
        if let Err(err) = fs::write(&path, &png) {
            self.diag.trace.push(format!("     ↳ hle frame-dump write failed path={} err={}", path.display(), err));
            return None;
        }
        let rendered = path.display().to_string();
        self.runtime.ui_graphics.graphics_dump_saved = self.runtime.ui_graphics.graphics_dump_saved.saturating_add(1);
        self.runtime.ui_graphics.graphics_last_dump_path = Some(rendered.clone());
        self.diag.trace.push(format!("     ↳ hle frame-dump saved origin={} source={} path={} size={}x{} bytes={} checksum=0x{:08x}", origin, if using_readback { "present-readback" } else { "framebuffer" }, rendered, width, height, png.len(), Self::checksum_bytes(&rgba)));
        Some(rendered)
    }

    fn render_presented_frame(&mut self, origin: &str, auto_scene_draws: usize) -> Option<String> {
        self.bootstrap_synthetic_graphics();
        self.runtime.ui_graphics.graphics_context_current = true;
        self.runtime.ui_graphics.graphics_surface_ready = true;
        self.runtime.ui_graphics.graphics_framebuffer_complete = true;
        self.runtime.ui_graphics.graphics_viewport_ready = true;
        if self.runtime.ui_graphics.graphics_viewport_width == 0 {
            self.runtime.ui_graphics.graphics_viewport_x = 0;
            self.runtime.ui_graphics.graphics_viewport_y = 0;
            self.runtime.ui_graphics.graphics_viewport_width = self.runtime.ui_graphics.graphics_surface_width;
        }
        if self.runtime.ui_graphics.graphics_viewport_height == 0 {
            self.runtime.ui_graphics.graphics_viewport_height = self.runtime.ui_graphics.graphics_surface_height;
        }
        let next_frame = self.runtime.ui_graphics.graphics_frame_index.saturating_add(1);
        let decision = decide_frame_source(
            self.runtime.graphics.uikit_framebuffer_dirty,
            self.runtime.graphics.guest_framebuffer_dirty,
            self.runtime.graphics.guest_draws_since_present as usize,
            auto_scene_draws,
            self.framebuffer_has_visible_pixels(),
            self.runtime.ui_graphics.graphics_present_calls > 0 || self.runtime.ui_graphics.graphics_frame_index > 0,
        );
        if matches!(decision.source, RenderFrameSource::SyntheticFallback) {
            self.rasterize_synthetic_frame(next_frame);
            self.runtime.ui_graphics.graphics_clear_calls = self.runtime.ui_graphics.graphics_clear_calls.saturating_add(1);
            self.runtime.ui_graphics.graphics_draw_calls = self.runtime.ui_graphics.graphics_draw_calls.saturating_add(1);
            self.runtime.ui_graphics.graphics_synthetic_fallback_present_calls = self
                .runtime.ui_graphics
                .graphics_synthetic_fallback_present_calls
                .saturating_add(1);
        }
        if matches!(decision.source, RenderFrameSource::Retained) {
            self.runtime.ui_graphics.graphics_retained_present_calls = self
                .runtime.ui_graphics
                .graphics_retained_present_calls
                .saturating_add(1);
        }
        if matches!(decision.source, RenderFrameSource::SyntheticScene) {
            self.runtime.ui_graphics.graphics_auto_scene_present_calls = self
                .runtime.ui_graphics
                .graphics_auto_scene_present_calls
                .saturating_add(1);
        }
        self.runtime.ui_graphics.graphics_present_calls = self.runtime.ui_graphics.graphics_present_calls.saturating_add(1);
        self.runtime.ui_graphics.graphics_frame_index = next_frame;
        self.runtime.ui_graphics.graphics_presented = true;
        self.runtime.ui_graphics.graphics_last_present_source = Some(decision.source.as_str().to_string());
        self.runtime.ui_graphics.graphics_last_present_decision = Some(decision.summary());
        self.push_graphics_event(format!("present frame#{} source={} reused={} reason={}", next_frame, decision.source.as_str(), if decision.reused_previous { "YES" } else { "NO" }, decision.reason));
        self.stage_full_surface_readback(decision.source.readback_origin());
        let width = self.runtime.ui_graphics.graphics_surface_width.max(1);
        let height = self.runtime.ui_graphics.graphics_surface_height.max(1);
        let expected = width.saturating_mul(height).saturating_mul(4) as usize;
        if self.runtime.graphics.synthetic_last_readback_rgba.len() == expected {
            publish_live_frame(LiveFramePacket {
                frame_index: next_frame,
                width,
                height,
                rgba: self.runtime.graphics.synthetic_last_readback_rgba.clone(),
                source: decision.source.as_str().to_string(),
                reason: decision.reason.to_string(),
                reused_previous: decision.reused_previous,
            });
        }
        self.runtime.graphics.guest_framebuffer_dirty = false;
        self.runtime.graphics.guest_draws_since_present = 0;
        self.runtime.graphics.uikit_framebuffer_dirty = false;
        let dump_path = self.maybe_dump_current_frame(origin);
        self.refresh_graphics_object_labels();
        dump_path
    }

    fn readback_pixels_to_guest(&mut self, x: u32, y: u32, width: u32, height: u32, pixels_ptr: u32) {
        self.runtime.ui_graphics.graphics_last_readback_x = x;
        self.runtime.ui_graphics.graphics_last_readback_y = y;
        self.runtime.ui_graphics.graphics_last_readback_width = width;
        self.runtime.ui_graphics.graphics_last_readback_height = height;
        if pixels_ptr == 0 {
            self.runtime.graphics.synthetic_last_readback_rgba.clear();
            self.runtime.ui_graphics.graphics_last_readback_bytes = 0;
            self.runtime.ui_graphics.graphics_last_readback_checksum = 0;
            self.runtime.ui_graphics.graphics_last_readback_origin = Some("glReadPixels(null)".to_string());
            self.push_graphics_event(format!("glReadPixels -> null target rect=({},{} {}x{})", x, y, width, height));
            return;
        }
        let rgba = self.snapshot_framebuffer_region_rgba(x, y, width, height);
        self.stage_readback_rgba("glReadPixels", rgba.clone());
        self.push_graphics_event(format!("glReadPixels rect=({},{} {}x{}) bytes={} checksum=0x{:08x}", x, y, width, height, rgba.len(), Self::checksum_bytes(&rgba)));
        let _ = self.write_bytes(pixels_ptr, &rgba);
    }
}
