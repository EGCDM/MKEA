use super::*;
use crate::BackendPolicy;

#[derive(Debug, Clone)]
pub struct MemoryArm32Backend {
    pub(crate) cpu: BackendCpuState,
    pub(crate) address_space: BackendAddressSpaceState,
    pub(crate) runtime: BackendRuntimeState,
    pub(crate) diag: BackendDiagnosticsState,
    title_profile: &'static dyn profiles::TitleProfile,
    pub(crate) exec: CpuExecutionState,
    pub(crate) tuning: BackendTuning,
}

impl Default for MemoryArm32Backend {
    fn default() -> Self {
        Self::with_config(&CoreConfig::default())
    }
}

const EXACT_EPILOGUE_SITE_PC: u32 = 0x0017_2478;
const EXACT_EPILOGUE_SITE_WORD: u32 = 0xE247_D058;
const EXACT_EPILOGUE_TRACE_START: u32 = 0x0017_2474;
const EXACT_EPILOGUE_TRACE_END: u32 = 0x0017_247C;
const EXACT_EPILOGUE_VPOP_BYTES: u32 = 64;
const SPRITE_TEXCOORDS_EPILOGUE_SITE_PC: u32 = 0x001A_AAF8;
const SPRITE_TEXCOORDS_EPILOGUE_SITE_WORD: u32 = 0xE247_D020;
const SPRITE_TEXCOORDS_EPILOGUE_VPOP_BYTES: u32 = 24;

include!("memory/objc_state.rs");
include!("memory/fs_state.rs");
include!("memory/host_input_state.rs");
include!("memory/scheduler_state.rs");
include!("memory/uikit_objects_state.rs");
include!("memory/uikit_runtime_state.rs");
include!("memory/uikit_graphics_state.rs");
include!("memory/uikit_network_state.rs");
include!("memory/uikit_cocos_state.rs");
include!("memory/state.rs");

impl MemoryArm32Backend {
    pub fn with_config(cfg: &CoreConfig) -> Self {
        let bundle_root = cfg.bundle_root.as_ref().map(PathBuf::from).filter(|path| !path.as_os_str().is_empty());
        let title_profile = profiles::detect_title_profile(bundle_root.as_deref());
        let mut backend = Self {
            cpu: BackendCpuState::default(),
            address_space: BackendAddressSpaceState {
                mapped: Vec::new(),
                trampoline_addr: cfg.trampoline_addr,
                trampoline_size: cfg.trampoline_size(),
            },
            runtime: BackendRuntimeState::new(cfg, bundle_root),
            diag: BackendDiagnosticsState::new(),
            title_profile,
            exec: CpuExecutionState::default(),
            tuning: BackendTuning::from_core_config(cfg),
        };
        backend.runtime.fs.bundle_resource_index = backend
            .runtime
            .fs
            .bundle_root
            .as_ref()
            .map(|path| Self::index_bundle_resources(path))
            .unwrap_or_default();
        if let Some(root) = backend.runtime.fs.bundle_root.clone() {
            backend.runtime.fs.bundle_roots.insert(HLE_FAKE_MAIN_BUNDLE, root);
        }
        backend.bootstrap_host_input_script();
        backend
    }
}

impl MemoryArm32Backend {
    pub(crate) fn objc_real_msgsend_dispatches(&self) -> u32 {
        self.runtime.objc.objc_real_msgsend_dispatches
    }

    pub(crate) fn active_profile(&self) -> &'static dyn profiles::TitleProfile {
        self.title_profile
    }

    pub(crate) fn has_specific_profile(&self) -> bool {
        !self.active_profile().is_default()
    }
}

// Split into focused include files to keep the backend reviewable without changing behavior.
include!("memory/host_input.rs");
include!("memory/shared.rs");
include!("memory/objc_runtime.rs");
include!("memory/cocos_runtime.rs");
include!("memory/scheduler_timer_runtime.rs");
include!("memory/scheduler_invocation_runtime.rs");
include!("memory/scheduler_action_runtime.rs");
include!("memory/gles1_soft.rs");
include!("memory/uikit_cocos_helpers.rs");
include!("memory/uikit_graphics_helpers.rs");
include!("memory/scheduler_runloop.rs");
include!("memory/uikit_network_runloop.rs");
include!("memory/scheduler_service.rs");
include!("memory/uikit.rs");
include!("memory/cpu/dispatch_cocos.rs");
include!("memory/cpu/dispatch_network.rs");
include!("memory/cpu/dispatch_graphics.rs");
include!("memory/cpu/dispatch_uikit.rs");
include!("memory/cpu/arm32_core.rs");
include!("memory/cpu/vfp.rs");

