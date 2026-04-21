impl MemoryArm32Backend {
// ARM32 execution core, tracing, and known runtime escape hatches.

    fn live_host_runloop_target_duration(&self) -> std::time::Duration {
        let secs = if self.runtime.ui_cocos.animation_interval_bits != 0 {
            Self::f32_from_bits(self.runtime.ui_cocos.animation_interval_bits)
        } else {
            1.0f32 / 60.0f32
        };
        let secs = if secs.is_finite() && secs > 0.0 {
            secs.clamp(1.0f32 / 240.0f32, 0.1f32)
        } else {
            1.0f32 / 60.0f32
        };
        std::time::Duration::from_secs_f64(secs as f64)
    }

    fn sleep_live_host_runloop_remainder(&self, tick_started: std::time::Instant) {
        let target = self.live_host_runloop_target_duration();
        let elapsed = tick_started.elapsed();
        if elapsed < target {
            std::thread::sleep(target - elapsed);
        }
    }

    fn host_unix_time_parts() -> (u32, u32) {
        use std::time::{SystemTime, UNIX_EPOCH};

        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => {
                let secs = duration.as_secs().min(u32::MAX as u64) as u32;
                let micros = duration.subsec_micros();
                (secs, micros)
            }
            Err(_) => (0, 0),
        }
    }

    fn host_cf_absolute_time_bits() -> u64 {
        const CF_ABSOLUTE_TIME_UNIX_EPOCH_DELTA_SECS: f64 = 978_307_200.0;

        let (secs, micros) = Self::host_unix_time_parts();
        let unix_secs = secs as f64 + (micros as f64 / 1_000_000.0);
        (unix_secs - CF_ABSOLUTE_TIME_UNIX_EPOCH_DELTA_SECS).to_bits()
    }


    fn sandbox_root_path(&self) -> Option<std::path::PathBuf> {
        let bundle_root = self.runtime.fs.bundle_root.clone()?;
        let install_root = bundle_root
            .parent()
            .filter(|p| p.file_name().and_then(|v| v.to_str()) == Some("Payload"))
            .and_then(|payload| payload.parent())
            .filter(|p| p.file_name().and_then(|v| v.to_str()) == Some("extracted"))
            .and_then(|extracted| extracted.parent())
            .filter(|p| p.file_name().and_then(|v| v.to_str()) == Some("build"))
            .and_then(|build| build.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| {
                bundle_root
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or(bundle_root.clone())
            });
        Some(install_root.join("sandbox"))
    }

    fn sandbox_directory_path(&self, directory: u32) -> Option<std::path::PathBuf> {
        let root = self.sandbox_root_path()?;
        let path = match directory {
            5 => root.join("Library"),
            9 => root.join("Documents"),
            13 => root.join("Library").join("Caches"),
            14 => root.join("Library").join("Application Support"),
            15 => root.join("Downloads"),
            17 => root.join("Media").join("Movies"),
            18 => root.join("Media").join("Music"),
            19 => root.join("Media").join("Pictures"),
            12 => root.join("Desktop"),
            21 => root.join("Public"),
            101 => root.join(".Trash"),
            _ => root.join("Documents"),
        };
        Some(path)
    }

    fn sandbox_home_path(&self) -> Option<std::path::PathBuf> {
        self.sandbox_root_path()
    }

    fn sandbox_tmp_path(&self) -> Option<std::path::PathBuf> {
        self.sandbox_root_path().map(|root| root.join("tmp"))
    }

    fn ensure_host_directory_exists(path: &std::path::Path) {
        let _ = std::fs::create_dir_all(path);
    }

    fn materialize_host_path_object(&mut self, label: &str, path: &std::path::Path) -> u32 {
        self.materialize_host_string_object(label, &path.display().to_string())
    }

    fn hle_search_path_array(&mut self, directory: u32, domain_mask: u32, _expand_tilde: u32) -> u32 {
        if domain_mask != 1 && domain_mask != 0xffff {
            return self.alloc_synthetic_array(format!("NSArray.searchPath.empty#{}", self.runtime.graphics.synthetic_arrays.len()));
        }
        let Some(path) = self.sandbox_directory_path(directory) else {
            return self.alloc_synthetic_array(format!("NSArray.searchPath.empty#{}", self.runtime.graphics.synthetic_arrays.len()));
        };
        Self::ensure_host_directory_exists(&path);
        let path_obj = self.materialize_host_path_object("NSString.searchPath", &path);
        let array = self.alloc_synthetic_array(format!("NSArray.searchPath#{}", self.runtime.graphics.synthetic_arrays.len()));
        let _ = self.synthetic_array_push(array, path_obj);
        array
    }

    fn host_path_from_string_value(&self, value: u32) -> Option<std::path::PathBuf> {
        let text = self.guest_string_value(value)?;
        if text.is_empty() {
            return None;
        }
        Some(std::path::PathBuf::from(text))
    }

    fn make_path_string_object(&mut self, label: &str, text: String) -> u32 {
        self.materialize_host_string_object(label, &text)
    }

    fn ensure_guest_wallclock_seeded(&mut self) {
        if self.runtime.ui_runtime.guest_time_seeded {
            return;
        }
        let (secs, micros) = Self::host_unix_time_parts();
        self.runtime.ui_runtime.guest_unix_micros = (secs as u64)
            .saturating_mul(1_000_000)
            .saturating_add(micros as u64);
        self.runtime.ui_runtime.guest_time_seeded = true;
    }

    fn guest_unix_time_parts(&mut self) -> (u32, u32) {
        self.ensure_guest_wallclock_seeded();
        let micros_total = self.runtime.ui_runtime.guest_unix_micros;
        let secs = (micros_total / 1_000_000).min(u32::MAX as u64) as u32;
        let micros = (micros_total % 1_000_000) as u32;
        (secs, micros)
    }

    fn advance_guest_wallclock_micros(&mut self, delta_micros: u64) {
        self.ensure_guest_wallclock_seeded();
        self.runtime.ui_runtime.guest_unix_micros = self
            .runtime
            .ui_runtime
            .guest_unix_micros
            .saturating_add(delta_micros);
    }

    fn advance_guest_wallclock_for_runloop_tick(&mut self) {
        let secs = if self.runtime.ui_cocos.animation_interval_bits != 0 {
            Self::f32_from_bits(self.runtime.ui_cocos.animation_interval_bits) as f64
        } else {
            1.0f64 / 60.0f64
        };
        let secs = if secs.is_finite() && secs > 0.0 { secs } else { 1.0f64 / 60.0f64 };
        let delta_micros = (secs * 1_000_000.0).round().max(1.0) as u64;
        self.advance_guest_wallclock_micros(delta_micros);
    }

    fn guest_cf_absolute_time_bits(&mut self) -> u64 {
        const CF_ABSOLUTE_TIME_UNIX_EPOCH_DELTA_SECS: f64 = 978_307_200.0;

        let (secs, micros) = self.guest_unix_time_parts();
        let unix_secs = secs as f64 + (micros as f64 / 1_000_000.0);
        (unix_secs - CF_ABSOLUTE_TIME_UNIX_EPOCH_DELTA_SECS).to_bits()
    }

    fn ensure_hle_prng_seeded(&mut self) -> u32 {
        if self.runtime.ui_runtime.prng_seeded {
            return self.runtime.ui_runtime.prng_last_seed;
        }
        let (secs, micros) = self.guest_unix_time_parts();
        let mut seed = secs ^ micros.rotate_left(11) ^ 0xA341_316C;
        if seed == 0 {
            seed = 1;
        }
        self.runtime.ui_runtime.prng_state = seed;
        self.runtime.ui_runtime.prng_seeded = true;
        self.runtime.ui_runtime.prng_last_seed = seed;
        seed
    }

    fn seed_hle_prng(&mut self, seed: u32) {
        let normalized = if seed == 0 { 1 } else { seed };
        self.runtime.ui_runtime.prng_state = normalized;
        self.runtime.ui_runtime.prng_seeded = true;
        self.runtime.ui_runtime.prng_last_seed = seed;
        self.runtime.ui_runtime.prng_draw_count = 0;
    }

    fn next_hle_prng31(&mut self) -> u32 {
        self.ensure_hle_prng_seeded();
        let next = self
            .runtime
            .ui_runtime
            .prng_state
            .wrapping_mul(1103515245)
            .wrapping_add(12345);
        self.runtime.ui_runtime.prng_state = next;
        self.runtime.ui_runtime.prng_draw_count = self.runtime.ui_runtime.prng_draw_count.saturating_add(1);
        next & 0x7fff_ffff
    }

    fn should_trace_hot_hle(counter: u32) -> bool {
        counter <= 4 || counter.is_power_of_two()
    }

    fn return_from_hle_stub(&mut self) {
        self.cpu.regs[15] = self.cpu.regs[14] & !1;
        self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
    }

    fn hle_sjlj_register(&mut self, index: u64, current_pc: u32, label: &str) -> CoreResult<StepControl> {
        let fc_ptr = self.cpu.regs[0];
        let prev = self.runtime.ui_runtime.sjlj_context_head;
        let mut wrote_prev = false;
        if fc_ptr != 0 && self.find_region(fc_ptr, 4).is_some() {
            self.write_u32_le(fc_ptr, prev)?;
            wrote_prev = true;
        }
        self.runtime.ui_runtime.sjlj_context_head = fc_ptr;
        self.runtime.ui_runtime.sjlj_register_count = self.runtime.ui_runtime.sjlj_register_count.saturating_add(1);
        let count = self.runtime.ui_runtime.sjlj_register_count;
        if Self::should_trace_hot_hle(count) {
            let detail = format!(
                "hle Unwind_SjLj_Register(fc={}, prev={}, head={}{}; count={})",
                self.describe_ptr(fc_ptr),
                self.describe_ptr(prev),
                self.describe_ptr(self.runtime.ui_runtime.sjlj_context_head),
                if wrote_prev {
                    format!(", wrote fc->prev @0x{fc_ptr:08x}")
                } else if fc_ptr != 0 {
                    ", guest fc unavailable; shadow chain only".to_string()
                } else {
                    String::new()
                },
                count,
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }
        self.cpu.regs[0] = 0;
        self.cpu.regs[0] = 0;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }

    fn hle_sjlj_unregister(&mut self, index: u64, current_pc: u32, label: &str) -> CoreResult<StepControl> {
        let fc_ptr = self.cpu.regs[0];
        let prev = if fc_ptr != 0 {
            self.read_u32_le(fc_ptr).unwrap_or(0)
        } else {
            0
        };
        self.runtime.ui_runtime.sjlj_context_head = prev;
        self.runtime.ui_runtime.sjlj_unregister_count = self.runtime.ui_runtime.sjlj_unregister_count.saturating_add(1);
        let count = self.runtime.ui_runtime.sjlj_unregister_count;
        if Self::should_trace_hot_hle(count) {
            let detail = format!(
                "hle Unwind_SjLj_Unregister(fc={}, next={}, head={}; count={})",
                self.describe_ptr(fc_ptr),
                self.describe_ptr(prev),
                self.describe_ptr(self.runtime.ui_runtime.sjlj_context_head),
                count,
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }
        self.cpu.regs[0] = 0;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }

    fn hle_sjlj_resume(&mut self, index: u64, current_pc: u32, label: &str) -> CoreResult<StepControl> {
        let exc_ptr = self.cpu.regs[0];
        self.runtime.ui_runtime.sjlj_resume_count = self.runtime.ui_runtime.sjlj_resume_count.saturating_add(1);
        let count = self.runtime.ui_runtime.sjlj_resume_count;
        if Self::should_trace_hot_hle(count) {
            let detail = format!(
                "hle Unwind_SjLj_Resume(exc={}, head={}; count={}) -> stop (resume path not implemented yet)",
                self.describe_ptr(exc_ptr),
                self.describe_ptr(self.runtime.ui_runtime.sjlj_context_head),
                count,
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }
        Ok(StepControl::Stop(format!(
            "_Unwind_SjLj_Resume requested (exc=0x{exc_ptr:08x}, head=0x{:08x}) but resume unwinding is not implemented",
            self.runtime.ui_runtime.sjlj_context_head,
        )))
    }

    fn hle_objc_sync_enter(&mut self, index: u64, current_pc: u32, label: &str) -> CoreResult<StepControl> {
        let object = self.cpu.regs[0];
        let thread_id = 1u32;
        self.runtime.ui_runtime.objc_sync_enter_count = self.runtime.ui_runtime.objc_sync_enter_count.saturating_add(1);
        let count = self.runtime.ui_runtime.objc_sync_enter_count;

        if object != 0 {
            let (owner, depth) = {
                let monitor = self
                    .runtime
                    .ui_runtime
                    .objc_sync_monitors
                    .entry(object)
                    .or_default();
                if monitor.owner_thread_id == 0 || monitor.owner_thread_id == thread_id {
                    monitor.owner_thread_id = thread_id;
                    monitor.recursion_depth = monitor.recursion_depth.saturating_add(1).max(1);
                } else {
                    self.runtime.ui_runtime.objc_sync_mismatch_count = self.runtime.ui_runtime.objc_sync_mismatch_count.saturating_add(1);
                }
                (monitor.owner_thread_id, monitor.recursion_depth)
            };

            if Self::should_trace_hot_hle(count) {
                let monitor_count = self.runtime.ui_runtime.objc_sync_monitors.len();
                let mismatch_count = self.runtime.ui_runtime.objc_sync_mismatch_count;
                let detail = format!(
                    "hle objc_sync_enter(obj={}, owner={}, depth={}, monitors={}, mismatches={}) -> 0",
                    self.describe_ptr(object),
                    owner,
                    depth,
                    monitor_count,
                    mismatch_count,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
            }
        } else if Self::should_trace_hot_hle(count) {
            let detail = format!(
                "hle objc_sync_enter(obj=nil, monitors={}, mismatches={}) -> 0",
                self.runtime.ui_runtime.objc_sync_monitors.len(),
                self.runtime.ui_runtime.objc_sync_mismatch_count,
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }

        self.cpu.regs[0] = 0;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }

    fn hle_objc_sync_exit(&mut self, index: u64, current_pc: u32, label: &str) -> CoreResult<StepControl> {
        let object = self.cpu.regs[0];
        let thread_id = 1u32;
        self.runtime.ui_runtime.objc_sync_exit_count = self.runtime.ui_runtime.objc_sync_exit_count.saturating_add(1);
        let count = self.runtime.ui_runtime.objc_sync_exit_count;

        let mut owner = 0u32;
        let mut depth = 0u32;
        let mut removed = false;
        let mut mismatched = false;

        if object != 0 {
            let should_remove = if let Some(monitor) = self.runtime.ui_runtime.objc_sync_monitors.get_mut(&object) {
                owner = monitor.owner_thread_id;
                depth = monitor.recursion_depth;
                if owner == 0 || owner == thread_id {
                    if depth > 1 {
                        monitor.recursion_depth -= 1;
                        depth = monitor.recursion_depth;
                    } else {
                        monitor.recursion_depth = 0;
                        monitor.owner_thread_id = 0;
                        depth = 0;
                        removed = true;
                    }
                    removed
                } else {
                    mismatched = true;
                    false
                }
            } else {
                mismatched = true;
                false
            };

            if should_remove {
                self.runtime.ui_runtime.objc_sync_monitors.remove(&object);
            }
        }

        if mismatched {
            self.runtime.ui_runtime.objc_sync_mismatch_count = self.runtime.ui_runtime.objc_sync_mismatch_count.saturating_add(1);
        }

        if Self::should_trace_hot_hle(count) {
            let detail = if object == 0 {
                format!(
                    "hle objc_sync_exit(obj=nil, monitors={}, mismatches={}) -> 0",
                    self.runtime.ui_runtime.objc_sync_monitors.len(),
                    self.runtime.ui_runtime.objc_sync_mismatch_count,
                )
            } else {
                format!(
                    "hle objc_sync_exit(obj={}, owner={}, depth={}, removed={}, monitors={}, mismatches={}) -> 0",
                    self.describe_ptr(object),
                    owner,
                    depth,
                    if removed { "YES" } else { "NO" },
                    self.runtime.ui_runtime.objc_sync_monitors.len(),
                    self.runtime.ui_runtime.objc_sync_mismatch_count,
                )
            };
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }

        self.cpu.regs[0] = 0;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }

    fn hle_libm_unary_f32<F>(
        &mut self,
        index: u64,
        current_pc: u32,
        label: &str,
        symbol: &str,
        call_count: u32,
        op: F,
    ) -> CoreResult<StepControl>
    where
        F: FnOnce(f32) -> f32,
    {
        let input_bits = self.cpu.regs[0];
        let input = Self::f32_from_bits(input_bits);
        let output = op(input);
        let output_bits = output.to_bits();
        if Self::should_trace_hot_hle(call_count) {
            let detail = format!(
                "hle {symbol}(arg={:?} / bits=0x{input_bits:08x}) -> {:?} / bits=0x{output_bits:08x} (count={call_count})",
                input,
                output,
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }
        self.cpu.regs[0] = output_bits;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }

    fn hle_libm_unary_f64<F>(
        &mut self,
        index: u64,
        current_pc: u32,
        label: &str,
        symbol: &str,
        call_count: u32,
        op: F,
    ) -> CoreResult<StepControl>
    where
        F: FnOnce(f64) -> f64,
    {
        let input_bits = ((self.cpu.regs[1] as u64) << 32) | (self.cpu.regs[0] as u64);
        let input = f64::from_bits(input_bits);
        let output = op(input);
        let output_bits = output.to_bits();
        if Self::should_trace_hot_hle(call_count) {
            let detail = format!(
                "hle {symbol}(arg={:?} / bits=0x{input_bits:016x}) -> {:?} / bits=0x{output_bits:016x} (count={call_count})",
                input,
                output,
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }
        self.cpu.regs[0] = output_bits as u32;
        self.cpu.regs[1] = (output_bits >> 32) as u32;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }

    fn hle_libm_binary_f32<F>(
        &mut self,
        index: u64,
        current_pc: u32,
        label: &str,
        symbol: &str,
        call_count: u32,
        op: F,
    ) -> CoreResult<StepControl>
    where
        F: FnOnce(f32, f32) -> f32,
    {
        let lhs_bits = self.cpu.regs[0];
        let rhs_bits = self.cpu.regs[1];
        let lhs = Self::f32_from_bits(lhs_bits);
        let rhs = Self::f32_from_bits(rhs_bits);
        let output = op(lhs, rhs);
        let output_bits = output.to_bits();
        if Self::should_trace_hot_hle(call_count) {
            let detail = format!(
                "hle {symbol}(lhs={:?} / bits=0x{lhs_bits:08x}, rhs={:?} / bits=0x{rhs_bits:08x}) -> {:?} / bits=0x{output_bits:08x} (count={call_count})",
                lhs,
                rhs,
                output,
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }
        self.cpu.regs[0] = output_bits;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }

    fn hle_libm_binary_f64<F>(
        &mut self,
        index: u64,
        current_pc: u32,
        label: &str,
        symbol: &str,
        call_count: u32,
        op: F,
    ) -> CoreResult<StepControl>
    where
        F: FnOnce(f64, f64) -> f64,
    {
        let lhs_bits = ((self.cpu.regs[1] as u64) << 32) | (self.cpu.regs[0] as u64);
        let rhs_bits = ((self.cpu.regs[3] as u64) << 32) | (self.cpu.regs[2] as u64);
        let lhs = f64::from_bits(lhs_bits);
        let rhs = f64::from_bits(rhs_bits);
        let output = op(lhs, rhs);
        let output_bits = output.to_bits();
        if Self::should_trace_hot_hle(call_count) {
            let detail = format!(
                "hle {symbol}(lhs={:?} / bits=0x{lhs_bits:016x}, rhs={:?} / bits=0x{rhs_bits:016x}) -> {:?} / bits=0x{output_bits:016x} (count={call_count})",
                lhs,
                rhs,
                output,
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }
        self.cpu.regs[0] = output_bits as u32;
        self.cpu.regs[1] = (output_bits >> 32) as u32;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }


    fn hle_libgcc_i32_binop<F>(
        &mut self,
        index: u64,
        current_pc: u32,
        label: &str,
        symbol: &str,
        call_count: u32,
        op: F,
    ) -> CoreResult<StepControl>
    where
        F: FnOnce(i32, i32) -> i32,
    {
        let lhs_bits = self.cpu.regs[0];
        let rhs_bits = self.cpu.regs[1];
        let lhs = lhs_bits as i32;
        let rhs = rhs_bits as i32;
        let (result, note) = if rhs == 0 {
            (0i32, "divisor=0 fallback")
        } else {
            (op(lhs, rhs), "ok")
        };
        if Self::should_trace_hot_hle(call_count) {
            let detail = format!(
                "hle {symbol}(lhs={} / bits=0x{lhs_bits:08x}, rhs={} / bits=0x{rhs_bits:08x}) -> {} / bits=0x{:08x} ({note}, count={call_count})",
                lhs,
                rhs,
                result,
                result as u32,
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }
        self.cpu.regs[0] = result as u32;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }

    fn hle_libgcc_u32_binop<F>(
        &mut self,
        index: u64,
        current_pc: u32,
        label: &str,
        symbol: &str,
        call_count: u32,
        op: F,
    ) -> CoreResult<StepControl>
    where
        F: FnOnce(u32, u32) -> u32,
    {
        let lhs = self.cpu.regs[0];
        let rhs = self.cpu.regs[1];
        let (result, note) = if rhs == 0 {
            (0u32, "divisor=0 fallback")
        } else {
            (op(lhs, rhs), "ok")
        };
        if Self::should_trace_hot_hle(call_count) {
            let detail = format!(
                "hle {symbol}(lhs={} / bits=0x{lhs:08x}, rhs={} / bits=0x{rhs:08x}) -> {} / bits=0x{result:08x} ({note}, count={call_count})",
                lhs,
                rhs,
                result,
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }
        self.cpu.regs[0] = result;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }


    const RB_TREE_RED: u32 = 0;
    const RB_TREE_BLACK: u32 = 1;
    const RB_TREE_COLOR_OFFSET: u32 = 0;
    const RB_TREE_PARENT_OFFSET: u32 = 4;
    const RB_TREE_LEFT_OFFSET: u32 = 8;
    const RB_TREE_RIGHT_OFFSET: u32 = 12;

    fn rb_tree_read_link(&self, node: u32, offset: u32) -> CoreResult<u32> {
        if node == 0 {
            Ok(0)
        } else {
            self.read_u32_le(node.wrapping_add(offset))
        }
    }

    fn rb_tree_write_link(&mut self, node: u32, offset: u32, value: u32) -> CoreResult<()> {
        if node == 0 {
            Ok(())
        } else {
            self.write_u32_le(node.wrapping_add(offset), value)
        }
    }

    fn rb_tree_read_color(&self, node: u32) -> CoreResult<u32> {
        if node == 0 {
            Ok(Self::RB_TREE_BLACK)
        } else {
            Ok(self.read_u32_le(node.wrapping_add(Self::RB_TREE_COLOR_OFFSET))?)
        }
    }

    fn rb_tree_write_color(&mut self, node: u32, color: u32) -> CoreResult<()> {
        if node == 0 {
            Ok(())
        } else {
            self.write_u32_le(node.wrapping_add(Self::RB_TREE_COLOR_OFFSET), color)
        }
    }

    fn rb_tree_validate_ptr(&self, ptr: u32, allow_zero: bool, what: &str) -> CoreResult<()> {
        if ptr == 0 {
            if allow_zero {
                return Ok(());
            }
            return Err(CoreError::Backend(format!(
                "rb_tree invalid {}: null pointer",
                what,
            )));
        }
        if ptr < 0x1000 || self.find_region(ptr, 16).is_none() {
            return Err(CoreError::Backend(format!(
                "rb_tree invalid {}: ptr=0x{ptr:08x} unmapped_or_low",
                what,
            )));
        }
        Ok(())
    }

    fn rb_tree_parent(&self, node: u32) -> CoreResult<u32> {
        self.rb_tree_read_link(node, Self::RB_TREE_PARENT_OFFSET)
    }

    fn rb_tree_set_parent(&mut self, node: u32, parent: u32) -> CoreResult<()> {
        self.rb_tree_write_link(node, Self::RB_TREE_PARENT_OFFSET, parent)
    }

    fn rb_tree_left(&self, node: u32) -> CoreResult<u32> {
        self.rb_tree_read_link(node, Self::RB_TREE_LEFT_OFFSET)
    }

    fn rb_tree_set_left(&mut self, node: u32, left: u32) -> CoreResult<()> {
        self.rb_tree_write_link(node, Self::RB_TREE_LEFT_OFFSET, left)
    }

    fn rb_tree_right(&self, node: u32) -> CoreResult<u32> {
        self.rb_tree_read_link(node, Self::RB_TREE_RIGHT_OFFSET)
    }

    fn rb_tree_set_right(&mut self, node: u32, right: u32) -> CoreResult<()> {
        self.rb_tree_write_link(node, Self::RB_TREE_RIGHT_OFFSET, right)
    }

    fn rb_tree_minimum(&self, mut node: u32) -> CoreResult<u32> {
        while node != 0 {
            let next = self.rb_tree_left(node)?;
            if next == 0 {
                break;
            }
            node = next;
        }
        Ok(node)
    }

    fn rb_tree_maximum(&self, mut node: u32) -> CoreResult<u32> {
        while node != 0 {
            let next = self.rb_tree_right(node)?;
            if next == 0 {
                break;
            }
            node = next;
        }
        Ok(node)
    }

    fn rb_tree_rotate_left(&mut self, node: u32, root: &mut u32) -> CoreResult<()> {
        if node == 0 {
            return Ok(());
        }
        let pivot = self.rb_tree_right(node)?;
        if pivot == 0 {
            return Ok(());
        }
        let pivot_left = self.rb_tree_left(pivot)?;
        self.rb_tree_set_right(node, pivot_left)?;
        if pivot_left != 0 {
            self.rb_tree_set_parent(pivot_left, node)?;
        }
        let parent = self.rb_tree_parent(node)?;
        self.rb_tree_set_parent(pivot, parent)?;
        if node == *root {
            *root = pivot;
        } else if node == self.rb_tree_left(parent)? {
            self.rb_tree_set_left(parent, pivot)?;
        } else {
            self.rb_tree_set_right(parent, pivot)?;
        }
        self.rb_tree_set_left(pivot, node)?;
        self.rb_tree_set_parent(node, pivot)?;
        Ok(())
    }

    fn rb_tree_rotate_right(&mut self, node: u32, root: &mut u32) -> CoreResult<()> {
        if node == 0 {
            return Ok(());
        }
        let pivot = self.rb_tree_left(node)?;
        if pivot == 0 {
            return Ok(());
        }
        let pivot_right = self.rb_tree_right(pivot)?;
        self.rb_tree_set_left(node, pivot_right)?;
        if pivot_right != 0 {
            self.rb_tree_set_parent(pivot_right, node)?;
        }
        let parent = self.rb_tree_parent(node)?;
        self.rb_tree_set_parent(pivot, parent)?;
        if node == *root {
            *root = pivot;
        } else if node == self.rb_tree_right(parent)? {
            self.rb_tree_set_right(parent, pivot)?;
        } else {
            self.rb_tree_set_left(parent, pivot)?;
        }
        self.rb_tree_set_right(pivot, node)?;
        self.rb_tree_set_parent(node, pivot)?;
        Ok(())
    }

    fn hle_rb_tree_increment(&mut self, index: u64, current_pc: u32, label: &str) -> CoreResult<StepControl> {
        let mut node = self.cpu.regs[0];
        self.rb_tree_validate_ptr(node, true, "increment.node")?;
        self.runtime.ui_runtime.rb_tree_increment_count = self.runtime.ui_runtime.rb_tree_increment_count.saturating_add(1);
        let count = self.runtime.ui_runtime.rb_tree_increment_count;
        if node != 0 {
            let right = self.rb_tree_right(node)?;
            if right != 0 {
                node = self.rb_tree_minimum(right)?;
            } else {
                let mut parent = self.rb_tree_parent(node)?;
                while node == self.rb_tree_right(parent)? {
                    node = parent;
                    parent = self.rb_tree_parent(parent)?;
                }
                if self.rb_tree_right(node)? != parent {
                    node = parent;
                }
            }
        }
        if Self::should_trace_hot_hle(count) {
            let detail = format!(
                "hle {label}(node={}) -> {} (count={count})",
                self.describe_ptr(self.cpu.regs[0]),
                self.describe_ptr(node),
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }
        self.cpu.regs[0] = node;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }

    fn hle_rb_tree_decrement(&mut self, index: u64, current_pc: u32, label: &str) -> CoreResult<StepControl> {
        let mut node = self.cpu.regs[0];
        self.rb_tree_validate_ptr(node, true, "decrement.node")?;
        self.runtime.ui_runtime.rb_tree_decrement_count = self.runtime.ui_runtime.rb_tree_decrement_count.saturating_add(1);
        let count = self.runtime.ui_runtime.rb_tree_decrement_count;
        if node != 0 {
            let parent = self.rb_tree_parent(node)?;
            if self.rb_tree_read_color(node)? == Self::RB_TREE_RED
                && parent != 0
                && self.rb_tree_parent(parent)? == node
            {
                node = self.rb_tree_right(node)?;
            } else {
                let left = self.rb_tree_left(node)?;
                if left != 0 {
                    node = self.rb_tree_maximum(left)?;
                } else {
                    let mut parent = self.rb_tree_parent(node)?;
                    while node == self.rb_tree_left(parent)? {
                        node = parent;
                        parent = self.rb_tree_parent(parent)?;
                    }
                    node = parent;
                }
            }
        }
        if Self::should_trace_hot_hle(count) {
            let detail = format!(
                "hle {label}(node={}) -> {} (count={count})",
                self.describe_ptr(self.cpu.regs[0]),
                self.describe_ptr(node),
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }
        self.cpu.regs[0] = node;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }

    fn hle_rb_tree_insert_and_rebalance(&mut self, index: u64, current_pc: u32, label: &str) -> CoreResult<StepControl> {
        let insert_left = self.cpu.regs[0] != 0;
        let mut node = self.cpu.regs[1];
        let parent = self.cpu.regs[2];
        let header = self.cpu.regs[3];
        self.rb_tree_validate_ptr(node, false, "insert.node")?;
        self.rb_tree_validate_ptr(parent, false, "insert.parent")?;
        self.rb_tree_validate_ptr(header, false, "insert.header")?;
        let mut root = self.rb_tree_parent(header)?;
        let mut leftmost = self.rb_tree_left(header)?;
        let mut rightmost = self.rb_tree_right(header)?;
        self.rb_tree_validate_ptr(root, true, "insert.root")?;
        self.rb_tree_validate_ptr(leftmost, true, "insert.leftmost")?;
        self.rb_tree_validate_ptr(rightmost, true, "insert.rightmost")?;

        self.runtime.ui_runtime.rb_tree_insert_count = self.runtime.ui_runtime.rb_tree_insert_count.saturating_add(1);
        let count = self.runtime.ui_runtime.rb_tree_insert_count;

        self.rb_tree_set_parent(node, parent)?;
        self.rb_tree_set_left(node, 0)?;
        self.rb_tree_set_right(node, 0)?;
        self.rb_tree_write_color(node, Self::RB_TREE_RED)?;

        if insert_left {
            self.rb_tree_set_left(parent, node)?;
            if parent == header {
                root = node;
                leftmost = node;
                rightmost = node;
            } else if parent == leftmost {
                leftmost = node;
            }
        } else {
            self.rb_tree_set_right(parent, node)?;
            if parent == header {
                root = node;
                leftmost = node;
                rightmost = node;
            } else if parent == rightmost {
                rightmost = node;
            }
        }

        while node != root {
            let node_parent = self.rb_tree_parent(node)?;
            if self.rb_tree_read_color(node_parent)? != Self::RB_TREE_RED {
                break;
            }
            let node_grandparent = self.rb_tree_parent(node_parent)?;
            if node_parent == self.rb_tree_left(node_grandparent)? {
                let uncle = self.rb_tree_right(node_grandparent)?;
                if self.rb_tree_read_color(uncle)? == Self::RB_TREE_RED {
                    self.rb_tree_write_color(node_parent, Self::RB_TREE_BLACK)?;
                    self.rb_tree_write_color(uncle, Self::RB_TREE_BLACK)?;
                    self.rb_tree_write_color(node_grandparent, Self::RB_TREE_RED)?;
                    node = node_grandparent;
                } else {
                    if node == self.rb_tree_right(node_parent)? {
                        node = node_parent;
                        self.rb_tree_rotate_left(node, &mut root)?;
                    }
                    let node_parent = self.rb_tree_parent(node)?;
                    let node_grandparent = self.rb_tree_parent(node_parent)?;
                    self.rb_tree_write_color(node_parent, Self::RB_TREE_BLACK)?;
                    self.rb_tree_write_color(node_grandparent, Self::RB_TREE_RED)?;
                    self.rb_tree_rotate_right(node_grandparent, &mut root)?;
                }
            } else {
                let uncle = self.rb_tree_left(node_grandparent)?;
                if self.rb_tree_read_color(uncle)? == Self::RB_TREE_RED {
                    self.rb_tree_write_color(node_parent, Self::RB_TREE_BLACK)?;
                    self.rb_tree_write_color(uncle, Self::RB_TREE_BLACK)?;
                    self.rb_tree_write_color(node_grandparent, Self::RB_TREE_RED)?;
                    node = node_grandparent;
                } else {
                    if node == self.rb_tree_left(node_parent)? {
                        node = node_parent;
                        self.rb_tree_rotate_right(node, &mut root)?;
                    }
                    let node_parent = self.rb_tree_parent(node)?;
                    let node_grandparent = self.rb_tree_parent(node_parent)?;
                    self.rb_tree_write_color(node_parent, Self::RB_TREE_BLACK)?;
                    self.rb_tree_write_color(node_grandparent, Self::RB_TREE_RED)?;
                    self.rb_tree_rotate_left(node_grandparent, &mut root)?;
                }
            }
        }
        self.rb_tree_write_color(root, Self::RB_TREE_BLACK)?;
        self.rb_tree_set_parent(header, root)?;
        self.rb_tree_set_left(header, leftmost)?;
        self.rb_tree_set_right(header, rightmost)?;

        if Self::should_trace_hot_hle(count) {
            let detail = format!(
                "hle {label}(insert_left={}, node={}, parent={}, header={}) -> root={} leftmost={} rightmost={} (count={count})",
                if insert_left { "YES" } else { "NO" },
                self.describe_ptr(self.cpu.regs[1]),
                self.describe_ptr(parent),
                self.describe_ptr(header),
                self.describe_ptr(root),
                self.describe_ptr(leftmost),
                self.describe_ptr(rightmost),
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }

        self.cpu.regs[0] = 0;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }

    fn hle_rb_tree_rebalance_for_erase(&mut self, index: u64, current_pc: u32, label: &str) -> CoreResult<StepControl> {
        let z = self.cpu.regs[0];
        let header = self.cpu.regs[1];
        self.rb_tree_validate_ptr(z, false, "erase.z")?;
        self.rb_tree_validate_ptr(header, false, "erase.header")?;
        let mut root = self.rb_tree_parent(header)?;
        let mut leftmost = self.rb_tree_left(header)?;
        let mut rightmost = self.rb_tree_right(header)?;
        self.rb_tree_validate_ptr(root, true, "erase.root")?;
        self.rb_tree_validate_ptr(leftmost, true, "erase.leftmost")?;
        self.rb_tree_validate_ptr(rightmost, true, "erase.rightmost")?;
        let mut y = z;
        let mut x = 0u32;
        let mut x_parent = 0u32;

        self.runtime.ui_runtime.rb_tree_erase_rebalance_count = self.runtime.ui_runtime.rb_tree_erase_rebalance_count.saturating_add(1);
        let count = self.runtime.ui_runtime.rb_tree_erase_rebalance_count;

        if self.rb_tree_left(y)? == 0 {
            x = self.rb_tree_right(y)?;
        } else if self.rb_tree_right(y)? == 0 {
            x = self.rb_tree_left(y)?;
        } else {
            y = self.rb_tree_right(y)?;
            y = self.rb_tree_minimum(y)?;
            x = self.rb_tree_right(y)?;
        }

        if y != z {
            let z_left = self.rb_tree_left(z)?;
            let z_right = self.rb_tree_right(z)?;
            let z_parent = self.rb_tree_parent(z)?;
            if z_left != 0 {
                self.rb_tree_set_parent(z_left, y)?;
            }
            self.rb_tree_set_left(y, z_left)?;
            if y != z_right {
                x_parent = self.rb_tree_parent(y)?;
                if x != 0 {
                    self.rb_tree_set_parent(x, x_parent)?;
                }
                self.rb_tree_set_left(x_parent, x)?;
                self.rb_tree_set_right(y, z_right)?;
                if z_right != 0 {
                    self.rb_tree_set_parent(z_right, y)?;
                }
            } else {
                x_parent = y;
            }
            if root == z {
                root = y;
            } else if z == self.rb_tree_left(z_parent)? {
                self.rb_tree_set_left(z_parent, y)?;
            } else {
                self.rb_tree_set_right(z_parent, y)?;
            }
            self.rb_tree_set_parent(y, z_parent)?;
            let y_color = self.rb_tree_read_color(y)?;
            let z_color = self.rb_tree_read_color(z)?;
            self.rb_tree_write_color(y, z_color)?;
            self.rb_tree_write_color(z, y_color)?;
            y = z;
        } else {
            x_parent = self.rb_tree_parent(y)?;
            if x != 0 {
                self.rb_tree_set_parent(x, x_parent)?;
            }
            if root == z {
                root = x;
            } else if z == self.rb_tree_left(x_parent)? {
                self.rb_tree_set_left(x_parent, x)?;
            } else {
                self.rb_tree_set_right(x_parent, x)?;
            }
            if leftmost == z {
                if self.rb_tree_right(z)? == 0 {
                    leftmost = x_parent;
                } else {
                    leftmost = self.rb_tree_minimum(x)?;
                }
            }
            if rightmost == z {
                if self.rb_tree_left(z)? == 0 {
                    rightmost = x_parent;
                } else {
                    rightmost = self.rb_tree_maximum(x)?;
                }
            }
        }

        if self.rb_tree_read_color(y)? != Self::RB_TREE_RED {
            while x != root && self.rb_tree_read_color(x)? == Self::RB_TREE_BLACK {
                if x_parent == 0 {
                    break;
                }
                if x == self.rb_tree_left(x_parent)? {
                    let mut w = self.rb_tree_right(x_parent)?;
                    if self.rb_tree_read_color(w)? == Self::RB_TREE_RED {
                        self.rb_tree_write_color(w, Self::RB_TREE_BLACK)?;
                        self.rb_tree_write_color(x_parent, Self::RB_TREE_RED)?;
                        self.rb_tree_rotate_left(x_parent, &mut root)?;
                        w = self.rb_tree_right(x_parent)?;
                    }
                    if self.rb_tree_read_color(self.rb_tree_left(w)?)? == Self::RB_TREE_BLACK
                        && self.rb_tree_read_color(self.rb_tree_right(w)?)? == Self::RB_TREE_BLACK
                    {
                        self.rb_tree_write_color(w, Self::RB_TREE_RED)?;
                        x = x_parent;
                        x_parent = self.rb_tree_parent(x_parent)?;
                    } else {
                        if self.rb_tree_read_color(self.rb_tree_right(w)?)? == Self::RB_TREE_BLACK {
                            self.rb_tree_write_color(self.rb_tree_left(w)?, Self::RB_TREE_BLACK)?;
                            self.rb_tree_write_color(w, Self::RB_TREE_RED)?;
                            self.rb_tree_rotate_right(w, &mut root)?;
                            w = self.rb_tree_right(x_parent)?;
                        }
                        self.rb_tree_write_color(w, self.rb_tree_read_color(x_parent)?)?;
                        self.rb_tree_write_color(x_parent, Self::RB_TREE_BLACK)?;
                        self.rb_tree_write_color(self.rb_tree_right(w)?, Self::RB_TREE_BLACK)?;
                        self.rb_tree_rotate_left(x_parent, &mut root)?;
                        break;
                    }
                } else {
                    let mut w = self.rb_tree_left(x_parent)?;
                    if self.rb_tree_read_color(w)? == Self::RB_TREE_RED {
                        self.rb_tree_write_color(w, Self::RB_TREE_BLACK)?;
                        self.rb_tree_write_color(x_parent, Self::RB_TREE_RED)?;
                        self.rb_tree_rotate_right(x_parent, &mut root)?;
                        w = self.rb_tree_left(x_parent)?;
                    }
                    if self.rb_tree_read_color(self.rb_tree_right(w)?)? == Self::RB_TREE_BLACK
                        && self.rb_tree_read_color(self.rb_tree_left(w)?)? == Self::RB_TREE_BLACK
                    {
                        self.rb_tree_write_color(w, Self::RB_TREE_RED)?;
                        x = x_parent;
                        x_parent = self.rb_tree_parent(x_parent)?;
                    } else {
                        if self.rb_tree_read_color(self.rb_tree_left(w)?)? == Self::RB_TREE_BLACK {
                            self.rb_tree_write_color(self.rb_tree_right(w)?, Self::RB_TREE_BLACK)?;
                            self.rb_tree_write_color(w, Self::RB_TREE_RED)?;
                            self.rb_tree_rotate_left(w, &mut root)?;
                            w = self.rb_tree_left(x_parent)?;
                        }
                        self.rb_tree_write_color(w, self.rb_tree_read_color(x_parent)?)?;
                        self.rb_tree_write_color(x_parent, Self::RB_TREE_BLACK)?;
                        self.rb_tree_write_color(self.rb_tree_left(w)?, Self::RB_TREE_BLACK)?;
                        self.rb_tree_rotate_right(x_parent, &mut root)?;
                        break;
                    }
                }
            }
            self.rb_tree_write_color(x, Self::RB_TREE_BLACK)?;
        }

        self.rb_tree_set_parent(header, root)?;
        self.rb_tree_set_left(header, leftmost)?;
        self.rb_tree_set_right(header, rightmost)?;

        if Self::should_trace_hot_hle(count) {
            let detail = format!(
                "hle {label}(z={}, header={}) -> eraseNode={} root={} leftmost={} rightmost={} (count={count})",
                self.describe_ptr(z),
                self.describe_ptr(header),
                self.describe_ptr(y),
                self.describe_ptr(root),
                self.describe_ptr(leftmost),
                self.describe_ptr(rightmost),
            );
            self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
        }

        self.cpu.regs[0] = y;
        self.return_from_hle_stub();
        Ok(StepControl::Continue)
    }

    fn suppress_exit_into_runloop(&mut self, index: u64, current_pc: u32, label: &str, code: u32) -> StepControl {
        self.runtime.ui_runtime.exit_suppressed = true;
        self.diag.trace.push(self.hle_trace_line(
            index,
            current_pc,
            label,
            &format!("hle exit(code={code}) suppressed; keeping synthetic UIApplicationMain runloop alive"),
        ));
        self.diag.trace.push(format!(
            "     ↳ ui activation window={} firstResponder={} state=active",
            self.describe_ptr(self.runtime.ui_objects.window),
            self.describe_ptr(self.runtime.ui_objects.first_responder),
        ));

        if self.tuning.live_host_mode {
            let mut keepalive_ticks = 0u32;
            while !is_stop_requested() {
                let tick_started = std::time::Instant::now();
                self.push_synthetic_runloop_tick("exit-suppressed-live", true);
                keepalive_ticks = keepalive_ticks.saturating_add(1);
                if keepalive_ticks <= 3 || keepalive_ticks % 60 == 0 {
                    self.diag.trace.push(format!(
                        "     ↳ live host keepalive tick#{} window={} firstResponder={} scene={} frames={} presents={} sources={} targetDtMs={:.3} workMs={:.3}",
                        keepalive_ticks,
                        self.describe_ptr(self.runtime.ui_objects.window),
                        self.describe_ptr(self.runtime.ui_objects.first_responder),
                        self.describe_ptr(self.runtime.ui_cocos.running_scene),
                        self.runtime.ui_graphics.graphics_frame_index,
                        self.runtime.ui_graphics.graphics_present_calls,
                        self.runtime.ui_runtime.runloop_sources,
                        self.live_host_runloop_target_duration().as_secs_f64() * 1000.0,
                        tick_started.elapsed().as_secs_f64() * 1000.0,
                    ));
                }
                self.sleep_live_host_runloop_remainder(tick_started);
            }
            let flushed_ticks = self.flush_loading_delayed_selectors_before_shutdown(
                "exit-suppressed-live-shutdown-flush",
                90,
            );
            if flushed_ticks > 0 {
                self.diag.trace.push(format!(
                    "     ↳ live host shutdown flush extraTicks={} scene={} delayedSelectors={} foundationTimers={}",
                    flushed_ticks,
                    self.describe_ptr(self.runtime.ui_cocos.running_scene),
                    self.runtime.scheduler.timers.delayed_selectors.len(),
                    self.count_attached_foundation_timers(),
                ));
            }
            return StepControl::Stop(format!(
                "live host shutdown requested (suppressed exit({code}), ticks={}, sources={} (last_tick {}->{}), net_events={}, delegate_callbacks={}, idle_after_completion={}, retained(response={},data={}), fault_events={}, fault_modes=[{}], last_error={}, conn_state={}, retry={}, gfx(surfaceReady={},presented={},frames={},presents={},readback={},rbCalls={},rbBytes={},changed={},stableStreak={},mono={}‰,uniqueFrames={}), scene(transitions={},runWith={},replace={},push={},running={},sceneTicks={},events={}/{}/{}), scheduler(mainLoop={},drawScene={},drawFrame={},schedule={},update={},invalidate={},renderCb={}))",
                self.runtime.ui_runtime.runloop_ticks,
                self.runtime.ui_runtime.runloop_sources,
                self.runtime.ui_runtime.last_tick_sources_before,
                self.runtime.ui_runtime.last_tick_sources_after,
                self.runtime.ui_network.network_events,
                self.runtime.ui_network.delegate_callbacks,
                self.runtime.ui_runtime.idle_ticks_after_completion,
                Self::retained_flag(self.runtime.ui_network.network_response_retained),
                Self::retained_flag(self.runtime.ui_network.network_data_retained),
                self.runtime.ui_network.network_fault_events,
                if self.runtime.ui_network.network_fault_history.is_empty() { "none".to_string() } else { self.runtime.ui_network.network_fault_history.join(",") },
                self.network_last_error_summary(),
                self.network_connection_state_name(),
                if self.network_should_retry() { "YES" } else { "NO" },
                Self::retained_flag(self.runtime.ui_graphics.graphics_surface_ready),
                Self::retained_flag(self.runtime.ui_graphics.graphics_presented),
                self.runtime.ui_graphics.graphics_frame_index,
                self.runtime.ui_graphics.graphics_present_calls,
                Self::retained_flag(self.runtime.ui_graphics.graphics_readback_ready),
                self.runtime.ui_graphics.graphics_readback_calls,
                self.runtime.ui_graphics.graphics_last_readback_bytes,
                if self.runtime.ui_graphics.graphics_readback_changed { "YES" } else { "NO" },
                self.runtime.ui_graphics.graphics_readback_stable_streak,
                self.runtime.ui_graphics.graphics_last_dominant_pct_milli,
                self.runtime.ui_graphics.graphics_unique_frames_saved,
                self.runtime.ui_cocos.scene_transition_calls.max(self.runtime.scene.synthetic_scene_transitions),
                self.runtime.ui_cocos.scene_run_with_scene_calls,
                self.runtime.ui_cocos.scene_replace_scene_calls,
                self.runtime.ui_cocos.scene_push_scene_calls,
                self.describe_ptr(self.runtime.ui_cocos.running_scene),
                self.runtime.scene.synthetic_running_scene_ticks,
                self.runtime.ui_cocos.scene_on_exit_events,
                self.runtime.ui_cocos.scene_on_enter_events,
                self.runtime.ui_cocos.scene_on_enter_transition_finish_events,
                self.runtime.ui_cocos.scheduler_mainloop_calls,
                self.runtime.ui_cocos.scheduler_draw_scene_calls,
                self.runtime.ui_cocos.scheduler_draw_frame_calls,
                self.runtime.ui_cocos.scheduler_schedule_calls,
                self.runtime.ui_cocos.scheduler_update_calls,
                self.runtime.ui_cocos.scheduler_invalidate_calls,
                self.runtime.ui_cocos.scheduler_render_callback_calls,
            ));
        }

        let tick_budget = self.tuning.synthetic_runloop_ticks;
        if tick_budget == 0 {
            return StepControl::Stop(format!(
                "strict runtime mode suppressed synthetic main runloop after exit({code}) (ticks={}, sources={}, net_events={}, gfx_frames={})",
                self.runtime.ui_runtime.runloop_ticks,
                self.runtime.ui_runtime.runloop_sources,
                self.runtime.ui_network.network_events,
                self.runtime.ui_graphics.graphics_frame_index,
            ));
        }
        for _ in 0..tick_budget {
            self.push_synthetic_runloop_tick("exit-suppressed", true);
        }
        StepControl::Stop(format!(
            "synthetic main runloop active (suppressed exit({code}), ticks={}, sources={} (last_tick {}->{}), net_events={}, delegate_callbacks={}, idle_after_completion={}, retained(response={},data={}), fault_events={}, fault_modes=[{}], last_error={}, conn_state={}, retry={}, gfx(surfaceReady={},presented={},frames={},presents={},readback={},rbCalls={},rbBytes={},changed={},stableStreak={},mono={}‰,uniqueFrames={}), scene(transitions={},runWith={},replace={},push={},running={},sceneTicks={},events={}/{}/{}), scheduler(mainLoop={},drawScene={},drawFrame={},schedule={},update={},invalidate={},renderCb={}))",
            self.runtime.ui_runtime.runloop_ticks,
            self.runtime.ui_runtime.runloop_sources,
            self.runtime.ui_runtime.last_tick_sources_before,
            self.runtime.ui_runtime.last_tick_sources_after,
            self.runtime.ui_network.network_events,
            self.runtime.ui_network.delegate_callbacks,
            self.runtime.ui_runtime.idle_ticks_after_completion,
            Self::retained_flag(self.runtime.ui_network.network_response_retained),
            Self::retained_flag(self.runtime.ui_network.network_data_retained),
            self.runtime.ui_network.network_fault_events,
            if self.runtime.ui_network.network_fault_history.is_empty() { "none".to_string() } else { self.runtime.ui_network.network_fault_history.join(",") },
            self.network_last_error_summary(),
            self.network_connection_state_name(),
            if self.network_should_retry() { "YES" } else { "NO" },
            Self::retained_flag(self.runtime.ui_graphics.graphics_surface_ready),
            Self::retained_flag(self.runtime.ui_graphics.graphics_presented),
            self.runtime.ui_graphics.graphics_frame_index,
            self.runtime.ui_graphics.graphics_present_calls,
            Self::retained_flag(self.runtime.ui_graphics.graphics_readback_ready),
            self.runtime.ui_graphics.graphics_readback_calls,
            self.runtime.ui_graphics.graphics_last_readback_bytes,
            if self.runtime.ui_graphics.graphics_readback_changed { "YES" } else { "NO" },
            self.runtime.ui_graphics.graphics_readback_stable_streak,
            self.runtime.ui_graphics.graphics_last_dominant_pct_milli,
            self.runtime.ui_graphics.graphics_unique_frames_saved,
            self.runtime.ui_cocos.scene_transition_calls.max(self.runtime.scene.synthetic_scene_transitions),
            self.runtime.ui_cocos.scene_run_with_scene_calls,
            self.runtime.ui_cocos.scene_replace_scene_calls,
            self.runtime.ui_cocos.scene_push_scene_calls,
            self.describe_ptr(self.runtime.ui_cocos.running_scene),
            self.runtime.scene.synthetic_running_scene_ticks,
            self.runtime.ui_cocos.scene_on_exit_events,
            self.runtime.ui_cocos.scene_on_enter_events,
            self.runtime.ui_cocos.scene_on_enter_transition_finish_events,
            self.runtime.ui_cocos.scheduler_mainloop_calls,
            self.runtime.ui_cocos.scheduler_draw_scene_calls,
            self.runtime.ui_cocos.scheduler_draw_frame_calls,
            self.runtime.ui_cocos.scheduler_schedule_calls,
            self.runtime.ui_cocos.scheduler_update_calls,
            self.runtime.ui_cocos.scheduler_invalidate_calls,
            self.runtime.ui_cocos.scheduler_render_callback_calls,
        ))
    }

    fn trace_branch_target(&mut self, op: &str, reg: usize, target: u32, return_lr: Option<u32>) {
        let mut line = format!("     ↳ {op} r{reg} -> 0x{target:08x}");
        if let Some(label) = self.symbol_label(target & !1) {
            line.push_str(&format!(" <{label}>"));
        }
        if let Some(lr) = return_lr {
            line.push_str(&format!(" lr=0x{lr:08x}"));
        }
        self.diag.trace.push(line);
    }

    fn hle_trace_line(&self, index: u64, pc: u32, label: &str, detail: &str) -> String {
        format!(
            "#{:02} pc=0x{:08x} <{}> {} | r0=0x{:08x} r1=0x{:08x} r2=0x{:08x} r3=0x{:08x} sp=0x{:08x} lr=0x{:08x}",
            index,
            pc,
            label,
            detail,
            self.cpu.regs[0],
            self.cpu.regs[1],
            self.cpu.regs[2],
            self.cpu.regs[3],
            self.cpu.regs[13],
            self.cpu.regs[14],
        )
    }

    pub(crate) fn handle_hle_stub(&mut self, index: u64, current_pc: u32) -> CoreResult<Option<StepControl>> {
        let Some(label) = self.symbol_label(current_pc).map(str::to_string) else {
            return Ok(None);
        };
        if let Some(control) = self.maybe_handle_network_hle_stub(index, current_pc, &label)? {
            return Ok(Some(control));
        }
        if let Some(control) = self.maybe_handle_graphics_hle_stub(index, current_pc, &label)? {
            return Ok(Some(control));
        }
        match label.as_str() {
            "Unwind_SjLj_Register" => {
                return Ok(Some(self.hle_sjlj_register(index, current_pc, &label)?));
            }
            "Unwind_SjLj_Unregister" => {
                return Ok(Some(self.hle_sjlj_unregister(index, current_pc, &label)?));
            }
            "Unwind_SjLj_Resume" => {
                return Ok(Some(self.hle_sjlj_resume(index, current_pc, &label)?));
            }
            "objc_sync_enter" => {
                return Ok(Some(self.hle_objc_sync_enter(index, current_pc, &label)?));
            }
            "objc_sync_exit" => {
                return Ok(Some(self.hle_objc_sync_exit(index, current_pc, &label)?));
            }
            "ZSt18_Rb_tree_incrementPSt18_Rb_tree_node_base" => {
                return Ok(Some(self.hle_rb_tree_increment(index, current_pc, &label)?));
            }
            "ZSt18_Rb_tree_decrementPSt18_Rb_tree_node_base" => {
                return Ok(Some(self.hle_rb_tree_decrement(index, current_pc, &label)?));
            }
            "ZSt28_Rb_tree_rebalance_for_erasePSt18_Rb_tree_node_baseRS_" => {
                return Ok(Some(self.hle_rb_tree_rebalance_for_erase(index, current_pc, &label)?));
            }
            "ZSt29_Rb_tree_insert_and_rebalancebPSt18_Rb_tree_node_baseS0_RS_" => {
                return Ok(Some(self.hle_rb_tree_insert_and_rebalance(index, current_pc, &label)?));
            }
            "srand" => {
                let seed = self.cpu.regs[0];
                self.seed_hle_prng(seed);
                let detail = format!(
                    "hle srand(seed={}) -> state=0x{:08x}",
                    seed,
                    self.runtime.ui_runtime.prng_state,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "srandom" => {
                let seed = self.cpu.regs[0];
                self.seed_hle_prng(seed);
                let detail = format!(
                    "hle srandom(seed={}) -> state=0x{:08x}",
                    seed,
                    self.runtime.ui_runtime.prng_state,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "rand" => {
                let value = self.next_hle_prng31() & 0x7fff;
                let detail = format!(
                    "hle rand() -> {} (draw#{}, seed={})",
                    value,
                    self.runtime.ui_runtime.prng_draw_count,
                    self.runtime.ui_runtime.prng_last_seed,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = value;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "random" => {
                let value = self.next_hle_prng31();
                let detail = format!(
                    "hle random() -> {} (draw#{}, seed={})",
                    value,
                    self.runtime.ui_runtime.prng_draw_count,
                    self.runtime.ui_runtime.prng_last_seed,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = value;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "roundf" => {
                self.runtime.ui_runtime.roundf_count = self.runtime.ui_runtime.roundf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "roundf", self.runtime.ui_runtime.roundf_count, |v| v.round())?));
            }
            "floorf" => {
                self.runtime.ui_runtime.floorf_count = self.runtime.ui_runtime.floorf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "floorf", self.runtime.ui_runtime.floorf_count, |v| v.floor())?));
            }
            "ceilf" => {
                self.runtime.ui_runtime.ceilf_count = self.runtime.ui_runtime.ceilf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "ceilf", self.runtime.ui_runtime.ceilf_count, |v| v.ceil())?));
            }
            "fabsf" => {
                self.runtime.ui_runtime.fabsf_count = self.runtime.ui_runtime.fabsf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "fabsf", self.runtime.ui_runtime.fabsf_count, |v| v.abs())?));
            }
            "sinf" => {
                self.runtime.ui_runtime.sinf_count = self.runtime.ui_runtime.sinf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "sinf", self.runtime.ui_runtime.sinf_count, |v| v.sin())?));
            }
            "cosf" => {
                self.runtime.ui_runtime.cosf_count = self.runtime.ui_runtime.cosf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "cosf", self.runtime.ui_runtime.cosf_count, |v| v.cos())?));
            }
            "tanf" => {
                self.runtime.ui_runtime.tanf_count = self.runtime.ui_runtime.tanf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "tanf", self.runtime.ui_runtime.tanf_count, |v| v.tan())?));
            }
            "asinf" => {
                self.runtime.ui_runtime.asinf_count = self.runtime.ui_runtime.asinf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "asinf", self.runtime.ui_runtime.asinf_count, |v| v.asin())?));
            }
            "acosf" => {
                self.runtime.ui_runtime.acosf_count = self.runtime.ui_runtime.acosf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "acosf", self.runtime.ui_runtime.acosf_count, |v| v.acos())?));
            }
            "atanf" => {
                self.runtime.ui_runtime.atanf_count = self.runtime.ui_runtime.atanf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "atanf", self.runtime.ui_runtime.atanf_count, |v| v.atan())?));
            }
            "expf" => {
                self.runtime.ui_runtime.expf_count = self.runtime.ui_runtime.expf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "expf", self.runtime.ui_runtime.expf_count, |v| v.exp())?));
            }
            "logf" => {
                self.runtime.ui_runtime.logf_count = self.runtime.ui_runtime.logf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "logf", self.runtime.ui_runtime.logf_count, |v| v.ln())?));
            }
            "sqrtf" => {
                self.runtime.ui_runtime.sqrtf_count = self.runtime.ui_runtime.sqrtf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f32(index, current_pc, &label, "sqrtf", self.runtime.ui_runtime.sqrtf_count, |v| v.sqrt())?));
            }
            "atan2f" => {
                self.runtime.ui_runtime.atan2f_count = self.runtime.ui_runtime.atan2f_count.saturating_add(1);
                return Ok(Some(self.hle_libm_binary_f32(index, current_pc, &label, "atan2f", self.runtime.ui_runtime.atan2f_count, |lhs, rhs| lhs.atan2(rhs))?));
            }
            "fmodf" => {
                self.runtime.ui_runtime.fmodf_count = self.runtime.ui_runtime.fmodf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_binary_f32(index, current_pc, &label, "fmodf", self.runtime.ui_runtime.fmodf_count, |lhs, rhs| lhs % rhs)?));
            }
            "fmaxf" => {
                self.runtime.ui_runtime.fmaxf_count = self.runtime.ui_runtime.fmaxf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_binary_f32(index, current_pc, &label, "fmaxf", self.runtime.ui_runtime.fmaxf_count, |lhs, rhs| lhs.max(rhs))?));
            }
            "fminf" => {
                self.runtime.ui_runtime.fminf_count = self.runtime.ui_runtime.fminf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_binary_f32(index, current_pc, &label, "fminf", self.runtime.ui_runtime.fminf_count, |lhs, rhs| lhs.min(rhs))?));
            }
            "powf" => {
                self.runtime.ui_runtime.powf_count = self.runtime.ui_runtime.powf_count.saturating_add(1);
                return Ok(Some(self.hle_libm_binary_f32(index, current_pc, &label, "powf", self.runtime.ui_runtime.powf_count, |lhs, rhs| lhs.powf(rhs))?));
            }
            "floor" => {
                self.runtime.ui_runtime.floor_count = self.runtime.ui_runtime.floor_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f64(index, current_pc, &label, "floor", self.runtime.ui_runtime.floor_count, |v| v.floor())?));
            }
            "ceil" => {
                self.runtime.ui_runtime.ceil_count = self.runtime.ui_runtime.ceil_count.saturating_add(1);
                return Ok(Some(self.hle_libm_unary_f64(index, current_pc, &label, "ceil", self.runtime.ui_runtime.ceil_count, |v| v.ceil())?));
            }
            "atan2" => {
                self.runtime.ui_runtime.atan2_count = self.runtime.ui_runtime.atan2_count.saturating_add(1);
                return Ok(Some(self.hle_libm_binary_f64(index, current_pc, &label, "atan2", self.runtime.ui_runtime.atan2_count, |lhs, rhs| lhs.atan2(rhs))?));
            }
            "modsi3" => {
                self.runtime.ui_runtime.modsi3_count = self.runtime.ui_runtime.modsi3_count.saturating_add(1);
                return Ok(Some(self.hle_libgcc_i32_binop(index, current_pc, &label, "modsi3", self.runtime.ui_runtime.modsi3_count, |lhs, rhs| {
                    if lhs == i32::MIN && rhs == -1 {
                        0
                    } else {
                        let quotient = (lhs as i64) / (rhs as i64);
                        let remainder = (lhs as i64) - quotient * (rhs as i64);
                        remainder as i32
                    }
                })?));
            }
            "divsi3" => {
                self.runtime.ui_runtime.divsi3_count = self.runtime.ui_runtime.divsi3_count.saturating_add(1);
                return Ok(Some(self.hle_libgcc_i32_binop(index, current_pc, &label, "divsi3", self.runtime.ui_runtime.divsi3_count, |lhs, rhs| {
                    ((lhs as i64) / (rhs as i64)) as i32
                })?));
            }
            "udivsi3" => {
                self.runtime.ui_runtime.udivsi3_count = self.runtime.ui_runtime.udivsi3_count.saturating_add(1);
                return Ok(Some(self.hle_libgcc_u32_binop(index, current_pc, &label, "udivsi3", self.runtime.ui_runtime.udivsi3_count, |lhs, rhs| lhs / rhs)?));
            }
            "umodsi3" => {
                self.runtime.ui_runtime.umodsi3_count = self.runtime.ui_runtime.umodsi3_count.saturating_add(1);
                return Ok(Some(self.hle_libgcc_u32_binop(index, current_pc, &label, "umodsi3", self.runtime.ui_runtime.umodsi3_count, |lhs, rhs| lhs % rhs)?));
            }
            "time" => {
                let time_out_ptr = self.cpu.regs[0];
                let (secs, _) = self.guest_unix_time_parts();
                if time_out_ptr != 0 {
                    let _ = self.write_u32_le(time_out_ptr, secs);
                }
                let detail = format!(
                    "hle time(tloc={}) -> {}{}",
                    self.describe_ptr(time_out_ptr),
                    secs,
                    if time_out_ptr != 0 {
                        format!(", wrote *tloc=0x{time_out_ptr:08x}")
                    } else {
                        String::new()
                    }
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = secs;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "gettimeofday" => {
                let tv_ptr = self.cpu.regs[0];
                let tz_ptr = self.cpu.regs[1];
                let (secs, micros) = self.guest_unix_time_parts();
                if tv_ptr != 0 {
                    let _ = self.write_u32_le(tv_ptr, secs);
                    let _ = self.write_u32_le(tv_ptr.wrapping_add(4), micros);
                }
                let detail = format!(
                    "hle gettimeofday(tv={}, tz={}) -> sec={} usec={}{}",
                    self.describe_ptr(tv_ptr),
                    self.describe_ptr(tz_ptr),
                    secs,
                    micros,
                    if tv_ptr != 0 {
                        format!(", wrote timeval@0x{tv_ptr:08x}")
                    } else {
                        String::new()
                    }
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFAbsoluteTimeGetCurrent" => {
                let bits = self.guest_cf_absolute_time_bits();
                let detail = format!(
                    "hle CFAbsoluteTimeGetCurrent() -> {:.6}",
                    f64::from_bits(bits)
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = bits as u32;
                self.cpu.regs[1] = (bits >> 32) as u32;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "usleep" => {
                let usec = self.cpu.regs[0];
                self.advance_guest_wallclock_micros(usec as u64);
                let detail = format!(
                    "hle usleep(usec={}) -> 0 (guest clock advanced, host sleep skipped)",
                    usec
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "exit" => {
                let code = self.cpu.regs[0];
                if self.runtime.ui_runtime.runloop_live && self.runtime.ui_runtime.launched && code == 0 {
                    return Ok(Some(self.suppress_exit_into_runloop(index, current_pc, &label, code)));
                }
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &format!("hle exit(code={code})")));
                return Ok(Some(StepControl::Stop(format!("process requested exit({code})"))));
            }
            "UIApplicationMain" => {
                self.install_uikit_labels();
                self.runtime.ui_runtime.launch_count = self.runtime.ui_runtime.launch_count.saturating_add(1);
                self.runtime.ui_runtime.delegate_set = true;
                if self.cpu.regs[3] != 0 {
                    self.runtime.ui_objects.delegate = self.cpu.regs[3];
                    self.assign_network_delegate_with_provenance(
                        "UIApplicationMain",
                        self.runtime.ui_objects.app,
                        self.runtime.ui_network.network_request,
                        self.cpu.regs[3],
                        "UIApplicationMain delegate parameter",
                    );
                    self.diag.object_labels
                        .entry(self.cpu.regs[3])
                        .or_insert_with(|| "UIApplication.delegate(instance)".to_string());
                }
                let argv0 = self.read_argv0(self.cpu.regs[1]).unwrap_or_else(|| "<unknown>".to_string());
                let principal = self.guest_string_value(self.cpu.regs[2]).unwrap_or_else(|| self.describe_ptr(self.cpu.regs[2]));
                let delegate_desc = self.guest_string_value(self.cpu.regs[3]).unwrap_or_else(|| self.describe_ptr(self.cpu.regs[3]));
                let detail = format!(
                    "hle UIApplicationMain(argc={}, argv=0x{:08x}, argv0='{}', principal={}, delegate={})",
                    self.cpu.regs[0], self.cpu.regs[1], argv0, principal, delegate_desc
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                if let Some(bridge) = self.prepare_real_uimain_bridge(self.cpu.regs[2], self.cpu.regs[3]) {
                    self.runtime.ui_runtime.launched = true;
                    self.runtime.ui_objects.delegate = bridge.receiver;
                    self.assign_network_delegate_with_provenance(
                        "UIApplicationMain.bridge",
                        self.runtime.ui_objects.app,
                        self.runtime.ui_network.network_request,
                        bridge.receiver,
                        "UIApplicationMain real bridge receiver",
                    );
                    self.diag.object_labels
                        .entry(bridge.receiver)
                        .or_insert_with(|| format!("UIApplication.delegate(instance)<{}>", bridge.delegate_class_name));
                    self.runtime.objc.objc_bridge_resume_lr = Some(self.cpu.regs[14]);
                    self.diag.trace.push(format!(
                        "     ↳ objc bridge delegate={} class={} selector={} imp=0x{:08x} return_stub=0x{:08x}",
                        bridge.delegate_name.clone().unwrap_or_else(|| self.describe_ptr(bridge.receiver)),
                        bridge.delegate_class_name,
                        bridge.selector_name,
                        bridge.imp,
                        bridge.return_stub,
                    ));
                    self.cpu.regs[0] = bridge.receiver;
                    self.cpu.regs[1] = bridge.selector_ptr;
                    self.cpu.regs[2] = self.runtime.ui_objects.app;
                    self.cpu.regs[3] = 0;
                    self.cpu.regs[14] = bridge.return_stub;
                    self.cpu.regs[15] = bridge.imp & !1;
                    self.cpu.thumb = (bridge.imp & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }
                self.runtime.ui_runtime.launched = true;
                self.bootstrap_synthetic_runloop();
                self.diag.trace.push(format!(
                    "     ↳ ui bootstrap app={} delegate={} window={} root={} launch#{}",
                    self.describe_ptr(self.runtime.ui_objects.app),
                    self.describe_ptr(self.runtime.ui_objects.delegate),
                    self.describe_ptr(self.runtime.ui_objects.window),
                    self.describe_ptr(self.runtime.ui_objects.root_controller),
                    self.runtime.ui_runtime.launch_count,
                ));
                self.diag.trace.push(format!(
                    "     ↳ ui activation window={} firstResponder={} state=active",
                    self.describe_ptr(self.runtime.ui_objects.window),
                    self.describe_ptr(self.runtime.ui_objects.first_responder),
                ));
                self.diag.trace.push(format!(
                    "     ↳ runloop bootstrap main={} mode={} sources={} timer={} displayLink={}",
                    self.describe_ptr(self.runtime.ui_objects.main_runloop),
                    self.describe_ptr(self.runtime.ui_objects.default_mode),
                    self.runtime.ui_runtime.runloop_sources,
                    self.describe_ptr(self.runtime.ui_objects.synthetic_timer),
                    self.describe_ptr(self.runtime.ui_cocos.synthetic_display_link),
                ));
                self.diag.trace.push(format!(
                    "     ↳ network bootstrap reachability={} conn={} request={} proxy={} url='{}' host='{}' method={}",
                    self.describe_ptr(self.runtime.ui_network.reachability),
                    self.describe_ptr(self.runtime.ui_network.network_connection),
                    self.describe_ptr(self.runtime.ui_network.network_request),
                    self.describe_ptr(self.runtime.ui_network.proxy_settings),
                    self.network_url_string(),
                    self.network_host_string(),
                    self.network_http_method(),
                ));
                self.diag.trace.push("     ↳ ui lifecycle application:didFinishLaunchingWithOptions: => YES (synthetic fallback)".to_string());
                self.diag.trace.push("     ↳ ui lifecycle applicationDidBecomeActive: => synthetic".to_string());
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "__audioqueue_callback_return_arm" | "__audioqueue_callback_return_thumb" => {
                let detail = format!(
                    "hle {} audioqueue-callback-return(r0=0x{:08x}, pending={})",
                    label,
                    self.cpu.regs[0],
                    self.runtime
                        .audio_queue
                        .callback_resume
                        .as_ref()
                        .map(|resume| resume.pending.len())
                        .unwrap_or(0),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.audioqueue_resume_after_callback_return()?;
                return Ok(Some(StepControl::Continue));
            }
            "__uimain_post_launch_arm" | "__uimain_post_launch_thumb" => {
                let detail = format!(
                    "hle {} return-to-UIApplicationMain(result=0x{:08x}, resume_lr=0x{:08x})",
                    label,
                    self.cpu.regs[0],
                    self.runtime.objc.objc_bridge_resume_lr.unwrap_or(0),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.finish_real_uimain_bridge();
                return Ok(Some(StepControl::Continue));
            }
            "class_createInstance" | "NSAllocateObject" => {
                let class_ptr = self.cpu.regs[0];
                let extra_bytes = self.cpu.regs[1];
                if label == "class_createInstance" {
                    self.runtime.objc.objc_class_create_instance_calls = self.runtime.objc.objc_class_create_instance_calls.saturating_add(1);
                } else {
                    self.runtime.objc.objc_alloc_with_zone_calls = self.runtime.objc.objc_alloc_with_zone_calls.saturating_add(1);
                }
                let ptr = self.objc_hle_alloc_like(class_ptr, extra_bytes, &label);
                let detail = format!(
                    "hle {}(class={}, extraBytes={}, zone={}) -> {} classHint={}",
                    label,
                    self.describe_ptr(class_ptr),
                    extra_bytes,
                    self.describe_ptr(self.cpu.regs[2]),
                    self.describe_ptr(ptr),
                    self.runtime.objc.objc_last_alloc_class.clone().unwrap_or_else(|| "<unknown>".to_string()),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = ptr;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "malloc" | "Znwm" | "Znam" => {
                let size = self.cpu.regs[0];
                let ptr = self.handle_guest_malloc(size);
                let detail = if ptr != 0 {
                    format!("hle {}(size={}) -> {}", label, size, self.describe_ptr(ptr))
                } else {
                    format!("hle {}(size={}) -> nil", label, size)
                };
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                if ptr == 0 {
                    return Ok(Some(StepControl::Stop(format!(
                        "{} failed to allocate {} bytes: {}",
                        label,
                        size,
                        self.runtime
                            .heap
                            .synthetic_heap_last_error
                            .clone()
                            .unwrap_or_else(|| "guest synthetic heap allocation failed".to_string())
                    ))));
                }
                self.cpu.regs[0] = ptr;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "calloc" => {
                let count = self.cpu.regs[0];
                let size = self.cpu.regs[1];
                let ptr = self.handle_guest_calloc(count, size);
                let detail = if ptr != 0 {
                    format!("hle calloc(count={}, size={}) -> {}", count, size, self.describe_ptr(ptr))
                } else {
                    format!("hle calloc(count={}, size={}) -> nil", count, size)
                };
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = ptr;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "realloc" => {
                let old_ptr = self.cpu.regs[0];
                let size = self.cpu.regs[1];
                let ptr = self.handle_guest_realloc(old_ptr, size);
                let detail = if ptr != 0 {
                    format!("hle realloc(ptr={}, size={}) -> {}", self.describe_ptr(old_ptr), size, self.describe_ptr(ptr))
                } else {
                    format!("hle realloc(ptr={}, size={}) -> nil", self.describe_ptr(old_ptr), size)
                };
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = ptr;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "free" | "ZdlPv" | "ZdaPv" => {
                let ptr = self.cpu.regs[0];
                let ok = self.free_synthetic_heap_block(ptr);
                let detail = format!("hle {}(ptr={}) -> {}", label, self.describe_ptr(ptr), if ok { 0 } else { -1 });
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFRetain" => {
                let value = self.cpu.regs[0];
                let detail = format!("hle CFRetain({}) -> {}", self.describe_ptr(value), self.describe_ptr(value));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = value;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFRelease" => {
                let value = self.cpu.regs[0];
                let detail = format!("hle CFRelease({})", self.describe_ptr(value));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFURLCreateFromFileSystemRepresentation" => {
                let allocator = self.cpu.regs[0];
                let buffer_ptr = self.cpu.regs[1];
                let buf_len = self.cpu.regs[2];
                let is_directory = self.cpu.regs[3] != 0;
                let result = match self.create_synthetic_file_url_from_fs_representation(buffer_ptr, buf_len, is_directory) {
                    Ok(obj) => obj,
                    Err(err) => {
                        self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &format!(
                            "hle CFURLCreateFromFileSystemRepresentation(allocator={}, buffer={}, len={}, isDirectory={}) failed: {}",
                            self.describe_ptr(allocator),
                            self.describe_ptr(buffer_ptr),
                            buf_len,
                            if is_directory { "YES" } else { "NO" },
                            err
                        )));
                        0
                    }
                };
                let path = if buffer_ptr != 0 && buf_len != 0 {
                    self.read_guest_bytes(buffer_ptr, buf_len)
                        .ok()
                        .map(|mut raw| {
                            if let Some(nul) = raw.iter().position(|b| *b == 0) {
                                raw.truncate(nul);
                            }
                            String::from_utf8_lossy(&raw).to_string()
                        })
                        .unwrap_or_else(|| self.describe_ptr(buffer_ptr))
                } else {
                    String::new()
                };
                let url_debug = self.url_like_debug_summary(result, is_directory);
                let detail = format!(
                    "hle CFURLCreateFromFileSystemRepresentation(allocator={}, buffer={}, len={}, isDirectory={}) -> {} path='{}' {}",
                    self.describe_ptr(allocator),
                    self.describe_ptr(buffer_ptr),
                    buf_len,
                    if is_directory { "YES" } else { "NO" },
                    self.describe_ptr(result),
                    path.replace('\n', "\\n"),
                    url_debug
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = result;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFURLCopyPathExtension" => {
                let url = self.cpu.regs[0];
                let result = if let Some(text) = self.synthetic_file_url_path_extension_value(url) {
                    self.materialize_host_string_object("NSString.CFURL.pathExtension", &text)
                } else if let Some(text) = self.guest_string_value(url) {
                    let ext = std::path::Path::new(text.trim()).extension().and_then(|v| v.to_str()).unwrap_or_default().to_string();
                    self.materialize_host_string_object("NSString.CFURL.pathExtension", &ext)
                } else {
                    0
                };
                let detail = format!("hle CFURLCopyPathExtension(url={}) -> {}", self.describe_ptr(url), self.describe_ptr(result));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = result;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "NSSearchPathForDirectoriesInDomains" => {
                let directory = self.cpu.regs[0];
                let domain_mask = self.cpu.regs[1];
                let expand_tilde = self.cpu.regs[2];
                let lr = self.cpu.regs[14];
                let sp = self.cpu.regs[13];
                let directory_name = match directory {
                    1 => "NSApplicationDirectory",
                    2 => "NSDemoApplicationDirectory",
                    3 => "NSDeveloperApplicationDirectory",
                    4 => "NSAdminApplicationDirectory",
                    5 => "NSLibraryDirectory",
                    7 => "NSUserDirectory",
                    8 => "NSDocumentationDirectory",
                    9 => "NSDocumentDirectory",
                    10 => "NSCoreServiceDirectory",
                    11 => "NSAutosavedInformationDirectory",
                    12 => "NSDesktopDirectory",
                    13 => "NSCachesDirectory",
                    14 => "NSApplicationSupportDirectory",
                    15 => "NSDownloadsDirectory",
                    16 => "NSInputMethodsDirectory",
                    17 => "NSMoviesDirectory",
                    18 => "NSMusicDirectory",
                    19 => "NSPicturesDirectory",
                    20 => "NSPrinterDescriptionDirectory",
                    21 => "NSSharedPublicDirectory",
                    22 => "NSPreferencePanesDirectory",
                    33 => "NSItemReplacementDirectory",
                    99 => "NSAllApplicationsDirectory",
                    100 => "NSAllLibrariesDirectory",
                    101 => "NSTrashDirectory",
                    _ => "<unknown>",
                };
                let domain_name = match domain_mask {
                    1 => "NSUserDomainMask",
                    2 => "NSLocalDomainMask",
                    4 => "NSNetworkDomainMask",
                    8 => "NSSystemDomainMask",
                    0xffff => "NSAllDomainsMask",
                    _ => "<unknown>",
                };
                let result = self.hle_search_path_array(directory, domain_mask, expand_tilde);
                let first = self.synthetic_array_get(result, 0);
                let first_text = self.guest_string_value(first).unwrap_or_default();
                let detail = format!(
                    "hle NSSearchPathForDirectoriesInDomains(dir={}={}, domain={}={}, expand={}, lr=0x{:08x}, sp=0x{:08x}) -> {} first={} text='{}'",
                    directory,
                    directory_name,
                    domain_mask,
                    domain_name,
                    expand_tilde,
                    lr,
                    sp,
                    self.describe_ptr(result),
                    self.describe_ptr(first),
                    first_text.replace('\n', "\\n")
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = result;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "NSHomeDirectory" => {
                let path = self.sandbox_home_path().unwrap_or_else(|| std::path::PathBuf::from("sandbox"));
                Self::ensure_host_directory_exists(&path);
                let result = self.materialize_host_path_object("NSString.NSHomeDirectory", &path);
                let detail = format!("hle NSHomeDirectory() -> {} path='{}'", self.describe_ptr(result), path.display());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = result;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "NSTemporaryDirectory" => {
                let path = self.sandbox_tmp_path().unwrap_or_else(|| std::path::PathBuf::from("sandbox/tmp"));
                Self::ensure_host_directory_exists(&path);
                let result = self.materialize_host_path_object("NSString.NSTemporaryDirectory", &path);
                let detail = format!("hle NSTemporaryDirectory() -> {} path='{}'", self.describe_ptr(result), path.display());
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = result;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CGDataProviderCreateWithURL" => {
                let url = self.cpu.regs[0];
                let result = self.create_synthetic_data_provider_from_url(url).unwrap_or(0);
                let detail = format!("hle CGDataProviderCreateWithURL(url={}) -> {}", self.describe_ptr(url), self.describe_ptr(result));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = result;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioFileOpenURL" | "ExtAudioFileOpenURL" => {
                const NO_ERR: u32 = 0;
                const FNF_ERR: u32 = (-43i32) as u32;
                self.runtime.audio_trace.audiofile_open_calls = self.runtime.audio_trace.audiofile_open_calls.saturating_add(1);
                let url = self.cpu.regs[0];
                let out_ptr = self.cpu.regs[3];
                let out_before = if out_ptr != 0 { self.read_u32_le(out_ptr).ok() } else { None };
                let url_debug = self.url_like_debug_summary(url, false);
                let handle = self.open_audio_file_from_url(url).unwrap_or(0);
                if out_ptr != 0 {
                    self.write_u32_le(out_ptr, handle)?;
                }
                let out_after = if out_ptr != 0 { self.read_u32_le(out_ptr).ok() } else { None };
                let status = if handle != 0 { NO_ERR } else { FNF_ERR };
                let detail = format!(
                    "hle {}(url={}, out={}) -> status={} handle={} out_before={} out_after={} {}",
                    label,
                    self.describe_ptr(url),
                    self.describe_ptr(out_ptr),
                    status as i32,
                    self.describe_ptr(handle),
                    out_before
                        .map(|value| self.describe_ptr(value))
                        .unwrap_or_else(|| "<unreadable>".to_string()),
                    out_after
                        .map(|value| self.describe_ptr(value))
                        .unwrap_or_else(|| "<unreadable>".to_string()),
                    url_debug
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.audio_trace_push_event(format!(
                    "audiofile.open label={} url={} handle={} status={} {}",
                    label,
                    self.describe_ptr(url),
                    self.describe_ptr(handle),
                    status as i32,
                    url_debug
                ));
                self.cpu.regs[0] = status;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioFileClose" => {
                const NO_ERR: u32 = 0;
                const PARAM_ERR: u32 = (-50i32) as u32;
                let file_id = self.cpu.regs[0];
                let closed = self.runtime.fs.host_files.remove(&file_id).is_some();
                self.runtime.fs.synthetic_audio_files.remove(&file_id);
                let status = if closed { NO_ERR } else { PARAM_ERR };
                let detail = format!("hle AudioFileClose(file={}) -> {}", self.describe_ptr(file_id), status as i32);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = status;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioFileReadBytes" => {
                const NO_ERR: u32 = 0;
                const PARAM_ERR: u32 = (-50i32) as u32;
                self.runtime.audio_trace.audiofile_read_bytes_calls = self.runtime.audio_trace.audiofile_read_bytes_calls.saturating_add(1);
                let file_id = self.cpu.regs[0];
                let use_cache = self.cpu.regs[1];
                let starting_lo = self.cpu.regs[2] as u64;
                let starting_hi = self.cpu.regs[3] as u64;
                let starting_byte = ((starting_hi << 32) | starting_lo) as usize;
                let io_num_bytes_ptr = self.peek_stack_u32(0).unwrap_or(0);
                let out_buffer_ptr = self.peek_stack_u32(1).unwrap_or(0);
                let requested = if io_num_bytes_ptr != 0 { self.read_u32_le(io_num_bytes_ptr).unwrap_or(0) as usize } else { 0 };
                let (status, actual) = if let Some(file) = self.runtime.fs.host_files.get(&file_id) {
                    let (chunk, actual_len) = {
                        let start = starting_byte.min(file.data.len());
                        let end = start.saturating_add(requested).min(file.data.len());
                        let chunk = file.data[start..end].to_vec();
                        let actual_len = chunk.len();
                        (chunk, actual_len)
                    };
                    if out_buffer_ptr != 0 && !chunk.is_empty() {
                        if self.find_region(out_buffer_ptr, chunk.len() as u32).is_none() {
                            let sp0 = self.peek_stack_u32(0).unwrap_or(0);
                            let sp1 = self.peek_stack_u32(1).unwrap_or(0);
                            let sp2 = self.peek_stack_u32(2).unwrap_or(0);
                            let sp3 = self.peek_stack_u32(3).unwrap_or(0);
                            return Err(CoreError::Backend(format!(
                                "AudioFileReadBytes outBuffer is unmapped: out=0x{out_buffer_ptr:08x} bytes={} sp=0x{:08x} stack[0]=0x{sp0:08x} stack[1]=0x{sp1:08x} stack[2]=0x{sp2:08x} stack[3]=0x{sp3:08x} ioNumBytes=0x{io_num_bytes_ptr:08x}",
                                chunk.len(),
                                self.cpu.regs[13],
                            )));
                        }
                        self.write_bytes(out_buffer_ptr, &chunk)?;
                    }
                    if io_num_bytes_ptr != 0 {
                        self.write_u32_le(io_num_bytes_ptr, actual_len.min(u32::MAX as usize) as u32)?;
                    }
                    (NO_ERR, actual_len)
                } else {
                    if io_num_bytes_ptr != 0 {
                        self.write_u32_le(io_num_bytes_ptr, 0)?;
                    }
                    (PARAM_ERR, 0)
                };
                let detail = format!(
                    "hle AudioFileReadBytes(file={}, useCache={}, start={}, ioBytes={}, out={}) -> status={} actual={}",
                    self.describe_ptr(file_id),
                    use_cache,
                    starting_byte,
                    self.describe_ptr(io_num_bytes_ptr),
                    self.describe_ptr(out_buffer_ptr),
                    status as i32,
                    actual
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.runtime.audio_trace.audiofile_bytes_served = self.runtime.audio_trace.audiofile_bytes_served.saturating_add(actual as u64);
                self.audio_trace_push_event(format!("audiofile.read_bytes file={} start={} requested={} actual={} out={}", self.describe_ptr(file_id), starting_byte, requested, actual, self.describe_ptr(out_buffer_ptr)));
                self.cpu.regs[0] = status;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioFileGetPropertyInfo" => {
                const NO_ERR: u32 = 0;
                const PARAM_ERR: u32 = (-50i32) as u32;
                let file_id = self.cpu.regs[0];
                let property_id = self.cpu.regs[1];
                let out_size_ptr = self.cpu.regs[2];
                let is_writable_ptr = self.cpu.regs[3];
                let state = self.runtime.fs.synthetic_audio_files.get(&file_id).cloned();
                let (status, size, property_name) = if let Some(state) = state {
                    let size = Self::synthetic_audio_file_property_size(property_id, &state.metadata).unwrap_or(0);
                    if out_size_ptr != 0 {
                        self.write_u32_le(out_size_ptr, size)?;
                    }
                    if is_writable_ptr != 0 {
                        self.write_u32_le(is_writable_ptr, 0)?;
                    }
                    (NO_ERR, size, Self::synthetic_audio_file_property_name(property_id))
                } else {
                    if out_size_ptr != 0 {
                        self.write_u32_le(out_size_ptr, 0)?;
                    }
                    if is_writable_ptr != 0 {
                        self.write_u32_le(is_writable_ptr, 0)?;
                    }
                    (PARAM_ERR, 0, Self::synthetic_audio_file_property_name(property_id))
                };
                let detail = format!(
                    "hle AudioFileGetPropertyInfo(file={}, {}, outSize={}, isWritable={}) -> status={} size={}",
                    self.describe_ptr(file_id),
                    property_name,
                    self.describe_ptr(out_size_ptr),
                    self.describe_ptr(is_writable_ptr),
                    status as i32,
                    size
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = status;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioFileGetProperty" => {
                const NO_ERR: u32 = 0;
                const PARAM_ERR: u32 = (-50i32) as u32;
                let file_id = self.cpu.regs[0];
                let property_id = self.cpu.regs[1];
                let io_size_ptr = self.cpu.regs[2];
                let out_ptr = self.cpu.regs[3];
                let requested_size = if io_size_ptr != 0 { self.read_u32_le(io_size_ptr).unwrap_or(0) } else { 0 };
                let state = self.runtime.fs.synthetic_audio_files.get(&file_id).cloned();
                let (status, actual_size, property_name) = if let Some(state) = state {
                    if let Some((payload, property_name)) = self.synthetic_audio_file_property_payload(property_id, &state.metadata, requested_size) {
                        let actual_size = payload.len().min(u32::MAX as usize) as u32;
                        if out_ptr != 0 && !payload.is_empty() {
                            self.write_bytes(out_ptr, &payload)?;
                        }
                        if io_size_ptr != 0 {
                            self.write_u32_le(io_size_ptr, actual_size)?;
                        }
                        (NO_ERR, actual_size, property_name)
                    } else {
                        if io_size_ptr != 0 {
                            self.write_u32_le(io_size_ptr, 0)?;
                        }
                        (PARAM_ERR, 0, Self::synthetic_audio_file_property_name(property_id))
                    }
                } else {
                    if io_size_ptr != 0 {
                        self.write_u32_le(io_size_ptr, 0)?;
                    }
                    (PARAM_ERR, 0, Self::synthetic_audio_file_property_name(property_id))
                };
                let detail = format!(
                    "hle AudioFileGetProperty(file={}, {}, ioSize={}, out={}) -> status={} size={}",
                    self.describe_ptr(file_id),
                    property_name,
                    self.describe_ptr(io_size_ptr),
                    self.describe_ptr(out_ptr),
                    status as i32,
                    actual_size
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = status;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioFileReadPackets" => {
                const NO_ERR: u32 = 0;
                const PARAM_ERR: u32 = (-50i32) as u32;
                self.runtime.audio_trace.audiofile_read_packets_calls = self.runtime.audio_trace.audiofile_read_packets_calls.saturating_add(1);
                let file_id = self.cpu.regs[0];
                let use_cache = self.cpu.regs[1];
                let out_num_bytes_ptr = self.cpu.regs[2];
                let packet_descs_ptr = self.cpu.regs[3];
                let starting_packet = self.read_u64_le(self.cpu.regs[13]).unwrap_or(0) as usize;
                let io_num_packets_ptr = self.peek_stack_u32(2).unwrap_or(0);
                let out_buffer_ptr = self.peek_stack_u32(3).unwrap_or(0);
                let requested_packets = if io_num_packets_ptr != 0 {
                    self.read_u32_le(io_num_packets_ptr).unwrap_or(0) as usize
                } else {
                    0
                };
                let state = self.runtime.fs.synthetic_audio_files.get(&file_id).cloned();
                let file_data = self.runtime.fs.host_files.get(&file_id).map(|file| file.data.clone());
                let (status, actual_packets, actual_bytes) = if let (Some(state), Some(file_data)) = (state, file_data) {
                    if !state.packet_table.is_empty() {
                        let start_index = starting_packet.min(state.packet_table.len());
                        let end_index = start_index.saturating_add(requested_packets).min(state.packet_table.len());
                        let mut packet_offset = 0u64;
                        let mut actual_packets = 0usize;
                        let mut chunk = Vec::new();
                        for entry in &state.packet_table[start_index..end_index] {
                            let start = entry.file_offset as usize;
                            let end = start.saturating_add(entry.byte_count as usize).min(file_data.len());
                            if start >= end {
                                break;
                            }
                            let packet_bytes = &file_data[start..end];
                            chunk.extend_from_slice(packet_bytes);
                            if packet_descs_ptr != 0 {
                                let desc = packet_descs_ptr.wrapping_add((actual_packets as u32).saturating_mul(16));
                                self.write_u64_le(desc, packet_offset)?;
                                self.write_u32_le(desc.wrapping_add(8), 0)?;
                                self.write_u32_le(desc.wrapping_add(12), packet_bytes.len().min(u32::MAX as usize) as u32)?;
                            }
                            packet_offset = packet_offset.saturating_add(packet_bytes.len() as u64);
                            actual_packets = actual_packets.saturating_add(1);
                        }
                        let actual_bytes = chunk.len();
                        if out_buffer_ptr != 0 && !chunk.is_empty() {
                            self.write_bytes(out_buffer_ptr, &chunk)?;
                        }
                        if out_num_bytes_ptr != 0 {
                            self.write_u32_le(out_num_bytes_ptr, actual_bytes.min(u32::MAX as usize) as u32)?;
                        }
                        if io_num_packets_ptr != 0 {
                            self.write_u32_le(io_num_packets_ptr, actual_packets.min(u32::MAX as usize) as u32)?;
                        }
                        (NO_ERR, actual_packets, actual_bytes)
                    } else {
                        let bytes_per_packet = state.metadata.bytes_per_packet.max(1) as usize;
                        let base_offset = state.metadata.audio_data_offset as usize;
                        let start = base_offset.saturating_add(starting_packet.saturating_mul(bytes_per_packet)).min(file_data.len());
                        let max_bytes = requested_packets.saturating_mul(bytes_per_packet);
                        let end = start.saturating_add(max_bytes).min(file_data.len());
                        let chunk = file_data[start..end].to_vec();
                        let actual_bytes = chunk.len();
                        let actual_packets = if bytes_per_packet == 0 { 0 } else { actual_bytes / bytes_per_packet };
                        if out_buffer_ptr != 0 && !chunk.is_empty() {
                            self.write_bytes(out_buffer_ptr, &chunk)?;
                        }
                        if out_num_bytes_ptr != 0 {
                            self.write_u32_le(out_num_bytes_ptr, actual_bytes.min(u32::MAX as usize) as u32)?;
                        }
                        if io_num_packets_ptr != 0 {
                            self.write_u32_le(io_num_packets_ptr, actual_packets.min(u32::MAX as usize) as u32)?;
                        }
                        if packet_descs_ptr != 0 {
                            for i in 0..actual_packets {
                                let desc = packet_descs_ptr.wrapping_add((i as u32).saturating_mul(16));
                                self.write_u64_le(desc, (i.saturating_mul(bytes_per_packet)) as u64)?;
                                self.write_u32_le(desc.wrapping_add(8), 0)?;
                                self.write_u32_le(desc.wrapping_add(12), bytes_per_packet.min(u32::MAX as usize) as u32)?;
                            }
                        }
                        (NO_ERR, actual_packets, actual_bytes)
                    }
                } else {
                    if out_num_bytes_ptr != 0 {
                        self.write_u32_le(out_num_bytes_ptr, 0)?;
                    }
                    if io_num_packets_ptr != 0 {
                        self.write_u32_le(io_num_packets_ptr, 0)?;
                    }
                    (PARAM_ERR, 0, 0)
                };
                let detail = format!(
                    "hle AudioFileReadPackets(file={}, useCache={}, startPacket={}, ioPackets={}, outBytes={}, packetDescs={}, out={}) -> status={} packets={} bytes={}",
                    self.describe_ptr(file_id),
                    use_cache,
                    starting_packet,
                    self.describe_ptr(io_num_packets_ptr),
                    self.describe_ptr(out_num_bytes_ptr),
                    self.describe_ptr(packet_descs_ptr),
                    self.describe_ptr(out_buffer_ptr),
                    status as i32,
                    actual_packets,
                    actual_bytes
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.runtime.audio_trace.audiofile_bytes_served = self.runtime.audio_trace.audiofile_bytes_served.saturating_add(actual_bytes as u64);
                self.audio_trace_push_event(format!("audiofile.read_packets file={} startPacket={} requestedPackets={} actualPackets={} bytes={} out={}", self.describe_ptr(file_id), starting_packet, requested_packets, actual_packets, actual_bytes, self.describe_ptr(out_buffer_ptr)));
                self.cpu.regs[0] = status;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioQueueNewOutput" => {
                let format_ptr = self.cpu.regs[0];
                let callback_ptr = self.cpu.regs[1];
                let user_data_ptr = self.cpu.regs[2];
                let callback_runloop = self.cpu.regs[3];
                let callback_runloop_mode = self.peek_stack_u32(0).unwrap_or(0);
                let flags = self.peek_stack_u32(1).unwrap_or(0);
                let out_queue_ptr = self.peek_stack_u32(2).unwrap_or(0);
                let result = self.audioqueue_create_output(
                    format_ptr,
                    callback_ptr,
                    user_data_ptr,
                    callback_runloop,
                    callback_runloop_mode,
                    flags,
                );
                let (status, queue_ptr, fmt_summary) = match result {
                    Ok(queue_ptr) => {
                        if out_queue_ptr != 0 {
                            self.write_u32_le(out_queue_ptr, queue_ptr)?;
                        }
                        let fmt = self
                            .runtime
                            .audio_queue
                            .queues
                            .get(&queue_ptr)
                            .and_then(|queue| queue.format.as_ref())
                            .map(|format| self.audioqueue_format_summary(Some(format)))
                            .unwrap_or_else(|| "<unknown-asbd>".to_string());
                        (AUDIOQUEUE_NO_ERR, queue_ptr, fmt)
                    }
                    Err(err) => {
                        if out_queue_ptr != 0 {
                            self.write_u32_le(out_queue_ptr, 0)?;
                        }
                        self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &format!("AudioQueueNewOutput materialization failed: {}", err)));
                        (AUDIOQUEUE_PARAM_ERR, 0, "<alloc-failed>".to_string())
                    }
                };
                let detail = format!(
                    "hle AudioQueueNewOutput(format={}, callback=0x{:08x}, userData={}, runLoop={}, mode={}, flags=0x{:08x}, outQueue={}) -> status={} queue={} format={}",
                    self.describe_ptr(format_ptr),
                    callback_ptr,
                    self.describe_ptr(user_data_ptr),
                    self.describe_ptr(callback_runloop),
                    self.describe_ptr(callback_runloop_mode),
                    flags,
                    self.describe_ptr(out_queue_ptr),
                    status as i32,
                    self.describe_ptr(queue_ptr),
                    fmt_summary,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = status;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioQueueAllocateBuffer" => {
                let queue_ptr = self.cpu.regs[0];
                let capacity = self.cpu.regs[1];
                let out_buffer_ptr = self.cpu.regs[2];
                let result = self.audioqueue_allocate_buffer(queue_ptr, capacity);
                let (status, buffer_ptr) = match result {
                    Ok(buffer_ptr) => {
                        if out_buffer_ptr != 0 {
                            self.write_u32_le(out_buffer_ptr, buffer_ptr)?;
                        }
                        (AUDIOQUEUE_NO_ERR, buffer_ptr)
                    }
                    Err(err) => {
                        if out_buffer_ptr != 0 {
                            self.write_u32_le(out_buffer_ptr, 0)?;
                        }
                        self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &format!("AudioQueueAllocateBuffer failed: {}", err)));
                        (AUDIOQUEUE_PARAM_ERR, 0)
                    }
                };
                let (audio_data_ptr, audio_capacity) = self
                    .runtime
                    .audio_queue
                    .buffers
                    .get(&buffer_ptr)
                    .map(|buffer| (buffer.audio_data_ptr, buffer.audio_data_capacity))
                    .unwrap_or((0, 0));
                let detail = format!(
                    "hle AudioQueueAllocateBuffer(queue={}, capacity={}, outBuffer={}) -> status={} buffer={} audioData={} audioCapacity={}",
                    self.describe_ptr(queue_ptr),
                    capacity,
                    self.describe_ptr(out_buffer_ptr),
                    status as i32,
                    self.describe_ptr(buffer_ptr),
                    self.describe_ptr(audio_data_ptr),
                    audio_capacity,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = status;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioQueueFreeBuffer" => {
                let queue_ptr = self.cpu.regs[0];
                let buffer_ptr = self.cpu.regs[1];
                let ok = self.audioqueue_free_buffer(queue_ptr, buffer_ptr);
                let detail = format!(
                    "hle AudioQueueFreeBuffer(queue={}, buffer={}) -> status={}",
                    self.describe_ptr(queue_ptr),
                    self.describe_ptr(buffer_ptr),
                    if ok { 0 } else { -50 },
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = if ok { AUDIOQUEUE_NO_ERR } else { AUDIOQUEUE_PARAM_ERR };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioQueueEnqueueBuffer" => {
                let queue_ptr = self.cpu.regs[0];
                let buffer_ptr = self.cpu.regs[1];
                let packet_desc_count = self.cpu.regs[2];
                let packet_descs_ptr = self.cpu.regs[3];
                let result = self.audioqueue_enqueue_buffer(queue_ptr, buffer_ptr, packet_desc_count, packet_descs_ptr);
                let (status, byte_size, capacity) = match result {
                    Ok((byte_size, capacity)) => (AUDIOQUEUE_NO_ERR, byte_size, capacity),
                    Err(err) => {
                        self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &format!("AudioQueueEnqueueBuffer failed: {}", err)));
                        (AUDIOQUEUE_PARAM_ERR, 0, 0)
                    }
                };
                let detail = format!(
                    "hle AudioQueueEnqueueBuffer(queue={}, buffer={}, packetDescCount={}, packetDescs={}) -> status={} byteSize={} capacity={}",
                    self.describe_ptr(queue_ptr),
                    self.describe_ptr(buffer_ptr),
                    packet_desc_count,
                    self.describe_ptr(packet_descs_ptr),
                    status as i32,
                    byte_size,
                    capacity,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = status;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioQueueAddPropertyListener" => {
                let queue_ptr = self.cpu.regs[0];
                let property_id = self.cpu.regs[1];
                let callback_ptr = self.cpu.regs[2];
                let user_data_ptr = self.cpu.regs[3];
                let ok = self.audioqueue_add_property_listener(queue_ptr, property_id, callback_ptr, user_data_ptr);
                let detail = format!(
                    "hle AudioQueueAddPropertyListener(queue={}, property=0x{:08x}, callback=0x{:08x}, userData={}) -> status={}",
                    self.describe_ptr(queue_ptr),
                    property_id,
                    callback_ptr,
                    self.describe_ptr(user_data_ptr),
                    if ok { 0 } else { -50 },
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = if ok { AUDIOQUEUE_NO_ERR } else { AUDIOQUEUE_PARAM_ERR };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioQueueSetParameter" => {
                let queue_ptr = self.cpu.regs[0];
                let parameter_id = self.cpu.regs[1];
                let value = f32::from_bits(self.cpu.regs[2]);
                let ok = self.audioqueue_set_parameter(queue_ptr, parameter_id, value);
                let detail = format!(
                    "hle AudioQueueSetParameter(queue={}, param=0x{:08x}, value={:.4}) -> status={}",
                    self.describe_ptr(queue_ptr),
                    parameter_id,
                    value,
                    if ok { 0 } else { -50 },
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = if ok { AUDIOQUEUE_NO_ERR } else { AUDIOQUEUE_PARAM_ERR };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioQueueSetProperty" => {
                let queue_ptr = self.cpu.regs[0];
                let property_id = self.cpu.regs[1];
                let data_ptr = self.cpu.regs[2];
                let data_size = self.cpu.regs[3];
                let ok = self.audioqueue_set_property(queue_ptr, property_id, data_ptr, data_size)?;
                let detail = format!(
                    "hle AudioQueueSetProperty(queue={}, property=0x{:08x}, data={}, dataSize={}) -> status={}",
                    self.describe_ptr(queue_ptr),
                    property_id,
                    self.describe_ptr(data_ptr),
                    data_size,
                    if ok { 0 } else { -50 },
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = if ok { AUDIOQUEUE_NO_ERR } else { AUDIOQUEUE_PARAM_ERR };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioQueueGetProperty" => {
                let queue_ptr = self.cpu.regs[0];
                let property_id = self.cpu.regs[1];
                let out_data_ptr = self.cpu.regs[2];
                let io_size_ptr = self.cpu.regs[3];
                let (ok, actual_size) = self.audioqueue_get_property(queue_ptr, property_id, out_data_ptr, io_size_ptr)?;
                let detail = format!(
                    "hle AudioQueueGetProperty(queue={}, property=0x{:08x}, outData={}, ioSize={}) -> status={} size={}",
                    self.describe_ptr(queue_ptr),
                    property_id,
                    self.describe_ptr(out_data_ptr),
                    self.describe_ptr(io_size_ptr),
                    if ok { 0 } else { -50 },
                    actual_size,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = if ok { AUDIOQUEUE_NO_ERR } else { AUDIOQUEUE_PARAM_ERR };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioQueuePrime" => {
                self.runtime.audio_trace.audioqueue_prime_calls = self.runtime.audio_trace.audioqueue_prime_calls.saturating_add(1);
                let queue_ptr = self.cpu.regs[0];
                let requested_frames = self.cpu.regs[1];
                let out_prepared_ptr = self.cpu.regs[2];
                let ok = self.runtime.audio_queue.queues.contains_key(&queue_ptr);
                if ok {
                    if let Some(queue) = self.runtime.audio_queue.queues.get_mut(&queue_ptr) {
                        queue.prime_count = queue.prime_count.saturating_add(1);
                    }
                    if out_prepared_ptr != 0 {
                        self.write_u32_le(out_prepared_ptr, requested_frames)?;
                    }
                    let pending = self.audioqueue_collect_state_change_callbacks(queue_ptr, false, true);
                    let detail = format!(
                        "hle AudioQueuePrime(queue={}, requestedFrames={}, outPrepared={}) -> status=0 callbacks={}",
                        self.describe_ptr(queue_ptr),
                        requested_frames,
                        self.describe_ptr(out_prepared_ptr),
                        pending.len(),
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    if self.audioqueue_begin_pending_callbacks(&label, self.cpu.regs[14], AUDIOQUEUE_NO_ERR, pending)? {
                        return Ok(Some(StepControl::Continue));
                    }
                } else if out_prepared_ptr != 0 {
                    self.write_u32_le(out_prepared_ptr, 0)?;
                }
                self.cpu.regs[0] = if ok { AUDIOQUEUE_NO_ERR } else { AUDIOQUEUE_PARAM_ERR };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioQueueStart" => {
                self.runtime.audio_trace.audioqueue_start_calls = self.runtime.audio_trace.audioqueue_start_calls.saturating_add(1);
                let queue_ptr = self.cpu.regs[0];
                let start_time_ptr = self.cpu.regs[1];
                let ok = self.audioqueue_set_running(queue_ptr, true);
                let callbacks = if ok {
                    if let Some(queue) = self.runtime.audio_queue.queues.get_mut(&queue_ptr) {
                        queue.start_count = queue.start_count.saturating_add(1);
                    }
                    self.audioqueue_collect_state_change_callbacks(queue_ptr, true, true)
                } else {
                    VecDeque::new()
                };
                let detail = format!(
                    "hle AudioQueueStart(queue={}, startTime={}) -> status={} callbacks={}",
                    self.describe_ptr(queue_ptr),
                    self.describe_ptr(start_time_ptr),
                    if ok { 0 } else { -50 },
                    callbacks.len(),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                if ok && self.audioqueue_begin_pending_callbacks(&label, self.cpu.regs[14], AUDIOQUEUE_NO_ERR, callbacks)? {
                    return Ok(Some(StepControl::Continue));
                }
                self.cpu.regs[0] = if ok { AUDIOQUEUE_NO_ERR } else { AUDIOQUEUE_PARAM_ERR };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioQueueStop" => {
                self.runtime.audio_trace.audioqueue_stop_calls = self.runtime.audio_trace.audioqueue_stop_calls.saturating_add(1);
                let queue_ptr = self.cpu.regs[0];
                let immediate = self.cpu.regs[1] != 0;
                let ok = self.audioqueue_set_running(queue_ptr, false);
                let callbacks = if ok {
                    if let Some(queue) = self.runtime.audio_queue.queues.get_mut(&queue_ptr) {
                        queue.stop_count = queue.stop_count.saturating_add(1);
                    }
                    self.audioqueue_collect_state_change_callbacks(queue_ptr, true, false)
                } else {
                    VecDeque::new()
                };
                let detail = format!(
                    "hle AudioQueueStop(queue={}, immediate={}) -> status={} callbacks={}",
                    self.describe_ptr(queue_ptr),
                    immediate,
                    if ok { 0 } else { -50 },
                    callbacks.len(),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                if ok && self.audioqueue_begin_pending_callbacks(&label, self.cpu.regs[14], AUDIOQUEUE_NO_ERR, callbacks)? {
                    return Ok(Some(StepControl::Continue));
                }
                self.cpu.regs[0] = if ok { AUDIOQUEUE_NO_ERR } else { AUDIOQUEUE_PARAM_ERR };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioQueueDispose" => {
                self.runtime.audio_trace.audioqueue_dispose_calls = self.runtime.audio_trace.audioqueue_dispose_calls.saturating_add(1);
                let queue_ptr = self.cpu.regs[0];
                let immediate = self.cpu.regs[1] != 0;
                let mut ok = self.runtime.audio_queue.queues.contains_key(&queue_ptr);
                if ok {
                    let buffer_list = self
                        .runtime
                        .audio_queue
                        .queues
                        .get(&queue_ptr)
                        .map(|queue| queue.allocated_buffers.clone())
                        .unwrap_or_default();
                    let callbacks = self.audioqueue_collect_state_change_callbacks(queue_ptr, true, false);
                    if let Some(queue) = self.runtime.audio_queue.queues.get_mut(&queue_ptr) {
                        queue.dispose_count = queue.dispose_count.saturating_add(1);
                        queue.is_running = false;
                    }
                    for buffer_ptr in buffer_list {
                        let _ = self.audioqueue_free_buffer(queue_ptr, buffer_ptr);
                    }
                    let _ = self.free_synthetic_heap_block(queue_ptr);
                    self.diag.object_labels.remove(&queue_ptr);
                    self.runtime.audio_queue.queues.remove(&queue_ptr);
                    let detail = format!(
                        "hle AudioQueueDispose(queue={}, immediate={}) -> status=0 callbacks={}",
                        self.describe_ptr(queue_ptr),
                        immediate,
                        callbacks.len(),
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                } else {
                    let detail = format!(
                        "hle AudioQueueDispose(queue={}, immediate={}) -> status=-50",
                        self.describe_ptr(queue_ptr),
                        immediate,
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                }
                self.cpu.regs[0] = if ok { AUDIOQUEUE_NO_ERR } else { AUDIOQUEUE_PARAM_ERR };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioSessionInitialize" => {
                let runloop = self.cpu.regs[0];
                let mode = self.cpu.regs[1];
                let interruption_listener = self.cpu.regs[2];
                let user_data = self.cpu.regs[3];
                let detail = format!(
                    "hle AudioSessionInitialize(runLoop={}, mode={}, listener=0x{:08x}, userData={}) -> status=0",
                    self.describe_ptr(runloop),
                    self.describe_ptr(mode),
                    interruption_listener,
                    self.describe_ptr(user_data),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.audio_trace_push_event(format!("audiosession.initialize runLoop={} mode={} listener=0x{:08x}", self.describe_ptr(runloop), self.describe_ptr(mode), interruption_listener));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioSessionSetActive" => {
                let active = self.cpu.regs[0];
                let detail = format!("hle AudioSessionSetActive(active={}) -> status=0", active);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.audio_trace_push_event(format!("audiosession.set_active active={}", active));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioSessionSetProperty" => {
                let property_id = self.cpu.regs[0];
                let data_size = self.cpu.regs[1];
                let data_ptr = self.cpu.regs[2];
                let detail = format!(
                    "hle AudioSessionSetProperty(property=0x{:08x}, dataSize={}, data={}) -> status=0",
                    property_id,
                    data_size,
                    self.describe_ptr(data_ptr),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.audio_trace_push_event(format!("audiosession.set_property property=0x{:08x} dataSize={} data={}", property_id, data_size, self.describe_ptr(data_ptr)));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioSessionGetProperty" => {
                let property_id = self.cpu.regs[0];
                let io_size_ptr = self.cpu.regs[1];
                let out_ptr = self.cpu.regs[2];
                let requested = if io_size_ptr != 0 { self.read_u32_le(io_size_ptr).unwrap_or(0) } else { 0 };
                if out_ptr != 0 && requested != 0 {
                    self.write_bytes(out_ptr, &vec![0u8; requested as usize])?;
                }
                let detail = format!(
                    "hle AudioSessionGetProperty(property=0x{:08x}, ioSize={}, out={}) -> status=0 size={}",
                    property_id,
                    self.describe_ptr(io_size_ptr),
                    self.describe_ptr(out_ptr),
                    requested,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.audio_trace_push_event(format!("audiosession.get_property property=0x{:08x} size={} out={}", property_id, requested, self.describe_ptr(out_ptr)));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioServicesCreateSystemSoundID" => {
                let url = self.cpu.regs[0];
                let out_id_ptr = self.cpu.regs[1];
                let sound_id = self.runtime.audio_trace.next_systemsound_id.max(1);
                self.runtime.audio_trace.next_systemsound_id = sound_id.saturating_add(1);
                let path = self.synthetic_file_url_path(url)
                    .or_else(|| self.synthetic_file_url_absolute_string_value(url))
                    .unwrap_or_else(|| self.describe_ptr(url));
                self.runtime.audio_trace.systemsound_create_calls = self.runtime.audio_trace.systemsound_create_calls.saturating_add(1);
                self.runtime.audio_trace.systemsounds.insert(sound_id, path.clone());
                if out_id_ptr != 0 {
                    self.write_u32_le(out_id_ptr, sound_id)?;
                }
                let detail = format!(
                    "hle AudioServicesCreateSystemSoundID(url={}, out={}) -> status=0 soundID={} path='{}'",
                    self.describe_ptr(url),
                    self.describe_ptr(out_id_ptr),
                    sound_id,
                    path,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.audio_trace_push_event(format!("systemsound.create id={} path='{}'", sound_id, path));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioServicesDisposeSystemSoundID" => {
                let sound_id = self.cpu.regs[0];
                self.runtime.audio_trace.systemsound_dispose_calls = self.runtime.audio_trace.systemsound_dispose_calls.saturating_add(1);
                let existed = self.runtime.audio_trace.systemsounds.remove(&sound_id).is_some();
                let detail = format!("hle AudioServicesDisposeSystemSoundID(soundID={}) -> status=0 existed={}", sound_id, if existed { "YES" } else { "NO" });
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.audio_trace_push_event(format!("systemsound.dispose id={} existed={}", sound_id, if existed { "YES" } else { "NO" }));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioServicesPlaySystemSound" => {
                let sound_id = self.cpu.regs[0];
                self.runtime.audio_trace.systemsound_play_calls = self.runtime.audio_trace.systemsound_play_calls.saturating_add(1);
                let path = self.runtime.audio_trace.systemsounds.get(&sound_id).cloned().unwrap_or_else(|| "<unknown>".to_string());
                let detail = format!("hle AudioServicesPlaySystemSound(soundID={}) path='{}'", sound_id, path);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.audio_trace_push_event(format!("systemsound.play id={} path='{}'", sound_id, path));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "AudioServicesAddSystemSoundCompletion" | "AudioServicesRemoveSystemSoundCompletion" => {
                let sound_id = self.cpu.regs[0];
                let detail = format!("hle {}(soundID={}) -> status=0", label, sound_id);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.audio_trace_push_event(format!("systemsound.completion action={} id={}", label, sound_id));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alcOpenDevice" => {
                let device_name_ptr = self.cpu.regs[0];
                let device_name = if device_name_ptr != 0 {
                    self.read_c_string(device_name_ptr, 160).unwrap_or_else(|| self.describe_ptr(device_name_ptr))
                } else {
                    "<default>".to_string()
                };
                let handle = match self.openal_device_handle() {
                    Ok(ptr) => ptr,
                    Err(err) => {
                        self.openal_set_alc_error(ALC_OUT_OF_MEMORY);
                        self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &format!("hle alcOpenDevice(name={}, ptr={}) failed: {}", device_name, self.describe_ptr(device_name_ptr), err)));
                        self.cpu.regs[0] = 0;
                        self.cpu.regs[15] = self.cpu.regs[14] & !1;
                        self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                        return Ok(Some(StepControl::Continue));
                    }
                };
                self.openal_take_alc_error();
                let detail = format!("hle alcOpenDevice(name={}, ptr={}) -> {}", device_name, self.describe_ptr(device_name_ptr), self.describe_ptr(handle));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = handle;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alcCreateContext" => {
                let device = self.cpu.regs[0];
                let attrs = self.cpu.regs[1];
                let current_device = self.runtime.openal.device_ptr;
                if device == 0 || current_device == 0 || device != current_device {
                    self.openal_set_alc_error(ALC_INVALID_DEVICE);
                    let detail = format!("hle alcCreateContext(device={}, attrs={}) -> nil", self.describe_ptr(device), self.describe_ptr(attrs));
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = 0;
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }
                let handle = match self.openal_context_handle() {
                    Ok(ptr) => ptr,
                    Err(err) => {
                        self.openal_set_alc_error(ALC_OUT_OF_MEMORY);
                        self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &format!("hle alcCreateContext(device={}, attrs={}) failed: {}", self.describe_ptr(device), self.describe_ptr(attrs), err)));
                        self.cpu.regs[0] = 0;
                        self.cpu.regs[15] = self.cpu.regs[14] & !1;
                        self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                        return Ok(Some(StepControl::Continue));
                    }
                };
                self.openal_take_alc_error();
                let detail = format!("hle alcCreateContext(device={}, attrs={}) -> {}", self.describe_ptr(device), self.describe_ptr(attrs), self.describe_ptr(handle));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = handle;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alcMakeContextCurrent" => {
                self.runtime.audio_trace.openal_make_current_calls = self.runtime.audio_trace.openal_make_current_calls.saturating_add(1);
                let ctx = self.cpu.regs[0];
                let ok = if ctx == 0 {
                    self.runtime.openal.current_context = 0;
                    self.openal_take_alc_error();
                    ALC_TRUE
                } else if ctx == self.runtime.openal.context_ptr && ctx != 0 {
                    self.runtime.openal.current_context = ctx;
                    self.openal_take_alc_error();
                    ALC_TRUE
                } else {
                    self.openal_set_alc_error(ALC_INVALID_CONTEXT);
                    ALC_FALSE
                };
                let detail = format!("hle alcMakeContextCurrent(ctx={}) -> {}", self.describe_ptr(ctx), ok);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = ok;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alcGetCurrentContext" => {
                let ctx = self.runtime.openal.current_context;
                let detail = format!("hle alcGetCurrentContext() -> {}", self.describe_ptr(ctx));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = ctx;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alcGetContextsDevice" => {
                let ctx = self.cpu.regs[0];
                let device = if ctx != 0 && ctx == self.runtime.openal.context_ptr {
                    self.openal_take_alc_error();
                    self.runtime.openal.device_ptr
                } else {
                    self.openal_set_alc_error(ALC_INVALID_CONTEXT);
                    0
                };
                let detail = format!("hle alcGetContextsDevice(ctx={}) -> {}", self.describe_ptr(ctx), self.describe_ptr(device));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = device;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alcSuspendContext" | "alcProcessContext" => {
                let ctx = self.cpu.regs[0];
                let detail = format!("hle {}(ctx={})", label, self.describe_ptr(ctx));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alcDestroyContext" => {
                let ctx = self.cpu.regs[0];
                if ctx != 0 && ctx == self.runtime.openal.context_ptr {
                    if self.runtime.openal.current_context == ctx {
                        self.runtime.openal.current_context = 0;
                    }
                    let _ = self.free_synthetic_heap_block(ctx);
                    self.runtime.openal.context_ptr = 0;
                    self.openal_take_alc_error();
                } else if ctx != 0 {
                    self.openal_set_alc_error(ALC_INVALID_CONTEXT);
                }
                let detail = format!("hle alcDestroyContext(ctx={})", self.describe_ptr(ctx));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alcCloseDevice" => {
                let device = self.cpu.regs[0];
                let ok = if device != 0 && device == self.runtime.openal.device_ptr {
                    if self.runtime.openal.context_ptr != 0 {
                        let _ = self.free_synthetic_heap_block(self.runtime.openal.context_ptr);
                    }
                    let _ = self.free_synthetic_heap_block(device);
                    self.runtime.openal.context_ptr = 0;
                    self.runtime.openal.current_context = 0;
                    self.runtime.openal.device_ptr = 0;
                    self.openal_take_alc_error();
                    ALC_TRUE
                } else {
                    self.openal_set_alc_error(ALC_INVALID_DEVICE);
                    ALC_FALSE
                };
                let detail = format!("hle alcCloseDevice(device={}) -> {}", self.describe_ptr(device), ok);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = ok;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alcGetProcAddress" => {
                let device = self.cpu.regs[0];
                let name_ptr = self.cpu.regs[1];
                let name = self.read_c_string(name_ptr, 128).unwrap_or_else(|| self.describe_ptr(name_ptr));
                let detail = format!("hle alcGetProcAddress(device={}, proc='{}') -> nil", self.describe_ptr(device), name);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alGenBuffers" => {
                let count = self.cpu.regs[0];
                let out_ptr = self.cpu.regs[1];
                if count != 0 && out_ptr == 0 {
                    self.openal_set_al_error(AL_INVALID_VALUE);
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &format!("hle alGenBuffers(count={}, out={}) -> invalid", count, self.describe_ptr(out_ptr))));
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }
                let ids = self.openal_gen_buffers(count, out_ptr)?;
                let detail = format!("hle alGenBuffers(count={}, out={}) -> {:?}", count, self.describe_ptr(out_ptr), ids);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alDeleteBuffers" => {
                let count = self.cpu.regs[0];
                let ids_ptr = self.cpu.regs[1];
                let mut ids = Vec::with_capacity(count as usize);
                for i in 0..count {
                    ids.push(self.read_u32_le(ids_ptr.wrapping_add(i * 4)).unwrap_or(0));
                }
                for id in &ids {
                    self.runtime.openal.buffers.remove(id);
                }
                self.openal_take_al_error();
                let detail = format!("hle alDeleteBuffers(count={}, ptr={}) ids={:?}", count, self.describe_ptr(ids_ptr), ids);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alGenSources" => {
                let count = self.cpu.regs[0];
                let out_ptr = self.cpu.regs[1];
                if count != 0 && out_ptr == 0 {
                    self.openal_set_al_error(AL_INVALID_VALUE);
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &format!("hle alGenSources(count={}, out={}) -> invalid", count, self.describe_ptr(out_ptr))));
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }
                let ids = self.openal_gen_sources(count, out_ptr)?;
                let detail = format!("hle alGenSources(count={}, out={}) -> {:?}", count, self.describe_ptr(out_ptr), ids);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alDeleteSources" => {
                let count = self.cpu.regs[0];
                let ids_ptr = self.cpu.regs[1];
                let mut ids = Vec::with_capacity(count as usize);
                for i in 0..count {
                    ids.push(self.read_u32_le(ids_ptr.wrapping_add(i * 4)).unwrap_or(0));
                }
                for id in &ids {
                    self.runtime.openal.sources.remove(id);
                }
                self.openal_take_al_error();
                let detail = format!("hle alDeleteSources(count={}, ptr={}) ids={:?}", count, self.describe_ptr(ids_ptr), ids);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alBufferData" => {
                self.runtime.audio_trace.openal_buffer_upload_calls = self.runtime.audio_trace.openal_buffer_upload_calls.saturating_add(1);
                let buffer = self.cpu.regs[0];
                let format = self.cpu.regs[1];
                let data_ptr = self.cpu.regs[2];
                let size = self.cpu.regs[3];
                let freq = self.read_u32_le(self.cpu.regs[13]).unwrap_or(0);
                let preview = if data_ptr != 0 && size != 0 {
                    self.read_guest_bytes(data_ptr, size.min(64)).unwrap_or_default()
                } else {
                    Vec::new()
                };
                let ok = if let Some(entry) = self.runtime.openal.buffers.get_mut(&buffer) {
                    entry.format = format;
                    entry.frequency = freq;
                    entry.byte_len = size;
                    entry.preview = preview.clone();
                    true
                } else {
                    false
                };
                if ok {
                    self.openal_take_al_error();
                } else {
                    self.openal_set_al_error(AL_INVALID_NAME);
                }
                if ok {
                    self.runtime.audio_trace.openal_bytes_uploaded = self.runtime.audio_trace.openal_bytes_uploaded.saturating_add(size as u64);
                    self.runtime.audio_trace.openal_last_buffer_format = Some(format!("{} freq={} size={}", Self::openal_format_name(format), freq, size));
                    self.audio_trace_push_event(format!(
                        "openal.buffer_data buffer={} format={} freq={} size={} previewHex=[{}] previewAscii='{}'",
                        buffer,
                        Self::openal_format_name(format),
                        freq,
                        size,
                        Self::audio_hex_preview(&preview),
                        Self::audio_ascii_preview(&preview),
                    ));
                } else {
                    self.runtime.audio_trace.unsupported_events = self.runtime.audio_trace.unsupported_events.saturating_add(1);
                    self.audio_trace_push_event(format!("openal.buffer_data.miss buffer={} format=0x{:04x} size={} freq={}", buffer, format, size, freq));
                }
                let detail = format!("hle alBufferData(buffer={}, format=0x{:04x}, data={}, size={}, freq={}) -> {}", buffer, format, self.describe_ptr(data_ptr), size, freq, ok);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alSourcei" => {
                let source_id = self.cpu.regs[0];
                let param = self.cpu.regs[1];
                let value = self.cpu.regs[2];
                let ok = if let Some(source) = self.runtime.openal.sources.get_mut(&source_id) {
                    source.ints.insert(param, value as i32);
                    if param == AL_BUFFER {
                        source.queued_buffers.clear();
                        source.processed_buffers.clear();
                        if value != 0 {
                            source.queued_buffers.push_back(value);
                            source.state = AL_STOPPED;
                        } else {
                            source.state = AL_INITIAL;
                        }
                    } else if param == AL_LOOPING {
                        source.ints.insert(AL_LOOPING, if value != 0 { AL_TRUE as i32 } else { AL_FALSE as i32 });
                    }
                    true
                } else {
                    false
                };
                if ok {
                    self.openal_take_al_error();
                } else {
                    self.openal_set_al_error(AL_INVALID_NAME);
                }
                let detail = format!("hle alSourcei(source={}, param=0x{:04x}, value=0x{:08x}) -> {}", source_id, param, value, ok);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alSourcef" => {
                let source_id = self.cpu.regs[0];
                let param = self.cpu.regs[1];
                let value = f32::from_bits(self.cpu.regs[2]);
                let ok = if let Some(source) = self.runtime.openal.sources.get_mut(&source_id) {
                    source.floats.insert(param, value);
                    true
                } else {
                    false
                };
                if ok {
                    self.openal_take_al_error();
                } else {
                    self.openal_set_al_error(AL_INVALID_NAME);
                }
                let detail = format!("hle alSourcef(source={}, param=0x{:04x}, value={:.4}) -> {}", source_id, param, value, ok);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alSourcefv" => {
                let source_id = self.cpu.regs[0];
                let param = self.cpu.regs[1];
                let values_ptr = self.cpu.regs[2];
                let count = if param == AL_ORIENTATION { 6 } else { 3 };
                let values = self.openal_read_f32_slice(values_ptr, count).unwrap_or_default();
                let ok = if let Some(source) = self.runtime.openal.sources.get_mut(&source_id) {
                    source.vectors.insert(param, values.clone());
                    true
                } else {
                    false
                };
                if ok {
                    self.openal_take_al_error();
                } else {
                    self.openal_set_al_error(AL_INVALID_NAME);
                }
                let detail = format!("hle alSourcefv(source={}, param=0x{:04x}, values_ptr={}) -> {} values={:?}", source_id, param, self.describe_ptr(values_ptr), ok, values);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alListenerf" => {
                let param = self.cpu.regs[0];
                let value = f32::from_bits(self.cpu.regs[1]);
                self.runtime.openal.listener_floats.insert(param, value);
                self.openal_take_al_error();
                let detail = format!("hle alListenerf(param=0x{:04x}, value={:.4})", param, value);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alListener3f" => {
                let param = self.cpu.regs[0];
                let values = vec![f32::from_bits(self.cpu.regs[1]), f32::from_bits(self.cpu.regs[2]), f32::from_bits(self.cpu.regs[3])];
                self.runtime.openal.listener_vectors.insert(param, values.clone());
                self.openal_take_al_error();
                let detail = format!("hle alListener3f(param=0x{:04x}, values={:?})", param, values);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alGetListenerf" => {
                let param = self.cpu.regs[0];
                let out_ptr = self.cpu.regs[1];
                let value = self.runtime.openal.listener_floats.get(&param).copied().unwrap_or(if param == AL_GAIN { 1.0 } else { 0.0 });
                if out_ptr != 0 {
                    self.write_u32_le(out_ptr, value.to_bits())?;
                }
                self.openal_take_al_error();
                let detail = format!("hle alGetListenerf(param=0x{:04x}, out={}) -> {:.4}", param, self.describe_ptr(out_ptr), value);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alDistanceModel" => {
                let model = self.cpu.regs[0];
                self.runtime.openal.distance_model = model;
                self.openal_take_al_error();
                let detail = format!("hle alDistanceModel(model=0x{:04x})", model);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alSourceQueueBuffers" => {
                let source_id = self.cpu.regs[0];
                let count = self.cpu.regs[1];
                let ids_ptr = self.cpu.regs[2];
                let mut ids = Vec::with_capacity(count as usize);
                for i in 0..count {
                    ids.push(self.read_u32_le(ids_ptr.wrapping_add(i * 4)).unwrap_or(0));
                }
                let missing = ids.iter().any(|id| !self.runtime.openal.buffers.contains_key(id));
                let ok = if missing {
                    self.openal_set_al_error(AL_INVALID_NAME);
                    false
                } else if self.openal_queue_buffers(source_id, &ids) {
                    self.openal_take_al_error();
                    true
                } else {
                    self.openal_set_al_error(AL_INVALID_NAME);
                    false
                };
                let detail = format!("hle alSourceQueueBuffers(source={}, count={}, ptr={}) -> {} ids={:?}", source_id, count, self.describe_ptr(ids_ptr), ok, ids);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alSourceUnqueueBuffers" => {
                let source_id = self.cpu.regs[0];
                let count = self.cpu.regs[1];
                let ids_ptr = self.cpu.regs[2];
                let ids = match self.openal_unqueue_buffers(source_id, count) {
                    Some(values) => {
                        if values.len() < count as usize {
                            self.openal_set_al_error(AL_INVALID_VALUE);
                        } else {
                            self.openal_take_al_error();
                        }
                        values
                    }
                    None => {
                        self.openal_set_al_error(AL_INVALID_NAME);
                        Vec::new()
                    }
                };
                for (i, id) in ids.iter().enumerate() {
                    self.write_u32_le(ids_ptr.wrapping_add((i as u32) * 4), *id)?;
                }
                let detail = format!("hle alSourceUnqueueBuffers(source={}, count={}, ptr={}) -> {:?}", source_id, count, self.describe_ptr(ids_ptr), ids);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alSourcePlay" | "alSourceStop" => {
                let source_id = self.cpu.regs[0];
                let new_state = if label == "alSourcePlay" { AL_PLAYING } else { AL_STOPPED };
                let ok = if let Some(source) = self.runtime.openal.sources.get_mut(&source_id) {
                    source.state = new_state;
                    if new_state == AL_STOPPED {
                        Self::openal_promote_processed(source);
                    }
                    true
                } else {
                    false
                };
                if ok {
                    if label == "alSourcePlay" {
                        self.runtime.audio_trace.openal_play_calls = self.runtime.audio_trace.openal_play_calls.saturating_add(1);
                    } else {
                        self.runtime.audio_trace.openal_stop_calls = self.runtime.audio_trace.openal_stop_calls.saturating_add(1);
                    }
                    self.runtime.audio_trace.openal_last_source_state = Some(format!("source={} {}", source_id, Self::openal_state_name(new_state)));
                    self.audio_trace_push_event(format!("openal.source_state source={} action={} state={}", source_id, label, Self::openal_state_name(new_state)));
                    self.openal_take_al_error();
                } else {
                    self.runtime.audio_trace.unsupported_events = self.runtime.audio_trace.unsupported_events.saturating_add(1);
                    self.audio_trace_push_event(format!("openal.source_state.miss source={} action={}", source_id, label));
                    self.openal_set_al_error(AL_INVALID_NAME);
                }
                let detail = format!("hle {}(source={}) -> {} state=0x{:04x}", label, source_id, ok, new_state);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alGetSourcei" => {
                let source_id = self.cpu.regs[0];
                let param = self.cpu.regs[1];
                let out_ptr = self.cpu.regs[2];
                let mut source_found = false;
                let value = if let Some(source) = self.runtime.openal.sources.get_mut(&source_id) {
                    source_found = true;
                    Self::openal_promote_processed(source);
                    match param {
                        AL_SOURCE_STATE => source.state as i32,
                        AL_BUFFER => *source.ints.get(&AL_BUFFER).unwrap_or(&0),
                        AL_LOOPING => *source.ints.get(&AL_LOOPING).unwrap_or(&0),
                        AL_BUFFERS_QUEUED => source.queued_buffers.len() as i32,
                        AL_BUFFERS_PROCESSED => source.processed_buffers.len() as i32,
                        _ => *source.ints.get(&param).unwrap_or(&0),
                    }
                } else {
                    0
                };
                if source_found {
                    self.openal_take_al_error();
                } else {
                    self.openal_set_al_error(AL_INVALID_NAME);
                }
                if out_ptr != 0 {
                    self.write_u32_le(out_ptr, value as u32)?;
                }
                let detail = format!("hle alGetSourcei(source={}, param=0x{:04x}, out={}) -> {}", source_id, param, self.describe_ptr(out_ptr), value);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "alGetError" => {
                let err = self.openal_take_al_error();
                let detail = format!("hle alGetError() -> 0x{:04x}", err);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = err;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "memset" => {
                let dst = self.cpu.regs[0];
                let value = (self.cpu.regs[1] & 0xff) as u8;
                let len = self.cpu.regs[2];
                let status = if len == 0 {
                    Ok(())
                } else {
                    self.write_bytes(dst, &vec![value; len as usize])
                };
                let detail = match status {
                    Ok(()) => format!("hle memset(dst=0x{dst:08x}, value=0x{:02x}, len={})", value, len),
                    Err(ref err) => format!("hle memset(dst=0x{dst:08x}, value=0x{:02x}, len={}) failed: {}", value, len, err),
                };
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = dst;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "memcpy" | "memmove" => {
                let dst = self.cpu.regs[0];
                let src = self.cpu.regs[1];
                let len = self.cpu.regs[2];
                let copied = if len == 0 {
                    Ok(Vec::new())
                } else {
                    self.read_guest_bytes(src, len)
                };
                let status = match copied {
                    Ok(bytes) => self.write_bytes(dst, &bytes),
                    Err(err) => Err(err),
                };
                let detail = match status {
                    Ok(()) => format!("hle {}(dst=0x{:08x}, src=0x{:08x}, len={})", label, dst, src, len),
                    Err(ref err) => format!("hle {}(dst=0x{:08x}, src=0x{:08x}, len={}) failed: {}", label, dst, src, len, err),
                };
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = dst;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "objc_copyStruct" => {
                let dst = self.cpu.regs[0];
                let src = self.cpu.regs[1];
                let len = self.cpu.regs[2];
                let copied = if len == 0 {
                    Ok(Vec::new())
                } else {
                    self.read_guest_bytes(src, len)
                };
                let status = match copied {
                    Ok(bytes) => self.write_bytes(dst, &bytes),
                    Err(err) => Err(err),
                };
                let detail = match status {
                    Ok(()) => format!("hle objc_copyStruct(dst=0x{:08x}, src=0x{:08x}, len={}, atomic={}, strong={})", dst, src, len, self.cpu.regs[3], self.peek_stack_u32(0).unwrap_or(0)),
                    Err(ref err) => format!("hle objc_copyStruct(dst=0x{:08x}, src=0x{:08x}, len={}) failed: {}", dst, src, len, err),
                };
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = dst;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "memcmp" => {
                let lhs = self.cpu.regs[0];
                let rhs = self.cpu.regs[1];
                let len = self.cpu.regs[2];
                let result = if len == 0 {
                    0i32
                } else {
                    match (self.read_guest_bytes(lhs, len), self.read_guest_bytes(rhs, len)) {
                        (Ok(left), Ok(right)) => {
                            let mut diff = 0i32;
                            for (a, b) in left.iter().zip(right.iter()) {
                                if a != b {
                                    diff = (*a as i32) - (*b as i32);
                                    break;
                                }
                            }
                            diff
                        }
                        _ => 0i32,
                    }
                };
                let detail = format!("hle memcmp(lhs=0x{:08x}, rhs=0x{:08x}, len={}) -> {}", lhs, rhs, len, result);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = result as u32;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "strlen" => {
                let ptr = self.cpu.regs[0];
                let len = self.read_c_string(ptr, 4096).map(|s| s.len() as u32).unwrap_or(0);
                let detail = format!("hle strlen({}) -> {}", self.describe_ptr(ptr), len);
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = len;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "strcmp" | "strncmp" => {
                let lhs = self.cpu.regs[0];
                let rhs = self.cpu.regs[1];
                let limit = if label == "strncmp" { self.cpu.regs[2] as usize } else { 4096usize };
                let left = self.read_c_string(lhs, limit).unwrap_or_default();
                let right = self.read_c_string(rhs, limit).unwrap_or_default();
                let left_cmp = if label == "strncmp" && left.len() > limit { &left[..limit] } else { left.as_str() };
                let right_cmp = if label == "strncmp" && right.len() > limit { &right[..limit] } else { right.as_str() };
                let result = match left_cmp.cmp(right_cmp) {
                    std::cmp::Ordering::Less => -1i32,
                    std::cmp::Ordering::Equal => 0i32,
                    std::cmp::Ordering::Greater => 1i32,
                };
                let detail = if label == "strncmp" {
                    format!("hle strncmp(lhs={}, rhs={}, n={}) -> {}", self.describe_ptr(lhs), self.describe_ptr(rhs), limit, result)
                } else {
                    format!("hle strcmp(lhs={}, rhs={}) -> {}", self.describe_ptr(lhs), self.describe_ptr(rhs), result)
                };
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = result as u32;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "strcpy" => {
                let dst = self.cpu.regs[0];
                let src = self.cpu.regs[1];
                let text = self.read_c_string(src, 4096).unwrap_or_default();
                let mut bytes = text.into_bytes();
                bytes.push(0);
                let status = self.write_bytes(dst, &bytes);
                let detail = match status {
                    Ok(()) => format!("hle strcpy(dst=0x{:08x}, src={})", dst, self.describe_ptr(src)),
                    Err(ref err) => format!("hle strcpy(dst=0x{:08x}, src={}) failed: {}", dst, self.describe_ptr(src), err),
                };
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = dst;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "strtok" => {
                let str_ptr = self.cpu.regs[0];
                let delim_ptr = self.cpu.regs[1];
                self.runtime.ui_runtime.strtok_call_count = self.runtime.ui_runtime.strtok_call_count.saturating_add(1);

                let mut delim_bytes = Vec::new();
                let delim_status = if delim_ptr == 0 {
                    Err("NULL delimiter pointer".to_string())
                } else {
                    let mut status: Result<(), String> = Ok(());
                    for i in 0..256u32 {
                        match self.read_u8(delim_ptr.wrapping_add(i)) {
                            Ok(0) => break,
                            Ok(b) => delim_bytes.push(b),
                            Err(err) => {
                                status = Err(err.to_string());
                                break;
                            }
                        }
                    }
                    status
                };

                if let Err(err) = delim_status {
                    self.runtime.ui_runtime.strtok_next_ptr = 0;
                    let detail = format!(
                        "hle strtok(str={}, delim={}) failed: {}",
                        self.describe_ptr(str_ptr),
                        self.describe_ptr(delim_ptr),
                        err
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = 0;
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }

                let is_delim = |byte: u8, delims: &[u8]| -> bool { delims.iter().any(|d| *d == byte) };
                let mut cursor = if str_ptr != 0 {
                    str_ptr
                } else {
                    self.runtime.ui_runtime.strtok_next_ptr
                };

                if cursor == 0 {
                    let detail = format!(
                        "hle strtok(str={}, delim={}) -> NULL (no continuation)",
                        self.describe_ptr(str_ptr),
                        self.describe_ptr(delim_ptr)
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = 0;
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }

                let mut result_ptr = 0u32;
                let mut failure: Option<String> = None;
                let mut terminated_at: Option<u32> = None;
                let max_scan = 8192u32;

                for _ in 0..max_scan {
                    match self.read_u8(cursor) {
                        Ok(0) => {
                            self.runtime.ui_runtime.strtok_next_ptr = 0;
                            cursor = 0;
                            break;
                        }
                        Ok(b) if is_delim(b, &delim_bytes) => {
                            cursor = cursor.wrapping_add(1);
                        }
                        Ok(_) => {
                            result_ptr = cursor;
                            break;
                        }
                        Err(err) => {
                            self.runtime.ui_runtime.strtok_next_ptr = 0;
                            failure = Some(err.to_string());
                            cursor = 0;
                            break;
                        }
                    }
                }

                if failure.is_none() && result_ptr != 0 {
                    let mut scan = result_ptr;
                    let mut hit_end = false;
                    for _ in 0..max_scan {
                        match self.read_u8(scan) {
                            Ok(0) => {
                                self.runtime.ui_runtime.strtok_next_ptr = 0;
                                hit_end = true;
                                break;
                            }
                            Ok(b) if is_delim(b, &delim_bytes) => {
                                match self.write_u8(scan, 0) {
                                    Ok(()) => {
                                        self.runtime.ui_runtime.strtok_next_ptr = scan.wrapping_add(1);
                                        terminated_at = Some(scan);
                                    }
                                    Err(err) => {
                                        self.runtime.ui_runtime.strtok_next_ptr = 0;
                                        failure = Some(err.to_string());
                                        result_ptr = 0;
                                    }
                                }
                                break;
                            }
                            Ok(_) => {
                                scan = scan.wrapping_add(1);
                            }
                            Err(err) => {
                                self.runtime.ui_runtime.strtok_next_ptr = 0;
                                failure = Some(err.to_string());
                                result_ptr = 0;
                                break;
                            }
                        }
                    }
                    if !hit_end && terminated_at.is_none() && failure.is_none() {
                        self.runtime.ui_runtime.strtok_next_ptr = 0;
                    }
                }

                if result_ptr == 0 {
                    self.runtime.ui_runtime.strtok_next_ptr = 0;
                }

                let token_preview = if result_ptr != 0 {
                    self.read_c_string(result_ptr, 128)
                        .unwrap_or_else(|| format!("0x{result_ptr:08x}"))
                } else {
                    "<null>".to_string()
                };
                let delim_preview = if delim_bytes.is_empty() {
                    String::new()
                } else {
                    String::from_utf8_lossy(&delim_bytes).into_owned()
                };
                let detail = if let Some(err) = failure {
                    format!(
                        "hle strtok(str={}, delim={}='{}') failed: {}",
                        self.describe_ptr(str_ptr),
                        self.describe_ptr(delim_ptr),
                        delim_preview,
                        err
                    )
                } else if result_ptr == 0 {
                    format!(
                        "hle strtok(str={}, delim={}='{}') -> NULL",
                        self.describe_ptr(str_ptr),
                        self.describe_ptr(delim_ptr),
                        delim_preview,
                    )
                } else {
                    format!(
                        "hle strtok(str={}, delim={}='{}') -> {} token='{}' next={}{}",
                        self.describe_ptr(str_ptr),
                        self.describe_ptr(delim_ptr),
                        delim_preview,
                        self.describe_ptr(result_ptr),
                        token_preview,
                        self.describe_ptr(self.runtime.ui_runtime.strtok_next_ptr),
                        terminated_at
                            .map(|at| format!(", wrote \\0 @0x{at:08x}"))
                            .unwrap_or_default(),
                    )
                };
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = result_ptr;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "bzero" => {
                let dst = self.cpu.regs[0];
                let len = self.cpu.regs[1];
                let status = if len == 0 {
                    Ok(())
                } else {
                    self.write_bytes(dst, &vec![0u8; len as usize])
                };
                let detail = match status {
                    Ok(()) => format!("hle bzero(dst=0x{dst:08x}, len={len})"),
                    Err(ref err) => format!("hle bzero(dst=0x{dst:08x}, len={len}) failed: {}", err),
                };
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "fopen" => {
                let request = self.read_c_string(self.cpu.regs[0], 512).unwrap_or_default();
                let mode = self.read_c_string(self.cpu.regs[1], 32).unwrap_or_else(|| "rb".to_string());
                let handle = self.open_bundle_file(&request, &mode).unwrap_or(0);
                let detail = if handle != 0 {
                    format!("hle fopen(path='{}', mode='{}') -> {}", request, mode, self.describe_ptr(handle))
                } else {
                    format!("hle fopen(path='{}', mode='{}') -> nil", request, mode)
                };
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = handle;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "fclose" => {
                let stream = self.cpu.regs[0];
                let closed = self.runtime.fs.host_files.remove(&stream).is_some();
                let detail = format!("hle fclose({}) -> {}", self.describe_ptr(stream), if closed { 0 } else { -1 });
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = if closed { 0 } else { u32::MAX };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "fread" => {
                let dst = self.cpu.regs[0];
                let size = self.cpu.regs[1] as usize;
                let nmemb = self.cpu.regs[2] as usize;
                let stream = self.cpu.regs[3];
                let requested = size.checked_mul(nmemb).unwrap_or(0);
                let (chunk, items_read, path, mode, write_failed) = if size == 0 || nmemb == 0 {
                    (Vec::new(), 0u32, String::new(), String::new(), false)
                } else if let Some(file) = self.runtime.fs.host_files.get_mut(&stream) {
                    let start = file.pos.min(file.data.len());
                    let available = file.data.len().saturating_sub(start).min(requested);
                    let end = start.saturating_add(available);
                    let chunk = file.data[start..end].to_vec();
                    file.pos = end;
                    file.eof = file.pos >= file.data.len();
                    file.error = false;
                    let items_read = (chunk.len() / size) as u32;
                    (chunk, items_read, file.path.clone(), file.mode.clone(), false)
                } else {
                    (Vec::new(), 0u32, String::new(), String::new(), true)
                };
                let mut write_failed_flag = write_failed;
                if !chunk.is_empty() && self.write_bytes(dst, &chunk).is_err() {
                    write_failed_flag = true;
                }
                if let Some(file) = self.runtime.fs.host_files.get_mut(&stream) {
                    if write_failed_flag {
                        file.error = true;
                    }
                }
                self.runtime.fs.file_read_ops = self.runtime.fs.file_read_ops.saturating_add(1);
                self.runtime.fs.file_bytes_read = self.runtime.fs.file_bytes_read.saturating_add(chunk.len().min(u32::MAX as usize) as u32);
                if !path.is_empty() {
                    self.runtime.fs.last_file_path = Some(path.clone());
                }
                if !mode.is_empty() {
                    self.runtime.fs.last_file_mode = Some(mode.clone());
                }
                let detail = format!(
                    "hle fread(dst=0x{dst:08x}, size={}, nmemb={}, stream={}) -> items={} bytes={}{}",
                    size,
                    nmemb,
                    self.describe_ptr(stream),
                    items_read,
                    chunk.len(),
                    if path.is_empty() { String::new() } else { format!(" path={}", path) }
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = items_read;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "fseek" => {
                let stream = self.cpu.regs[0];
                let offset = self.cpu.regs[1] as i32 as i64;
                let whence = self.cpu.regs[2];
                let result = if let Some(file) = self.runtime.fs.host_files.get_mut(&stream) {
                    let base = match whence {
                        0 => 0i64,
                        1 => file.pos as i64,
                        2 => file.data.len() as i64,
                        _ => -1,
                    };
                    if base < 0 {
                        file.error = true;
                        u32::MAX
                    } else {
                        let next = base.saturating_add(offset);
                        if next < 0 {
                            file.error = true;
                            u32::MAX
                        } else {
                            file.pos = (next as usize).min(file.data.len());
                            file.eof = file.pos >= file.data.len();
                            file.error = false;
                            0
                        }
                    }
                } else {
                    u32::MAX
                };
                let detail = format!("hle fseek({}, offset={}, whence={}) -> {}", self.describe_ptr(stream), offset, whence, if result == 0 { 0 } else { -1 });
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = result;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "ftell" => {
                let stream = self.cpu.regs[0];
                let pos = self.runtime.fs.host_files.get(&stream).map(|file| file.pos.min(u32::MAX as usize) as u32).unwrap_or(u32::MAX);
                let detail = format!("hle ftell({}) -> {}", self.describe_ptr(stream), if pos == u32::MAX { -1i64 } else { pos as i64 });
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = pos;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "feof" => {
                let stream = self.cpu.regs[0];
                let eof = self.runtime.fs.host_files.get(&stream).map(|file| file.eof).unwrap_or(false);
                let detail = format!("hle feof({}) -> {}", self.describe_ptr(stream), if eof { 1 } else { 0 });
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = if eof { 1 } else { 0 };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "ferror" => {
                let stream = self.cpu.regs[0];
                let err = self.runtime.fs.host_files.get(&stream).map(|file| file.error).unwrap_or(false);
                let detail = format!("hle ferror({}) -> {}", self.describe_ptr(stream), if err { 1 } else { 0 });
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = if err { 1 } else { 0 };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "clearerr" => {
                let stream = self.cpu.regs[0];
                if let Some(file) = self.runtime.fs.host_files.get_mut(&stream) {
                    file.error = false;
                    file.eof = false;
                }
                let detail = format!("hle clearerr({})", self.describe_ptr(stream));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "rewind" => {
                let stream = self.cpu.regs[0];
                if let Some(file) = self.runtime.fs.host_files.get_mut(&stream) {
                    file.pos = 0;
                    file.eof = false;
                    file.error = false;
                }
                let detail = format!("hle rewind({})", self.describe_ptr(stream));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "fgetc" => {
                let stream = self.cpu.regs[0];
                let value = if let Some(file) = self.runtime.fs.host_files.get_mut(&stream) {
                    if file.pos < file.data.len() {
                        let byte = file.data[file.pos];
                        file.pos += 1;
                        file.eof = file.pos >= file.data.len();
                        file.error = false;
                        byte as u32
                    } else {
                        file.eof = true;
                        u32::MAX
                    }
                } else {
                    u32::MAX
                };
                let detail = format!("hle fgetc({}) -> {}", self.describe_ptr(stream), if value == u32::MAX { -1i64 } else { value as i64 });
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = value;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFRunLoopGetCurrent" | "CFRunLoopGetMain" => {
                self.bootstrap_synthetic_runloop();
                let detail = format!(
                    "hle {}() -> {}",
                    label,
                    self.describe_ptr(self.runtime.ui_objects.main_runloop),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = self.runtime.ui_objects.main_runloop;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFNetworkCopySystemProxySettings" => {
                self.install_uikit_labels();
                let detail = format!(
                    "hle CFNetworkCopySystemProxySettings() -> {}",
                    self.describe_ptr(self.runtime.ui_network.proxy_settings),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = self.runtime.ui_network.proxy_settings;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "SCNetworkReachabilityCreateWithName" => {
                self.install_uikit_labels();
                let host = self.read_c_string(self.cpu.regs[1], 128).unwrap_or_else(|| "<synthetic-host>".to_string());
                let detail = format!(
                    "hle SCNetworkReachabilityCreateWithName(allocator={}, host='{}') -> {}",
                    self.describe_ptr(self.cpu.regs[0]),
                    host,
                    self.describe_ptr(self.runtime.ui_network.reachability),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = self.runtime.ui_network.reachability;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "SCNetworkReachabilitySetCallback" => {
                self.runtime.ui_network.reachability_callback_set = true;
                self.bootstrap_synthetic_runloop();
                let detail = format!(
                    "hle SCNetworkReachabilitySetCallback(target={}, callback={}, context={}) -> YES",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 1;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "SCNetworkReachabilityScheduleWithRunLoop" => {
                self.bootstrap_synthetic_runloop();
                self.runtime.ui_network.reachability_scheduled = true;
                self.recalc_runloop_sources();
                let detail = format!(
                    "hle SCNetworkReachabilityScheduleWithRunLoop(target={}, runloop={}, mode={}) -> YES",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 1;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "SCNetworkReachabilityUnscheduleFromRunLoop" => {
                self.runtime.ui_network.reachability_scheduled = false;
                self.recalc_runloop_sources();
                let detail = format!(
                    "hle SCNetworkReachabilityUnscheduleFromRunLoop(target={}, runloop={}, mode={}) -> YES",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 1;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "SCNetworkReachabilityGetFlags" => {
                self.install_uikit_labels();
                let flags_ptr = self.cpu.regs[1];
                let flags = self.reachability_flags();
                if flags_ptr != 0 {
                    self.write_u32_le(flags_ptr, flags)?;
                }
                let detail = format!(
                    "hle SCNetworkReachabilityGetFlags(target={}, out=0x{:08x}) -> {}",
                    self.describe_ptr(self.cpu.regs[0]),
                    flags_ptr,
                    self.reachability_flags_label(),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 1;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFStreamCreatePairWithSocket" | "CFStreamCreatePairWithSocketToHost" => {
                self.bootstrap_synthetic_runloop();
                let (read_out, write_out, detail) = if label == "CFStreamCreatePairWithSocketToHost" {
                    let read_out = self.cpu.regs[3];
                    let write_out = self.read_u32_le(self.cpu.regs[13]).unwrap_or(0);
                    let host = self.read_c_string(self.cpu.regs[1], 128).unwrap_or_else(|| self.network_host_string().to_string());
                    (
                        read_out,
                        write_out,
                        format!(
                            "hle {}(allocator={}, host='{}', port={}, outRead=0x{:08x}, outWrite=0x{:08x}) -> read={} write={}",
                            label,
                            self.describe_ptr(self.cpu.regs[0]),
                            host,
                            self.cpu.regs[2],
                            read_out,
                            write_out,
                            self.describe_ptr(self.runtime.ui_network.read_stream),
                            self.describe_ptr(self.runtime.ui_network.write_stream),
                        ),
                    )
                } else {
                    let read_out = self.cpu.regs[2];
                    let write_out = self.cpu.regs[3];
                    (
                        read_out,
                        write_out,
                        format!(
                            "hle CFStreamCreatePairWithSocket(allocator={}, socket={}, outRead=0x{:08x}, outWrite=0x{:08x}) -> read={} write={}",
                            self.describe_ptr(self.cpu.regs[0]),
                            self.cpu.regs[1],
                            read_out,
                            write_out,
                            self.describe_ptr(self.runtime.ui_network.read_stream),
                            self.describe_ptr(self.runtime.ui_network.write_stream),
                        ),
                    )
                };
                if read_out != 0 {
                    self.write_u32_le(read_out, self.runtime.ui_network.read_stream)?;
                }
                if write_out != 0 {
                    self.write_u32_le(write_out, self.runtime.ui_network.write_stream)?;
                }
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFReadStreamSetClient" => {
                self.runtime.ui_network.read_stream_client_set = true;
                self.runtime.ui_network.read_stream_client_flags = self.cpu.regs[1];
                self.runtime.ui_network.read_stream_client_callback = self.cpu.regs[2];
                self.runtime.ui_network.read_stream_client_context = self.cpu.regs[3];
                let detail = format!(
                    "hle CFReadStreamSetClient(stream={}, flags=0x{:08x}, callback={}, context={}) -> YES",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.cpu.regs[1],
                    self.describe_ptr(self.cpu.regs[2]),
                    self.describe_ptr(self.cpu.regs[3]),
                );
                self.push_callback_trace(format!(
                    "stream.setClient kind=read stream={} flags=0x{:08x} callback={} context={} origin={}",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.cpu.regs[1],
                    self.describe_ptr(self.cpu.regs[2]),
                    self.describe_ptr(self.cpu.regs[3]),
                    label,
                ));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 1;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFWriteStreamSetClient" => {
                self.runtime.ui_network.write_stream_client_set = true;
                self.runtime.ui_network.write_stream_client_flags = self.cpu.regs[1];
                self.runtime.ui_network.write_stream_client_callback = self.cpu.regs[2];
                self.runtime.ui_network.write_stream_client_context = self.cpu.regs[3];
                let detail = format!(
                    "hle CFWriteStreamSetClient(stream={}, flags=0x{:08x}, callback={}, context={}) -> YES",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.cpu.regs[1],
                    self.describe_ptr(self.cpu.regs[2]),
                    self.describe_ptr(self.cpu.regs[3]),
                );
                self.push_callback_trace(format!(
                    "stream.setClient kind=write stream={} flags=0x{:08x} callback={} context={} origin={}",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.cpu.regs[1],
                    self.describe_ptr(self.cpu.regs[2]),
                    self.describe_ptr(self.cpu.regs[3]),
                    label,
                ));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 1;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFReadStreamScheduleWithRunLoop" => {
                self.bootstrap_synthetic_runloop();
                self.runtime.ui_network.read_stream_scheduled = true;
                self.recalc_runloop_sources();
                self.refresh_network_object_labels();
                let detail = format!(
                    "hle CFReadStreamScheduleWithRunLoop(stream={}, runloop={}, mode={}) -> scheduled",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFWriteStreamScheduleWithRunLoop" => {
                self.bootstrap_synthetic_runloop();
                self.runtime.ui_network.write_stream_scheduled = true;
                self.recalc_runloop_sources();
                self.refresh_network_object_labels();
                let detail = format!(
                    "hle CFWriteStreamScheduleWithRunLoop(stream={}, runloop={}, mode={}) -> scheduled",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFReadStreamUnscheduleFromRunLoop" => {
                self.runtime.ui_network.read_stream_scheduled = false;
                self.recalc_runloop_sources();
                self.refresh_network_object_labels();
                let detail = format!(
                    "hle CFReadStreamUnscheduleFromRunLoop(stream={}, runloop={}, mode={}) -> unscheduled",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFWriteStreamUnscheduleFromRunLoop" => {
                self.runtime.ui_network.write_stream_scheduled = false;
                self.recalc_runloop_sources();
                self.refresh_network_object_labels();
                let detail = format!(
                    "hle CFWriteStreamUnscheduleFromRunLoop(stream={}, runloop={}, mode={}) -> unscheduled",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFReadStreamOpen" => {
                self.runtime.ui_network.read_stream_open = true;
                self.sync_stream_transport_state();
                self.refresh_network_object_labels();
                let detail = format!(
                    "hle CFReadStreamOpen(stream={}) -> YES status={}",
                    self.describe_ptr(self.cpu.regs[0]),
                    Self::stream_status_name(self.runtime.ui_network.read_stream_status),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 1;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFWriteStreamOpen" => {
                self.runtime.ui_network.write_stream_open = true;
                self.sync_stream_transport_state();
                self.refresh_network_object_labels();
                let detail = format!(
                    "hle CFWriteStreamOpen(stream={}) -> YES status={}",
                    self.describe_ptr(self.cpu.regs[0]),
                    Self::stream_status_name(self.runtime.ui_network.write_stream_status),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 1;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFReadStreamClose" => {
                self.runtime.ui_network.read_stream_open = false;
                self.runtime.ui_network.read_stream_status = 6;
                self.refresh_network_object_labels();
                self.recalc_runloop_sources();
                let detail = format!("hle CFReadStreamClose(stream={})", self.describe_ptr(self.cpu.regs[0]));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFWriteStreamClose" => {
                self.runtime.ui_network.write_stream_open = false;
                self.runtime.ui_network.write_stream_status = 6;
                self.refresh_network_object_labels();
                self.recalc_runloop_sources();
                let detail = format!("hle CFWriteStreamClose(stream={})", self.describe_ptr(self.cpu.regs[0]));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFReadStreamGetStatus" => {
                self.sync_stream_transport_state();
                let detail = format!(
                    "hle CFReadStreamGetStatus(stream={}) -> {}",
                    self.describe_ptr(self.cpu.regs[0]),
                    Self::stream_status_name(self.runtime.ui_network.read_stream_status),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = self.runtime.ui_network.read_stream_status;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFWriteStreamGetStatus" => {
                self.sync_stream_transport_state();
                let detail = format!(
                    "hle CFWriteStreamGetStatus(stream={}) -> {}",
                    self.describe_ptr(self.cpu.regs[0]),
                    Self::stream_status_name(self.runtime.ui_network.write_stream_status),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = self.runtime.ui_network.write_stream_status;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFReadStreamHasBytesAvailable" => {
                self.sync_stream_transport_state();
                let available = self.read_stream_has_bytes_available();
                let detail = format!(
                    "hle CFReadStreamHasBytesAvailable(stream={}) -> {}",
                    self.describe_ptr(self.cpu.regs[0]),
                    if available { "YES" } else { "NO" },
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = if available { 1 } else { 0 };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFWriteStreamCanAcceptBytes" => {
                self.sync_stream_transport_state();
                let can_accept = self.write_stream_can_accept_bytes();
                let detail = format!(
                    "hle CFWriteStreamCanAcceptBytes(stream={}) -> {}",
                    self.describe_ptr(self.cpu.regs[0]),
                    if can_accept { "YES" } else { "NO" },
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = if can_accept { 1 } else { 0 };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFReadStreamRead" => {
                self.sync_stream_transport_state();
                let buf = self.cpu.regs[1];
                let max_len = self.cpu.regs[2] as usize;
                let payload = self.synthetic_payload_bytes();
                let start = self.runtime.ui_network.read_stream_bytes_consumed as usize;
                let available = payload.len().saturating_sub(start);
                let copy_len = available.min(max_len);
                if copy_len > 0 && buf != 0 {
                    self.write_mem(buf, &payload[start..start + copy_len])?;
                }
                self.runtime.ui_network.read_stream_bytes_consumed = self.runtime.ui_network.read_stream_bytes_consumed.saturating_add(copy_len as u32);
                self.runtime.ui_network.read_stream_events = self.runtime.ui_network.read_stream_events.saturating_add(1);
                self.sync_stream_transport_state();
                self.refresh_network_object_labels();
                let detail = format!(
                    "hle CFReadStreamRead(stream={}, buf=0x{:08x}, maxLen={}) -> {} status={} consumed={}/{}",
                    self.describe_ptr(self.cpu.regs[0]),
                    buf,
                    max_len,
                    copy_len,
                    Self::stream_status_name(self.runtime.ui_network.read_stream_status),
                    self.runtime.ui_network.read_stream_bytes_consumed,
                    payload.len(),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = copy_len as u32;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFWriteStreamWrite" => {
                self.sync_stream_transport_state();
                let len = self.cpu.regs[2];
                self.runtime.ui_network.write_stream_bytes_written = self.runtime.ui_network.write_stream_bytes_written.saturating_add(len);
                self.runtime.ui_network.write_stream_events = self.runtime.ui_network.write_stream_events.saturating_add(1);
                self.sync_stream_transport_state();
                self.refresh_network_object_labels();
                let detail = format!(
                    "hle CFWriteStreamWrite(stream={}, buf=0x{:08x}, len={}) -> {} status={}",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.cpu.regs[1],
                    len,
                    len,
                    Self::stream_status_name(self.runtime.ui_network.write_stream_status),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = len;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFReadStreamCopyProperty" | "CFWriteStreamSetProperty" | "CFReadStreamSetProperty" => {
                let detail = format!(
                    "hle {}(stream={}, property={}, value={}) -> {}",
                    label,
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                    if label == "CFReadStreamCopyProperty" { "nil" } else { "YES" },
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = if label == "CFReadStreamCopyProperty" { 0 } else { 1 };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFReadStreamGetError" | "CFWriteStreamGetError" => {
                let detail = format!(
                    "hle {}(stream={}) -> domain=0 error=0",
                    label,
                    self.describe_ptr(self.cpu.regs[0]),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[1] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFRunLoopAddTimer" | "CFRunLoopAddSource" => {
                self.bootstrap_synthetic_runloop();
                let attached = if label == "CFRunLoopAddTimer" {
                    self.attach_foundation_timer(self.cpu.regs[1], label.as_str())
                } else {
                    false
                };
                self.recalc_runloop_sources();
                let detail = format!(
                    "hle {}(runloop={}, obj={}, mode={}) attachedTimer={}",
                    label,
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                    if attached { "YES" } else { "NO" },
                );
                self.push_callback_trace(format!(
                    "runloop.add label={} runloop={} obj={} mode={} attachedTimer={} sources={} watchedScene={} watchOrigin={}",
                    label,
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                    if attached { "YES" } else { "NO" },
                    self.runtime.ui_runtime.runloop_sources,
                    self.describe_ptr(self.scheduler_trace_watch_scene()),
                    self.runtime.scheduler.trace.window_origin.clone().unwrap_or_else(|| "<none>".to_string()),
                ));
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFRunLoopRemoveTimer" | "CFRunLoopRemoveSource" => {
                let removed = if label == "CFRunLoopRemoveTimer" {
                    self.invalidate_foundation_timer(self.cpu.regs[1], label.as_str())
                } else {
                    false
                };
                self.recalc_runloop_sources();
                let detail = format!(
                    "hle {}(runloop={}, obj={}, mode={}) removedTimer={}",
                    label,
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                    if removed { "YES" } else { "NO" },
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = 0;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "CFRunLoopRunInMode" => {
                self.bootstrap_synthetic_runloop();
                let handled = self.runtime.ui_runtime.runloop_sources > 0;
                self.push_callback_trace(format!(
                    "runloop.run label={} mode={} handled={} sourcesBefore={} watchedScene={} watchOrigin={} readClient={} writeClient={}",
                    label,
                    self.describe_ptr(self.cpu.regs[0]),
                    if handled { "YES" } else { "NO" },
                    self.runtime.ui_runtime.runloop_sources,
                    self.describe_ptr(self.scheduler_trace_watch_scene()),
                    self.runtime.scheduler.trace.window_origin.clone().unwrap_or_else(|| "<none>".to_string()),
                    self.describe_ptr(self.runtime.ui_network.read_stream_client_callback),
                    self.describe_ptr(self.runtime.ui_network.write_stream_client_callback),
                ));
                self.diag.trace.push(self.hle_trace_line(
                    index,
                    current_pc,
                    &label,
                    &format!(
                        "hle CFRunLoopRunInMode(mode={}, seconds=<synthetic>, returnAfter=0)",
                        self.describe_ptr(self.cpu.regs[0]),
                    ),
                ));
                self.push_synthetic_runloop_tick("CFRunLoopRunInMode", handled);
                self.cpu.regs[0] = if handled { 4 } else { 3 };
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "objc_getProperty" => {
                let receiver = self.cpu.regs[0];
                let selector_ptr = self.cpu.regs[1];
                let offset = self.cpu.regs[2];
                let atomic = self.cpu.regs[3];
                let selector = self
                    .objc_read_selector_name(selector_ptr)
                    .unwrap_or_else(|| format!("sel@0x{selector_ptr:08x}"));
                let addr = receiver.wrapping_add(offset);
                let value = if receiver != 0 {
                    self.read_u32_le(addr).unwrap_or(0)
                } else {
                    0
                };
                let detail = format!(
                    "hle objc_getProperty(receiver={}, sel={}, offset=0x{:08x}, atomic={}, slot={}, value={})",
                    self.describe_ptr(receiver),
                    selector,
                    offset,
                    atomic,
                    self.describe_ptr(addr),
                    self.describe_ptr(value),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = value;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "objc_setProperty" => {
                let receiver = self.cpu.regs[0];
                let selector_ptr = self.cpu.regs[1];
                let offset = self.cpu.regs[2];
                let value = self.cpu.regs[3];
                let atomic = self.peek_stack_u32(0).unwrap_or(0);
                let should_copy = self.peek_stack_u32(1).unwrap_or(0);
                let selector = self
                    .objc_read_selector_name(selector_ptr)
                    .unwrap_or_else(|| format!("sel@0x{selector_ptr:08x}"));
                let addr = receiver.wrapping_add(offset);
                if receiver != 0 {
                    self.write_u32_le(addr, value)?;
                }
                let detail = format!(
                    "hle objc_setProperty(receiver={}, sel={}, offset=0x{:08x}, value={}, atomic={}, copy={}, slot={})",
                    self.describe_ptr(receiver),
                    selector,
                    offset,
                    self.describe_ptr(value),
                    atomic,
                    should_copy,
                    self.describe_ptr(addr),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "objc_msgSendSuper2_stret" => {
                self.install_uikit_labels();
                let out_ptr = self.cpu.regs[0];
                let super_ptr = self.cpu.regs[1];
                let receiver = self.read_u32_le(super_ptr).ok().unwrap_or(0);
                let current_class = self.read_u32_le(super_ptr.wrapping_add(4)).ok().unwrap_or(0);
                let selector = self
                    .objc_read_selector_name(self.cpu.regs[2])
                    .unwrap_or_else(|| format!("sel@0x{:08x}", self.cpu.regs[2]));
                let arg1 = self.cpu.regs[3];
                let receiver_desc = self.describe_ptr(receiver);
                let arg1_desc = self.describe_ptr(arg1);
                let class_desc = self
                    .objc_class_name_for_ptr(current_class)
                    .unwrap_or_else(|| format!("class@0x{current_class:08x}"));
                self.record_objc_selector(
                    &format!("super::stret::{}", selector),
                    format!("super::stret {} recv={} class={} arg1={} out={}", selector, receiver_desc, class_desc, arg1_desc, self.describe_ptr(out_ptr)),
                );
                let handled = match selector.as_str() {
                    "bounds" | "applicationFrame" | "frame" => {
                        let bits = self.ui_rect_bits_for_selector(receiver, selector.as_str());
                        self.write_cg_rect_bits_to_guest(out_ptr, bits)?;
                        Some(Self::ui_rect_bits_to_string(bits))
                    }
                    "winSize" | "viewSize" => {
                        let (width, height, authority) = self.ui_authoritative_surface_size();
                        self.write_cg_size_to_guest(out_ptr, width, height)?;
                        Some(format!("CGSize({},{}) authority={}", width, height, authority))
                    }
                    "contentSize" | "size" => {
                        let (fallback_w, fallback_h, _) = self.ui_authoritative_surface_size();
                        let (width, height) = self
                            .runtime.graphics.synthetic_sprites
                            .get(&receiver)
                            .map(|state| (state.width, state.height))
                            .filter(|(w, h)| *w != 0 || *h != 0)
                            .or_else(|| self.synthetic_texture_dimensions(receiver))
                            .unwrap_or((fallback_w, fallback_h));
                        self.write_cg_size_to_guest(out_ptr, width, height)?;
                        Some(format!("CGSize({},{})", width, height))
                    }
                    "position" | "origin" => {
                        let bits = self
                            .runtime.graphics.synthetic_sprites
                            .get(&receiver)
                            .map(|state| [state.position_x_bits, state.position_y_bits])
                            .unwrap_or([0, 0]);
                        self.write_cg_point_to_guest_bits(out_ptr, bits)?;
                        Some(format!("CGPoint({:.3},{:.3})", Self::f32_from_bits(bits[0]), Self::f32_from_bits(bits[1])))
                    }
                    "anchorPoint" | "anchorPointInPixels" => {
                        let default_anchor = if self.diag.object_labels.get(&receiver).map(|label| label.contains("CCSprite")).unwrap_or(false) { 0.5f32.to_bits() } else { 0 };
                        let bits = self
                            .runtime.graphics.synthetic_sprites
                            .get(&receiver)
                            .map(|state| {
                                if selector == "anchorPointInPixels" {
                                    let content_w = if state.untrimmed_explicit && state.untrimmed_w_bits != 0 {
                                        Self::f32_from_bits(state.untrimmed_w_bits)
                                    } else {
                                        state.width as f32
                                    };
                                    let content_h = if state.untrimmed_explicit && state.untrimmed_h_bits != 0 {
                                        Self::f32_from_bits(state.untrimmed_h_bits)
                                    } else {
                                        state.height as f32
                                    };
                                    [
                                        if state.anchor_pixels_explicit {
                                            state.anchor_pixels_x_bits
                                        } else {
                                            ((if state.anchor_x_bits != 0 { Self::f32_from_bits(state.anchor_x_bits) } else { Self::f32_from_bits(default_anchor) }) * content_w).to_bits()
                                        },
                                        if state.anchor_pixels_explicit {
                                            state.anchor_pixels_y_bits
                                        } else {
                                            ((if state.anchor_y_bits != 0 { Self::f32_from_bits(state.anchor_y_bits) } else { Self::f32_from_bits(default_anchor) }) * content_h).to_bits()
                                        },
                                    ]
                                } else {
                                    [
                                        if state.anchor_x_bits != 0 { state.anchor_x_bits } else { default_anchor },
                                        if state.anchor_y_bits != 0 { state.anchor_y_bits } else { default_anchor },
                                    ]
                                }
                            })
                            .unwrap_or([default_anchor, default_anchor]);
                        self.write_cg_point_to_guest_bits(out_ptr, bits)?;
                        Some(format!("CGPoint({:.3},{:.3})", Self::f32_from_bits(bits[0]), Self::f32_from_bits(bits[1])))
                    }
                    _ => None,
                };
                if let Some(note) = handled {
                    let detail = format!(
                        "hle objc_msgSendSuper2_stret(out={}, super={}, receiver={}, class={}, sel={}, arg1={}, wrote={})",
                        self.describe_ptr(out_ptr),
                        self.describe_ptr(super_ptr),
                        receiver_desc,
                        class_desc,
                        selector,
                        arg1_desc,
                        note,
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = out_ptr;
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }
                if let Some(imp) = self.objc_lookup_imp_for_super_call(current_class, &selector, true) {
                    self.runtime.objc.objc_super_msgsend_dispatches = self.runtime.objc.objc_super_msgsend_dispatches.saturating_add(1);
                    self.runtime.objc.objc_real_msgsend_dispatches = self.runtime.objc.objc_real_msgsend_dispatches.saturating_add(1);
                    self.runtime.objc.objc_last_real_selector = Some(format!("super::stret::{}", selector));
                    let detail = format!(
                        "real objc_msgSendSuper2_stret(out={}, super={}, receiver={}, class={}, sel={}, arg1={}, imp=0x{:08x})",
                        self.describe_ptr(out_ptr),
                        self.describe_ptr(super_ptr),
                        receiver_desc,
                        class_desc,
                        selector,
                        arg1_desc,
                        imp,
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[1] = receiver;
                    self.cpu.regs[15] = imp & !1;
                    self.cpu.thumb = (imp & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }
                let zero_len = match selector.as_str() {
                    "contentSize" | "position" | "origin" | "anchorPoint" | "anchorPointInPixels" | "winSize" | "size" | "viewSize" => 8,
                    _ => 16,
                };
                self.write_bytes(out_ptr, &vec![0; zero_len as usize])?;
                let detail = format!(
                    "hle/fallback objc_msgSendSuper2_stret(out={}, super={}, receiver={}, class={}, sel={}, arg1={}, zero={})",
                    self.describe_ptr(out_ptr),
                    self.describe_ptr(super_ptr),
                    receiver_desc,
                    class_desc,
                    selector,
                    arg1_desc,
                    zero_len,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = out_ptr;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "objc_msgSend_stret" => {
                self.install_uikit_labels();
                let out_ptr = self.cpu.regs[0];
                let receiver = self.cpu.regs[1];
                let selector = self
                    .objc_read_selector_name(self.cpu.regs[2])
                    .unwrap_or_else(|| format!("sel@0x{:08x}", self.cpu.regs[2]));
                let arg1 = self.cpu.regs[3];
                let receiver_desc = self.describe_ptr(receiver);
                let arg1_desc = self.describe_ptr(arg1);
                self.record_objc_selector(
                    &format!("stret::{}", selector),
                    format!("stret {} recv={} arg1={} out={}", selector, receiver_desc, arg1_desc, self.describe_ptr(out_ptr)),
                );
                if let Some(control) = self.maybe_dispatch_synthetic_input_objc_msgsend_stret(
                    index,
                    current_pc,
                    out_ptr,
                    receiver,
                    &selector,
                    arg1,
                    &receiver_desc,
                    &arg1_desc,
                )? {
                    return Ok(Some(control));
                }
                let handled = match selector.as_str() {
                    "bounds" | "applicationFrame" | "frame" => {
                        let bits = self.ui_rect_bits_for_selector(receiver, selector.as_str());
                        self.write_cg_rect_bits_to_guest(out_ptr, bits)?;
                        Some(Self::ui_rect_bits_to_string(bits))
                    }
                    "winSize" | "viewSize" => {
                        let (width, height, authority) = self.ui_authoritative_surface_size();
                        self.write_cg_size_to_guest(out_ptr, width, height)?;
                        Some(format!("CGSize({},{}) authority={}", width, height, authority))
                    }
                    "contentSize" | "size" => {
                        let (fallback_w, fallback_h, _) = self.ui_authoritative_surface_size();
                        let (width, height) = self
                            .runtime.graphics.synthetic_sprites
                            .get(&receiver)
                            .map(|state| (state.width, state.height))
                            .filter(|(w, h)| *w != 0 || *h != 0)
                            .or_else(|| self.synthetic_texture_dimensions(receiver))
                            .unwrap_or((fallback_w, fallback_h));
                        self.write_cg_size_to_guest(out_ptr, width, height)?;
                        Some(format!("CGSize({},{})", width, height))
                    }
                    "position" | "origin" => {
                        let bits = self
                            .runtime.graphics.synthetic_sprites
                            .get(&receiver)
                            .map(|state| [state.position_x_bits, state.position_y_bits])
                            .unwrap_or([0, 0]);
                        self.write_cg_point_to_guest_bits(out_ptr, bits)?;
                        Some(format!("CGPoint({:.3},{:.3})", Self::f32_from_bits(bits[0]), Self::f32_from_bits(bits[1])))
                    }
                    "anchorPoint" | "anchorPointInPixels" => {
                        let default_anchor = if self.diag.object_labels.get(&receiver).map(|label| label.contains("CCSprite")).unwrap_or(false) { 0.5f32.to_bits() } else { 0 };
                        let bits = self
                            .runtime.graphics.synthetic_sprites
                            .get(&receiver)
                            .map(|state| {
                                if selector == "anchorPointInPixels" {
                                    let content_w = if state.untrimmed_explicit && state.untrimmed_w_bits != 0 {
                                        Self::f32_from_bits(state.untrimmed_w_bits)
                                    } else {
                                        state.width as f32
                                    };
                                    let content_h = if state.untrimmed_explicit && state.untrimmed_h_bits != 0 {
                                        Self::f32_from_bits(state.untrimmed_h_bits)
                                    } else {
                                        state.height as f32
                                    };
                                    [
                                        if state.anchor_pixels_explicit {
                                            state.anchor_pixels_x_bits
                                        } else {
                                            ((if state.anchor_x_bits != 0 { Self::f32_from_bits(state.anchor_x_bits) } else { Self::f32_from_bits(default_anchor) }) * content_w).to_bits()
                                        },
                                        if state.anchor_pixels_explicit {
                                            state.anchor_pixels_y_bits
                                        } else {
                                            ((if state.anchor_y_bits != 0 { Self::f32_from_bits(state.anchor_y_bits) } else { Self::f32_from_bits(default_anchor) }) * content_h).to_bits()
                                        },
                                    ]
                                } else {
                                    [
                                        if state.anchor_x_bits != 0 { state.anchor_x_bits } else { default_anchor },
                                        if state.anchor_y_bits != 0 { state.anchor_y_bits } else { default_anchor },
                                    ]
                                }
                            })
                            .unwrap_or([default_anchor, default_anchor]);
                        self.write_cg_point_to_guest_bits(out_ptr, bits)?;
                        Some(format!("CGPoint({:.3},{:.3})", Self::f32_from_bits(bits[0]), Self::f32_from_bits(bits[1])))
                    }
                    _ => None,
                };
                if let Some(note) = handled {
                    self.maybe_trace_widget_selector_state(receiver, &selector, "stret-hle");
                    let detail = format!(
                        "hle objc_msgSend_stret(out={}, receiver={}, sel={}, arg1={}, wrote={})",
                        self.describe_ptr(out_ptr),
                        receiver_desc,
                        selector,
                        arg1_desc,
                        note,
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = out_ptr;
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }
                if let Some(imp) = self.objc_lookup_imp_for_receiver(receiver, &selector) {
                    self.maybe_trace_widget_selector_state(receiver, &selector, "stret-real");
                    self.runtime.objc.objc_real_msgsend_dispatches = self.runtime.objc.objc_real_msgsend_dispatches.saturating_add(1);
                    self.runtime.objc.objc_last_real_selector = Some(format!("stret::{}", selector));
                    let detail = format!(
                        "real objc_msgSend_stret(out={}, receiver={}, sel={}, arg1={}, imp=0x{:08x})",
                        self.describe_ptr(out_ptr),
                        receiver_desc,
                        selector,
                        arg1_desc,
                        imp,
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.observe_real_uikit_objc_msgsend(receiver, &selector, arg1, 0);
                    self.cpu.regs[15] = imp & !1;
                    self.cpu.thumb = (imp & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }
                let zero_len = match selector.as_str() {
                    "contentSize" | "position" | "origin" | "anchorPoint" | "anchorPointInPixels" | "winSize" | "size" | "viewSize" => 8,
                    _ => 16,
                };
                self.write_bytes(out_ptr, &vec![0; zero_len as usize])?;
                let detail = format!(
                    "hle/fallback objc_msgSend_stret(out={}, receiver={}, sel={}, arg1={}, zero={})",
                    self.describe_ptr(out_ptr),
                    receiver_desc,
                    selector,
                    arg1_desc,
                    zero_len,
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = out_ptr;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "objc_msgSendSuper" | "objc_msgSendSuper2" => {
                self.install_uikit_labels();
                let selector = self
                    .objc_read_selector_name(self.cpu.regs[1])
                    .unwrap_or_else(|| format!("sel@0x{:08x}", self.cpu.regs[1]));
                let super_ptr = self.cpu.regs[0];
                let receiver = self.read_u32_le(super_ptr).unwrap_or(0);
                let current_class = self.read_u32_le(super_ptr.wrapping_add(4)).unwrap_or(0);
                let arg2 = self.cpu.regs[2];
                let arg3 = self.cpu.regs[3];
                let receiver_desc = self.describe_ptr(receiver);
                let arg2_desc = self.describe_ptr(arg2);
                let arg3_desc = self.describe_ptr(arg3);
                let class_desc = self
                    .objc_class_name_for_ptr(current_class)
                    .unwrap_or_else(|| format!("class@0x{current_class:08x}"));
                self.record_objc_selector(
                    &format!("super::{}", selector),
                    format!("super::{} recv={} class={} arg2={} arg3={}", selector, receiver_desc, class_desc, arg2_desc, arg3_desc),
                );
                let mut scene_progress_destination_updated = false;
                if selector.starts_with("initWithDestinationScene:") {
                    let origin = format!("{}::{}", label, selector);
                    self.note_synthetic_splash_destination(receiver, arg2, &origin);
                    scene_progress_destination_updated = true;
                }
                self.push_scene_progress_selector_event(&label, receiver, &class_desc, &selector, arg2, arg3, None, scene_progress_destination_updated);
                self.push_scheduler_trace_selector_event(&label, receiver, &class_desc, &selector, arg2, arg3, None);
                self.runtime.objc.objc_last_super_selector = Some(selector.clone());
                self.runtime.objc.objc_last_super_receiver = Some(receiver);
                self.runtime.objc.objc_last_super_class = Some(current_class);
                self.runtime.objc.objc_last_super_imp = None;

                if selector == "alloc" || selector == "allocWithZone:" || selector == "new" {
                    if selector == "alloc" || selector == "new" {
                        self.runtime.objc.objc_alloc_calls = self.runtime.objc.objc_alloc_calls.saturating_add(1);
                    } else {
                        self.runtime.objc.objc_alloc_with_zone_calls = self.runtime.objc.objc_alloc_with_zone_calls.saturating_add(1);
                    }
                    let result = self.objc_hle_alloc_like(receiver, 0, &format!("super::{selector}"));
                    let detail = format!(
                        "hle/alloc {}(super={}, recv={}, class={}, sel={}, arg2={}, arg3={}, result={})",
                        label,
                        self.describe_ptr(super_ptr),
                        receiver_desc,
                        class_desc,
                        selector,
                        arg2_desc,
                        arg3_desc,
                        self.describe_ptr(result),
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = result;
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }

                if selector == "init" && self.should_fastpath_cocos_super_init(&class_desc, receiver) {
                    let result = if receiver != 0 { receiver } else { 0 };
                    self.objc_note_init_result(receiver, result);
                    let note = self.apply_cocos_init_defaults(receiver, &class_desc, "super init fastpath");
                    let mut detail = format!(
                        "hle/fastpath {}(super={}, recv={}, class={}, sel={}, arg2={}, arg3={}, result={})",
                        label,
                        self.describe_ptr(super_ptr),
                        receiver_desc,
                        class_desc,
                        selector,
                        arg2_desc,
                        arg3_desc,
                        self.describe_ptr(result),
                    );
                    if !note.is_empty() {
                        detail.push_str(&format!(", note={}", note));
                    }
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = result;
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }

                let receiver_is_view_like = self.ui_object_is_view_like(receiver) || self.ui_object_is_layer_like(receiver);
                if selector.starts_with("initWithFrame:") && receiver_is_view_like {
                    let result = if receiver != 0 { receiver } else { 0 };
                    self.objc_note_init_result(receiver, result);
                    let mut note = String::new();
                    if let Some((bits, source)) = self.read_msgsend_rect_arg() {
                        self.ui_set_frame_bits(receiver, bits);
                        self.ui_set_bounds_bits(receiver, Self::ui_rect_size_bits(bits));
                        note = format!(" frame={} via={}", Self::ui_rect_bits_to_string(bits), source);
                    }
                    let detail = format!(
                        "hle/fastpath {}(super={}, recv={}, class={}, sel={}, arg2={}, arg3={}, result={}){}",
                        label,
                        self.describe_ptr(super_ptr),
                        receiver_desc,
                        class_desc,
                        selector,
                        arg2_desc,
                        arg3_desc,
                        self.describe_ptr(result),
                        note,
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = result;
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }

                if let Some((result, note)) = self.maybe_handle_cocos_fastpath(&selector, receiver, arg2, arg3) {
                    let detail = format!(
                        "hle/fastpath {}(super={}, recv={}, class={}, sel={}, arg2={}, arg3={}, result={})",
                        label,
                        self.describe_ptr(super_ptr),
                        receiver_desc,
                        class_desc,
                        selector,
                        arg2_desc,
                        arg3_desc,
                        self.describe_ptr(result),
                    );
                    let mut detail = detail;
                    if !note.is_empty() {
                        detail.push_str(&format!(", note={}", note));
                    }
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = result;
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }

                let skip_current_class = label == "objc_msgSendSuper2";
                if let Some(imp) = self.objc_lookup_imp_for_super_call(current_class, &selector, skip_current_class) {
                    self.runtime.objc.objc_super_msgsend_dispatches = self.runtime.objc.objc_super_msgsend_dispatches.saturating_add(1);
                    self.runtime.objc.objc_real_msgsend_dispatches = self.runtime.objc.objc_real_msgsend_dispatches.saturating_add(1);
                    self.runtime.objc.objc_last_real_selector = Some(format!("super::{}", selector));
                    self.runtime.objc.objc_last_super_imp = Some(imp);
                    let detail = format!(
                        "real {}(super={}, recv={}, class={}, sel={}, arg2={}, arg3={}, imp=0x{:08x})",
                        label,
                        self.describe_ptr(super_ptr),
                        receiver_desc,
                        class_desc,
                        selector,
                        arg2_desc,
                        arg3_desc,
                        imp,
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = receiver;
                    self.cpu.regs[15] = imp & !1;
                    self.cpu.thumb = (imp & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }

                let should_return_receiver = receiver != 0
                    && (selector.starts_with("init")
                        || selector == "retain"
                        || selector == "autorelease"
                        || selector == "self");
                let result = if should_return_receiver { receiver } else { 0 };
                if selector.starts_with("init") {
                    self.objc_note_init_result(receiver, result);
                }
                self.runtime.objc.objc_super_msgsend_fallback_returns = self.runtime.objc.objc_super_msgsend_fallback_returns.saturating_add(1);
                let detail = format!(
                    "hle/fallback {}(super={}, recv={}, class={}, sel={}, arg2={}, arg3={}, result={})",
                    label,
                    self.describe_ptr(super_ptr),
                    receiver_desc,
                    class_desc,
                    selector,
                    arg2_desc,
                    arg3_desc,
                    self.describe_ptr(result),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = result;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            "objc_msgSend" => {
                self.install_uikit_labels();
                let selector = self
                    .objc_read_selector_name(self.cpu.regs[1])
                    .unwrap_or_else(|| format!("sel@0x{:08x}", self.cpu.regs[1]));
                let receiver = self.cpu.regs[0];
                let arg2 = self.cpu.regs[2];
                let arg3 = self.cpu.regs[3];
                let receiver_desc = self.describe_ptr(receiver);
                let arg2_desc = self.describe_ptr(arg2);
                let arg3_desc = self.describe_ptr(arg3);
                self.record_objc_selector(
                    &selector,
                    format!("{} recv={} arg2={} arg3={}", selector, receiver_desc, arg2_desc, arg3_desc),
                );
                let mut scene_progress_destination_updated = false;
                if selector.starts_with("initWithDestinationScene:") {
                    self.note_synthetic_splash_destination(receiver, arg2, "objc_msgSend");
                    scene_progress_destination_updated = true;
                }
                let receiver_class_desc = self.objc_class_name_for_receiver(receiver).unwrap_or_default();
                self.objc_note_observed_receiver(receiver, &selector);
                self.push_scene_progress_selector_event("objc_msgSend", receiver, &receiver_class_desc, &selector, arg2, arg3, None, scene_progress_destination_updated);
                self.push_scheduler_trace_selector_event("objc_msgSend", receiver, &receiver_class_desc, &selector, arg2, arg3, None);
                if selector == "class" {
                    let result = self.objc_class_ptr_for_receiver(receiver).unwrap_or(receiver);
                    let detail = format!(
                        "hle objc_msgSend(receiver={}, sel={}, arg2={}, arg3={}, result={})",
                        receiver_desc,
                        selector,
                        arg2_desc,
                        arg3_desc,
                        self.describe_ptr(result),
                    );
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = result;
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }
                if selector == "alloc" || selector == "allocWithZone:" || selector == "new" {
                    if selector == "alloc" || selector == "new" {
                        self.runtime.objc.objc_alloc_calls = self.runtime.objc.objc_alloc_calls.saturating_add(1);
                    } else {
                        self.runtime.objc.objc_alloc_with_zone_calls = self.runtime.objc.objc_alloc_with_zone_calls.saturating_add(1);
                    }

                    let mut custom_alloc_imp = None;
                    let mut custom_alloc_class = None::<String>;
                    if receiver != 0 && self.runtime.objc.objc_classes_by_ptr.contains_key(&receiver) {
                        self.ensure_objc_metadata_indexed();
                        self.ensure_objc_class_hierarchy_indexed(receiver);
                        if let Some(class_name) = self
                            .runtime
                            .objc
                            .objc_classes_by_ptr
                            .get(&receiver)
                            .map(|info| info.name.clone())
                        {
                            if Self::audio_is_objc_audio_class(&class_name) {
                                self.audio_trace_note_objc_audio_selector(&class_name, &selector, None, false);
                            }
                            // Important: do NOT globally prefer real +alloc for every class that
                            // defines a meta-method. Alive4ever has startup classes (for example
                            // Director/FastDirector paths) whose custom allocs depend on broader
                            // runtime semantics and can regress bootstrap when we bypass the stable
                            // HLE alloc path.
                            //
                            // We only escape to the guest allocator path for classes whose custom
                            // meta-alloc participates in the network/bootstrap ownership chain.
                            let allow_real_custom_alloc = self.objc_should_prefer_real_meta_alloc(receiver, &class_name, &selector);
                            if allow_real_custom_alloc {
                                custom_alloc_class = Some(class_name);
                                custom_alloc_imp = self
                                    .runtime
                                    .objc
                                    .objc_classes_by_ptr
                                    .get(&receiver)
                                    .and_then(|info| info.meta_methods.get(&selector).copied())
                                    .filter(|imp| *imp != 0);
                            }
                        }
                    }

                    if let Some(imp) = custom_alloc_imp {
                        self.runtime.objc.objc_real_msgsend_dispatches = self.runtime.objc.objc_real_msgsend_dispatches.saturating_add(1);
                        self.runtime.objc.objc_last_real_selector = Some(selector.clone());
                        let detail = format!(
                            "real/custom-alloc objc_msgSend(receiver={}, sel={}, arg2={}, arg3={}, imp=0x{:08x}, class={})",
                            receiver_desc,
                            selector,
                            arg2_desc,
                            arg3_desc,
                            imp,
                            custom_alloc_class.as_deref().unwrap_or("<unknown>"),
                        );
                        self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                        self.observe_real_uikit_objc_msgsend(receiver, &selector, arg2, arg3);
                        self.arm_real_audio_selector_return_watch(
                            &selector,
                            receiver,
                            custom_alloc_class.as_deref().unwrap_or(""),
                            arg2,
                            arg3,
                            imp,
                            current_pc,
                            self.cpu.regs[14],
                        );
                        let network_watch_aux = if selector == "initWithRequest:delegate:startImmediately:" {
                            self.peek_stack_u32(0).unwrap_or(0)
                        } else {
                            0
                        };
                        self.arm_real_network_selector_return_watch(&selector, receiver, arg2, arg3, network_watch_aux, current_pc, self.cpu.regs[14]);
                        self.cpu.regs[15] = imp & !1;
                        self.cpu.thumb = (imp & 1) != 0;
                        return Ok(Some(StepControl::Continue));
                    }

                    let result = self.objc_hle_alloc_like(receiver, 0, &selector);
                    let mut detail = format!(
                        "hle objc_msgSend(receiver={}, sel={}, arg2={}, arg3={}, result={})",
                        receiver_desc,
                        selector,
                        arg2_desc,
                        arg3_desc,
                        self.describe_ptr(result),
                    );
                    if let Some(class_name) = self.runtime.objc.objc_last_alloc_class.clone() {
                        detail.push_str(&format!(", note=objc alloc class={class_name}"));
                    }
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = result;
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }
                if let Some((result, note)) = self.maybe_handle_objc_singleton_fastpath(&selector, receiver) {
                    let detail = format!(
                        "hle/fastpath objc_msgSend(receiver={}, sel={}, arg2={}, arg3={}, result={})",
                        receiver_desc,
                        selector,
                        arg2_desc,
                        arg3_desc,
                        self.describe_ptr(result),
                    );
                    let mut detail = detail;
                    if !note.is_empty() {
                        detail.push_str(&format!(", note={}", note));
                    }
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.cpu.regs[0] = result;
                    self.cpu.regs[15] = self.cpu.regs[14] & !1;
                    self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }
                if let Some(control) = self.maybe_dispatch_synthetic_input_objc_msgsend(
                    index,
                    current_pc,
                    receiver,
                    &selector,
                    arg2,
                    arg3,
                    &receiver_desc,
                    &arg2_desc,
                    &arg3_desc,
                )? {
                    return Ok(Some(control));
                }

                if let Some(control) = self.maybe_dispatch_cocos_objc_msgsend(
                    index,
                    current_pc,
                    receiver,
                    &selector,
                    arg2,
                    arg3,
                    &receiver_desc,
                    &arg2_desc,
                    &arg3_desc,
                )? {
                    return Ok(Some(control));
                }
                if let Some(imp) = self.objc_lookup_imp_for_receiver(receiver, &selector) {
                    self.runtime.objc.objc_real_msgsend_dispatches = self.runtime.objc.objc_real_msgsend_dispatches.saturating_add(1);
                    self.runtime.objc.objc_last_real_selector = Some(selector.clone());
                    let mut detail = format!(
                        "real objc_msgSend(receiver={}, sel={}, arg2={}, arg3={}, imp=0x{:08x})",
                        receiver_desc,
                        selector,
                        arg2_desc,
                        arg3_desc,
                        imp,
                    );
                    if let Some(note) = self.classify_escaped_fastpath(&selector, receiver, arg2, arg3) {
                        detail.push_str(&format!(", note={}", note));
                    }
                    self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                    self.observe_real_uikit_objc_msgsend(receiver, &selector, arg2, arg3);
                    self.arm_real_audio_selector_return_watch(
                        &selector,
                        receiver,
                        &receiver_class_desc,
                        arg2,
                        arg3,
                        imp,
                        current_pc,
                        self.cpu.regs[14],
                    );
                    let network_watch_aux = if selector == "initWithRequest:delegate:startImmediately:" {
                        self.peek_stack_u32(0).unwrap_or(0)
                    } else {
                        0
                    };
                    self.arm_real_network_selector_return_watch(&selector, receiver, arg2, arg3, network_watch_aux, current_pc, self.cpu.regs[14]);
                    self.note_scheduler_selector(&selector, receiver);
                    self.arm_real_scheduler_selector_return_watch(&selector, receiver, arg2, arg3, current_pc, self.cpu.regs[14]);
                    self.cpu.regs[15] = imp & !1;
                    self.cpu.thumb = (imp & 1) != 0;
                    return Ok(Some(StepControl::Continue));
                }
                if let Some(control) = self.maybe_dispatch_graphics_objc_msgsend(
                    index,
                    current_pc,
                    receiver,
                    &selector,
                    arg2,
                    arg3,
                    &receiver_desc,
                    &arg2_desc,
                    &arg3_desc,
                )? {
                    return Ok(Some(control));
                }
                if let Some(control) = self.maybe_dispatch_network_objc_msgsend(
                    index,
                    current_pc,
                    receiver,
                    &selector,
                    arg2,
                    arg3,
                    &receiver_desc,
                    &arg2_desc,
                    &arg3_desc,
                )? {
                    return Ok(Some(control));
                }
                if let Some(control) = self.maybe_dispatch_uikit_objc_msgsend(
                    index,
                    current_pc,
                    receiver,
                    &selector,
                    arg2,
                    arg3,
                    &receiver_desc,
                    &arg2_desc,
                    &arg3_desc,
                )? {
                    return Ok(Some(control));
                }
                self.note_scheduler_selector(&selector, receiver);
                let mut note: Option<String> = None;
                let result = match selector.as_str() {
                    "name" if self.runtime.ui_runtime.synthetic_notifications.contains_key(&receiver) => {
                        self.synthetic_notification_fields(receiver).map(|(name_ptr, _, _)| name_ptr).unwrap_or(0)
                    },
                    "object" if self.runtime.ui_runtime.synthetic_notifications.contains_key(&receiver) => {
                        self.synthetic_notification_fields(receiver).map(|(_, object, _)| object).unwrap_or(0)
                    },
                    "userInfo" => self.synthetic_notification_fields(receiver).map(|(_, _, user_info)| user_info).unwrap_or_else(|| self.runtime.scheduler.timers.foundation_timers.get(&receiver).map(|entry| entry.user_info).unwrap_or(0)),
                    "playbackState" if self.runtime.ui_runtime.movie_players.contains_key(&receiver) => {
                        self.runtime.ui_runtime.movie_players.get(&receiver).map(|state| {
                            if state.is_playing { 1 } else if state.pause_count > 0 && state.playback_remaining_ticks > 0 { 2 } else { 0 }
                        }).unwrap_or(0)
                    },
                    "loadState" if self.runtime.ui_runtime.movie_players.contains_key(&receiver) => {
                        self.runtime.ui_runtime.movie_players.get(&receiver).map(|state| if state.prepared || state.is_playing { 3 } else { 0 }).unwrap_or(0)
                    },
                    "mainBundle" => HLE_FAKE_MAIN_BUNDLE,
                    "bundleWithPath:" => {
                        let path_text = self.guest_string_value(arg2).unwrap_or_default();
                        if let Some(root) = self.resolve_bundle_directory_path(&path_text) {
                            let label = format!("NSBundle.bundle<'{}'>", root.file_name().and_then(|v| v.to_str()).unwrap_or("bundle"));
                            let obj = self.materialize_bundle_object(&label, root.clone());
                            note = Some(format!("bundleWithPath hit path={}", root.display()));
                            obj
                        } else {
                            note = Some(format!("bundleWithPath miss path={}", path_text));
                            0
                        }
                    }
                    "signatureWithObjCTypes:" => {
                        let objc_types = self.guest_string_value(arg2);
                        let sig = self.create_synthetic_method_signature(receiver, None, objc_types.clone(), selector.as_str());
                        self.trace_synthetic_invocation(format!("signatureWithObjCTypes receiver={} objcTypes={} result={}", self.describe_ptr(receiver), objc_types.as_deref().unwrap_or("<none>"), self.describe_ptr(sig)));
                        sig
                    }
                    "methodSignatureForSelector:" | "instanceMethodSignatureForSelector:" => {
                        let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{:08x}", arg2));
                        let sig = self.create_synthetic_method_signature(receiver, Some(selector_name.clone()), None, selector.as_str());
                        self.trace_synthetic_invocation(format!("methodSignature receiver={} selector={} result={}", self.describe_ptr(receiver), selector_name, self.describe_ptr(sig)));
                        sig
                    }
                    "invocationWithMethodSignature:" => {
                        let inv = self.create_synthetic_invocation(receiver, arg2, selector.as_str());
                        self.trace_synthetic_invocation(format!("create receiver={} signature={} result={}", self.describe_ptr(receiver), self.describe_ptr(arg2), self.describe_ptr(inv)));
                        inv
                    }
                    "scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:" => {
                        let interval_bits = self.nstimeinterval_f32_bits_from_regs(arg2, arg3);
                        let target = self.peek_stack_u32(0).unwrap_or(arg3);
                        let selector_ptr = self.peek_stack_u32(1).unwrap_or(0);
                        let selector_name = self
                            .decode_cocos_schedule_selector_name(selector_ptr)
                            .unwrap_or_else(|| format!("0x{:08x}", selector_ptr));
                        let user_info = self.peek_stack_u32(2).unwrap_or(0);
                        let repeats = self.peek_stack_u32(3).unwrap_or(0) != 0;
                        let timer = self.register_foundation_timer(0, target, &selector_name, interval_bits, repeats, user_info, true, selector.as_str());
                        note = Some(format!(
                            "scheduledTimer intervalBits=0x{:08x} target={} selector={} repeats={} userInfo={}",
                            interval_bits,
                            self.describe_ptr(target),
                            selector_name,
                            if repeats { "YES" } else { "NO" },
                            self.describe_ptr(user_info)
                        ));
                        timer
                    }
                    "timerWithTimeInterval:target:selector:userInfo:repeats:" => {
                        let interval_bits = self.nstimeinterval_f32_bits_from_regs(arg2, arg3);
                        let target = self.peek_stack_u32(0).unwrap_or(arg3);
                        let selector_ptr = self.peek_stack_u32(1).unwrap_or(0);
                        let selector_name = self
                            .decode_cocos_schedule_selector_name(selector_ptr)
                            .unwrap_or_else(|| format!("0x{:08x}", selector_ptr));
                        let user_info = self.peek_stack_u32(2).unwrap_or(0);
                        let repeats = self.peek_stack_u32(3).unwrap_or(0) != 0;
                        let timer = self.register_foundation_timer(0, target, &selector_name, interval_bits, repeats, user_info, false, selector.as_str());
                        note = Some(format!(
                            "timer intervalBits=0x{:08x} target={} selector={} repeats={} userInfo={}",
                            interval_bits,
                            self.describe_ptr(target),
                            selector_name,
                            if repeats { "YES" } else { "NO" },
                            self.describe_ptr(user_info)
                        ));
                        timer
                    }
                    "timerWithTarget:selector:" | "timerWithTarget:selector:interval:" | "timerWithTarget:selector:repeat:" | "timerWithTarget:selector:interval:repeat:" => {
                        let selector_name = self.decode_cocos_schedule_selector_name(arg3).unwrap_or_else(|| format!("0x{:08x}", arg3));
                        let mut interval_bits = 0u32;
                        let mut repeats = false;
                        if selector == "timerWithTarget:selector:interval:" || selector == "timerWithTarget:selector:interval:repeat:" {
                            interval_bits = self.peek_stack_u32(0).unwrap_or(0);
                        }
                        if selector == "timerWithTarget:selector:repeat:" {
                            repeats = self.peek_stack_u32(0).unwrap_or(0) != 0;
                        } else if selector == "timerWithTarget:selector:interval:repeat:" {
                            repeats = self.peek_stack_u32(1).unwrap_or(0) != 0;
                        }
                        let timer = self.register_foundation_timer(0, arg2, &selector_name, interval_bits, repeats, 0, false, selector.as_str());
                        note = Some(format!("timerWithTarget target={} selector={} intervalBits=0x{:08x} repeats={}", self.describe_ptr(arg2), selector_name, interval_bits, if repeats { "YES" } else { "NO" }));
                        timer
                    }
                    "performSelector:" => {
                        let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{:08x}", arg2));
                        let invoked = self.invoke_objc_selector_now(receiver, &selector_name, 0, 0, 180_000, selector.as_str());
                        note = Some(format!("performSelector target={} selector={} invoked={}", self.describe_ptr(receiver), selector_name, if invoked { "YES" } else { "NO" }));
                        receiver
                    }
                    "performSelector:withObject:" => {
                        let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{:08x}", arg2));
                        let invoked = self.invoke_objc_selector_now(receiver, &selector_name, arg3, 0, 180_000, selector.as_str());
                        note = Some(format!("performSelector target={} selector={} object={} invoked={}", self.describe_ptr(receiver), selector_name, self.describe_ptr(arg3), if invoked { "YES" } else { "NO" }));
                        receiver
                    }
                    "performSelector:withObject:withObject:" => {
                        let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{:08x}", arg2));
                        let second_arg = self.peek_stack_u32(0).unwrap_or(0);
                        let invoked = self.invoke_objc_selector_now(receiver, &selector_name, arg3, second_arg, 180_000, selector.as_str());
                        note = Some(format!("performSelector target={} selector={} object={} secondObject={} invoked={}", self.describe_ptr(receiver), selector_name, self.describe_ptr(arg3), self.describe_ptr(second_arg), if invoked { "YES" } else { "NO" }));
                        receiver
                    }
                    "performSelector:withObject:afterDelay:" | "performSelector:withObject:afterDelay:inModes:" => {
                        let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{:08x}", arg2));
                        let delay_bits = self.nstimeinterval_f32_bits_from_stack_words(0);
                        self.schedule_delayed_selector(receiver, &selector_name, arg3, delay_bits, selector.as_str());
                        note = Some(format!(
                            "performSelector delayed target={} selector={} object={} delayBits=0x{:08x} delaySecs={:.6}",
                            self.describe_ptr(receiver),
                            selector_name,
                            self.describe_ptr(arg3),
                            delay_bits,
                            Self::f32_from_bits(delay_bits)
                        ));
                        receiver
                    }
                    "performSelector:target:argument:order:modes:" => {
                        let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{:08x}", arg2));
                        let object_arg = self.peek_stack_u32(0).unwrap_or(0);
                        self.schedule_delayed_selector(arg3, &selector_name, object_arg, 0, selector.as_str());
                        note = Some(format!("performSelector ordered target={} selector={} object={} queued=YES", self.describe_ptr(arg3), selector_name, self.describe_ptr(object_arg)));
                        receiver
                    }
                    "performSelectorOnMainThread:withObject:waitUntilDone:" => {
                        let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{:08x}", arg2));
                        let wait = self.peek_stack_u32(0).unwrap_or(0) != 0;
                        if wait {
                            let invoked = self.invoke_objc_selector_now(receiver, &selector_name, arg3, 0, 180_000, selector.as_str());
                            note = Some(format!("performSelectorOnMainThread target={} selector={} object={} wait=YES invoked={}", self.describe_ptr(receiver), selector_name, self.describe_ptr(arg3), if invoked { "YES" } else { "NO" }));
                        } else {
                            self.schedule_delayed_selector(receiver, &selector_name, arg3, 0, selector.as_str());
                            note = Some(format!("performSelectorOnMainThread target={} selector={} object={} wait=NO queued=YES", self.describe_ptr(receiver), selector_name, self.describe_ptr(arg3)));
                        }
                        receiver
                    }
                    "performSelector:onThread:withObject:waitUntilDone:" => {
                        let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{:08x}", arg2));
                        let wait = self.peek_stack_u32(1).unwrap_or(0) != 0;
                        if wait {
                            let invoked = self.invoke_objc_selector_now(receiver, &selector_name, self.peek_stack_u32(0).unwrap_or(0), 0, 180_000, selector.as_str());
                            note = Some(format!("performSelectorOnThread target={} selector={} wait=YES invoked={}", self.describe_ptr(receiver), selector_name, if invoked { "YES" } else { "NO" }));
                        } else {
                            self.schedule_delayed_selector(receiver, &selector_name, self.peek_stack_u32(0).unwrap_or(0), 0, selector.as_str());
                            note = Some(format!("performSelectorOnThread target={} selector={} wait=NO queued=YES", self.describe_ptr(receiver), selector_name));
                        }
                        receiver
                    }
                    "methodSignature" => self.runtime.scheduler.invocations.invocations.get(&receiver).map(|entry| entry.signature).unwrap_or(0),
                    "retainArguments" => {
                        if let Some(entry) = self.runtime.scheduler.invocations.invocations.get_mut(&receiver) {
                            entry.retained_arguments = true;
                            self.trace_synthetic_invocation(format!("retainArguments invocation={}", self.describe_ptr(receiver)));
                        }
                        receiver
                    }
                    "setTarget:" => {
                        if let Some(entry) = self.runtime.scheduler.invocations.invocations.get_mut(&receiver) {
                            entry.target = arg2;
                            self.trace_synthetic_invocation(format!("setTarget invocation={} target={}", self.describe_ptr(receiver), self.describe_ptr(arg2)));
                        }
                        receiver
                    }
                    "target" => self.runtime.scheduler.invocations.invocations.get(&receiver).map(|entry| entry.target).unwrap_or(0),
                    "setSelector:" => {
                        let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{:08x}", arg2));
                        if let Some(entry) = self.runtime.scheduler.invocations.invocations.get_mut(&receiver) {
                            entry.selector_ptr = arg2;
                            entry.selector_name = Some(selector_name.clone());
                        }
                        self.trace_synthetic_invocation(format!("setSelector invocation={} selector={} selPtr={}", self.describe_ptr(receiver), selector_name, self.describe_ptr(arg2)));
                        receiver
                    }
                    "selector" => self.runtime.scheduler.invocations.invocations.get(&receiver).map(|entry| entry.selector_ptr).unwrap_or(0),
                    "setArgument:atIndex:" => {
                        let value = self.synthetic_invocation_argument_value(arg2);
                        if let Some(entry) = self.runtime.scheduler.invocations.invocations.get_mut(&receiver) {
                            entry.arguments.insert(arg3, value);
                        }
                        self.trace_synthetic_invocation(format!("setArgument invocation={} index={} location={} value={}", self.describe_ptr(receiver), arg3, self.describe_ptr(arg2), self.describe_ptr(value)));
                        receiver
                    }
                    "getArgument:atIndex:" => {
                        let value = self.runtime.scheduler.invocations.invocations.get(&receiver).and_then(|entry| entry.arguments.get(&arg3).copied()).unwrap_or(0);
                        if arg2 != 0 {
                            let _ = self.write_u32_le(arg2, value);
                        }
                        self.trace_synthetic_invocation(format!("getArgument invocation={} index={} out={} value={}", self.describe_ptr(receiver), arg3, self.describe_ptr(arg2), self.describe_ptr(value)));
                        receiver
                    }
                    "setReturnValue:" => {
                        let value = self.synthetic_invocation_argument_value(arg2);
                        if let Some(entry) = self.runtime.scheduler.invocations.invocations.get_mut(&receiver) {
                            entry.last_result = value;
                        }
                        self.trace_synthetic_invocation(format!("setReturnValue invocation={} location={} value={}", self.describe_ptr(receiver), self.describe_ptr(arg2), self.describe_ptr(value)));
                        receiver
                    }
                    "getReturnValue:" => {
                        let value = self.runtime.scheduler.invocations.invocations.get(&receiver).map(|entry| entry.last_result).unwrap_or(0);
                        if arg2 != 0 {
                            let _ = self.write_u32_le(arg2, value);
                        }
                        self.trace_synthetic_invocation(format!("getReturnValue invocation={} out={} value={}", self.describe_ptr(receiver), self.describe_ptr(arg2), self.describe_ptr(value)));
                        receiver
                    }
                    "invoke" | "invokeWithTarget:" => {
                        let (target, selector_name, arg_a, arg_b) = if let Some(entry) = self.runtime.scheduler.invocations.invocations.get(&receiver) {
                            (
                                if selector == "invokeWithTarget:" && arg2 != 0 { arg2 } else { entry.target },
                                entry.selector_name.clone().unwrap_or_else(|| format!("0x{:08x}", entry.selector_ptr)),
                                entry.arguments.get(&2).copied().unwrap_or(0),
                                entry.arguments.get(&3).copied().unwrap_or(0),
                            )
                        } else {
                            (0, String::new(), 0, 0)
                        };
                        let invoked = if target != 0 && !selector_name.is_empty() {
                            self.invoke_objc_selector_now(target, &selector_name, arg_a, arg_b, 180_000, selector.as_str())
                        } else {
                            false
                        };
                        if let Some(entry) = self.runtime.scheduler.invocations.invocations.get_mut(&receiver) {
                            entry.invoke_count = entry.invoke_count.saturating_add(1);
                            entry.last_result = self.cpu.regs[0];
                        }
                        self.trace_synthetic_invocation(format!("invoke invocation={} target={} selector={} arg2={} arg3={} invoked={} via={}", self.describe_ptr(receiver), self.describe_ptr(target), if selector_name.is_empty() { "<none>" } else { &selector_name }, self.describe_ptr(arg_a), self.describe_ptr(arg_b), if invoked { "YES" } else { "NO" }, selector));
                        receiver
                    }
                    "currentRequest" | "originalRequest" => self.runtime.ui_network.network_request,
                    "currentConnection" => {
                        if self.runtime.ui_network.network_faulted || self.runtime.ui_network.network_cancelled || self.runtime.ui_network.network_timeout_armed {
                            self.runtime.ui_network.fault_connection
                        } else {
                            self.runtime.ui_network.network_connection
                        }
                    }
                    "currentResponse" | "response" => {
                        if receiver == self.runtime.ui_network.fault_connection {
                            if self.network_fault_has_response() { self.runtime.ui_network.network_response } else { 0 }
                        } else if self.runtime.ui_network.network_completed || self.runtime.ui_network.network_response_retained {
                            self.runtime.ui_network.network_response
                        } else {
                            0
                        }
                    }
                    "error" | "lastError" => { if self.runtime.ui_network.network_faulted || self.runtime.ui_network.network_cancelled { self.runtime.ui_network.network_error } else { 0 } },
                    "delegate" => {
                        if receiver == self.runtime.ui_network.network_connection || receiver == self.runtime.ui_network.fault_connection {
                            self.current_network_delegate()
                        } else {
                            self.runtime.ui_objects.delegate
                        }
                    },
                    "URL" | "mainDocumentURL" => {
                        if receiver == self.runtime.ui_network.network_request || receiver == self.runtime.ui_network.network_response {
                            self.runtime.ui_network.network_url
                        } else {
                            self.runtime.ui_network.network_url
                        }
                    },
                    "absoluteString" => {
                        if let Some(text) = self.synthetic_file_url_absolute_string_value(receiver) {
                            self.materialize_host_string_object("NSString.fileURL.absoluteString", &text)
                        } else {
                            HLE_FAKE_NSSTRING_URL_ABSOLUTE
                        }
                    }
                    "host" => {
                        if self.runtime.fs.synthetic_file_urls.contains_key(&receiver) {
                            0
                        } else {
                            HLE_FAKE_NSSTRING_URL_HOST
                        }
                    }
                    "path" => {
                        if let Some(text) = self.synthetic_file_url_path(receiver) {
                            self.materialize_host_string_object("NSString.fileURL.path", &text)
                        } else {
                            HLE_FAKE_NSSTRING_URL_PATH
                        }
                    }
                    "pathExtension" => {
                        if let Some(text) = self.synthetic_file_url_path_extension_value(receiver) {
                            self.materialize_host_string_object("NSString.fileURL.pathExtension", &text)
                        } else if let Some(text) = self.guest_string_value(receiver) {
                            let ext = std::path::Path::new(text.trim()).extension().and_then(|v| v.to_str()).unwrap_or_default().to_string();
                            self.materialize_host_string_object("NSString.pathExtension", &ext)
                        } else {
                            0
                        }
                    }
                    "lastPathComponent" => {
                        if let Some(text) = self.synthetic_file_url_last_path_component_value(receiver) {
                            self.materialize_host_string_object("NSString.fileURL.lastPathComponent", &text)
                        } else if let Some(text) = self.guest_string_value(receiver) {
                            let name = std::path::Path::new(text.trim()).file_name().and_then(|v| v.to_str()).unwrap_or_default().to_string();
                            self.materialize_host_string_object("NSString.lastPathComponent", &name)
                        } else {
                            0
                        }
                    }
                    "isFileURL" => {
                        if self.runtime.fs.synthetic_file_urls.contains_key(&receiver) { 1 } else { 0 }
                    }
                    "HTTPMethod" => HLE_FAKE_NSSTRING_HTTP_METHOD,
                    "HTTPBody" | "allHTTPHeaderFields" => self.runtime.ui_network.network_request,
                    "statusCode" => 200,
                    "presentedFrameCount" => self.runtime.ui_graphics.graphics_frame_index,
                    "drawableWidth" => self.runtime.ui_graphics.graphics_surface_width,
                    "drawableHeight" => self.runtime.ui_graphics.graphics_surface_height,
                    "MIMEType" => HLE_FAKE_NSSTRING_MIME_TYPE,
                    "domain" => if receiver == self.runtime.ui_network.network_error { HLE_FAKE_NSSTRING_ERROR_DOMAIN } else { 0 },
                    "localizedDescription" => if receiver == self.runtime.ui_network.network_error { HLE_FAKE_NSSTRING_ERROR_DESCRIPTION } else { 0 },
                    "bundlePath" | "resourcePath" => {
                        self.bundle_root_string_for_receiver(receiver)
                            .map(|path| self.materialize_host_string_object("NSString.bundlePath", &path))
                            .unwrap_or(0)
                    },
                    "stringByAppendingPathComponent:" => {
                        let lhs = self.guest_string_value(receiver).unwrap_or_default();
                        let rhs = self.guest_string_value(arg2).unwrap_or_default();
                        if lhs.is_empty() {
                            self.make_path_string_object("NSString.path.append", rhs)
                        } else if rhs.is_empty() {
                            self.make_path_string_object("NSString.path.append", lhs)
                        } else {
                            let mut path = std::path::PathBuf::from(lhs);
                            path.push(rhs);
                            self.make_path_string_object("NSString.path.append", path.display().to_string())
                        }
                    },
                    "stringByDeletingLastPathComponent" => {
                        let lhs = self.guest_string_value(receiver).unwrap_or_default();
                        if lhs.is_empty() {
                            0
                        } else {
                            let path = std::path::PathBuf::from(lhs);
                            let out = path.parent().map(|p| p.display().to_string()).unwrap_or_default();
                            self.make_path_string_object("NSString.path.deleteLast", out)
                        }
                    },
                    "stringByAppendingPathExtension:" => {
                        let lhs = self.guest_string_value(receiver).unwrap_or_default();
                        let rhs = self.guest_string_value(arg2).unwrap_or_default();
                        if lhs.is_empty() {
                            0
                        } else if rhs.is_empty() {
                            self.make_path_string_object("NSString.path.appendExt", lhs)
                        } else {
                            self.make_path_string_object("NSString.path.appendExt", format!("{}.{}", lhs.trim_end_matches('.'), rhs.trim_start_matches('.')))
                        }
                    },
                    "stringByExpandingTildeInPath" | "stringByStandardizingPath" => {
                        let lhs = self.guest_string_value(receiver).unwrap_or_default();
                        if lhs.starts_with("~/") {
                            if let Some(home) = self.sandbox_home_path() {
                                let suffix = lhs.trim_start_matches("~/");
                                self.make_path_string_object("NSString.path.standardized", home.join(suffix).display().to_string())
                            } else {
                                self.make_path_string_object("NSString.path.standardized", lhs)
                            }
                        } else {
                            self.make_path_string_object("NSString.path.standardized", lhs)
                        }
                    },
                    "CGImage" => {
                        let mut bg_trace: Option<String> = None;
                        let result = if let Some(texture) = self.runtime.graphics.synthetic_textures.get(&receiver) {
                            let image_obj = texture.image;
                            let is_bg = texture.source_key.eq_ignore_ascii_case("menu_background.png")
                                || texture.source_path.eq_ignore_ascii_case("menu_background.png")
                                || texture
                                    .source_path
                                    .rsplit(['/', '\\'])
                                    .next()
                                    .map(|v| v.eq_ignore_ascii_case("menu_background.png"))
                                    .unwrap_or(false);
                            if is_bg {
                                let fp = self
                                    .runtime.graphics.synthetic_images
                                    .get(&image_obj)
                                    .map(|img| sample_rgba_fingerprint(&img.rgba, img.width.max(1), img.height.max(1)))
                                    .unwrap_or_else(|| "missing-image".to_string());
                                bg_trace = Some(format!(
                                    "     ↳ ab-bgimg-cgimage via=texture receiver={} result={} texKey={} texPath={} texPma={} fp={}",
                                    self.describe_ptr(receiver),
                                    self.describe_ptr(image_obj),
                                    texture.source_key,
                                    texture.source_path,
                                    if texture.has_premultiplied_alpha { "YES" } else { "NO" },
                                    fp,
                                ));
                            }
                            image_obj
                        } else if self.runtime.graphics.synthetic_images.contains_key(&receiver) {
                            let label = self.diag.object_labels.get(&receiver).cloned().unwrap_or_default();
                            if label.to_ascii_lowercase().contains("menu_background.png") {
                                let fp = self
                                    .runtime.graphics.synthetic_images
                                    .get(&receiver)
                                    .map(|img| sample_rgba_fingerprint(&img.rgba, img.width.max(1), img.height.max(1)))
                                    .unwrap_or_else(|| "missing-image".to_string());
                                bg_trace = Some(format!(
                                    "     ↳ ab-bgimg-cgimage via=image-self receiver={} result={} label={} fp={}",
                                    self.describe_ptr(receiver),
                                    self.describe_ptr(receiver),
                                    label,
                                    fp,
                                ));
                            }
                            receiver
                        } else {
                            0
                        };
                        if let Some(line) = bg_trace {
                            self.diag.trace.push(line);
                        }
                        result
                    }
                    "UTF8String" | "cStringUsingEncoding:" => {
                        let had_backing = self.string_backing(receiver).is_some();
                        if let Some(text) = self.ensure_string_backing_for_value(receiver, "NSString.guest") {
                            if !had_backing {
                                note = Some(format!(
                                    "foundation {} bridged receiver={} -> ptr={} len={} text='{}'",
                                    selector,
                                    self.describe_ptr(receiver),
                                    self.describe_ptr(text.ptr),
                                    text.len,
                                    text.text.chars().take(64).collect::<String>().replace('\n', "\\n"),
                                ));
                            }
                            text.ptr
                        } else {
                            0
                        }
                    },
                    "code" => if receiver == self.runtime.ui_network.network_error { self.network_error_code() as u32 } else { 0 },
                    "expectedContentLength" => self.network_payload_len(),
                    "length" => {
                        if let Some(text) = self.ensure_string_backing_for_value(receiver, "NSString.guest") {
                            text.len
                        } else if let Some(blob) = self.blob_backing(receiver) {
                            blob.len
                        } else {
                            self.network_payload_len()
                        }
                    },
                    "bytes" => {
                        if let Some(blob) = self.blob_backing(receiver) { blob.ptr } else { 0 }
                    },
                    "firstResponder" => self.runtime.ui_objects.first_responder,
                    "applicationState" => if self.runtime.ui_runtime.app_active { 0 } else { 1 },
                    "contentScaleFactor" => self.ui_content_scale_bits_for_object(receiver),
                    "bounds" | "applicationFrame" => self.runtime.ui_objects.screen,
                    "currentHandler" => {
                        note = Some(format!("assertion currentHandler -> {}", self.describe_ptr(receiver)));
                        if receiver != 0 { receiver } else { self.alloc_synthetic_ui_object("NSAssertionHandler.synthetic#0") }
                    },
                    "handleFailureInMethod:object:file:lineNumber:description:" => {
                        let file_ptr = self.peek_stack_u32(0).unwrap_or(0);
                        let line = self.peek_stack_u32(1).unwrap_or(0);
                        let desc_ptr = self.peek_stack_u32(2).unwrap_or(0);
                        let method = self.guest_string_value(arg2).unwrap_or_else(|| self.describe_ptr(arg2));
                        let file = self.guest_string_value(file_ptr).unwrap_or_else(|| self.describe_ptr(file_ptr));
                        let desc = self.guest_string_value(desc_ptr).unwrap_or_else(|| self.describe_ptr(desc_ptr));
                        note = Some(format!("assertion suppressed method={} object={} file={} line={} desc={}", method, self.describe_ptr(arg3), file, line, desc));
                        0
                    },
                    "defaultCenter" => {
                        let center = self.ensure_notification_center_default();
                        note = Some(format!("NSNotificationCenter defaultCenter -> {} observers={}", self.describe_ptr(center), self.runtime.ui_runtime.notification_observers.get(&center).map(|items| items.len()).unwrap_or(0)));
                        center
                    },
                    "addObserver:selector:name:object:" => {
                        let name_ptr = self.peek_stack_u32(0).unwrap_or(0);
                        let object_ptr = self.peek_stack_u32(1).unwrap_or(0);
                        let selector_name = self.objc_read_selector_name(arg3).unwrap_or_else(|| format!("0x{arg3:08x}"));
                        self.register_synthetic_notification_observer(receiver, arg2, arg3, name_ptr, object_ptr, selector.as_str());
                        note = Some(format!(
                            "NSNotificationCenter addObserver observer={} selector={} name={} object={} center={}",
                            self.describe_ptr(arg2),
                            selector_name,
                            self.synthetic_notification_name_desc(name_ptr),
                            self.describe_ptr(object_ptr),
                            self.describe_ptr(if receiver != 0 { receiver } else { self.runtime.ui_runtime.notification_center_default }),
                        ));
                        receiver
                    },
                    "removeObserver:" => {
                        let removed = self.remove_synthetic_notification_observer(receiver, arg2, None, None, selector.as_str());
                        note = Some(format!("NSNotificationCenter removeObserver observer={} removed={} center={}", self.describe_ptr(arg2), removed, self.describe_ptr(if receiver != 0 { receiver } else { self.runtime.ui_runtime.notification_center_default })));
                        receiver
                    },
                    "removeObserver:name:object:" => {
                        let name_ptr = self.peek_stack_u32(0).unwrap_or(0);
                        let object_ptr = self.peek_stack_u32(1).unwrap_or(0);
                        let removed = self.remove_synthetic_notification_observer(receiver, arg2, Some(name_ptr), Some(object_ptr), selector.as_str());
                        note = Some(format!(
                            "NSNotificationCenter removeObserver observer={} name={} object={} removed={} center={}",
                            self.describe_ptr(arg2),
                            self.synthetic_notification_name_desc(name_ptr),
                            self.describe_ptr(object_ptr),
                            removed,
                            self.describe_ptr(if receiver != 0 { receiver } else { self.runtime.ui_runtime.notification_center_default }),
                        ));
                        receiver
                    },
                    "initWithContentURL:" => {
                        let url_desc = self.resolve_path_from_url_like_value(arg2, false)
                            .map(|path| path.display().to_string())
                            .or_else(|| self.guest_string_value(arg2))
                            .unwrap_or_else(|| self.describe_ptr(arg2));
                        let state = self.runtime.ui_runtime.movie_players.entry(receiver).or_default();
                        state.content_url = arg2;
                        state.prepared = false;
                        state.is_playing = false;
                        state.playback_started_tick = 0;
                        state.playback_finish_tick = 0;
                        state.playback_remaining_ticks = 0;
                        state.playback_duration_ticks = 0;
                        self.diag.object_labels.insert(receiver, format!("MPMoviePlayerController.instance(synth)<{}>", url_desc.chars().take(96).collect::<String>()));
                        self.push_callback_trace(format!(
                            "movie.init origin={} player={} contentURL={} window={} launched={} active={}",
                            selector,
                            self.describe_ptr(receiver),
                            url_desc,
                            self.describe_ptr(self.runtime.ui_objects.window),
                            if self.runtime.ui_runtime.launched { "YES" } else { "NO" },
                            if self.runtime.ui_runtime.app_active { "YES" } else { "NO" },
                        ));
                        note = Some(format!("movie init contentURL={} player={}", url_desc, self.describe_ptr(receiver)));
                        receiver
                    },
                    "setShouldAutoplay:" if self.runtime.ui_runtime.movie_players.contains_key(&receiver) => {
                        if let Some(state) = self.runtime.ui_runtime.movie_players.get_mut(&receiver) {
                            state.should_autoplay = arg2 != 0;
                        }
                        let autostarted = if arg2 != 0 {
                            self.maybe_autostart_synthetic_movie_player(receiver, selector.as_str())
                        } else {
                            false
                        };
                        self.push_callback_trace(format!(
                            "movie.autoplay player={} shouldAutoplay={} origin={} autostarted={}",
                            self.describe_ptr(receiver),
                            if arg2 != 0 { "YES" } else { "NO" },
                            selector,
                            if autostarted { "YES" } else { "NO" },
                        ));
                        note = Some(format!("movie shouldAutoplay={} player={} autostarted={}", if arg2 != 0 { "YES" } else { "NO" }, self.describe_ptr(receiver), if autostarted { "YES" } else { "NO" }));
                        receiver
                    },
                    "view" if self.runtime.ui_runtime.movie_players.contains_key(&receiver) => {
                        let view = self.ensure_synthetic_movie_player_view(receiver);
                        let autostarted = self.maybe_autostart_synthetic_movie_player(receiver, selector.as_str());
                        self.push_callback_trace(format!(
                            "movie.view player={} view={} origin={} superview={} autostarted={}",
                            self.describe_ptr(receiver),
                            self.describe_ptr(view),
                            selector,
                            self.describe_ptr(self.runtime.ui_objects.view_superviews.get(&view).copied().unwrap_or(0)),
                            if autostarted { "YES" } else { "NO" },
                        ));
                        note = Some(format!("movie view player={} -> {} autostarted={}", self.describe_ptr(receiver), self.describe_ptr(view), if autostarted { "YES" } else { "NO" }));
                        view
                    },
                    "play" if self.runtime.ui_runtime.movie_players.contains_key(&receiver) => {
                        let (plays, autoplay, content_url, started, finish_tick, duration_ticks) = {
                            let state = self.runtime.ui_runtime.movie_players.get_mut(&receiver).unwrap();
                            state.play_count = state.play_count.saturating_add(1);
                            let plays = state.play_count;
                            let autoplay = state.should_autoplay;
                            let content_url = state.content_url;
                            let _ = state;
                            let started = self.start_synthetic_movie_playback(receiver, selector.as_str());
                            let state = self.runtime.ui_runtime.movie_players.get(&receiver).unwrap();
                            (plays, autoplay, content_url, started, state.playback_finish_tick, state.playback_duration_ticks)
                        };
                        let url_desc = if content_url != 0 {
                            self.resolve_path_from_url_like_value(content_url, false)
                                .map(|path| path.display().to_string())
                                .or_else(|| self.guest_string_value(content_url))
                                .unwrap_or_else(|| self.describe_ptr(content_url))
                        } else {
                            "<none>".to_string()
                        };
                        let observer_count = self.runtime.ui_runtime.notification_observers.values().map(|items| items.len()).sum::<usize>();
                        self.push_callback_trace(format!(
                            "movie.play player={} contentURL={} shouldAutoplay={} playCount={} observers={} window={} sceneRunning={} finishTick={} durationTicks={} started={} origin={}",
                            self.describe_ptr(receiver),
                            url_desc,
                            if autoplay { "YES" } else { "NO" },
                            plays,
                            observer_count,
                            self.describe_ptr(self.runtime.ui_objects.window),
                            self.describe_ptr(self.runtime.ui_cocos.running_scene),
                            finish_tick,
                            duration_ticks,
                            if started { "YES" } else { "NO" },
                            selector,
                        ));
                        note = Some(format!("movie play player={} observers={} url={} started={} finishTick={} durationTicks={}", self.describe_ptr(receiver), observer_count, url_desc, if started { "YES" } else { "NO" }, finish_tick, duration_ticks));
                        receiver
                    },
                    "pause" if self.runtime.ui_runtime.movie_players.contains_key(&receiver) => {
                        let (pauses, remaining) = {
                            let now_tick = self.runtime.ui_runtime.runloop_ticks;
                            let state = self.runtime.ui_runtime.movie_players.get_mut(&receiver).unwrap();
                            state.pause_count = state.pause_count.saturating_add(1);
                            if state.is_playing && state.playback_finish_tick > now_tick {
                                state.playback_remaining_ticks = state.playback_finish_tick.saturating_sub(now_tick);
                            }
                            state.is_playing = false;
                            state.playback_finish_tick = 0;
                            (state.pause_count, state.playback_remaining_ticks)
                        };
                        self.push_callback_trace(format!("movie.pause player={} pauseCount={} remainingTicks={} origin={}", self.describe_ptr(receiver), pauses, remaining, selector));
                        note = Some(format!("movie pause player={} count={} remainingTicks={}", self.describe_ptr(receiver), pauses, remaining));
                        receiver
                    },
                    "stop" if self.runtime.ui_runtime.movie_players.contains_key(&receiver) => {
                        let stops = {
                            let state = self.runtime.ui_runtime.movie_players.get_mut(&receiver).unwrap();
                            state.stop_count = state.stop_count.saturating_add(1);
                            state.is_playing = false;
                            state.playback_started_tick = 0;
                            state.playback_finish_tick = 0;
                            state.playback_remaining_ticks = 0;
                            state.stop_count
                        };
                        self.push_callback_trace(format!("movie.stop player={} stopCount={} origin={}", self.describe_ptr(receiver), stops, selector));
                        note = Some(format!("movie stop player={} count={}", self.describe_ptr(receiver), stops));
                        receiver
                    },
                    "prepareToPlay" if self.runtime.ui_runtime.movie_players.contains_key(&receiver) => {
                        let duration_ticks = self.resolve_movie_duration_ticks(receiver, selector.as_str());
                        if let Some(state) = self.runtime.ui_runtime.movie_players.get_mut(&receiver) {
                            state.prepared = true;
                            state.playback_duration_ticks = duration_ticks.max(1);
                            if state.playback_remaining_ticks == 0 {
                                state.playback_remaining_ticks = state.playback_duration_ticks;
                            }
                        }
                        self.push_callback_trace(format!("movie.prepare player={} durationTicks={} origin={}", self.describe_ptr(receiver), duration_ticks.max(1), selector));
                        note = Some(format!("movie prepare player={} durationTicks={}", self.describe_ptr(receiver), duration_ticks.max(1)));
                        receiver
                    },
                    "standardUserDefaults" => {
                        let defaults = if receiver != 0 {
                            receiver
                        } else {
                            self.alloc_synthetic_ui_object("NSUserDefaults.standard(synth)")
                        };
                        self.runtime.graphics.synthetic_dictionaries.entry(defaults).or_default();
                        self.diag.object_labels
                            .entry(defaults)
                            .or_insert_with(|| "NSUserDefaults.standard(synth)".to_string());
                        note = Some(format!("NSUserDefaults standard -> {} entries={}", self.describe_ptr(defaults), self.runtime.graphics.synthetic_dictionaries.get(&defaults).map(|dict| dict.entries.len()).unwrap_or(0)));
                        defaults
                    },
                    "integerForKey:" | "boolForKey:" => {
                        if let Some(dict) = self.runtime.graphics.synthetic_dictionaries.get(&receiver) {
                            let key = self.synthetic_dictionary_key(arg2);
                            let raw = dict.entries.get(&key).copied().unwrap_or(0);
                            let value = if selector == "boolForKey:" {
                                if raw != 0 { 1 } else { 0 }
                            } else {
                                raw
                            };
                            note = Some(format!("NSUserDefaults {} key='{}' -> {}", selector, key, value));
                            value
                        } else {
                            note = Some(format!("NSUserDefaults {} on non-dictionary receiver {} -> 0", selector, self.describe_ptr(receiver)));
                            0
                        }
                    },
                    "dataForKey:" => {
                        if let Some(dict) = self.runtime.graphics.synthetic_dictionaries.get(&receiver) {
                            let key = self.synthetic_dictionary_key(arg2);
                            let value = dict.entries.get(&key).copied().unwrap_or(0);
                            let result = if value != 0 && self.blob_backing(value).is_some() {
                                value
                            } else {
                                0
                            };
                            note = Some(format!("NSUserDefaults dataForKey key='{}' -> {}", key, self.describe_ptr(result)));
                            result
                        } else {
                            note = Some(format!("NSUserDefaults dataForKey on non-dictionary receiver {} -> nil", self.describe_ptr(receiver)));
                            0
                        }
                    },
                    "stringWithFormat:" => {
                        let template = self.guest_string_value(arg2).unwrap_or_default();
                        let arg_value = arg3 as i32;
                        let arg_text = format!("{}", arg_value);
                        let formatted = if template.contains("%d") {
                            template.replacen("%d", &arg_text, 1)
                        } else if template.contains("%u") {
                            template.replacen("%u", &(arg3 as u32).to_string(), 1)
                        } else if template.contains("%i") {
                            template.replacen("%i", &arg_text, 1)
                        } else if template.contains("%ld") {
                            template.replacen("%ld", &((arg3 as i32) as i64).to_string(), 1)
                        } else if template.contains("%lu") {
                            template.replacen("%lu", &(arg3 as u64).to_string(), 1)
                        } else if template.is_empty() {
                            arg_text.clone()
                        } else {
                            format!("{}{}", template, arg_text)
                        };
                        let result = if receiver != 0 {
                            receiver
                        } else {
                            self.alloc_synthetic_ui_object("NSString.synthetic.format")
                        };
                        let label = format!("NSString.synthetic.format<'{}'>", formatted.chars().take(64).collect::<String>().replace('\n', "\\n"));
                        let _ = self.ensure_string_backing(result, label, &formatted);
                        note = Some(format!("stringWithFormat template='{}' value={} -> '{}'", template.replace('\n', "\\n"), arg3, formatted.replace('\n', "\\n")));
                        result
                    },
                    "unarchiveObjectWithData:" => {
                        let result = if arg2 == 0 {
                            0
                        } else if let Some(blob) = self.blob_backing(arg2).cloned() {
                            if blob.len == 0 {
                                0
                            } else {
                                let obj = self.alloc_synthetic_ui_object(format!("NSKeyedUnarchiver.object#{}", self.runtime.heap.synthetic_blob_backing.len()));
                                let preview = if blob.preview_ascii.is_empty() {
                                    format!("{} bytes", blob.len)
                                } else {
                                    blob.preview_ascii.clone()
                                };
                                let _ = self.ensure_string_backing(obj, format!("NSKeyedUnarchiver.object<'{}'>", preview.chars().take(64).collect::<String>().replace('\n', "\\n")), &preview);
                                obj
                            }
                        } else {
                            0
                        };
                        note = Some(format!("unarchiveObjectWithData data={} -> {}", self.describe_ptr(arg2), self.describe_ptr(result)));
                        result
                    },
                    "arrayWithObject:" | "arrayWithObjects:" | "arrayWithObjects:count:" | "arrayWithCapacity:" | "array" => {
                        let class_name = self.objc_receiver_class_name_hint(receiver).unwrap_or_default();
                        let mutable = class_name.contains("NSMutableArray") || class_name.contains("MutableArray");
                        let label_prefix = if mutable { "NSMutableArray" } else { "NSArray" };
                        let array = self.alloc_synthetic_array(format!("{}.synthetic#{}", label_prefix, self.runtime.graphics.synthetic_arrays.len()));
                        let items = match selector.as_str() {
                            "arrayWithObject:" => {
                                if arg2 != 0 { vec![arg2] } else { Vec::new() }
                            }
                            "arrayWithObjects:" => self.collect_objc_variadic_object_list(arg2, arg3, 32),
                            "arrayWithObjects:count:" => self.read_u32_list(arg2, arg3 as usize, 256),
                            _ => Vec::new(),
                        };
                        for item in items.iter().copied() {
                            let _ = self.synthetic_array_push(array, item);
                        }
                        note = Some(format!(
                            "array created {} class={} count={} items=[{}]",
                            self.describe_ptr(array),
                            if class_name.is_empty() { label_prefix } else { &class_name },
                            self.synthetic_array_len(array),
                            items.iter().map(|item| self.describe_ptr(*item)).collect::<Vec<_>>().join(", ")
                        ));
                        array
                    },
                    "initWithObject:" | "initWithObjects:" | "initWithObjects:count:" => {
                        let class_name = self.objc_receiver_class_name_hint(receiver).unwrap_or_default();
                        let mutable = class_name.contains("NSMutableArray") || class_name.contains("MutableArray");
                        let label_prefix = if mutable { "NSMutableArray" } else { "NSArray" };
                        let items = match selector.as_str() {
                            "initWithObject:" => {
                                if arg2 != 0 { vec![arg2] } else { Vec::new() }
                            }
                            "initWithObjects:" => self.collect_objc_variadic_object_list(arg2, arg3, 32),
                            "initWithObjects:count:" => self.read_u32_list(arg2, arg3 as usize, 256),
                            _ => Vec::new(),
                        };
                        {
                            let entry = self.runtime.graphics.synthetic_arrays.entry(receiver).or_default();
                            entry.items.clear();
                            entry.mutation_count = entry.mutation_count.saturating_add(1);
                            for item in items.iter().copied() {
                                entry.items.push(item);
                                entry.mutation_count = entry.mutation_count.saturating_add(1);
                            }
                        }
                        let default_label = format!("{}.synthetic#{}", label_prefix, self.runtime.graphics.synthetic_arrays.len());
                        self.diag.object_labels
                            .entry(receiver)
                            .or_insert(default_label);
                        note = Some(format!(
                            "array init {} class={} count={} items=[{}]",
                            self.describe_ptr(receiver),
                            if class_name.is_empty() { label_prefix } else { &class_name },
                            self.synthetic_array_len(receiver),
                            items.iter().map(|item| self.describe_ptr(*item)).collect::<Vec<_>>().join(", ")
                        ));
                        receiver
                    },
                    "count" => {
                        if self.runtime.graphics.synthetic_arrays.contains_key(&receiver) {
                            self.synthetic_array_len(receiver) as u32
                        } else if let Some(dict) = self.runtime.graphics.synthetic_dictionaries.get(&receiver) {
                            dict.entries.len() as u32
                        } else {
                            0
                        }
                    },
                    "objectAtIndex:" => {
                        if self.runtime.graphics.synthetic_arrays.contains_key(&receiver) {
                            let result = self.synthetic_array_get(receiver, arg2 as usize);
                            note = Some(format!("array objectAtIndex {} -> {}", arg2, self.describe_ptr(result)));
                            result
                        } else {
                            0
                        }
                    },
                    "lastObject" => {
                        if self.runtime.graphics.synthetic_arrays.contains_key(&receiver) {
                            let len = self.synthetic_array_len(receiver);
                            let result = if len == 0 { 0 } else { self.synthetic_array_get(receiver, len.saturating_sub(1)) };
                            note = Some(format!("array lastObject -> {}", self.describe_ptr(result)));
                            result
                        } else {
                            0
                        }
                    },
                    "addObject:" => {
                        if self.runtime.graphics.synthetic_arrays.contains_key(&receiver) {
                            if arg2 == 0 {
                                note = Some(format!("array addObject nil ignored count={}", self.synthetic_array_len(receiver)));
                                receiver
                            } else {
                                let idx = self.synthetic_array_append_unique(receiver, arg2);
                                note = Some(format!("array addObject {} index={} count={}", self.describe_ptr(arg2), idx, self.synthetic_array_len(receiver)));
                                receiver
                            }
                        } else {
                            0
                        }
                    },
                    "insertObject:atIndex:" => {
                        if self.runtime.graphics.synthetic_arrays.contains_key(&receiver) {
                            let index = self.peek_stack_u32(0).unwrap_or(0) as usize;
                            let idx = self.synthetic_array_insert_or_move(receiver, index, arg2);
                            note = Some(format!("array insertObject {} index={} count={}", self.describe_ptr(arg2), idx, self.synthetic_array_len(receiver)));
                            receiver
                        } else {
                            0
                        }
                    },
                    "removeObject:" => {
                        if self.runtime.graphics.synthetic_arrays.contains_key(&receiver) {
                            let removed = self.synthetic_array_remove_value(receiver, arg2);
                            note = Some(format!("array removeObject {} removed={} count={}", self.describe_ptr(arg2), if removed { "YES" } else { "NO" }, self.synthetic_array_len(receiver)));
                            receiver
                        } else {
                            0
                        }
                    },
                    "removeObjectAtIndex:" => {
                        if let Some(array) = self.runtime.graphics.synthetic_arrays.get_mut(&receiver) {
                            let index = arg2 as usize;
                            let removed = if index < array.items.len() {
                                array.items.remove(index);
                                array.mutation_count = array.mutation_count.saturating_add(1);
                                true
                            } else {
                                false
                            };
                            note = Some(format!("array removeObjectAtIndex {} removed={} count={}", index, if removed { "YES" } else { "NO" }, array.items.len()));
                            receiver
                        } else {
                            0
                        }
                    },
                    "countByEnumeratingWithState:objects:count:" => {
                        if let Some(array) = self.runtime.graphics.synthetic_arrays.get(&receiver).cloned() {
                            let state_ptr = arg2;
                            let objects_ptr = arg3;
                            let count = self.peek_stack_u32(0).unwrap_or(0).max(1) as usize;
                            let prior_state = if state_ptr != 0 { self.read_u32_le(state_ptr).unwrap_or(0) } else { 0 };
                            if state_ptr == 0 || objects_ptr == 0 || prior_state != 0 || array.items.is_empty() {
                                if state_ptr != 0 {
                                    let _ = self.write_u32_le(state_ptr, 1);
                                }
                                note = Some(format!("array fast-enum -> 0 state={} count={}", prior_state, array.items.len()));
                                0
                            } else {
                                let n = array.items.len().min(count);
                                for (i, item) in array.items.iter().take(n).enumerate() {
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
                                let _ = self.write_u32_le(mutation_ptr, array.mutation_count);
                                note = Some(format!("array fast-enum -> {} state={} objects={}", n, self.describe_ptr(state_ptr), self.describe_ptr(objects_ptr)));
                                n as u32
                            }
                        } else {
                            0
                        }
                    },
                    "dictionaryWithCapacity:" | "dictionary" => {
                        let dict = self.alloc_synthetic_ui_object(format!("NSMutableDictionary.synthetic#{}", self.runtime.graphics.synthetic_dictionaries.len()));
                        self.runtime.graphics.synthetic_dictionaries.insert(dict, SyntheticDictionary::default());
                        note = Some(format!("dictionary created {}", self.describe_ptr(dict)));
                        dict
                    },
                    "objectForKey:" => {
                        if let Some(dict) = self.runtime.graphics.synthetic_dictionaries.get(&receiver) {
                            let key = self.synthetic_dictionary_key(arg2);
                            let result = dict.entries.get(&key).copied().unwrap_or(0);
                            note = Some(format!("dictionary lookup key='{}' -> {}", key, self.describe_ptr(result)));
                            result
                        } else {
                            0
                        }
                    },
                    "setObject:forKey:" | "setValue:forKey:" => {
                        if receiver != 0 {
                            let key = self.synthetic_dictionary_key(arg3);
                            let value_desc = self.describe_ptr(arg2);
                            {
                                let entry = self.runtime.graphics.synthetic_dictionaries.entry(receiver).or_default();
                                entry.entries.insert(key.clone(), arg2);
                            }
                            note = Some(format!("dictionary store key='{}' value={}", key, value_desc));
                        }
                        receiver
                    },
                    "containsObjectForKey:" => {
                        if let Some(dict) = self.runtime.graphics.synthetic_dictionaries.get(&receiver) {
                            let key = self.synthetic_dictionary_key(arg2);
                            let hit = dict.entries.contains_key(&key);
                            note = Some(format!("dictionary contains key='{}' -> {}", key, if hit { "YES" } else { "NO" }));
                            if hit { 1 } else { 0 }
                        } else {
                            0
                        }
                    },
                    "defaultManager" => self.alloc_synthetic_ui_object("NSFileManager.default(synth)"),
                    "fileExistsAtPath:" => {
                        if let Some(path) = self.host_path_from_string_value(arg2) {
                            let hit = path.exists();
                            note = Some(format!("fileExistsAtPath path={} -> {}", path.display(), if hit { "YES" } else { "NO" }));
                            if hit { 1 } else { 0 }
                        } else {
                            note = Some("fileExistsAtPath path=<nil> -> NO".to_string());
                            0
                        }
                    },
                    "fileExistsAtPath:isDirectory:" => {
                        if let Some(path) = self.host_path_from_string_value(arg2) {
                            let meta = std::fs::metadata(&path).ok();
                            let hit = meta.is_some();
                            let is_dir = meta.map(|m| m.is_dir()).unwrap_or(false);
                            if arg3 != 0 {
                                let _ = self.write_u32_le(arg3, if is_dir { 1 } else { 0 });
                            }
                            note = Some(format!("fileExistsAtPath:isDirectory path={} -> hit={} isDir={} out={}", path.display(), if hit { "YES" } else { "NO" }, if is_dir { "YES" } else { "NO" }, self.describe_ptr(arg3)));
                            if hit { 1 } else { 0 }
                        } else {
                            if arg3 != 0 {
                                let _ = self.write_u32_le(arg3, 0);
                            }
                            note = Some("fileExistsAtPath:isDirectory path=<nil> -> NO".to_string());
                            0
                        }
                    },
                    "createDirectoryAtPath:withIntermediateDirectories:attributes:error:" => {
                        let attrs = self.peek_stack_u32(0).unwrap_or(0);
                        let err_out = self.peek_stack_u32(1).unwrap_or(0);
                        if let Some(path) = self.host_path_from_string_value(arg2) {
                            let created = if arg3 != 0 {
                                std::fs::create_dir_all(&path).is_ok()
                            } else {
                                std::fs::create_dir(&path).is_ok() || path.is_dir()
                            };
                            if err_out != 0 {
                                let _ = self.write_u32_le(err_out, 0);
                            }
                            note = Some(format!("createDirectoryAtPath path={} intermediates={} attrs={} errOut={} -> {}", path.display(), if arg3 != 0 { "YES" } else { "NO" }, self.describe_ptr(attrs), self.describe_ptr(err_out), if created { "YES" } else { "NO" }));
                            if created { 1 } else { 0 }
                        } else {
                            if err_out != 0 {
                                let _ = self.write_u32_le(err_out, 0);
                            }
                            note = Some(format!("createDirectoryAtPath path=<nil> intermediates={} attrs={} errOut={} -> NO", if arg3 != 0 { "YES" } else { "NO" }, self.describe_ptr(attrs), self.describe_ptr(err_out)));
                            0
                        }
                    },
                    "retain" | "autorelease" | "release" | "init" | "self" => receiver,
                    "pathForResource:ofType:" => {
                        let name = self.guest_string_value(arg2).unwrap_or_default();
                        let ext = self.guest_string_value(arg3);
                        if let Some(path) = self.resolve_bundle_resource_path_for_receiver(receiver, &name, ext.as_deref()) {
                            self.runtime.fs.last_resource_name = Some(name.clone());
                            self.runtime.fs.last_resource_path = Some(path.display().to_string());
                            self.materialize_host_string_object("NSString.bundleResourcePath", &path.display().to_string())
                        } else {
                            self.runtime.fs.last_resource_name = Some(name);
                            self.runtime.fs.last_resource_path = None;
                            0
                        }
                    }
                    "imageNamed:" => {
                        let name = self.guest_string_value(arg2).unwrap_or_else(|| format!("0x{arg2:08x}"));
                        let result = self.load_bundle_image_named(&name).unwrap_or(0);
                        note = Some(if result != 0 {
                            format!("imageNamed hit name='{}' path={}", name, self.runtime.fs.last_resource_path.clone().unwrap_or_default())
                        } else {
                            format!("imageNamed miss name='{}'", name)
                        });
                        result
                    }
                    "imageWithContentsOfFile:" | "initWithContentsOfFile:" => {
                        let path_text = self.guest_string_value(arg2).unwrap_or_default();
                        let resolved = self.resolve_bundle_file_path_for_receiver(receiver, &path_text);
                        let display_path = resolved
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| path_text.clone());
                        let result = if let Some(path) = resolved {
                            if path.extension().and_then(|v| v.to_str()).map(|v| v.eq_ignore_ascii_case("png")).unwrap_or(false) {
                                self.runtime.fs.last_resource_name = Some(path.file_name().and_then(|v| v.to_str()).unwrap_or(&path_text).to_string());
                                self.runtime.fs.last_resource_path = Some(path.display().to_string());
                                self.load_png_image_from_path(&display_path.to_ascii_lowercase(), &path).unwrap_or(0)
                            } else {
                                0
                            }
                        } else {
                            0
                        };
                        note = Some(if result != 0 {
                            format!("imageFromFile hit path={}", display_path)
                        } else {
                            format!("imageFromFile miss path={}", display_path)
                        });
                        result
                    }
                    "initWithContentsOfURL:error:" if Self::audio_is_objc_player_class(&receiver_class_desc) => {
                        let resource = self
                            .resolve_path_from_url_like_value(arg2, false)
                            .map(|path| path.display().to_string())
                            .or_else(|| self.guest_string_value(arg2))
                            .unwrap_or_else(|| self.describe_ptr(arg2));
                        let state = self.runtime.ui_runtime.audio_players.entry(receiver).or_default();
                        state.content_url = arg2;
                        state.content_data = 0;
                        state.prepared = false;
                        state.is_playing = false;
                        if state.volume == 0.0 {
                            state.volume = 1.0;
                        }
                        self.diag.object_labels.insert(receiver, format!("AVAudioPlayer.instance(synth)<{}>", resource.chars().take(96).collect::<String>()));
                        self.audio_trace_note_objc_audio_selector(&receiver_class_desc, selector.as_str(), Some(resource.clone()), false);
                        note = Some(format!("audio player initURL player={} resource={}", self.describe_ptr(receiver), resource));
                        receiver
                    },
                    "initWithData:error:" if Self::audio_is_objc_player_class(&receiver_class_desc) => {
                        let resource = format!("data:{}", self.describe_ptr(arg2));
                        let state = self.runtime.ui_runtime.audio_players.entry(receiver).or_default();
                        state.content_url = 0;
                        state.content_data = arg2;
                        state.prepared = false;
                        state.is_playing = false;
                        if state.volume == 0.0 {
                            state.volume = 1.0;
                        }
                        self.diag.object_labels.insert(receiver, format!("AVAudioPlayer.instance(synth)<{}>", resource));
                        self.audio_trace_note_objc_audio_selector(&receiver_class_desc, selector.as_str(), Some(resource.clone()), false);
                        note = Some(format!("audio player initData player={} resource={}", self.describe_ptr(receiver), resource));
                        receiver
                    },
                    "setDelegate:" if Self::audio_is_objc_player_class(&receiver_class_desc) => {
                        let state = self.runtime.ui_runtime.audio_players.entry(receiver).or_default();
                        state.delegate = arg2;
                        note = Some(format!("audio player setDelegate player={} delegate={}", self.describe_ptr(receiver), self.describe_ptr(arg2)));
                        receiver
                    },
                    "prepareToPlay" if Self::audio_is_objc_player_class(&receiver_class_desc) => {
                        let resource = self.runtime.ui_runtime.audio_players.get(&receiver).and_then(|s| if s.content_url != 0 { self.resolve_path_from_url_like_value(s.content_url, false).map(|p| p.display().to_string()).or_else(|| self.guest_string_value(s.content_url)) } else if s.content_data != 0 { Some(format!("data:{}", self.describe_ptr(s.content_data))) } else { None });
                        let prepare_count = {
                            let state = self.runtime.ui_runtime.audio_players.entry(receiver).or_default();
                            state.prepare_count = state.prepare_count.saturating_add(1);
                            state.prepared = true;
                            if state.volume == 0.0 {
                                state.volume = 1.0;
                            }
                            state.prepare_count
                        };
                        self.audio_trace_note_objc_audio_selector(&receiver_class_desc, selector.as_str(), resource.clone(), false);
                        note = Some(format!("audio player prepare player={} count={} resource={}", self.describe_ptr(receiver), prepare_count, resource.unwrap_or_else(|| "<none>".to_string())));
                        1
                    },
                    "play" if Self::audio_is_objc_player_class(&receiver_class_desc) => {
                        let resource = self.runtime.ui_runtime.audio_players.get(&receiver).and_then(|s| if s.content_url != 0 { self.resolve_path_from_url_like_value(s.content_url, false).map(|p| p.display().to_string()).or_else(|| self.guest_string_value(s.content_url)) } else if s.content_data != 0 { Some(format!("data:{}", self.describe_ptr(s.content_data))) } else { None });
                        let play_count = {
                            let state = self.runtime.ui_runtime.audio_players.entry(receiver).or_default();
                            state.play_count = state.play_count.saturating_add(1);
                            state.prepared = true;
                            state.is_playing = true;
                            if state.volume == 0.0 {
                                state.volume = 1.0;
                            }
                            state.play_count
                        };
                        self.audio_trace_note_objc_audio_selector(&receiver_class_desc, selector.as_str(), resource.clone(), false);
                        note = Some(format!("audio player play player={} count={} resource={}", self.describe_ptr(receiver), play_count, resource.unwrap_or_else(|| "<none>".to_string())));
                        1
                    },
                    "pause" if Self::audio_is_objc_player_class(&receiver_class_desc) => {
                        let pause_count = {
                            let state = self.runtime.ui_runtime.audio_players.entry(receiver).or_default();
                            state.pause_count = state.pause_count.saturating_add(1);
                            state.is_playing = false;
                            state.pause_count
                        };
                        self.audio_trace_note_objc_audio_selector(&receiver_class_desc, selector.as_str(), None, false);
                        note = Some(format!("audio player pause player={} count={}", self.describe_ptr(receiver), pause_count));
                        receiver
                    },
                    "stop" if Self::audio_is_objc_player_class(&receiver_class_desc) => {
                        let stop_count = {
                            let state = self.runtime.ui_runtime.audio_players.entry(receiver).or_default();
                            state.stop_count = state.stop_count.saturating_add(1);
                            state.is_playing = false;
                            state.stop_count
                        };
                        self.audio_trace_note_objc_audio_selector(&receiver_class_desc, selector.as_str(), None, false);
                        note = Some(format!("audio player stop player={} count={}", self.describe_ptr(receiver), stop_count));
                        receiver
                    },
                    "playing" if Self::audio_is_objc_player_class(&receiver_class_desc) => {
                        if self.runtime.ui_runtime.audio_players.get(&receiver).map(|s| s.is_playing).unwrap_or(false) { 1 } else { 0 }
                    },
                    "setVolume:" if Self::audio_is_objc_player_class(&receiver_class_desc) => {
                        let volume = f32::from_bits(arg2);
                        let state = self.runtime.ui_runtime.audio_players.entry(receiver).or_default();
                        state.volume = volume;
                        self.audio_trace_note_objc_audio_selector(&receiver_class_desc, selector.as_str(), None, false);
                        note = Some(format!("audio player setVolume player={} volume={:.3}", self.describe_ptr(receiver), volume));
                        receiver
                    },
                    "volume" if Self::audio_is_objc_player_class(&receiver_class_desc) => {
                        self.runtime.ui_runtime.audio_players.get(&receiver).map(|s| s.volume.to_bits()).unwrap_or_else(|| 1.0f32.to_bits())
                    },
                    "setNumberOfLoops:" if Self::audio_is_objc_player_class(&receiver_class_desc) => {
                        let loops = arg2 as i32;
                        let state = self.runtime.ui_runtime.audio_players.entry(receiver).or_default();
                        state.number_of_loops = loops;
                        self.audio_trace_note_objc_audio_selector(&receiver_class_desc, selector.as_str(), None, false);
                        note = Some(format!("audio player setLoops player={} loops={}", self.describe_ptr(receiver), loops));
                        receiver
                    },
                    "numberOfLoops" if Self::audio_is_objc_player_class(&receiver_class_desc) => {
                        self.runtime.ui_runtime.audio_players.get(&receiver).map(|s| s.number_of_loops as u32).unwrap_or(0)
                    },
                    "preloadBackgroundMusic:" | "playBackgroundMusic:" | "playBackgroundMusic:loop:" | "stopBackgroundMusic" | "pauseBackgroundMusic" | "resumeBackgroundMusic" | "setBackgroundMusicVolume:" | "preloadEffect:" | "playEffect:" | "playEffect:loop:" | "playEffect:pitch:pan:gain:" | "stopEffect:" | "unloadEffect:" | "setEffectsVolume:" if Self::audio_is_objc_engine_class(&receiver_class_desc) => {
                        let resource = self
                            .resolve_path_from_url_like_value(arg2, false)
                            .map(|path| path.display().to_string())
                            .or_else(|| self.guest_string_value(arg2));
                        self.audio_trace_note_objc_audio_selector(&receiver_class_desc, selector.as_str(), resource.clone(), false);
                        note = Some(format!("audio engine selector={} receiver={} resource={}", selector, self.describe_ptr(receiver), resource.unwrap_or_else(|| "<none>".to_string())));
                        match selector.as_str() {
                            "playEffect:" | "playEffect:loop:" | "playEffect:pitch:pan:gain:" => {
                                self.runtime.audio_trace.next_objc_audio_effect_id = self.runtime.audio_trace.next_objc_audio_effect_id.saturating_add(1).max(1);
                                self.runtime.audio_trace.next_objc_audio_effect_id
                            }
                            _ => receiver,
                        }
                    },
                    _ if Self::audio_is_objc_audio_class(&receiver_class_desc) || Self::audio_is_objc_audio_selector(selector.as_str()) => {
                        let resource = self
                            .resolve_path_from_url_like_value(arg2, false)
                            .map(|path| path.display().to_string())
                            .or_else(|| self.guest_string_value(arg2));
                        self.audio_trace_note_objc_audio_selector(&receiver_class_desc, selector.as_str(), resource.clone(), true);
                        note = Some(format!("objc audio fallback class={} selector={} resource={}", if receiver_class_desc.is_empty() { "<unknown>" } else { &receiver_class_desc }, selector, resource.unwrap_or_else(|| "<none>".to_string())));
                        receiver
                    },
                    "new" => {
                        if receiver == self.runtime.ui_objects.app {
                            self.runtime.ui_objects.app
                        } else if receiver == self.runtime.ui_graphics.eagl_context {
                            self.runtime.ui_graphics.eagl_context
                        } else {
                            receiver
                        }
                    },
                    "respondsToSelector:" | "isKindOfClass:" | "isMemberOfClass:" | "isViewLoaded" | "isOpaque" => 1,
                    "description" => {
                        if let Some(text) = self.synthetic_file_url_absolute_string_value(receiver) {
                            self.materialize_host_string_object("NSString.fileURL.description", &text)
                        } else if receiver == self.runtime.ui_network.network_url {
                            HLE_FAKE_NSSTRING_URL_ABSOLUTE
                        } else if receiver == self.runtime.ui_network.network_request {
                            HLE_FAKE_NSSTRING_HTTP_METHOD
                        } else if receiver == self.runtime.ui_network.network_response {
                            HLE_FAKE_NSSTRING_MIME_TYPE
                        } else if receiver == self.runtime.ui_network.network_error {
                            HLE_FAKE_NSSTRING_ERROR_DESCRIPTION
                        } else if self.string_backing(receiver).is_some() {
                            receiver
                        } else {
                            self.runtime.ui_network.network_data
                        }
                    },
                    _ => receiver,
                };
                let result = if selector.starts_with("init") && result == 0 && self.runtime.objc.objc_classes_by_ptr.contains_key(&receiver) {
                    self.objc_hle_alloc_like(receiver, 0, "init-fallback")
                } else {
                    result
                };
                if selector.starts_with("init") {
                    self.objc_note_init_result(receiver, result);
                }
                let mut detail = format!(
                    "hle objc_msgSend(receiver={}, sel={}, arg2={}, arg3={}, result={})",
                    receiver_desc,
                    selector,
                    arg2_desc,
                    arg3_desc,
                    self.describe_ptr(result),
                );
                if let Some(extra) = note {
                    detail.push_str(&format!(", note={extra}"));
                }
                self.diag.trace.push(self.hle_trace_line(index, current_pc, &label, &detail));
                self.cpu.regs[0] = result;
                self.cpu.regs[15] = self.cpu.regs[14] & !1;
                self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
                return Ok(Some(StepControl::Continue));
            }
            _ => {}
        }
        Ok(None)
    }

    fn trace_thumb_line(&self, index: u64, pc: u32, halfword: u16) -> String {
        let label = self
            .symbol_label(pc)
            .map(|name| format!(" <{name}>"))
            .unwrap_or_default();
        format!(
            "#{:02} pc=0x{:08x}{} {} | r0=0x{:08x} r1=0x{:08x} r2=0x{:08x} r3=0x{:08x} sp=0x{:08x} lr=0x{:08x}",
            index,
            pc,
            label,
            format_thumb_halfword(halfword),
            self.cpu.regs[0],
            self.cpu.regs[1],
            self.cpu.regs[2],
            self.cpu.regs[3],
            self.cpu.regs[13],
            self.cpu.regs[14],
        )
    }

    pub(crate) fn step_thumb(&mut self, halfword: u16, current_pc: u32) -> CoreResult<StepControl> {
        match halfword & 0xFF87 {
            0x4700 | 0x4780 => {
                let rm = ((halfword >> 3) & 0xF) as usize;
                let target = self.cpu.regs[rm];
                if target == 0 {
                    return Ok(StepControl::Stop(format!("thumb bx/blx r{rm} resolved to 0x00000000")));
                }
                let is_blx = (halfword & 0x0080) != 0;
                if is_blx {
                    let lr = (current_pc.wrapping_add(2)) | 1;
                    self.cpu.regs[14] = lr;
                    self.trace_branch_target("thumb blx", rm, target, Some(lr));
                } else {
                    self.trace_branch_target("thumb bx", rm, target, None);
                }
                self.cpu.thumb = (target & 1) != 0;
                self.cpu.regs[15] = target & !1;
                return Ok(StepControl::Continue);
            }
            _ => {}
        }

        if (halfword & 0xFE00) == 0xB400 {
            let r = ((halfword >> 8) & 1) != 0;
            let reg_list = (halfword & 0x00FF) as u16;
            let mut regs = Vec::new();
            for reg in 0..8usize {
                if (reg_list & (1 << reg)) != 0 {
                    regs.push(reg);
                }
            }
            if r {
                regs.push(14);
            }
            let count = regs.len() as u32;
            let mut addr = self.cpu.regs[13].wrapping_sub(count * 4);
            for reg in regs {
                let value = self.cpu.regs[reg];
                self.write_u32_le(addr, value)?;
                addr = addr.wrapping_add(4);
            }
            self.cpu.regs[13] = self.cpu.regs[13].wrapping_sub(count * 4);
            self.cpu.regs[15] = current_pc.wrapping_add(2);
            return Ok(StepControl::Continue);
        }

        if (halfword & 0xFE00) == 0xBC00 {
            let p = ((halfword >> 8) & 1) != 0;
            let reg_list = (halfword & 0x00FF) as u16;
            let mut addr = self.cpu.regs[13];
            for reg in 0..8usize {
                if (reg_list & (1 << reg)) != 0 {
                    self.cpu.regs[reg] = self.read_u32_le(addr)?;
                    addr = addr.wrapping_add(4);
                }
            }
            if p {
                let value = self.read_u32_le(addr)?;
                addr = addr.wrapping_add(4);
                let control = self.set_reg_branch_aware(15, value);
                self.cpu.regs[13] = addr;
                return Ok(control);
            }
            self.cpu.regs[13] = addr;
            self.cpu.regs[15] = current_pc.wrapping_add(2);
            return Ok(StepControl::Continue);
        }

        if (halfword & 0xF800) == 0x2000 {
            let rd = ((halfword >> 8) & 0x7) as usize;
            let imm = (halfword & 0xFF) as u32;
            self.cpu.regs[rd] = imm;
            self.apply_logic_flags(imm, self.cpu.flags.c);
            self.cpu.regs[15] = current_pc.wrapping_add(2);
            return Ok(StepControl::Continue);
        }

        if (halfword & 0xF800) == 0x2800 {
            let rn = ((halfword >> 8) & 0x7) as usize;
            let imm = (halfword & 0xFF) as u32;
            let lhs = self.cpu.regs[rn];
            let result = lhs.wrapping_sub(imm);
            self.apply_sub_flags(lhs, imm, result);
            self.cpu.regs[15] = current_pc.wrapping_add(2);
            return Ok(StepControl::Continue);
        }

        if (halfword & 0xF800) == 0x3000 {
            let rd = ((halfword >> 8) & 0x7) as usize;
            let imm = (halfword & 0xFF) as u32;
            let lhs = self.cpu.regs[rd];
            let result = lhs.wrapping_add(imm);
            self.cpu.regs[rd] = result;
            self.apply_add_flags(lhs, imm, result);
            self.cpu.regs[15] = current_pc.wrapping_add(2);
            return Ok(StepControl::Continue);
        }

        if (halfword & 0xF800) == 0x3800 {
            let rd = ((halfword >> 8) & 0x7) as usize;
            let imm = (halfword & 0xFF) as u32;
            let lhs = self.cpu.regs[rd];
            let result = lhs.wrapping_sub(imm);
            self.cpu.regs[rd] = result;
            self.apply_sub_flags(lhs, imm, result);
            self.cpu.regs[15] = current_pc.wrapping_add(2);
            return Ok(StepControl::Continue);
        }

        if (halfword & 0xF800) == 0x4800 {
            let rd = ((halfword >> 8) & 0x7) as usize;
            let imm = ((halfword & 0xFF) as u32) << 2;
            let base = current_pc.wrapping_add(4) & !3;
            let value = self.read_u32_le(base.wrapping_add(imm))?;
            self.cpu.regs[rd] = value;
            self.cpu.regs[15] = current_pc.wrapping_add(2);
            return Ok(StepControl::Continue);
        }

        if (halfword & 0xFF80) == 0xB000 {
            let imm = ((halfword & 0x7F) as u32) << 2;
            self.cpu.regs[13] = self.cpu.regs[13].wrapping_add(imm);
            self.cpu.regs[15] = current_pc.wrapping_add(2);
            return Ok(StepControl::Continue);
        }

        if (halfword & 0xFF80) == 0xB080 {
            let imm = ((halfword & 0x7F) as u32) << 2;
            self.cpu.regs[13] = self.cpu.regs[13].wrapping_sub(imm);
            self.cpu.regs[15] = current_pc.wrapping_add(2);
            return Ok(StepControl::Continue);
        }

        if (halfword & 0xF800) == 0xE000 {
            let imm11 = (halfword & 0x07FF) as i16;
            let signed = ((imm11 << 5) as i16 >> 4) as i32;
            self.cpu.regs[15] = current_pc.wrapping_add(4).wrapping_add(signed as u32);
            return Ok(StepControl::Continue);
        }

        Err(CoreError::Unsupported(format!(
            "thumb opcode is not implemented yet for 0x{halfword:04x}"
        )))
    }



// Core ARM data-processing and load/store execution.

    fn cond_pass(&self, cond: u32) -> bool {
        match cond {
            0x0 => self.cpu.flags.z,
            0x1 => !self.cpu.flags.z,
            0x2 => self.cpu.flags.c,
            0x3 => !self.cpu.flags.c,
            0x4 => self.cpu.flags.n,
            0x5 => !self.cpu.flags.n,
            0x6 => self.cpu.flags.v,
            0x7 => !self.cpu.flags.v,
            0x8 => self.cpu.flags.c && !self.cpu.flags.z,
            0x9 => !self.cpu.flags.c || self.cpu.flags.z,
            0xA => self.cpu.flags.n == self.cpu.flags.v,
            0xB => self.cpu.flags.n != self.cpu.flags.v,
            0xC => !self.cpu.flags.z && (self.cpu.flags.n == self.cpu.flags.v),
            0xD => self.cpu.flags.z || (self.cpu.flags.n != self.cpu.flags.v),
            0xE => true,
            _ => false,
        }
    }

    fn apply_logic_flags(&mut self, value: u32, carry: bool) {
        self.cpu.flags.n = (value & 0x8000_0000) != 0;
        self.cpu.flags.z = value == 0;
        self.cpu.flags.c = carry;
    }

    fn apply_add_flags(&mut self, lhs: u32, rhs: u32, result: u32) {
        let wide = lhs as u64 + rhs as u64;
        self.cpu.flags.n = (result & 0x8000_0000) != 0;
        self.cpu.flags.z = result == 0;
        self.cpu.flags.c = wide > 0xFFFF_FFFF;
        self.cpu.flags.v = (((lhs ^ result) & (rhs ^ result)) & 0x8000_0000) != 0;
    }

    fn apply_sub_flags(&mut self, lhs: u32, rhs: u32, result: u32) {
        self.cpu.flags.n = (result & 0x8000_0000) != 0;
        self.cpu.flags.z = result == 0;
        self.cpu.flags.c = lhs >= rhs;
        self.cpu.flags.v = (((lhs ^ rhs) & (lhs ^ result)) & 0x8000_0000) != 0;
    }

    fn decode_imm_operand2(word: u32) -> (u32, bool) {
        let imm8 = word & 0xFF;
        let rotate = ((word >> 8) & 0xF) * 2;
        let value = imm8.rotate_right(rotate);
        let carry = if rotate == 0 {
            false
        } else {
            (value & 0x8000_0000) != 0
        };
        (value, carry)
    }

    fn shift_with_carry(value: u32, shift_type: u32, shift_imm: u32, carry_in: bool) -> (u32, bool) {
        match shift_type {
            0 => {
                if shift_imm == 0 {
                    (value, carry_in)
                } else {
                    let carry = ((value >> (32 - shift_imm)) & 1) != 0;
                    (value.wrapping_shl(shift_imm), carry)
                }
            }
            1 => {
                let amt = if shift_imm == 0 { 32 } else { shift_imm };
                if amt == 32 {
                    (0, (value & 0x8000_0000) != 0)
                } else {
                    let carry = ((value >> (amt - 1)) & 1) != 0;
                    (value >> amt, carry)
                }
            }
            2 => {
                let amt = if shift_imm == 0 { 32 } else { shift_imm };
                if amt >= 32 {
                    let fill = if (value & 0x8000_0000) != 0 { u32::MAX } else { 0 };
                    (fill, (value & 0x8000_0000) != 0)
                } else {
                    let carry = ((value >> (amt - 1)) & 1) != 0;
                    (((value as i32) >> amt) as u32, carry)
                }
            }
            3 => {
                if shift_imm == 0 {
                    let carry_bit = if carry_in { 1u32 } else { 0u32 };
                    let carry = (value & 1) != 0;
                    (((carry_bit << 31) | (value >> 1)), carry)
                } else {
                    let amt = shift_imm % 32;
                    let rotated = value.rotate_right(amt);
                    let carry = (rotated & 0x8000_0000) != 0;
                    (rotated, carry)
                }
            }
            _ => (value, carry_in),
        }
    }

    fn shift_with_carry_reg(value: u32, shift_type: u32, shift_reg_value: u32, carry_in: bool) -> (u32, bool) {
        let shift = shift_reg_value & 0xFF;
        if shift == 0 {
            return (value, carry_in);
        }
        match shift_type {
            0 => {
                if shift < 32 {
                    let carry = ((value >> (32 - shift)) & 1) != 0;
                    (value.wrapping_shl(shift), carry)
                } else if shift == 32 {
                    (0, (value & 1) != 0)
                } else {
                    (0, false)
                }
            }
            1 => {
                if shift < 32 {
                    let carry = ((value >> (shift - 1)) & 1) != 0;
                    (value >> shift, carry)
                } else if shift == 32 {
                    (0, (value & 0x8000_0000) != 0)
                } else {
                    (0, false)
                }
            }
            2 => {
                if shift < 32 {
                    let carry = ((value >> (shift - 1)) & 1) != 0;
                    (((value as i32) >> shift) as u32, carry)
                } else {
                    let fill = if (value & 0x8000_0000) != 0 { u32::MAX } else { 0 };
                    (fill, (value & 0x8000_0000) != 0)
                }
            }
            3 => {
                let amount = shift % 32;
                if amount == 0 {
                    (value, (value & 0x8000_0000) != 0)
                } else {
                    let rotated = value.rotate_right(amount);
                    let carry = (rotated & 0x8000_0000) != 0;
                    (rotated, carry)
                }
            }
            _ => (value, carry_in),
        }
    }

    fn decode_reg_operand2(&mut self, word: u32, current_pc: u32) -> CoreResult<(u32, bool)> {
        let rm = (word & 0xF) as usize;
        let shift_type = (word >> 5) & 0x3;
        let rm_value = self.reg_operand(rm, current_pc);
        if (word & (1 << 4)) != 0 {
            let rs = ((word >> 8) & 0xF) as usize;
            let rs_value = self.reg_operand(rs, current_pc);
            let (value, carry) = Self::shift_with_carry_reg(rm_value, shift_type, rs_value, self.cpu.flags.c);
            self.exec.arm_reg_shift_operand2_ops = self.exec.arm_reg_shift_operand2_ops.saturating_add(1);
            let shift_name = match shift_type {
                0 => "lsl",
                1 => "lsr",
                2 => "asr",
                3 => "ror",
                _ => "shift",
            };
            self.exec.arm_last_reg_shift = Some(format!(
                "r{rm}, {shift_name} by r{rs} (0x{:02x}) -> 0x{:08x}",
                rs_value & 0xFF,
                value
            ));
            Ok((value, carry))
        } else {
            let shift_imm = (word >> 7) & 0x1F;
            Ok(Self::shift_with_carry(rm_value, shift_type, shift_imm, self.cpu.flags.c))
        }
    }

    fn try_exec_armv6_extend(&mut self, word: u32, current_pc: u32) -> Option<CoreResult<StepControl>> {
        let kind = word & 0x0FFF_03F0;
        let (name, result) = match kind {
            0x06AF_0070 => {
                let rm = (word & 0xF) as usize;
                let rotate = ((word >> 10) & 0x3) * 8;
                let src = self.reg_operand(rm, current_pc).rotate_right(rotate);
                ("sxtb", ((src as u8) as i8 as i32) as u32)
            }
            0x06BF_0070 => {
                let rm = (word & 0xF) as usize;
                let rotate = ((word >> 10) & 0x3) * 8;
                let src = self.reg_operand(rm, current_pc).rotate_right(rotate);
                ("sxth", ((src as u16) as i16 as i32) as u32)
            }
            0x06EF_0070 => {
                let rm = (word & 0xF) as usize;
                let rotate = ((word >> 10) & 0x3) * 8;
                let src = self.reg_operand(rm, current_pc).rotate_right(rotate);
                ("uxtb", src & 0xFF)
            }
            0x06FF_0070 => {
                let rm = (word & 0xF) as usize;
                let rotate = ((word >> 10) & 0x3) * 8;
                let src = self.reg_operand(rm, current_pc).rotate_right(rotate);
                ("uxth", src & 0xFFFF)
            }
            _ => return None,
        };

        let rd = ((word >> 12) & 0xF) as usize;
        let branch_control = self.set_reg_branch_aware(rd, result);
        let resolved = match branch_control {
            StepControl::Continue => {
                if rd != 15 {
                    self.cpu.regs[15] = current_pc.wrapping_add(4);
                }
                StepControl::Continue
            }
            StepControl::Stop(reason) => StepControl::Stop(reason),
        };
        self.diag.trace.push(format!(
            "     ↳ armv6 extend {} result=0x{:08x}",
            name, result
        ));
        Some(Ok(resolved))
    }

    fn decode_data_processing_operand2(&mut self, word: u32, current_pc: u32) -> CoreResult<(u32, bool)> {
        if (word & (1 << 25)) != 0 {
            Ok(Self::decode_imm_operand2(word))
        } else {
            self.decode_reg_operand2(word, current_pc)
        }
    }

    fn is_extra_load_store(word: u32) -> bool {
        ((word >> 25) & 0x7) == 0
            && (word & (1 << 7)) != 0
            && (word & (1 << 4)) != 0
            && ((word >> 5) & 0x3) != 0
    }

    fn exec_extra_load_store(&mut self, word: u32, current_pc: u32) -> CoreResult<StepControl> {
        let p = ((word >> 24) & 1) != 0;
        let u = ((word >> 23) & 1) != 0;
        let i = ((word >> 22) & 1) != 0;
        let w = ((word >> 21) & 1) != 0;
        let l = ((word >> 20) & 1) != 0;
        let rn = ((word >> 16) & 0xF) as usize;
        let rd = ((word >> 12) & 0xF) as usize;
        let sh = (word >> 5) & 0x3;

        let base = self.reg_operand(rn, current_pc);
        let offset = if i {
            (((word >> 8) & 0xF) << 4) | (word & 0xF)
        } else {
            let rm = (word & 0xF) as usize;
            self.reg_operand(rm, current_pc)
        };
        let offset_addr = if u { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let address = if p { offset_addr } else { base };

        let op_name = match (l, sh) {
            (false, 0x1) => "strh",
            (true, 0x1) => "ldrh",
            (true, 0x2) => "ldrsb",
            (true, 0x3) => "ldrsh",
            _ => {
                return Err(CoreError::Unsupported(format!(
                    "extra load/store form is not implemented yet for 0x{word:08x}"
                )))
            }
        };

        self.exec.arm_extra_load_store_ops = self.exec.arm_extra_load_store_ops.saturating_add(1);
        if l {
            self.exec.arm_extra_load_store_loads = self.exec.arm_extra_load_store_loads.saturating_add(1);
        } else {
            self.exec.arm_extra_load_store_stores = self.exec.arm_extra_load_store_stores.saturating_add(1);
        }
        self.exec.arm_last_extra_load_store = Some(format!(
            "{op_name} r{rd}, [r{rn}{}0x{:x}] @ 0x{:08x}",
            if u { ", #+" } else { ", #-" },
            offset,
            address
        ));

        if l {
            let value = match sh {
                0x1 => self.read_u16_le(address)? as u32,
                0x2 => (self.read_u8(address)? as i8 as i32) as u32,
                0x3 => (self.read_u16_le(address)? as i16 as i32) as u32,
                _ => unreachable!(),
            };
            let branch_control = self.set_reg_branch_aware(rd, value);
            if (!p || w) && rn != 15 {
                self.cpu.regs[rn] = offset_addr;
            }
            match branch_control {
                StepControl::Continue => {
                    if rd != 15 {
                        self.cpu.regs[15] = current_pc.wrapping_add(4);
                    }
                    Ok(StepControl::Continue)
                }
                StepControl::Stop(reason) => Ok(StepControl::Stop(reason)),
            }
        } else {
            match sh {
                0x1 => self.write_u16_le(address, self.reg_operand(rd, current_pc) as u16)?,
                _ => unreachable!(),
            }
            if (!p || w) && rn != 15 {
                self.cpu.regs[rn] = offset_addr;
            }
            self.cpu.regs[15] = current_pc.wrapping_add(4);
            Ok(StepControl::Continue)
        }
    }

    fn exec_branch(&mut self, word: u32, current_pc: u32) -> StepControl {
        let link = ((word >> 24) & 1) != 0;
        let imm24 = word & 0x00FF_FFFF;
        let signed = (((imm24 << 8) as i32) >> 6) as i32;
        let target = current_pc
            .wrapping_add(8)
            .wrapping_add(signed as u32);
        if link {
            self.cpu.regs[14] = current_pc.wrapping_add(4);
        }
        self.cpu.regs[15] = target;
        StepControl::Continue
    }

    fn exec_bx(&mut self, word: u32, current_pc: u32) -> StepControl {
        let rm = (word & 0xF) as usize;
        let target = self.reg_operand(rm, current_pc);
        if target == 0 {
            return StepControl::Stop(format!("bx r{rm} resolved to 0x00000000"));
        }
        self.trace_branch_target("bx", rm, target, None);
        self.cpu.thumb = (target & 1) != 0;
        self.cpu.regs[15] = target & !1;
        StepControl::Continue
    }

    fn exec_blx_reg(&mut self, word: u32, current_pc: u32) -> StepControl {
        let rm = (word & 0xF) as usize;
        let target = self.reg_operand(rm, current_pc);
        if target == 0 {
            return StepControl::Stop(format!("blx r{rm} resolved to 0x00000000"));
        }
        let lr = current_pc.wrapping_add(4);
        self.cpu.regs[14] = lr;
        self.trace_branch_target("blx", rm, target, Some(lr));
        self.cpu.thumb = (target & 1) != 0;
        self.cpu.regs[15] = target & !1;
        StepControl::Continue
    }

    fn exec_single_data_transfer(&mut self, word: u32, current_pc: u32) -> CoreResult<StepControl> {
        let i = ((word >> 25) & 1) != 0;
        let p = ((word >> 24) & 1) != 0;
        let u = ((word >> 23) & 1) != 0;
        let b = ((word >> 22) & 1) != 0;
        let w = ((word >> 21) & 1) != 0;
        let l = ((word >> 20) & 1) != 0;
        let rn = ((word >> 16) & 0xF) as usize;
        let rd = ((word >> 12) & 0xF) as usize;

        // STRB/LDRB support for common bootstrap/runtime access patterns.
        // We intentionally zero-extend LDRB and store low 8 bits on STRB.

        let base = self.reg_operand(rn, current_pc);
        let offset = if i {
            let (val, _) = self.decode_reg_operand2(word, current_pc)?;
            val
        } else {
            word & 0xFFF
        };

        let offset_addr = if u { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let address = if p { offset_addr } else { base };

        if l {
            let value = if b {
                self.read_u8(address)? as u32
            } else {
                self.read_u32_le(address)?
            };
            let branch_control = self.set_reg_branch_aware(rd, value);
            if (!p || w) && rn != 15 {
                self.cpu.regs[rn] = offset_addr;
            }
            match branch_control {
                StepControl::Continue => {
                    if rd != 15 {
                        self.cpu.regs[15] = current_pc.wrapping_add(4);
                    }
                    Ok(StepControl::Continue)
                }
                StepControl::Stop(reason) => Ok(StepControl::Stop(reason)),
            }
        } else {
            let value = self.reg_operand(rd, current_pc);
            if b {
                self.write_u8(address, (value & 0xFF) as u8)?;
            } else {
                self.write_u32_le(address, value)?;
            }
            if (!p || w) && rn != 15 {
                self.cpu.regs[rn] = offset_addr;
            }
            self.cpu.regs[15] = current_pc.wrapping_add(4);
            Ok(StepControl::Continue)
        }
    }


    fn exec_block_data_transfer(&mut self, word: u32, current_pc: u32) -> CoreResult<StepControl> {
        let p = ((word >> 24) & 1) != 0;
        let u = ((word >> 23) & 1) != 0;
        let s = ((word >> 22) & 1) != 0;
        let w = ((word >> 21) & 1) != 0;
        let l = ((word >> 20) & 1) != 0;
        let rn = ((word >> 16) & 0xF) as usize;
        let reg_list = word & 0xFFFF;

        if s {
            return Err(CoreError::Unsupported(format!(
                "^ suffix / user-bank block transfer is not implemented yet for 0x{word:08x}"
            )));
        }
        if reg_list == 0 {
            return Err(CoreError::Unsupported(format!(
                "empty register list block transfer is not implemented yet for 0x{word:08x}"
            )));
        }

        let reg_count = reg_list.count_ones();
        let base = self.reg_operand(rn, current_pc);
        let start_addr = match (p, u) {
            (false, true) => base,
            (true, true) => base.wrapping_add(4),
            (false, false) => base.wrapping_sub(4 * (reg_count.saturating_sub(1))),
            (true, false) => base.wrapping_sub(4 * reg_count),
        };
        let final_base = if u {
            base.wrapping_add(4 * reg_count)
        } else {
            base.wrapping_sub(4 * reg_count)
        };

        let mut addr = start_addr;
        let rn_in_list = (reg_list & (1 << rn)) != 0;
        let mut branch_control = StepControl::Continue;

        for reg in 0..16usize {
            if (reg_list & (1 << reg)) == 0 {
                continue;
            }
            if l {
                let value = self.read_u32_le(addr)?;
                branch_control = self.set_reg_branch_aware(reg, value);
            } else {
                let value = self.reg_operand(reg, current_pc);
                self.write_u32_le(addr, value)?;
            }
            addr = addr.wrapping_add(4);
            if matches!(branch_control, StepControl::Stop(_)) {
                break;
            }
        }

        if w && !(l && rn_in_list) {
            self.cpu.regs[rn] = final_base;
        }

        match branch_control {
            StepControl::Continue => {
                if !l || (reg_list & (1 << 15)) == 0 {
                    self.cpu.regs[15] = current_pc.wrapping_add(4);
                }
                Ok(StepControl::Continue)
            }
            StepControl::Stop(reason) => Ok(StepControl::Stop(reason)),
        }
    }

    fn is_probably_valid_stack_addr(&self, addr: u32, bytes: u32) -> bool {
        if addr == 0 || (addr & 3) != 0 {
            return false;
        }
        self.find_region(addr, bytes).is_some()
    }

    fn repair_known_frame_epilogue_sp(&mut self, current_pc: u32, word: u32, before_sp: u32, computed_sp: u32, op2: u32) -> Option<u32> {
        let (site_pc, site_word, bytes, site_name) = if current_pc == EXACT_EPILOGUE_SITE_PC && word == EXACT_EPILOGUE_SITE_WORD {
            (EXACT_EPILOGUE_SITE_PC, EXACT_EPILOGUE_SITE_WORD, EXACT_EPILOGUE_VPOP_BYTES, "exact")
        } else if current_pc == SPRITE_TEXCOORDS_EPILOGUE_SITE_PC && word == SPRITE_TEXCOORDS_EPILOGUE_SITE_WORD {
            (
                SPRITE_TEXCOORDS_EPILOGUE_SITE_PC,
                SPRITE_TEXCOORDS_EPILOGUE_SITE_WORD,
                SPRITE_TEXCOORDS_EPILOGUE_VPOP_BYTES,
                "sprite-texcoords",
            )
        } else {
            return None;
        };

        self.exec.arm_exact_epilogue_site_hits = self.exec.arm_exact_epilogue_site_hits.saturating_add(1);
        self.exec.arm_exact_epilogue_last_pc = Some(current_pc);
        self.exec.arm_exact_epilogue_last_before_sp = Some(before_sp);
        self.exec.arm_exact_epilogue_last_after_sp = Some(computed_sp);
        self.exec.arm_exact_epilogue_last_r0 = Some(self.cpu.regs[0]);
        self.exec.arm_exact_epilogue_last_r7 = Some(self.cpu.regs[7]);
        self.exec.arm_exact_epilogue_last_r8 = Some(self.cpu.regs[8]);
        self.exec.arm_exact_epilogue_last_lr = Some(self.cpu.regs[14]);

        if self.is_probably_valid_stack_addr(computed_sp, bytes) {
            self.exec.arm_exact_epilogue_last_repair = Some(format!("{site_name}:not-needed"));
            return Some(computed_sp);
        }

        let current_sp_candidate = before_sp;
        let r0_candidate = self.cpu.regs[0];
        let fp_candidate = self.cpu.regs[7].wrapping_sub(op2);
        let candidates = [
            ("current-sp", current_sp_candidate, true),
            ("r0-saved", r0_candidate, true),
            ("fp-minus-imm", fp_candidate, false),
        ];

        for (label, candidate, restore_fp) in candidates {
            if self.is_probably_valid_stack_addr(candidate, bytes) {
                self.exec.arm_exact_epilogue_repairs = self.exec.arm_exact_epilogue_repairs.saturating_add(1);
                self.exec.arm_exact_epilogue_last_after_sp = Some(candidate);
                if restore_fp {
                    self.cpu.regs[7] = candidate.wrapping_add(op2);
                }
                self.exec.arm_exact_epilogue_last_repair = Some(format!(
                    "{site_name}:{label}: before=0x{before_sp:08x} computed=0x{computed_sp:08x} repaired=0x{candidate:08x} r0=0x{:08x} r7=0x{:08x} imm=0x{op2:x}",
                    self.cpu.regs[0],
                    self.cpu.regs[7],
                ));
                self.diag.trace.push(format!(
                    "     ↳ frame-epilogue repair[{site_name}] pc=0x{site_pc:08x} word=0x{site_word:08x} before_sp=0x{before_sp:08x} computed_sp=0x{computed_sp:08x} repaired_sp=0x{candidate:08x} via={label} r0=0x{:08x} r7=0x{:08x} r8=0x{:08x} lr=0x{:08x}",
                    self.cpu.regs[0],
                    self.cpu.regs[7],
                    self.cpu.regs[8],
                    self.cpu.regs[14],
                ));
                return Some(candidate);
            }
        }

        self.exec.arm_exact_epilogue_last_repair = Some(format!(
            "{site_name}:unrepaired: before=0x{before_sp:08x} computed=0x{computed_sp:08x} r0=0x{:08x} r7=0x{:08x} r8=0x{:08x} lr=0x{:08x}",
            self.cpu.regs[0],
            self.cpu.regs[7],
            self.cpu.regs[8],
            self.cpu.regs[14],
        ));
        self.diag.trace.push(format!(
            "     ↳ frame-epilogue invalid[{site_name}] pc=0x{site_pc:08x} word=0x{site_word:08x} before_sp=0x{before_sp:08x} computed_sp=0x{computed_sp:08x} r0=0x{:08x} r7=0x{:08x} r8=0x{:08x} lr=0x{:08x}",
            self.cpu.regs[0],
            self.cpu.regs[7],
            self.cpu.regs[8],
            self.cpu.regs[14],
        ));
        Some(computed_sp)
    }

    pub(crate) fn record_exact_epilogue_trace(&mut self, current_pc: u32, word: u32) {
        if !(EXACT_EPILOGUE_TRACE_START..=EXACT_EPILOGUE_TRACE_END).contains(&current_pc) {
            return;
        }
        self.diag.trace.push(format!(
            "     ↳ exact-epilogue regs pc=0x{current_pc:08x} word=0x{word:08x} r0=0x{:08x} r7=0x{:08x} r8=0x{:08x} sp=0x{:08x} lr=0x{:08x}",
            self.cpu.regs[0],
            self.cpu.regs[7],
            self.cpu.regs[8],
            self.cpu.regs[13],
            self.cpu.regs[14],
        ));
    }

    pub(crate) fn record_audiofile_probe_trace(&mut self, current_pc: u32, word: u32) {
        const AUDIOFILE_GETPROPERTY_PROBE_START: u32 = 0x0001_23dc;
        const AUDIOFILE_GETPROPERTY_PROBE_END: u32 = 0x0001_23ec;
        if !(AUDIOFILE_GETPROPERTY_PROBE_START..=AUDIOFILE_GETPROPERTY_PROBE_END).contains(&current_pc) {
            return;
        }
        let r5 = self.cpu.regs[5];
        let r6 = self.cpu.regs[6];
        let slot = if r6 != 0 { self.read_u32_le(r6).ok() } else { None };
        let io_size = if self.cpu.regs[13] != 0 {
            self.read_u32_le(self.cpu.regs[13]).ok()
        } else {
            None
        };
        self.diag.trace.push(format!(
            "     ↳ audiofile-probe pc=0x{current_pc:08x} word=0x{word:08x} r5(out)=0x{r5:08x} r6(slot)=0x{r6:08x} *(r6)={} sp=0x{:08x} *(sp)={} lr=0x{:08x}",
            slot
                .map(|value| format!("0x{value:08x}"))
                .unwrap_or_else(|| "<unreadable>".to_string()),
            self.cpu.regs[13],
            io_size
                .map(|value| format!("0x{value:08x}"))
                .unwrap_or_else(|| "<unreadable>".to_string()),
            self.cpu.regs[14],
        ));
    }

    fn repair_exact_epilogue_sp(&mut self, current_pc: u32, word: u32, before_sp: u32, computed_sp: u32, op2: u32) -> u32 {
        if let Some(repaired) = self.repair_known_frame_epilogue_sp(current_pc, word, before_sp, computed_sp, op2) {
            if current_pc != EXACT_EPILOGUE_SITE_PC || word != EXACT_EPILOGUE_SITE_WORD {
                return repaired;
            }
            if repaired != computed_sp || self.is_probably_valid_stack_addr(repaired, EXACT_EPILOGUE_VPOP_BYTES) {
                return repaired;
            }
        }

        self.exec.arm_exact_epilogue_site_hits = self.exec.arm_exact_epilogue_site_hits.saturating_add(1);
        self.exec.arm_exact_epilogue_last_pc = Some(current_pc);
        self.exec.arm_exact_epilogue_last_before_sp = Some(before_sp);
        self.exec.arm_exact_epilogue_last_after_sp = Some(computed_sp);
        self.exec.arm_exact_epilogue_last_r0 = Some(self.cpu.regs[0]);
        self.exec.arm_exact_epilogue_last_r7 = Some(self.cpu.regs[7]);
        self.exec.arm_exact_epilogue_last_r8 = Some(self.cpu.regs[8]);
        self.exec.arm_exact_epilogue_last_lr = Some(self.cpu.regs[14]);

        if current_pc != EXACT_EPILOGUE_SITE_PC || word != EXACT_EPILOGUE_SITE_WORD {
            self.exec.arm_exact_epilogue_last_repair = Some("site-mismatch".to_string());
            return computed_sp;
        }

        if self.is_probably_valid_stack_addr(computed_sp, EXACT_EPILOGUE_VPOP_BYTES) {
            self.exec.arm_exact_epilogue_last_repair = Some("not-needed".to_string());
            return computed_sp;
        }

        let r0_candidate = self.cpu.regs[0];
        let current_sp_candidate = before_sp;
        let fp_candidate = self.cpu.regs[7].wrapping_sub(op2);
        let candidates = [
            ("r0-saved", r0_candidate),
            ("current-sp", current_sp_candidate),
            ("fp-minus-imm", fp_candidate),
        ];

        for (label, candidate) in candidates {
            if self.is_probably_valid_stack_addr(candidate, EXACT_EPILOGUE_VPOP_BYTES) {
                self.exec.arm_exact_epilogue_repairs = self.exec.arm_exact_epilogue_repairs.saturating_add(1);
                self.exec.arm_exact_epilogue_last_after_sp = Some(candidate);
                self.exec.arm_exact_epilogue_last_repair = Some(format!(
                    "{label}: before=0x{before_sp:08x} computed=0x{computed_sp:08x} repaired=0x{candidate:08x} r0=0x{:08x} r7=0x{:08x} imm=0x{op2:x}",
                    self.cpu.regs[0],
                    self.cpu.regs[7],
                ));
                if label == "r0-saved" {
                    self.cpu.regs[7] = candidate.wrapping_add(op2);
                }
                self.diag.trace.push(format!(
                    "     ↳ exact-epilogue repair pc=0x{current_pc:08x} word=0x{word:08x} before_sp=0x{before_sp:08x} computed_sp=0x{computed_sp:08x} repaired_sp=0x{candidate:08x} via={label} r0=0x{:08x} r7=0x{:08x} r8=0x{:08x} lr=0x{:08x}",
                    self.cpu.regs[0],
                    self.cpu.regs[7],
                    self.cpu.regs[8],
                    self.cpu.regs[14],
                ));
                return candidate;
            }
        }

        self.exec.arm_exact_epilogue_last_repair = Some(format!(
            "unrepaired: before=0x{before_sp:08x} computed=0x{computed_sp:08x} r0=0x{:08x} r7=0x{:08x} r8=0x{:08x} lr=0x{:08x}",
            self.cpu.regs[0],
            self.cpu.regs[7],
            self.cpu.regs[8],
            self.cpu.regs[14],
        ));
        self.diag.trace.push(format!(
            "     ↳ exact-epilogue invalid pc=0x{current_pc:08x} word=0x{word:08x} before_sp=0x{before_sp:08x} computed_sp=0x{computed_sp:08x} r0=0x{:08x} r7=0x{:08x} r8=0x{:08x} lr=0x{:08x}",
            self.cpu.regs[0],
            self.cpu.regs[7],
            self.cpu.regs[8],
            self.cpu.regs[14],
        ));
        computed_sp
    }

    fn exec_data_processing(&mut self, word: u32, current_pc: u32) -> CoreResult<StepControl> {
        let opcode = (word >> 21) & 0xF;
        let s = ((word >> 20) & 1) != 0;
        let rn = ((word >> 16) & 0xF) as usize;
        let rd = ((word >> 12) & 0xF) as usize;
        let lhs = self.reg_operand(rn, current_pc);
        let (op2, op2_carry) = self.decode_data_processing_operand2(word, current_pc)?;

        let branch_control = match opcode {
            0x0 => {
                let result = lhs & op2;
                if s {
                    self.apply_logic_flags(result, op2_carry);
                }
                self.set_reg_branch_aware(rd, result)
            }
            0x1 => {
                let result = lhs ^ op2;
                if s {
                    self.apply_logic_flags(result, op2_carry);
                }
                self.set_reg_branch_aware(rd, result)
            }
            0x2 => {
                let mut result = lhs.wrapping_sub(op2);
                if rd == 13 && (current_pc == EXACT_EPILOGUE_SITE_PC || current_pc == SPRITE_TEXCOORDS_EPILOGUE_SITE_PC) {
                    result = self.repair_exact_epilogue_sp(current_pc, word, self.cpu.regs[13], result, op2);
                }
                if s {
                    self.apply_sub_flags(lhs, op2, result);
                }
                self.set_reg_branch_aware(rd, result)
            }
            0x3 => {
                let result = op2.wrapping_sub(lhs);
                if s {
                    self.apply_sub_flags(op2, lhs, result);
                }
                self.set_reg_branch_aware(rd, result)
            }
            0x4 => {
                let result = lhs.wrapping_add(op2);
                if s {
                    self.apply_add_flags(lhs, op2, result);
                }
                self.set_reg_branch_aware(rd, result)
            }
            0x5 => {
                let carry_in = if self.cpu.flags.c { 1u32 } else { 0u32 };
                let result = lhs.wrapping_add(op2).wrapping_add(carry_in);
                if s {
                    let wide = lhs as u64 + op2 as u64 + carry_in as u64;
                    self.cpu.flags.n = (result & 0x8000_0000) != 0;
                    self.cpu.flags.z = result == 0;
                    self.cpu.flags.c = wide > 0xFFFF_FFFF;
                    self.cpu.flags.v = (((lhs ^ result) & (op2 ^ result)) & 0x8000_0000) != 0;
                }
                self.set_reg_branch_aware(rd, result)
            }
            0x6 => {
                let carry_in = if self.cpu.flags.c { 1u32 } else { 0u32 };
                let borrow = 1u32.wrapping_sub(carry_in);
                let result = lhs.wrapping_sub(op2).wrapping_sub(borrow);
                if s {
                    let rhs_total = op2.wrapping_add(borrow);
                    self.apply_sub_flags(lhs, rhs_total, result);
                }
                self.set_reg_branch_aware(rd, result)
            }
            0x7 => {
                let carry_in = if self.cpu.flags.c { 1u32 } else { 0u32 };
                let borrow = 1u32.wrapping_sub(carry_in);
                let result = op2.wrapping_sub(lhs).wrapping_sub(borrow);
                if s {
                    let rhs_total = lhs.wrapping_add(borrow);
                    self.apply_sub_flags(op2, rhs_total, result);
                }
                self.set_reg_branch_aware(rd, result)
            }
            0x8 => {
                let result = lhs & op2;
                self.apply_logic_flags(result, op2_carry);
                StepControl::Continue
            }
            0x9 => {
                let result = lhs ^ op2;
                self.apply_logic_flags(result, op2_carry);
                StepControl::Continue
            }
            0xA => {
                let result = lhs.wrapping_sub(op2);
                self.apply_sub_flags(lhs, op2, result);
                StepControl::Continue
            }
            0xB => {
                let result = lhs.wrapping_add(op2);
                self.apply_add_flags(lhs, op2, result);
                StepControl::Continue
            }
            0xC => {
                let result = lhs | op2;
                if s {
                    self.apply_logic_flags(result, op2_carry);
                }
                self.set_reg_branch_aware(rd, result)
            }
            0xD => {
                let result = op2;
                if s {
                    self.apply_logic_flags(result, op2_carry);
                }
                self.set_reg_branch_aware(rd, result)
            }
            0xE => {
                let result = lhs & !op2;
                if s {
                    self.apply_logic_flags(result, op2_carry);
                }
                self.set_reg_branch_aware(rd, result)
            }
            0xF => {
                let result = !op2;
                if s {
                    self.apply_logic_flags(result, op2_carry);
                }
                self.set_reg_branch_aware(rd, result)
            }
            _ => {
                return Err(CoreError::Unsupported(format!(
                    "data-processing opcode {} is not implemented yet for 0x{word:08x}",
                    opcode
                )));
            }
        };

        match branch_control {
            StepControl::Continue => {
                if rd != 15 && opcode != 0xA {
                    self.cpu.regs[15] = current_pc.wrapping_add(4);
                } else if opcode == 0xA {
                    self.cpu.regs[15] = current_pc.wrapping_add(4);
                }
                Ok(StepControl::Continue)
            }
            StepControl::Stop(reason) => Ok(StepControl::Stop(reason)),
        }
    }

    pub(crate) fn step_arm(&mut self, word: u32, current_pc: u32) -> CoreResult<StepControl> {
        let cond = (word >> 28) & 0xF;
        if !self.cond_pass(cond) {
            self.cpu.regs[15] = current_pc.wrapping_add(4);
            return Ok(StepControl::Continue);
        }

        if (word & 0x0FFF_FFF0) == 0x012F_FF10 {
            return Ok(self.exec_bx(word, current_pc));
        }
        if (word & 0x0FFF_FFF0) == 0x012F_FF30 {
            return Ok(self.exec_blx_reg(word, current_pc));
        }
        if let Some(result) = self.try_exec_exact_vfp_opcode_override(word, current_pc) {
            return result;
        }
        if let Some(result) = self.try_exec_vfp_literal_single_transfer(word, current_pc) {
            return result;
        }
        if let Some(result) = self.try_exec_vfp_scalar_data_processing(word, current_pc) {
            return result;
        }
        if let Some(result) = self.try_exec_vfp_vmov_arm_sreg(word, current_pc) {
            return result;
        }
        if let Some(result) = self.try_exec_vfp_vmov_scalar(word, current_pc) {
            return result;
        }
        if let Some(result) = self.try_exec_vfp_unary_scalar_data_processing(word, current_pc) {
            return result;
        }
        if let Some(result) = self.try_exec_vfp_convert_between_float_int(word, current_pc) {
            return result;
        }
        if let Some(result) = self.try_exec_vfp_compare(word, current_pc) {
            return result;
        }
        if let Some(result) = self.try_exec_vfp_vmrs_apsr(word, current_pc) {
            return result;
        }
        if let Some(result) = self.try_exec_vfp_load_store_multiple(word, current_pc) {
            return result;
        }
        if let Some(result) = self.try_exec_armv6_extend(word, current_pc) {
            return result;
        }
        if Self::is_extra_load_store(word) {
            return self.exec_extra_load_store(word, current_pc);
        }

        let class = (word >> 25) & 0x7;
        match class {
            0b101 => Ok(self.exec_branch(word, current_pc)),
            0b100 => self.exec_block_data_transfer(word, current_pc),
            0b010 | 0b011 => self.exec_single_data_transfer(word, current_pc),
            0b110 | 0b111 => Err(CoreError::Unsupported(format!(
                "unhandled ARM coprocessor/SVC instruction 0x{word:08x} at pc=0x{current_pc:08x}; refusing to decode it as data-processing"
            ))),
            _ => self.exec_data_processing(word, current_pc),
        }
    }

    fn trace_line(&self, index: u64, pc: u32, word: u32) -> String {
        let label = self
            .symbol_label(pc)
            .map(|name| format!(" <{name}>"))
            .unwrap_or_default();
        format!(
            "#{:02} pc=0x{:08x}{} {} | r0=0x{:08x} r1=0x{:08x} r2=0x{:08x} r3=0x{:08x} sp=0x{:08x} lr=0x{:08x}",
            index,
            pc,
            label,
            format_arm_word(word),
            self.cpu.regs[0],
            self.cpu.regs[1],
            self.cpu.regs[2],
            self.cpu.regs[3],
            self.cpu.regs[13],
            self.cpu.regs[14],
        )
    }
}
