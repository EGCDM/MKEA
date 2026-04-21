impl MemoryArm32Backend {
    fn resolve_guest_gl_name(&self, raw: u32, preferred: u32, current: u32) -> u32 {
        if raw == 0 || raw == preferred || raw == current {
            return raw;
        }
        if self.find_region(raw, 4).is_some() {
            if let Ok(indirect) = self.read_u32_le(raw) {
                if indirect == preferred || indirect == current {
                    return indirect;
                }
            }
        }
        raw
    }

    fn read_guest_gl_name_array(&self, names_ptr: u32, count: u32, preferred: u32, current: u32) -> Vec<u32> {
        if count == 0 {
            return Vec::new();
        }
        let mut names = Vec::new();
        let byte_len = count.saturating_mul(4);
        if names_ptr != 0 && self.find_region(names_ptr, byte_len).is_some() {
            for i in 0..count {
                let addr = names_ptr.wrapping_add(i.wrapping_mul(4));
                if let Ok(raw) = self.read_u32_le(addr) {
                    names.push(self.resolve_guest_gl_name(raw, preferred, current));
                }
            }
        }
        if names.is_empty() {
            names.push(self.resolve_guest_gl_name(names_ptr, preferred, current));
        }
        names
    }

    fn describe_gl_name_list(&self, names: &[u32]) -> String {
        if names.is_empty() {
            return "[]".to_string();
        }
        let rendered: Vec<String> = names.iter().map(|name| self.describe_ptr(*name)).collect();
        format!("[{}]", rendered.join(", "))
    }



    fn cg_affine_read_words_from_args(&self, reg_start: usize, stack_words: u32, total_words: usize) -> [u32; 6] {
        let mut out = [0u32; 6];
        let reg_cap = total_words.min(4usize.saturating_sub(reg_start));
        for i in 0..reg_cap {
            out[i] = self.cpu.regs[reg_start + i];
        }
        for i in reg_cap..total_words.min(6) {
            out[i] = self.peek_stack_u32(stack_words + (i - reg_cap) as u32).unwrap_or(0);
        }
        out
    }

    fn cg_affine_from_words(words: [u32; 6]) -> [f32; 6] {
        [
            Self::f32_from_bits(words[0]),
            Self::f32_from_bits(words[1]),
            Self::f32_from_bits(words[2]),
            Self::f32_from_bits(words[3]),
            Self::f32_from_bits(words[4]),
            Self::f32_from_bits(words[5]),
        ]
    }

    fn cg_affine_write_to_ptr(&mut self, ptr: u32, t: [f32; 6]) -> CoreResult<()> {
        for (idx, value) in t.into_iter().enumerate() {
            self.write_u32_le(ptr.wrapping_add((idx as u32).wrapping_mul(4)), value.to_bits())?;
        }
        Ok(())
    }

    fn cg_affine_identity() -> [f32; 6] {
        [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
    }

    fn cg_affine_make_rotation(angle: f32) -> [f32; 6] {
        let s = angle.sin();
        let c = angle.cos();
        [c, s, -s, c, 0.0, 0.0]
    }

    fn cg_affine_concat(t1: [f32; 6], t2: [f32; 6]) -> [f32; 6] {
        [
            t1[0] * t2[0] + t1[1] * t2[2],
            t1[0] * t2[1] + t1[1] * t2[3],
            t1[2] * t2[0] + t1[3] * t2[2],
            t1[2] * t2[1] + t1[3] * t2[3],
            t1[4] * t2[0] + t1[5] * t2[2] + t2[4],
            t1[4] * t2[1] + t1[5] * t2[3] + t2[5],
        ]
    }

    fn cg_affine_invert(t: [f32; 6]) -> [f32; 6] {
        let det = t[0] * t[3] - t[1] * t[2];
        [
            t[3] / det,
            -t[1] / det,
            -t[2] / det,
            t[0] / det,
            (t[2] * t[5] - t[3] * t[4]) / det,
            (t[1] * t[4] - t[0] * t[5]) / det,
        ]
    }

    fn cg_affine_is_identity(t: [f32; 6]) -> bool {
        t[0] == 1.0 && t[1] == 0.0 && t[2] == 0.0 && t[3] == 1.0 && t[4] == 0.0 && t[5] == 0.0
    }

    fn cg_rect_apply_affine(rect: [f32; 4], t: [f32; 6]) -> [f32; 4] {
        let x = rect[0];
        let y = rect[1];
        let w = rect[2];
        let h = rect[3];
        let points = [
            (x, y),
            (x + w, y),
            (x, y + h),
            (x + w, y + h),
        ];
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for (px, py) in points {
            let tx = t[0] * px + t[2] * py + t[4];
            let ty = t[1] * px + t[3] * py + t[5];
            min_x = min_x.min(tx);
            min_y = min_y.min(ty);
            max_x = max_x.max(tx);
            max_y = max_y.max(ty);
        }
        [min_x, min_y, max_x - min_x, max_y - min_y]
    }

    fn maybe_handle_graphics_hle_stub(
        &mut self,
        index: u64,
        current_pc: u32,
        label: &str,
    ) -> CoreResult<Option<StepControl>> {
        match label {
            "CGAffineTransformMakeRotation" => {
                let out_ptr = self.cpu.regs[0];
                let angle = Self::f32_from_bits(self.cpu.regs[1]);
                let result = Self::cg_affine_make_rotation(angle);
                self.cg_affine_write_to_ptr(out_ptr, result)?;
                self.cpu.regs[0] = out_ptr;
                self.return_from_hle_stub();
                return Ok(Some(StepControl::Continue));
            }
            "CGAffineTransformConcat" => {
                let out_ptr = self.cpu.regs[0];
                let t1 = Self::cg_affine_from_words(self.cg_affine_read_words_from_args(1, 0, 6));
                let t2 = Self::cg_affine_from_words([
                    self.peek_stack_u32(3).unwrap_or(0),
                    self.peek_stack_u32(4).unwrap_or(0),
                    self.peek_stack_u32(5).unwrap_or(0),
                    self.peek_stack_u32(6).unwrap_or(0),
                    self.peek_stack_u32(7).unwrap_or(0),
                    self.peek_stack_u32(8).unwrap_or(0),
                ]);
                let result = Self::cg_affine_concat(t1, t2);
                self.cg_affine_write_to_ptr(out_ptr, result)?;
                self.cpu.regs[0] = out_ptr;
                self.return_from_hle_stub();
                return Ok(Some(StepControl::Continue));
            }
            "CGAffineTransformInvert" => {
                let out_ptr = self.cpu.regs[0];
                let t = Self::cg_affine_from_words(self.cg_affine_read_words_from_args(1, 0, 6));
                let result = Self::cg_affine_invert(t);
                self.cg_affine_write_to_ptr(out_ptr, result)?;
                self.cpu.regs[0] = out_ptr;
                self.return_from_hle_stub();
                return Ok(Some(StepControl::Continue));
            }
            "CGAffineTransformIsIdentity" => {
                let t = Self::cg_affine_from_words(self.cg_affine_read_words_from_args(0, 0, 6));
                self.cpu.regs[0] = if Self::cg_affine_is_identity(t) { 1 } else { 0 };
                self.return_from_hle_stub();
                return Ok(Some(StepControl::Continue));
            }
            "CGAffineTransformRotate" => {
                let out_ptr = self.cpu.regs[0];
                let t = Self::cg_affine_from_words(self.cg_affine_read_words_from_args(1, 0, 6));
                let angle = Self::f32_from_bits(self.peek_stack_u32(3).unwrap_or(0));
                let result = Self::cg_affine_concat(t, Self::cg_affine_make_rotation(angle));
                self.cg_affine_write_to_ptr(out_ptr, result)?;
                self.cpu.regs[0] = out_ptr;
                self.return_from_hle_stub();
                return Ok(Some(StepControl::Continue));
            }
            "CGAffineTransformScale" => {
                let out_ptr = self.cpu.regs[0];
                let t = Self::cg_affine_from_words(self.cg_affine_read_words_from_args(1, 0, 6));
                let sx = Self::f32_from_bits(self.peek_stack_u32(3).unwrap_or(0));
                let sy = Self::f32_from_bits(self.peek_stack_u32(4).unwrap_or(0));
                let result = Self::cg_affine_concat(t, [sx, 0.0, 0.0, sy, 0.0, 0.0]);
                self.cg_affine_write_to_ptr(out_ptr, result)?;
                self.cpu.regs[0] = out_ptr;
                self.return_from_hle_stub();
                return Ok(Some(StepControl::Continue));
            }
            "CGAffineTransformTranslate" => {
                let out_ptr = self.cpu.regs[0];
                let t = Self::cg_affine_from_words(self.cg_affine_read_words_from_args(1, 0, 6));
                let tx = Self::f32_from_bits(self.peek_stack_u32(3).unwrap_or(0));
                let ty = Self::f32_from_bits(self.peek_stack_u32(4).unwrap_or(0));
                let result = Self::cg_affine_concat(t, [1.0, 0.0, 0.0, 1.0, tx, ty]);
                self.cg_affine_write_to_ptr(out_ptr, result)?;
                self.cpu.regs[0] = out_ptr;
                self.return_from_hle_stub();
                return Ok(Some(StepControl::Continue));
            }
            "CGRectApplyAffineTransform" => {
                let out_ptr = self.cpu.regs[0];
                let rect = [
                    Self::f32_from_bits(self.cpu.regs[1]),
                    Self::f32_from_bits(self.cpu.regs[2]),
                    Self::f32_from_bits(self.cpu.regs[3]),
                    Self::f32_from_bits(self.peek_stack_u32(0).unwrap_or(0)),
                ];
                let t_words = [
                    self.peek_stack_u32(1).unwrap_or(0),
                    self.peek_stack_u32(2).unwrap_or(0),
                    self.peek_stack_u32(3).unwrap_or(0),
                    self.peek_stack_u32(4).unwrap_or(0),
                    self.peek_stack_u32(5).unwrap_or(0),
                    self.peek_stack_u32(6).unwrap_or(0),
                ];
                let result = Self::cg_rect_apply_affine(rect, Self::cg_affine_from_words(t_words));
                for (idx, value) in result.into_iter().enumerate() {
                    self.write_u32_le(out_ptr.wrapping_add((idx as u32).wrapping_mul(4)), value.to_bits())?;
                }
                self.cpu.regs[0] = out_ptr;
                self.return_from_hle_stub();
                return Ok(Some(StepControl::Continue));
            }
            "glGenFramebuffersOES" | "glGenRenderbuffersOES" => {
                let count = self.cpu.regs[0];
                let out_ptr = self.cpu.regs[1];
                for i in 0..count {
                    let value = if label == "glGenFramebuffersOES" {
                        self.runtime.ui_graphics.gl_framebuffer
                    } else {
                        self.runtime.ui_graphics.gl_renderbuffer
                    };
                    let _ = self.write_u32_le(out_ptr.wrapping_add(i.wrapping_mul(4)), value);
                }
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                self.refresh_graphics_object_labels();
                let generated = if label == "glGenFramebuffersOES" {
                    self.describe_ptr(self.runtime.ui_graphics.gl_framebuffer)
                } else {
                    self.describe_ptr(self.runtime.ui_graphics.gl_renderbuffer)
                };
                let detail = format!("hle {}(count={}, out=0x{:08x}) -> {}", label, count, out_ptr, generated);
                let call_count = self.record_gl_call(&label, detail.clone());
                if Self::should_trace_hot_hle(call_count) {
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                }
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glBindFramebufferOES" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let raw_fb = self.cpu.regs[1];
                let bound_fb = self.resolve_guest_gl_name(
                    raw_fb,
                    self.runtime.ui_graphics.gl_framebuffer,
                    self.runtime.graphics.current_bound_framebuffer,
                );
                self.runtime.graphics.current_bound_framebuffer = bound_fb;
                self.runtime.ui_graphics.graphics_framebuffer_complete =
                    bound_fb == self.runtime.ui_graphics.gl_framebuffer && self.runtime.ui_graphics.graphics_surface_ready;
                self.refresh_graphics_object_labels();
                let detail = format!(
                    "hle glBindFramebufferOES(target=0x{:x}, fbRaw={}, fbResolved={} current={})",
                    self.cpu.regs[0],
                    self.describe_ptr(raw_fb),
                    self.describe_ptr(bound_fb),
                    self.describe_ptr(self.runtime.graphics.current_bound_framebuffer),
                );
                let call_count = self.record_gl_call(&label, detail.clone());
                if Self::should_trace_hot_hle(call_count) {
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                }
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glBindRenderbufferOES" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let raw_rb = self.cpu.regs[1];
                let bound_rb = self.resolve_guest_gl_name(
                    raw_rb,
                    self.runtime.ui_graphics.gl_renderbuffer,
                    self.runtime.graphics.current_bound_renderbuffer,
                );
                self.runtime.graphics.current_bound_renderbuffer = bound_rb;
                self.refresh_graphics_object_labels();
                let detail = format!(
                    "hle glBindRenderbufferOES(target=0x{:x}, rbRaw={}, rbResolved={} current={})",
                    self.cpu.regs[0],
                    self.describe_ptr(raw_rb),
                    self.describe_ptr(bound_rb),
                    self.describe_ptr(self.runtime.graphics.current_bound_renderbuffer),
                );
                let call_count = self.record_gl_call(&label, detail.clone());
                if Self::should_trace_hot_hle(call_count) {
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                }
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glRenderbufferStorageOES" | "glFramebufferRenderbufferOES" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                if label == "glRenderbufferStorageOES" {
                    let width = self.cpu.regs[2];
                    let height = self.cpu.regs[3];
                    if width != 0 {
                        self.runtime.ui_graphics.graphics_surface_width = width;
                    }
                    if height != 0 {
                        self.runtime.ui_graphics.graphics_surface_height = height;
                    }
                }
                self.runtime.ui_graphics.graphics_surface_ready = true;
                self.runtime.ui_graphics.graphics_framebuffer_complete = true;
                self.runtime.ui_graphics.graphics_layer_attached = true;
                self.runtime.ui_graphics.graphics_viewport_ready = true;
                self.runtime.ui_graphics.graphics_readback_ready = false;
                self.runtime.ui_graphics.graphics_presented = false;
                if self.runtime.ui_graphics.graphics_viewport_width == 0 {
                    self.runtime.ui_graphics.graphics_viewport_x = 0;
                    self.runtime.ui_graphics.graphics_viewport_y = 0;
                    self.runtime.ui_graphics.graphics_viewport_width = self.runtime.ui_graphics.graphics_surface_width;
                }
                if self.runtime.ui_graphics.graphics_viewport_height == 0 {
                    self.runtime.ui_graphics.graphics_viewport_height = self.runtime.ui_graphics.graphics_surface_height;
                }
                self.refresh_graphics_object_labels();
                let detail = if label == "glRenderbufferStorageOES" {
                    format!(
                        "hle glRenderbufferStorageOES(target=0x{:x}, format=0x{:x}, size={}x{}, rb={}) -> surfaceReady={} framebufferComplete={}",
                        self.cpu.regs[0],
                        self.cpu.regs[1],
                        self.runtime.ui_graphics.graphics_surface_width,
                        self.runtime.ui_graphics.graphics_surface_height,
                        self.describe_ptr(self.runtime.graphics.current_bound_renderbuffer),
                        Self::retained_flag(self.runtime.ui_graphics.graphics_surface_ready),
                        Self::retained_flag(self.runtime.ui_graphics.graphics_framebuffer_complete),
                    )
                } else {
                    format!(
                        "hle glFramebufferRenderbufferOES(target=0x{:x}, attachment=0x{:x}, rbTarget=0x{:x}, rb={}) -> surfaceReady={} framebufferComplete={}",
                        self.cpu.regs[0],
                        self.cpu.regs[1],
                        self.cpu.regs[2],
                        self.describe_ptr(self.resolve_guest_gl_name(self.cpu.regs[3], self.runtime.ui_graphics.gl_renderbuffer, self.runtime.graphics.current_bound_renderbuffer)),
                        Self::retained_flag(self.runtime.ui_graphics.graphics_surface_ready),
                        Self::retained_flag(self.runtime.ui_graphics.graphics_framebuffer_complete),
                    )
                };
                let call_count = self.record_gl_call(&label, detail.clone());
                if Self::should_trace_hot_hle(call_count) {
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                }
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glDeleteFramebuffersOES" | "glDeleteRenderbuffersOES" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let count = self.cpu.regs[0];
                let names_ptr = self.cpu.regs[1];
                let deleting_renderbuffer = label == "glDeleteRenderbuffersOES";
                let preferred = if deleting_renderbuffer {
                    self.runtime.ui_graphics.gl_renderbuffer
                } else {
                    self.runtime.ui_graphics.gl_framebuffer
                };
                let current = if deleting_renderbuffer {
                    self.runtime.graphics.current_bound_renderbuffer
                } else {
                    self.runtime.graphics.current_bound_framebuffer
                };
                let names = self.read_guest_gl_name_array(names_ptr, count, preferred, current);
                let deleted_current = names.iter().any(|name| *name != 0 && (*name == current || *name == preferred));
                if deleting_renderbuffer {
                    if deleted_current {
                        self.runtime.graphics.current_bound_renderbuffer = 0;
                    }
                    self.runtime.ui_graphics.graphics_surface_ready = false;
                    self.runtime.ui_graphics.graphics_readback_ready = false;
                    self.runtime.ui_graphics.graphics_presented = false;
                    self.runtime.ui_graphics.graphics_framebuffer_complete =
                        self.runtime.graphics.current_bound_framebuffer == self.runtime.ui_graphics.gl_framebuffer
                            && self.runtime.ui_graphics.graphics_surface_ready;
                } else if deleted_current {
                    self.runtime.graphics.current_bound_framebuffer = 0;
                    self.runtime.ui_graphics.graphics_framebuffer_complete = false;
                }
                self.refresh_graphics_object_labels();
                let detail = format!(
                    "hle {}(count={}, names_ptr=0x{:08x}, names={}) -> deletedCurrent={} surfaceReady={} framebufferComplete={}",
                    label,
                    count,
                    names_ptr,
                    self.describe_gl_name_list(&names),
                    if deleted_current { "YES" } else { "NO" },
                    Self::retained_flag(self.runtime.ui_graphics.graphics_surface_ready),
                    Self::retained_flag(self.runtime.ui_graphics.graphics_framebuffer_complete),
                );
                let call_count = self.record_gl_call(&label, detail.clone());
                if Self::should_trace_hot_hle(call_count) {
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                }
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glCheckFramebufferStatusOES" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                self.cpu.regs[0] = if self.runtime.ui_graphics.graphics_framebuffer_complete { 0x8CD5 } else { 0x8CD6 };
                let detail = format!("hle glCheckFramebufferStatusOES(target=0x{:x}) -> 0x{:04x}", self.cpu.regs[0], self.cpu.regs[0]);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glViewport" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                self.runtime.ui_graphics.graphics_viewport_ready = true;
                let raw_x = self.cpu.regs[0];
                let raw_y = self.cpu.regs[1];
                let raw_w = self.cpu.regs[2];
                let raw_h = self.cpu.regs[3];
                let fallback_w = if self.runtime.ui_graphics.graphics_surface_width != 0 {
                    self.runtime.ui_graphics.graphics_surface_width
                } else {
                    self.runtime.ui_graphics.graphics_viewport_width
                };
                let fallback_h = if self.runtime.ui_graphics.graphics_surface_height != 0 {
                    self.runtime.ui_graphics.graphics_surface_height
                } else {
                    self.runtime.ui_graphics.graphics_viewport_height
                };
                let width = if raw_w == 0 && fallback_w != 0 { fallback_w } else { raw_w };
                let height = if raw_h == 0 && fallback_h != 0 { fallback_h } else { raw_h };
                let normalized = (width != raw_w) || (height != raw_h);
                self.runtime.ui_graphics.graphics_viewport_x = raw_x;
                self.runtime.ui_graphics.graphics_viewport_y = raw_y;
                self.runtime.ui_graphics.graphics_viewport_width = width;
                self.runtime.ui_graphics.graphics_viewport_height = height;
                self.refresh_graphics_object_labels();
                let detail = if normalized {
                    format!("hle glViewport(x={}, y={}, w={}, h={}) -> normalized {}x{} using surface fallback", raw_x, raw_y, raw_w, raw_h, width, height)
                } else {
                    format!("hle glViewport(x={}, y={}, w={}, h={})", raw_x, raw_y, width, height)
                };
                self.push_graphics_event(format!("viewport <- ({},{} {}x{}){}", raw_x, raw_y, width, height, if normalized { " normalized-from-zero" } else { "" }));
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glClear" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                self.runtime.ui_graphics.graphics_clear_calls = self.runtime.ui_graphics.graphics_clear_calls.saturating_add(1);
                self.fill_framebuffer_rgba(self.runtime.graphics.current_clear_rgba);
                self.runtime.graphics.guest_framebuffer_dirty = true;
                self.runtime.ui_graphics.graphics_last_guest_draw_checksum = Self::checksum_bytes(&self.runtime.graphics.synthetic_framebuffer);
                let detail = format!("hle glClear(mask=0x{:x}) -> rgba({},{},{},{}) guestDirty=YES checksum=0x{:08x}", self.cpu.regs[0], self.runtime.graphics.current_clear_rgba[0], self.runtime.graphics.current_clear_rgba[1], self.runtime.graphics.current_clear_rgba[2], self.runtime.graphics.current_clear_rgba[3], self.runtime.ui_graphics.graphics_last_guest_draw_checksum);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glDrawArrays" | "glDrawElements" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                self.runtime.ui_graphics.graphics_draw_calls = self.runtime.ui_graphics.graphics_draw_calls.saturating_add(1);
                self.runtime.graphics.guest_draws_since_present = self.runtime.graphics.guest_draws_since_present.saturating_add(1);
                let mode = self.cpu.regs[0];
                let (first, count, index_type, indices_ptr) = if label == "glDrawArrays" {
                    (self.cpu.regs[1], self.cpu.regs[2], None, None)
                } else {
                    let ptr = self.read_u32_le(self.cpu.regs[13]).ok().or_else(|| Some(self.cpu.regs[3]));
                    (0, self.cpu.regs[1], Some(self.cpu.regs[2]), ptr)
                };
                let next_frame = self.runtime.ui_graphics.graphics_frame_index.saturating_add(1);
                let detail = if let Some((vertices, checksum)) = self.draw_guest_geometry(mode, first, count, index_type, indices_ptr) {
                    self.refresh_graphics_object_labels();
                    format!(
                        "hle {}(mode=0x{:x}:{}, first={}, count={}, indices={}) -> guest-draw vertices={} framePending=YES nextFrame={} checksum=0x{:08x}",
                        label,
                        mode,
                        Self::graphics_draw_mode_name(mode),
                        first,
                        count,
                        indices_ptr.map(|ptr| self.describe_ptr(ptr)).unwrap_or_else(|| "none".to_string()),
                        vertices,
                        next_frame,
                        checksum,
                    )
                } else {
                    self.rasterize_synthetic_frame(next_frame);
                    format!(
                        "hle {}(mode=0x{:x}:{}, first={}, count={}, indices={}) -> synthetic-fallback framePending=YES nextFrame={} fbBytes={}",
                        label,
                        mode,
                        Self::graphics_draw_mode_name(mode),
                        first,
                        count,
                        indices_ptr.map(|ptr| self.describe_ptr(ptr)).unwrap_or_else(|| "none".to_string()),
                        next_frame,
                        self.runtime.ui_graphics.graphics_framebuffer_bytes,
                    )
                };
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glReadPixels" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                self.runtime.ui_graphics.graphics_readback_calls = self.runtime.ui_graphics.graphics_readback_calls.saturating_add(1);
                self.runtime.ui_graphics.graphics_readback_ready = true;
                let pixels_ptr = self.read_u32_le(self.cpu.regs[13].wrapping_add(8)).or_else(|_| self.read_u32_le(self.cpu.regs[13])).unwrap_or(0);
                self.readback_pixels_to_guest(self.cpu.regs[0], self.cpu.regs[1], self.cpu.regs[2], self.cpu.regs[3], pixels_ptr);
                self.refresh_graphics_object_labels();
                let detail = format!("hle glReadPixels(x={}, y={}, w={}, h={}, pixels={}) -> readback {} byte(s) checksum=0x{:08x}", self.cpu.regs[0], self.cpu.regs[1], self.cpu.regs[2], self.cpu.regs[3], self.describe_ptr(pixels_ptr), self.runtime.ui_graphics.graphics_last_readback_bytes, self.runtime.ui_graphics.graphics_last_readback_checksum);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glGetError" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let err = self.runtime.ui_graphics.graphics_last_error;
                self.runtime.ui_graphics.graphics_last_error = 0;
                let detail = format!("hle glGetError() -> 0x{:x}", err);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = err;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glClearColor" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                self.runtime.graphics.current_clear_rgba = [
                    Self::gl_float_to_u8(self.cpu.regs[0]),
                    Self::gl_float_to_u8(self.cpu.regs[1]),
                    Self::gl_float_to_u8(self.cpu.regs[2]),
                    Self::gl_float_to_u8(self.cpu.regs[3]),
                ];
                let detail = format!("hle glClearColor(r={}, g={}, b={}, a={}) -> rgba({},{},{},{})", self.cpu.regs[0], self.cpu.regs[1], self.cpu.regs[2], self.cpu.regs[3], self.runtime.graphics.current_clear_rgba[0], self.runtime.graphics.current_clear_rgba[1], self.runtime.graphics.current_clear_rgba[2], self.runtime.graphics.current_clear_rgba[3]);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glEnableClientState" | "glDisableClientState" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let enable = label == "glEnableClientState";
                let array = self.cpu.regs[0];
                let detail = match array {
                    GL_VERTEX_ARRAY => {
                        self.runtime.graphics.gl_vertex_array.enabled = enable;
                        format!("hle {}(array=0x{:x}:{}) -> {}", label, array, Self::graphics_client_array_name(array).unwrap_or("unknown"), if enable { "ENABLED" } else { "DISABLED" })
                    }
                    GL_COLOR_ARRAY => {
                        self.runtime.graphics.gl_color_array.enabled = enable;
                        format!("hle {}(array=0x{:x}:{}) -> {}", label, array, Self::graphics_client_array_name(array).unwrap_or("unknown"), if enable { "ENABLED" } else { "DISABLED" })
                    }
                    GL_TEXTURE_COORD_ARRAY => {
                        self.runtime.graphics.gl_texcoord_array.enabled = enable;
                        format!("hle {}(array=0x{:x}:{}) -> {}", label, array, Self::graphics_client_array_name(array).unwrap_or("unknown"), if enable { "ENABLED" } else { "DISABLED" })
                    }
                    _ => format!("hle {}(array=0x{:x}) -> ignored", label, array),
                };
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glVertexPointer" | "glColorPointer" | "glTexCoordPointer" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let ptr = self.read_u32_le(self.cpu.regs[13]).ok().or_else(|| Some(self.cpu.regs[3])).unwrap_or(0);
                let target = if label == "glVertexPointer" {
                    &mut self.runtime.graphics.gl_vertex_array
                } else if label == "glColorPointer" {
                    &mut self.runtime.graphics.gl_color_array
                } else {
                    &mut self.runtime.graphics.gl_texcoord_array
                };
                target.size = self.cpu.regs[0];
                target.ty = self.cpu.regs[1];
                target.stride = self.cpu.regs[2];
                target.ptr = ptr;
                let configured = target.configured();
                let enabled = target.enabled;
                let size = target.size;
                let ty = target.ty;
                let stride = target.stride;
                let target_ptr = target.ptr;
                let detail = format!(
                    "hle {}(size={}, type=0x{:x}, stride={}, ptr={}) -> configured={} enabled={}",
                    label,
                    size,
                    ty,
                    stride,
                    self.describe_ptr(target_ptr),
                    if configured { "YES" } else { "NO" },
                    if enabled { "YES" } else { "NO" },
                );
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glScissor" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let raw_x = self.cpu.regs[0];
                let raw_y = self.cpu.regs[1];
                let raw_w = self.cpu.regs[2];
                let raw_h = self.cpu.regs[3];
                let fallback_w = if self.runtime.ui_graphics.graphics_viewport_width != 0 {
                    self.runtime.ui_graphics.graphics_viewport_width
                } else {
                    self.runtime.ui_graphics.graphics_surface_width
                };
                let fallback_h = if self.runtime.ui_graphics.graphics_viewport_height != 0 {
                    self.runtime.ui_graphics.graphics_viewport_height
                } else {
                    self.runtime.ui_graphics.graphics_surface_height
                };
                let width = if raw_w == 0 && fallback_w != 0 { fallback_w } else { raw_w };
                let height = if raw_h == 0 && fallback_h != 0 { fallback_h } else { raw_h };
                let normalized = (width != raw_w) || (height != raw_h);
                self.runtime.ui_graphics.graphics_scissor_x = raw_x;
                self.runtime.ui_graphics.graphics_scissor_y = raw_y;
                self.runtime.ui_graphics.graphics_scissor_width = width;
                self.runtime.ui_graphics.graphics_scissor_height = height;
                let detail = if normalized {
                    format!("hle glScissor(x={}, y={}, w={}, h={}) -> normalized {}x{} using viewport/surface fallback", raw_x, raw_y, raw_w, raw_h, width, height)
                } else {
                    format!("hle glScissor(x={}, y={}, w={}, h={})", raw_x, raw_y, width, height)
                };
                self.push_graphics_event(format!("scissor rect <- ({},{} {}x{}) enabled={}{}", raw_x, raw_y, width, height, if self.runtime.ui_graphics.graphics_scissor_enabled { "YES" } else { "NO" }, if normalized { " normalized-from-zero" } else { "" }));
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glEnable" | "glDisable" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                const GL_SCISSOR_TEST: u32 = 0x0C11;
                let enable = label == "glEnable";
                match self.cpu.regs[0] {
                    GL_SCISSOR_TEST => {
                        self.runtime.ui_graphics.graphics_scissor_enabled = enable;
                        self.push_graphics_event(format!("scissor test <- {}", if self.runtime.ui_graphics.graphics_scissor_enabled { "ENABLED" } else { "DISABLED" }));
                    }
                    GL_TEXTURE_2D => {
                        self.runtime.graphics.gl_texture_2d_enabled = enable;
                    }
                    GL_BLEND => {
                        self.runtime.graphics.gl_blend_enabled = enable;
                    }
                    _ => {}
                }
                let detail = format!("hle {}(cap=0x{:x}) -> ok", label, self.cpu.regs[0]);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glColor4f" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                self.runtime.graphics.gl_current_color = [
                    Self::gl_float_to_u8(self.cpu.regs[0]),
                    Self::gl_float_to_u8(self.cpu.regs[1]),
                    Self::gl_float_to_u8(self.cpu.regs[2]),
                    Self::gl_float_to_u8(self.cpu.regs[3]),
                ];
                let detail = format!(
                    "hle glColor4f(r=0x{:08x}, g=0x{:08x}, b=0x{:08x}, a=0x{:08x}) -> rgba({},{},{},{})",
                    self.cpu.regs[0],
                    self.cpu.regs[1],
                    self.cpu.regs[2],
                    self.cpu.regs[3],
                    self.runtime.graphics.gl_current_color[0],
                    self.runtime.graphics.gl_current_color[1],
                    self.runtime.graphics.gl_current_color[2],
                    self.runtime.graphics.gl_current_color[3],
                );
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glMatrixMode" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let raw_mode = self.cpu.regs[0];
                let detail = if let Some(mode) = GraphicsMatrixMode::from_gl(raw_mode) {
                    self.runtime.ui_graphics.graphics_matrices.current_mode = mode;
                    format!("hle glMatrixMode(mode=0x{:x}:{}) -> ok", raw_mode, mode.as_str())
                } else {
                    format!("hle glMatrixMode(mode=0x{:x}:unknown) -> ignored", raw_mode)
                };
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glLoadIdentity" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let mode = self.runtime.ui_graphics.graphics_matrices.current_mode;
                self.gl_set_current_matrix(mode, gl_identity_mat4());
                let detail = format!("hle glLoadIdentity(mode={}) -> ok", mode.as_str());
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glPushMatrix" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let mode = self.runtime.ui_graphics.graphics_matrices.current_mode;
                let top = self.gl_current_matrix(mode);
                self.gl_matrix_stack_mut(mode).push(top);
                let depth = self.gl_matrix_stack_ref(mode).len();
                let detail = format!("hle glPushMatrix(mode={}) -> depth={}", mode.as_str(), depth);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glPopMatrix" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let mode = self.runtime.ui_graphics.graphics_matrices.current_mode;
                let stack = self.gl_matrix_stack_mut(mode);
                let popped = if stack.len() > 1 { stack.pop().is_some() } else { false };
                let depth = stack.len();
                let detail = format!("hle glPopMatrix(mode={}) -> popped={} depth={}", mode.as_str(), if popped { "YES" } else { "NO" }, depth);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glTranslatef" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let mode = self.runtime.ui_graphics.graphics_matrices.current_mode;
                let tx = self.gl_read_call_arg_f32(0);
                let ty = self.gl_read_call_arg_f32(1);
                let tz = self.gl_read_call_arg_f32(2);
                let updated = Self::gl_mat4_mul(self.gl_current_matrix(mode), Self::gl_mat4_translate(tx, ty, tz));
                self.gl_set_current_matrix(mode, updated);
                let detail = format!("hle glTranslatef(mode={}, x={:.3}, y={:.3}, z={:.3}) -> ok", mode.as_str(), tx, ty, tz);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glRotatef" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let mode = self.runtime.ui_graphics.graphics_matrices.current_mode;
                let angle = self.gl_read_call_arg_f32(0);
                let x = self.gl_read_call_arg_f32(1);
                let y = self.gl_read_call_arg_f32(2);
                let z = self.gl_read_call_arg_f32(3);
                let updated = Self::gl_mat4_mul(self.gl_current_matrix(mode), Self::gl_mat4_rotate(angle, x, y, z));
                self.gl_set_current_matrix(mode, updated);
                let detail = format!("hle glRotatef(mode={}, angle={:.3}, axis=({:.3},{:.3},{:.3})) -> ok", mode.as_str(), angle, x, y, z);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glScalef" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let mode = self.runtime.ui_graphics.graphics_matrices.current_mode;
                let sx = self.gl_read_call_arg_f32(0);
                let sy = self.gl_read_call_arg_f32(1);
                let sz = self.gl_read_call_arg_f32(2);
                let updated = Self::gl_mat4_mul(self.gl_current_matrix(mode), Self::gl_mat4_scale(sx, sy, sz));
                self.gl_set_current_matrix(mode, updated);
                let detail = format!("hle glScalef(mode={}, x={:.3}, y={:.3}, z={:.3}) -> ok", mode.as_str(), sx, sy, sz);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glOrthof" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let mode = self.runtime.ui_graphics.graphics_matrices.current_mode;
                let left = self.gl_read_call_arg_f32(0);
                let right = self.gl_read_call_arg_f32(1);
                let bottom = self.gl_read_call_arg_f32(2);
                let top = self.gl_read_call_arg_f32(3);
                let near = self.gl_read_call_arg_f32(4);
                let far = self.gl_read_call_arg_f32(5);
                let detail = if let Some(ortho) = Self::gl_mat4_ortho(left, right, bottom, top, near, far) {
                    let updated = Self::gl_mat4_mul(self.gl_current_matrix(mode), ortho);
                    self.gl_set_current_matrix(mode, updated);
                    format!("hle glOrthof(mode={}, l={:.3}, r={:.3}, b={:.3}, t={:.3}, n={:.3}, f={:.3}) -> ok", mode.as_str(), left, right, bottom, top, near, far)
                } else {
                    format!("hle glOrthof(mode={}, l={:.3}, r={:.3}, b={:.3}, t={:.3}, n={:.3}, f={:.3}) -> degenerate", mode.as_str(), left, right, bottom, top, near, far)
                };
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glFrustumf" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let mode = self.runtime.ui_graphics.graphics_matrices.current_mode;
                let left = self.gl_read_call_arg_f32(0);
                let right = self.gl_read_call_arg_f32(1);
                let bottom = self.gl_read_call_arg_f32(2);
                let top = self.gl_read_call_arg_f32(3);
                let near = self.gl_read_call_arg_f32(4);
                let far = self.gl_read_call_arg_f32(5);
                let detail = if let Some(frustum) = Self::gl_mat4_frustum(left, right, bottom, top, near, far) {
                    let updated = Self::gl_mat4_mul(self.gl_current_matrix(mode), frustum);
                    self.gl_set_current_matrix(mode, updated);
                    format!("hle glFrustumf(mode={}, l={:.3}, r={:.3}, b={:.3}, t={:.3}, n={:.3}, f={:.3}) -> ok", mode.as_str(), left, right, bottom, top, near, far)
                } else {
                    format!("hle glFrustumf(mode={}, l={:.3}, r={:.3}, b={:.3}, t={:.3}, n={:.3}, f={:.3}) -> degenerate", mode.as_str(), left, right, bottom, top, near, far)
                };
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glMultMatrixf" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let mode = self.runtime.ui_graphics.graphics_matrices.current_mode;
                let ptr = self.cpu.regs[0];
                let detail = if let Some(rhs) = self.gl_read_guest_matrix_f32(ptr) {
                    let updated = Self::gl_mat4_mul(self.gl_current_matrix(mode), rhs);
                    self.gl_set_current_matrix(mode, updated);
                    format!("hle glMultMatrixf(mode={}, ptr={}) -> ok", mode.as_str(), self.describe_ptr(ptr))
                } else {
                    format!("hle glMultMatrixf(mode={}, ptr={}) -> unreadable", mode.as_str(), self.describe_ptr(ptr))
                };
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glBindTexture" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let target = self.cpu.regs[0];
                let name = self.cpu.regs[1];
                if target == GL_TEXTURE_2D {
                    self.runtime.graphics.current_bound_texture_name = name;
                    if name != 0 {
                        let entry = self.runtime.graphics.guest_gl_textures.entry(name).or_default();
                        entry.target = target;
                    }
                }
                let detail = format!("hle glBindTexture(target=0x{:x}, texture={}) -> current={}", target, self.describe_ptr(name), self.describe_ptr(self.runtime.graphics.current_bound_texture_name));
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glTexImage2D" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let target = self.cpu.regs[0];
                let level = self.cpu.regs[1];
                let internal_format = self.cpu.regs[2];
                let width = self.cpu.regs[3];
                let height = self.peek_stack_u32(0).unwrap_or(0);
                let border = self.peek_stack_u32(1).unwrap_or(0);
                let format = self.peek_stack_u32(2).unwrap_or(internal_format);
                let ty = self.peek_stack_u32(3).unwrap_or(GL_UNSIGNED_BYTE);
                let pixels_ptr = self.peek_stack_u32(4).unwrap_or(0);
                let decoded = self.decode_guest_texture_rgba(width, height, format, ty, pixels_ptr);
                let detail = if target == GL_TEXTURE_2D && level == 0 && border == 0 && self.runtime.graphics.current_bound_texture_name != 0 {
                    if let Some(pixels_rgba) = decoded {
                        let tex_name = self.runtime.graphics.current_bound_texture_name;
                        let bytes_len = {
                            let tex = self.runtime.graphics.guest_gl_textures.entry(tex_name).or_default();
                            tex.target = target;
                            tex.width = width;
                            tex.height = height;
                            tex.internal_format = internal_format;
                            tex.format = format;
                            tex.ty = ty;
                            tex.pixels_rgba = pixels_rgba;
                            tex.upload_count = tex.upload_count.saturating_add(1);
                            tex.pixels_rgba.len()
                        };
                        format!(
                            "hle glTexImage2D(target=0x{:x}, level={}, internal=0x{:x}, size={}x{}, border={}, format=0x{:x}, type=0x{:x}, pixels={}) -> uploaded texture={} bytes={}",
                            target,
                            level,
                            internal_format,
                            width,
                            height,
                            border,
                            format,
                            ty,
                            self.describe_ptr(pixels_ptr),
                            self.describe_ptr(tex_name),
                            bytes_len,
                        )
                    } else {
                        self.runtime.ui_graphics.graphics_last_error = 0x0500;
                        format!(
                            "hle glTexImage2D(target=0x{:x}, level={}, internal=0x{:x}, size={}x{}, border={}, format=0x{:x}, type=0x{:x}, pixels={}) -> unsupported-upload",
                            target, level, internal_format, width, height, border, format, ty, self.describe_ptr(pixels_ptr)
                        )
                    }
                } else {
                    format!(
                        "hle glTexImage2D(target=0x{:x}, level={}, internal=0x{:x}, size={}x{}, border={}, format=0x{:x}, type=0x{:x}, pixels={}) -> ignored",
                        target, level, internal_format, width, height, border, format, ty, self.describe_ptr(pixels_ptr)
                    )
                };
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glTexParameteri" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let target = self.cpu.regs[0];
                let pname = self.cpu.regs[1];
                let param = self.cpu.regs[2];
                if target == GL_TEXTURE_2D && self.runtime.graphics.current_bound_texture_name != 0 {
                    let tex = self
                        .runtime
                        .graphics
                        .guest_gl_textures
                        .entry(self.runtime.graphics.current_bound_texture_name)
                        .or_default();
                    match pname {
                        GL_TEXTURE_MIN_FILTER => tex.min_filter = param,
                        GL_TEXTURE_MAG_FILTER => tex.mag_filter = param,
                        GL_TEXTURE_WRAP_S => tex.wrap_s = param,
                        GL_TEXTURE_WRAP_T => tex.wrap_t = param,
                        _ => {}
                    }
                }
                let detail = format!("hle glTexParameteri(target=0x{:x}, pname=0x{:x}, param=0x{:x}) -> ok", target, pname, param);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glBlendFunc" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                self.runtime.graphics.gl_blend_src_factor = self.cpu.regs[0];
                self.runtime.graphics.gl_blend_dst_factor = self.cpu.regs[1];
                let detail = format!("hle glBlendFunc(sfactor=0x{:x}, dfactor=0x{:x}) -> ok", self.cpu.regs[0], self.cpu.regs[1]);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glColor4ub" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                self.runtime.graphics.gl_current_color = [
                    (self.cpu.regs[0] & 0xff) as u8,
                    (self.cpu.regs[1] & 0xff) as u8,
                    (self.cpu.regs[2] & 0xff) as u8,
                    (self.cpu.regs[3] & 0xff) as u8,
                ];
                let detail = format!("hle glColor4ub(r={}, g={}, b={}, a={}) -> rgba({},{},{},{})", self.cpu.regs[0] & 0xff, self.cpu.regs[1] & 0xff, self.cpu.regs[2] & 0xff, self.cpu.regs[3] & 0xff, self.runtime.graphics.gl_current_color[0], self.runtime.graphics.gl_current_color[1], self.runtime.graphics.gl_current_color[2], self.runtime.graphics.gl_current_color[3]);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glGenTextures" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let count = self.cpu.regs[0];
                let out_ptr = self.cpu.regs[1];
                let mut names = Vec::new();
                for i in 0..count {
                    let name = self.runtime.graphics.guest_gl_texture_name_cursor.max(1);
                    self.runtime.graphics.guest_gl_texture_name_cursor = name.saturating_add(1);
                    let _ = self.write_u32_le(out_ptr.wrapping_add(i.wrapping_mul(4)), name);
                    self.runtime.graphics.guest_gl_textures.entry(name).or_default();
                    names.push(name);
                }
                let detail = format!("hle glGenTextures(count={}, out={}) -> {}", count, self.describe_ptr(out_ptr), self.describe_gl_name_list(&names));
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glDeleteTextures" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let count = self.cpu.regs[0];
                let names_ptr = self.cpu.regs[1];
                let mut names = Vec::new();
                if names_ptr != 0 && self.find_region(names_ptr, count.saturating_mul(4)).is_some() {
                    for i in 0..count {
                        if let Ok(name) = self.read_u32_le(names_ptr.wrapping_add(i.wrapping_mul(4))) {
                            names.push(name);
                        }
                    }
                }
                for name in &names {
                    self.runtime.graphics.guest_gl_textures.remove(name);
                    if self.runtime.graphics.current_bound_texture_name == *name {
                        self.runtime.graphics.current_bound_texture_name = 0;
                    }
                }
                let detail = format!("hle glDeleteTextures(count={}, names={}) -> ok", count, self.describe_gl_name_list(&names));
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glTexEnvi" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let target = self.cpu.regs[0];
                let pname = self.cpu.regs[1];
                let param = self.cpu.regs[2];
                if target == GL_TEXTURE_ENV && pname == GL_TEXTURE_ENV_MODE {
                    self.runtime.graphics.gl_tex_env_mode = param;
                }
                let detail = format!("hle glTexEnvi(target=0x{:x}, pname=0x{:x}, param=0x{:x}) -> ok", target, pname, param);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glGetIntegerv" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let pname = self.cpu.regs[0];
                let out_ptr = self.cpu.regs[1];
                match pname {
                    0x0BA2 => {
                        let _ = self.write_u32_le(out_ptr, self.runtime.ui_graphics.graphics_viewport_x);
                        let _ = self.write_u32_le(out_ptr.wrapping_add(4), self.runtime.ui_graphics.graphics_viewport_y);
                        let _ = self.write_u32_le(out_ptr.wrapping_add(8), self.runtime.ui_graphics.graphics_viewport_width);
                        let _ = self.write_u32_le(out_ptr.wrapping_add(12), self.runtime.ui_graphics.graphics_viewport_height);
                    }
                    0x8069 => {
                        let _ = self.write_u32_le(out_ptr, self.runtime.graphics.current_bound_texture_name);
                    }
                    0x0BA0 => {
                        let mode = match self.runtime.ui_graphics.graphics_matrices.current_mode {
                            GraphicsMatrixMode::ModelView => GL_MODELVIEW,
                            GraphicsMatrixMode::Projection => GL_PROJECTION,
                            GraphicsMatrixMode::Texture => GL_TEXTURE,
                        };
                        let _ = self.write_u32_le(out_ptr, mode);
                    }
                    0x0D33 => {
                        let _ = self.write_u32_le(out_ptr, 1024);
                    }
                    _ => {
                        let _ = self.write_u32_le(out_ptr, 0);
                    }
                }
                let detail = format!("hle glGetIntegerv(pname=0x{:x}, out={}) -> ok", pname, self.describe_ptr(out_ptr));
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "glHint" | "glBufferData" | "glBufferSubData" | "glBindBuffer" | "glGenBuffers" | "glDeleteBuffers" | "glCompressedTexImage2D" | "glGenerateMipmapOES" | "glFramebufferTexture2DOES" | "glColorMask" | "glDepthFunc" | "glClearDepthf" | "glLineWidth" | "glPointSizePointerOES" => {
                self.runtime.ui_graphics.graphics_gl_calls = self.runtime.ui_graphics.graphics_gl_calls.saturating_add(1);
                let detail = format!("hle {}(...) -> ok", label);
                self.record_gl_call(&label, detail.clone());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "UIGraphicsBeginImageContext" => {
                let (w, h) = self.read_cg_size_from_regs().unwrap_or((self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1)));
                let ctx = self.begin_uigraphics_context(w, h);
                let detail = format!("hle UIGraphicsBeginImageContext(size={}x{}) -> {} stackDepth={}", w, h, self.describe_ptr(ctx), self.runtime.graphics.uigraphics_stack.len());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "UIGraphicsGetCurrentContext" => {
                let ctx = self.runtime.graphics.current_uigraphics_context;
                let detail = format!("hle UIGraphicsGetCurrentContext() -> {}", self.describe_ptr(ctx));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = ctx;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "UIGraphicsGetImageFromCurrentImageContext" => {
                let image = self.create_image_from_context(self.runtime.graphics.current_uigraphics_context).unwrap_or(0);
                let detail = format!("hle UIGraphicsGetImageFromCurrentImageContext() -> {}", self.describe_ptr(image));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = image;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "UIGraphicsEndImageContext" => {
                let popped = self.pop_uigraphics_context();
                let detail = format!("hle UIGraphicsEndImageContext() -> popped {} current={} stackDepth={}", self.describe_ptr(popped), self.describe_ptr(self.runtime.graphics.current_uigraphics_context), self.runtime.graphics.uigraphics_stack.len());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "UIGraphicsPushContext" => {
                self.push_uigraphics_context(self.cpu.regs[0]);
                let detail = format!("hle UIGraphicsPushContext({}) current={} stackDepth={}", self.describe_ptr(self.cpu.regs[0]), self.describe_ptr(self.runtime.graphics.current_uigraphics_context), self.runtime.graphics.uigraphics_stack.len());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "UIGraphicsPopContext" => {
                let popped = self.pop_uigraphics_context();
                let detail = format!("hle UIGraphicsPopContext() -> popped {} current={} stackDepth={}", self.describe_ptr(popped), self.describe_ptr(self.runtime.graphics.current_uigraphics_context), self.runtime.graphics.uigraphics_stack.len());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "UIImagePNGRepresentation" | "UIImageJPEGRepresentation" => {
                let image = self.cpu.regs[0];
                let encoded = self.encode_synthetic_image_png(image).unwrap_or_default();
                let data_obj = if encoded.is_empty() { 0 } else {
                    let tag = if label == "UIImagePNGRepresentation" { "NSData.synthetic.png" } else { "NSData.synthetic.jpeg" };
                    let obj = self.alloc_synthetic_ui_object(format!("{}#{}", tag, self.runtime.heap.synthetic_blob_backing.len()));
                    let _ = self.ensure_blob_backing(obj, tag.to_string(), &encoded);
                    obj
                };
                let detail = format!("hle {}(image={}) -> {} bytes={} ", label, self.describe_ptr(image), self.describe_ptr(data_obj), encoded.len());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, detail.trim_end()));
                self.cpu.regs[0] = data_obj;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CGContextSetFillColor" => {
                let ctx = self.cpu.regs[0];
                let rgba = self.read_fill_components_rgba(self.cpu.regs[1]).unwrap_or([255, 255, 255, 255]);
                let ok = self.set_bitmap_context_fill_rgba(ctx, rgba);
                let detail = format!("hle CGContextSetFillColor(ctx={}, comps={}) -> rgba({},{},{},{}) applied={}", self.describe_ptr(ctx), self.describe_ptr(self.cpu.regs[1]), rgba[0], rgba[1], rgba[2], rgba[3], if ok { "YES" } else { "NO" });
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CGContextSetGrayFillColor" => {
                let ctx = self.cpu.regs[0];
                let gray = Self::gl_float_to_u8(self.cpu.regs[1]);
                let alpha = Self::gl_float_to_u8(self.cpu.regs[2]);
                let rgba = [gray, gray, gray, alpha];
                let ok = self.set_bitmap_context_fill_rgba(ctx, rgba);
                let detail = format!("hle CGContextSetGrayFillColor(ctx={}) -> rgba({},{},{},{}) applied={}", self.describe_ptr(ctx), rgba[0], rgba[1], rgba[2], rgba[3], if ok { "YES" } else { "NO" });
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CGContextFillRect" => {
                let ctx = self.cpu.regs[0];
                let rect = self.read_cg_rect_after_ctx().unwrap_or((0, 0, self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1)));
                let rgba = self.runtime.graphics.synthetic_bitmap_contexts.get(&ctx).map(|v| v.fill_rgba).unwrap_or([255, 255, 255, 255]);
                let applied = if self.fill_bitmap_context_rect(ctx, rect) {
                    false
                } else {
                    self.ensure_framebuffer_backing();
                    Self::ui_fill_rect_rgba(&mut self.runtime.graphics.synthetic_framebuffer, self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1), rect.0, rect.1, rect.2, rect.3, rgba);
                    self.runtime.ui_graphics.graphics_uikit_draw_ops = self.runtime.ui_graphics.graphics_uikit_draw_ops.saturating_add(1);
                    self.runtime.graphics.uikit_framebuffer_dirty = true;
                    self.runtime.ui_graphics.graphics_last_ui_source = Some("CGContextFillRect(framebuffer)".to_string());
                    true
                };
                let detail = format!("hle CGContextFillRect(ctx={}, rect=({},{} {}x{})) -> target={} rgba({},{},{},{})", self.describe_ptr(ctx), rect.0, rect.1, rect.2, rect.3, if applied { "framebuffer" } else { "bitmap" }, rgba[0], rgba[1], rgba[2], rgba[3]);
                self.push_graphics_event(format!("CGContextFillRect rect=({},{} {}x{}) target={}", rect.0, rect.1, rect.2, rect.3, if applied { "framebuffer" } else { "bitmap" }));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CGContextClearRect" => {
                let ctx = self.cpu.regs[0];
                let rect = self.read_cg_rect_after_ctx().unwrap_or((0, 0, self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1)));
                let cleared = if self.clear_bitmap_context_rect(ctx, rect) {
                    false
                } else {
                    self.ensure_framebuffer_backing();
                    Self::ui_clear_rect_rgba(&mut self.runtime.graphics.synthetic_framebuffer, self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1), rect.0, rect.1, rect.2, rect.3);
                    self.runtime.ui_graphics.graphics_uikit_draw_ops = self.runtime.ui_graphics.graphics_uikit_draw_ops.saturating_add(1);
                    self.runtime.graphics.uikit_framebuffer_dirty = true;
                    self.runtime.ui_graphics.graphics_last_ui_source = Some("CGContextClearRect(framebuffer)".to_string());
                    true
                };
                let detail = format!("hle CGContextClearRect(ctx={}, rect=({},{} {}x{})) -> target={}", self.describe_ptr(ctx), rect.0, rect.1, rect.2, rect.3, if cleared { "framebuffer" } else { "bitmap" });
                self.push_graphics_event(format!("CGContextClearRect rect=({},{} {}x{}) target={}", rect.0, rect.1, rect.2, rect.3, if cleared { "framebuffer" } else { "bitmap" }));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CGContextDrawImage" => {
                let ctx = self.cpu.regs[0];
                let rect = self.read_cg_rect_after_ctx().unwrap_or((0, 0, self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1)));
                let image = self.read_u32_le(self.cpu.regs[13]).ok().unwrap_or(self.runtime.graphics.last_uikit_image_object);
                let drew = if self.composite_image_into_context(ctx, image, rect) {
                    false
                } else if let Some(image_obj) = self.runtime.graphics.synthetic_images.get(&image).cloned() {
                    self.ensure_framebuffer_backing();
                    Self::composite_rgba_scaled_into(&mut self.runtime.graphics.synthetic_framebuffer, self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1), &image_obj.rgba, image_obj.width, image_obj.height, rect.0, rect.1, rect.2, rect.3);
                    self.runtime.ui_graphics.graphics_uikit_draw_ops = self.runtime.ui_graphics.graphics_uikit_draw_ops.saturating_add(1);
                    self.runtime.graphics.uikit_framebuffer_dirty = true;
                    self.runtime.ui_graphics.graphics_last_ui_source = Some("CGContextDrawImage(framebuffer)".to_string());
                    true
                } else {
                    false
                };
                let detail = format!("hle CGContextDrawImage(ctx={}, rect=({},{} {}x{}), image={}) -> target={}", self.describe_ptr(ctx), rect.0, rect.1, rect.2, rect.3, self.describe_ptr(image), if drew { "framebuffer" } else { "bitmap" });
                self.push_graphics_event(format!("CGContextDrawImage rect=({},{} {}x{}) image={} target={}", rect.0, rect.1, rect.2, rect.3, self.describe_ptr(image), if drew { "framebuffer" } else { "bitmap" }));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CGContextRelease" | "CGImageRelease" => {
                let detail = format!("hle {}({}) -> ok", label, self.describe_ptr(self.cpu.regs[0]));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            _ => return Ok(None),
        }
    }

    fn maybe_dispatch_graphics_objc_msgsend(
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
        let mut note: Option<String> = None;
        let result = match selector {
            "layer" => {
                let default_frame = self.ui_surface_rect_bits();
                let default_bounds = Self::ui_rect_size_bits(default_frame);
                self.ui_attach_layer_to_view(receiver, self.runtime.ui_graphics.eagl_layer);
                self.runtime.ui_objects
                    .view_frames_bits
                    .entry(receiver)
                    .or_insert(default_frame);
                self.runtime.ui_objects
                    .view_bounds_bits
                    .entry(receiver)
                    .or_insert(default_bounds);
                self.runtime.ui_graphics.eagl_layer
            },
            "currentContext" => {
                if self.runtime.ui_graphics.graphics_context_current {
                    self.runtime.ui_graphics.eagl_context
                } else {
                    0
                }
            }
            "context" => self.runtime.ui_graphics.eagl_context,
            "initWithAPI:" => {
                self.bootstrap_synthetic_graphics();
                note = Some(format!("EAGLContext api <- {}", arg2));
                self.runtime.ui_graphics.eagl_context
            }
            "setCurrentContext:" => {
                self.bootstrap_synthetic_graphics();
                self.runtime.ui_graphics.graphics_context_current = arg2 != 0;
                self.refresh_graphics_object_labels();
                note = Some(format!("currentContext <- {}", self.describe_ptr(arg2)));
                1
            }
            "renderbufferStorage:fromDrawable:" => {
                self.bootstrap_synthetic_graphics();
                self.runtime.ui_graphics.graphics_context_current = true;
                self.runtime.ui_graphics.graphics_layer_attached = arg3 != 0;
                self.runtime.ui_graphics.graphics_surface_ready = true;
                self.runtime.ui_graphics.graphics_framebuffer_complete = true;
                self.runtime.ui_graphics.graphics_viewport_ready = true;
                if arg3 != 0 {
                    if let Some(update) = self.ui_refresh_surface_from_drawable(arg3, "renderbufferStorage") {
                        note = Some(update);
                    }
                }
                if self.runtime.ui_graphics.graphics_viewport_width == 0 {
                    self.runtime.ui_graphics.graphics_viewport_x = 0;
                    self.runtime.ui_graphics.graphics_viewport_y = 0;
                    self.runtime.ui_graphics.graphics_viewport_width =
                        self.runtime.ui_graphics.graphics_surface_width;
                }
                if self.runtime.ui_graphics.graphics_viewport_height == 0 {
                    self.runtime.ui_graphics.graphics_viewport_height =
                        self.runtime.ui_graphics.graphics_surface_height;
                }
                self.refresh_graphics_object_labels();
                let base = format!(
                    "renderbufferStorage ctx={} drawable={} size={}x{}",
                    self.describe_ptr(receiver),
                    self.describe_ptr(arg3),
                    self.runtime.ui_graphics.graphics_surface_width,
                    self.runtime.ui_graphics.graphics_surface_height
                );
                note = Some(match note.take() {
                    Some(extra) => format!("{} {}", base, extra),
                    None => base,
                });
                1
            }
            "presentRenderbuffer:" => {
                self.bootstrap_synthetic_graphics();
                self.runtime.ui_graphics.graphics_context_current = true;
                self.runtime.ui_graphics.graphics_surface_ready = true;
                self.runtime.ui_graphics.graphics_framebuffer_complete = true;
                let dump_path = self.render_presented_frame("objc.presentRenderbuffer", 0);
                self.push_graphics_event(format!(
                    "presentRenderbuffer rb={} frame#{} source={} readback={} viewport=({},{} {}x{})",
                    self.describe_ptr(arg2),
                    self.runtime.ui_graphics.graphics_frame_index,
                    self.runtime.ui_graphics.graphics_last_present_source.clone().unwrap_or_default(),
                    self.runtime.ui_graphics.graphics_last_readback_origin.clone().unwrap_or_default(),
                    self.runtime.ui_graphics.graphics_viewport_x,
                    self.runtime.ui_graphics.graphics_viewport_y,
                    self.runtime.ui_graphics.graphics_viewport_width,
                    self.runtime.ui_graphics.graphics_viewport_height
                ));
                note = Some(format!(
                    "presentRenderbuffer rb={} frame#{}{}",
                    self.describe_ptr(arg2),
                    self.runtime.ui_graphics.graphics_frame_index,
                    dump_path
                        .as_ref()
                        .map(|p| format!(" dump={}", p))
                        .unwrap_or_default()
                ));
                1
            }
            "setDrawableProperties:" => {
                self.bootstrap_synthetic_graphics();
                self.runtime.ui_graphics.graphics_layer_attached = true;
                self.refresh_graphics_object_labels();
                note = Some(format!("drawableProperties <- {}", self.describe_ptr(arg2)));
                receiver
            }
            "setOpaque:" => {
                self.bootstrap_synthetic_graphics();
                note = Some(format!(
                    "layer opaque <- {}",
                    if arg2 != 0 { "YES" } else { "NO" }
                ));
                receiver
            }
            "setNeedsDisplay" | "layoutIfNeeded" | "layoutSubviews" => {
                let geometry_note = if selector == "layoutSubviews" {
                    self.ui_refresh_surface_from_drawable(receiver, "layoutSubviews")
                } else {
                    None
                };
                let composited = self.composite_current_uikit_surface_to_framebuffer(selector);
                note = Some(match geometry_note {
                    Some(extra) => format!(
                        "ui refresh on {} composited={} {}",
                        self.describe_ptr(receiver),
                        if composited { "YES" } else { "NO" },
                        extra,
                    ),
                    None => format!(
                        "ui refresh on {} composited={}",
                        self.describe_ptr(receiver),
                        if composited { "YES" } else { "NO" }
                    ),
                });
                receiver
            }
            "display" | "displayIfNeeded" => {
                let composited = self.composite_current_uikit_surface_to_framebuffer(selector);
                note = Some(format!(
                    "ui display on {} composited={}",
                    self.describe_ptr(receiver),
                    if composited { "YES" } else { "NO" }
                ));
                receiver
            }
            "drawFrame:" => {
                self.simulate_graphics_tick();
                note = Some(format!(
                    "drawFrame on {} frame#{}",
                    self.describe_ptr(receiver),
                    self.runtime.ui_graphics.graphics_frame_index
                ));
                0
            }
            _ => return Ok(None),
        };

        self.finish_objc_msgsend_hle_dispatch(
            index,
            current_pc,
            "objc_msgSend",
            receiver_desc,
            selector,
            arg2_desc,
            arg3_desc,
            result,
            note,
        )
    }
}