impl CpuBackend for MemoryArm32Backend {
    fn policy(&self) -> BackendPolicy {
        BackendPolicy::SyntheticRuntime
    }

    fn map(&mut self, addr: u32, size: u32, prot: u32) -> CoreResult<()> {
        if size == 0 {
            return Err(CoreError::Backend(format!("backend refused zero-size map at 0x{addr:08x}")));
        }
        let end = addr
            .checked_add(size)
            .ok_or_else(|| CoreError::Backend(format!("backend map overflow at 0x{addr:08x} size=0x{size:x}")))?;
        for existing in &self.address_space.mapped {
            let overlaps = addr < existing.end() && end > existing.addr;
            if overlaps {
                return Err(CoreError::Backend(format!(
                    "backend map overlaps existing region 0x{:08x}-0x{:08x}",
                    existing.addr,
                    existing.end()
                )));
            }
        }
        self.address_space.mapped.push(BackendRegion {
            addr,
            size,
            prot,
            data: vec![0u8; size as usize],
        });
        Ok(())
    }

    fn write_mem(&mut self, addr: u32, data: &[u8]) -> CoreResult<()> {
        let size = data.len() as u32;
        let region = self
            .find_region_mut(addr, size)
            .ok_or_else(|| CoreError::Backend(format!("backend cannot write 0x{:x} bytes at 0x{addr:08x}", data.len())))?;
        let offset = (addr - region.addr) as usize;
        region.data[offset..offset + data.len()].copy_from_slice(data);
        self.diag.writes.push((addr, data.len()));
        Ok(())
    }

    fn set_pc(&mut self, pc: u32, thumb: bool) -> CoreResult<()> {
        self.cpu.regs[15] = pc & !1;
        self.cpu.thumb = thumb;
        Ok(())
    }

    fn set_sp(&mut self, sp: u32) -> CoreResult<()> {
        self.cpu.regs[13] = sp;
        Ok(())
    }

    fn set_initial_registers(&mut self, regs: &InitialRegisters) -> CoreResult<()> {
        self.cpu.regs[0] = regs.r0;
        self.cpu.regs[1] = regs.r1;
        self.cpu.regs[2] = regs.r2;
        self.cpu.regs[3] = regs.r3;
        self.cpu.regs[13] = regs.sp;
        self.cpu.regs[14] = regs.lr;
        self.cpu.regs[15] = regs.pc & !1;
        self.cpu.thumb = regs.thumb;
        Ok(())
    }

    fn seed_objc_metadata_sections(&mut self, sections: &[SectionInfo]) {
        self.install_objc_metadata_sections(sections);
    }

    fn run(&mut self, max_instructions: u64) -> CoreResult<()> {
        let pc = self.cpu.regs[15];
        let _sp = self.cpu.regs[13];
        if pc == 0 {
            return Err(CoreError::Backend("backend run requested before PC was installed".into()));
        }
        if _sp == 0 {
            return Err(CoreError::Backend("backend run requested before SP was installed".into()));
        }

        self.diag.first_instruction_addr = Some(pc);
        self.diag.entry_bytes_present = self.find_region(pc, if self.cpu.thumb { 2 } else { 4 }).is_some();
        if !self.diag.entry_bytes_present {
            self.diag.status = format!("entry fetch failed: no mapped bytes at 0x{pc:08x}");
            self.diag.stop_reason = self.diag.status.clone();
            return Err(CoreError::Backend(self.diag.status.clone()));
        }

        let region = self
            .find_region(pc, if self.cpu.thumb { 2 } else { 4 })
            .ok_or_else(|| CoreError::Backend(format!("entry fetch lost region at 0x{pc:08x}")))?;
        if (region.prot & GUEST_PROT_EXEC) == 0 {
            self.diag.status = format!("entry fetch failed: 0x{pc:08x} is not executable");
            self.diag.stop_reason = self.diag.status.clone();
            return Err(CoreError::Backend(self.diag.status.clone()));
        }

        if !self.cpu.thumb && (pc & 3) != 0 {
            self.diag.status = format!("entry fetch failed: ARM PC 0x{pc:08x} is not 4-byte aligned");
            self.diag.stop_reason = self.diag.status.clone();
            return Err(CoreError::Backend(self.diag.status.clone()));
        }

        // The old fixed 2048 clamp is too aggressive for titles like Above & Below:
        // by the time the bootstrap has created the background and started composing
        // the menu layers, the micro-run is already out of budget and we never reach
        // the later objc dispatches/presents that attach the buttons.
        //
        // Keep the run bounded, but size it from the caller budget instead of hard-clamping
        // every bootstrap to 2048 instructions.
        let requested_steps = max_instructions.max(1);
        let live_host = self.tuning.live_host_mode || self.tuning.host_input_script_path.is_some();
        let hard_bootstrap_cap = if self.tuning.dump_frames {
            65_536
        } else if live_host {
            requested_steps
        } else {
            16_384
        };
        let max_steps = requested_steps.min(hard_bootstrap_cap);
        let bootstrap_gate = if live_host { max_steps } else { max_steps.min(256) };
        self.diag.trace.push(format!(
            "bootstrap budget request={} effective={} gate={} dump_frames={}",
            requested_steps,
            max_steps,
            bootstrap_gate,
            if self.tuning.dump_frames { "YES" } else { "NO" }
        ));
        for idx in 0..max_steps {
            if is_stop_requested() {
                self.diag.stop_reason = "host shutdown requested".to_string();
                self.diag.status = "run interrupted by live host shutdown request".to_string();
                break;
            }
            let current_pc = self.cpu.regs[15];
            self.exec.current_exec_pc = current_pc;
            self.exec.current_exec_word = 0;
            self.exec.current_exec_thumb = self.cpu.thumb;

            if let Some(control) = self.handle_hle_stub(idx, current_pc)? {
                self.diag.executed_instructions += 1;
                match control {
                    StepControl::Continue => {
                        self.process_runtime_post_step_hooks("memory:hle-continue");
                        if self.diag.stop_reason == "not-started"
                            && idx + 1 >= bootstrap_gate
                            && idx + 1 < max_steps
                            && !self.runtime.objc.objc_bridge_succeeded
                            && self.runtime.objc.objc_real_msgsend_dispatches == 0
                        {
                            self.diag.stop_reason = format!("step budget {} reached", bootstrap_gate);
                            break;
                        }
                        continue;
                    }
                    StepControl::Stop(reason) => {
                        self.diag.stop_reason = reason;
                        break;
                    }
                }
            }

            if self.cpu.thumb {
                let Some(region) = self.find_region(current_pc, 2) else {
                    self.diag.stop_reason = format!("thumb fetch hit unmapped memory at 0x{current_pc:08x}");
                    break;
                };
                if (region.prot & GUEST_PROT_EXEC) == 0 {
                    self.diag.stop_reason = format!("thumb fetch hit non-executable memory at 0x{current_pc:08x}");
                    break;
                }
                let halfword = self.read_u16_le(current_pc)?;
                self.exec.current_exec_word = halfword as u32;
                if self.diag.first_instruction.is_none() {
                    self.diag.first_instruction = Some(halfword as u32);
                }
                self.diag.trace.push(self.trace_thumb_line(idx, current_pc, halfword));
                self.diag.executed_instructions += 1;
                match self.step_thumb(halfword, current_pc) {
                    Ok(StepControl::Continue) => {
                        self.process_runtime_post_step_hooks("memory:thumb-step");
                    }
                    Ok(StepControl::Stop(reason)) => {
                        self.diag.stop_reason = reason;
                        break;
                    }
                    Err(err) => {
                        self.diag.stop_reason = format!("thumb step error at 0x{current_pc:08x}: {err}");
                        break;
                    }
                }
                if self.diag.stop_reason == "not-started"
                    && idx + 1 >= bootstrap_gate
                    && idx + 1 < max_steps
                    && !self.runtime.objc.objc_bridge_succeeded
                    && self.runtime.objc.objc_real_msgsend_dispatches == 0
                {
                    self.diag.stop_reason = format!("step budget {} reached", bootstrap_gate);
                    break;
                }
                continue;
            }

            let Some(region) = self.find_region(current_pc, 4) else {
                self.diag.stop_reason = format!("fetch hit unmapped memory at 0x{current_pc:08x}");
                break;
            };
            if (region.prot & GUEST_PROT_EXEC) == 0 {
                self.diag.stop_reason = format!("fetch hit non-executable memory at 0x{current_pc:08x}");
                break;
            }
            if (current_pc & 3) != 0 {
                self.diag.stop_reason = format!("ARM PC 0x{current_pc:08x} became unaligned");
                break;
            }

            let word = self.read_u32_le(current_pc)?;
            self.exec.current_exec_word = word;
            if self.diag.first_instruction.is_none() {
                self.diag.first_instruction = Some(word);
            }
            self.diag.trace.push(self.trace_line(idx, current_pc, word));
            self.record_exact_epilogue_trace(current_pc, word);
            self.record_audiofile_probe_trace(current_pc, word);
            self.diag.executed_instructions += 1;

            match self.step_arm(word, current_pc) {
                Ok(StepControl::Continue) => {
                    self.process_runtime_post_step_hooks("memory:arm-step");
                }
                Ok(StepControl::Stop(reason)) => {
                    self.diag.stop_reason = reason;
                    break;
                }
                Err(err) => {
                    self.diag.stop_reason = format!("step error at 0x{current_pc:08x}: {err}");
                    break;
                }
            }

            if self.diag.stop_reason == "not-started"
                && idx + 1 >= bootstrap_gate
                && idx + 1 < max_steps
                && !self.runtime.objc.objc_bridge_succeeded
                && self.runtime.objc.objc_real_msgsend_dispatches == 0
            {
                self.diag.stop_reason = format!("step budget {} reached", bootstrap_gate);
                break;
            }
        }

        if self.diag.stop_reason == "not-started" {
            self.diag.stop_reason = format!("step budget {} reached", max_steps);
        }
        self.diag.status = format!(
            "micro-executed {} instruction(s); stop: {}",
            self.diag.executed_instructions, self.diag.stop_reason
        );
        Ok(())
    }

    fn install_symbol_label(&mut self, addr: u32, label: &str) -> CoreResult<()> {
        self.diag.symbol_labels.insert(addr, label.to_string());
        Ok(())
    }

    fn execution_summary(&self) -> RuntimeBackendExecutionSummary {
        RuntimeBackendExecutionSummary {
            backend_policy: "synthetic-runtime".to_string(),
            total_steps: self.diag.executed_instructions,
            native_steps: 0,
            shadow_steps: self.diag.executed_instructions,
            shadow_trap_steps: 0,
            shadow_fallback_steps: 0,
            shadow_handoff_steps: 0,
            trap_dispatches: 0,
            fallback_dispatches: 0,
            handoff_count: 0,
            native_share_milli: 0,
            shadow_share_milli: if self.diag.executed_instructions == 0 { 0 } else { 1000 },
            trap_classes: Vec::new(),
            top_stop_sites: Vec::new(),
            semantics_candidates: Vec::new(),
            last_trap_class: None,
            last_trap_reason: None,
            last_handoff_reason: None,
        }
    }

    fn snapshot(&self) -> BackendSnapshot {
        BackendSnapshot {
            backend: "memory".to_string(),
            status: self.diag.status.clone(),
            stop_reason: self.diag.stop_reason.clone(),
            first_instruction_addr: self.diag.first_instruction_addr,
            first_instruction: self.diag.first_instruction,
            first_instruction_text: self.diag.first_instruction.map(format_arm_word),
            entry_bytes_present: self.diag.entry_bytes_present,
            executed_instructions: self.diag.executed_instructions,
            final_pc: Some(self.cpu.regs[15]),
            final_sp: Some(self.cpu.regs[13]),
            final_lr: Some(self.cpu.regs[14]),
            trace: self.diag.trace.clone(),
            runtime_state: Some(self.runtime_state_snapshot()),
            backend_execution: Some(self.execution_summary()),
        }
    }
}
