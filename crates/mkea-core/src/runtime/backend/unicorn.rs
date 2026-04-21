use std::collections::{BTreeSet, HashMap, VecDeque};

use mkea_loader::SectionInfo;

use crate::{
    config::{CoreConfig, RuntimeMode},
    error::{CoreError, CoreResult},
    runtime::{GUEST_PROT_EXEC, GUEST_PROT_READ, GUEST_PROT_WRITE, is_stop_requested},
    types::InitialRegisters,
};

use super::{BackendPolicy, BackendSnapshot, CpuBackend, MemoryArm32Backend};
use crate::runtime::diagnostics::{
    RuntimeBackendExecutionSummary, RuntimeBackendSemanticsCandidate, RuntimeBackendStopSite, RuntimeCountEntry,
};

use crate::runtime::engine::{format_arm_word, format_thumb_halfword, StepControl};

const UNICORN_HYBRID_BOOTSTRAP_SILENT_STEPS: u64 = 4_096;
const UNICORN_HYBRID_SPIN_SILENT_STEPS: u64 = 2_048;
const UNICORN_HYBRID_HARD_SILENT_STEPS: u64 = 32_768;
const UNICORN_HYBRID_SPIN_WINDOW: usize = 512;
const UNICORN_HYBRID_SPIN_UNIQUE_PCS: usize = 8;
const TRAP_CLASS_UNSUPPORTED_INSTRUCTION: &str = "unsupported-instruction";
const TRAP_CLASS_UNSUPPORTED_SYSCALL_STUB: &str = "unsupported-syscall-stub";
const TRAP_CLASS_MEMORY_FAULT: &str = "memory-fault";
const TRAP_CLASS_OBJC_UNRESOLVED_DISPATCH: &str = "objc-unresolved-dispatch";
const TRAP_CLASS_GRAPHICS_FALLBACK: &str = "graphics-fallback";
const TRAP_CLASS_UNKNOWN: &str = "unknown";

#[cfg(feature = "unicorn")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StopSiteKey {
    trap_class: String,
    pc: u32,
    thumb: bool,
    symbol: Option<String>,
    reason: Option<String>,
}

#[cfg(feature = "unicorn")]
use unicorn_engine::{RegisterARM, UcHookId, Unicorn};
#[cfg(feature = "unicorn")]
use unicorn_engine::unicorn_const::{uc_error, Arch, HookType, MemType, Mode, Prot};

#[cfg(feature = "unicorn")]
#[derive(Debug, Default, Clone)]
struct UnicornHookState {
    trap_points: HashMap<u32, String>,
    trap_hit: Option<(u32, String)>,
    write_events: Vec<(u32, usize)>,
    last_pc: Option<u32>,
    last_size: Option<u32>,
}

#[cfg(feature = "unicorn")]
impl UnicornHookState {
    fn take_trap(&mut self) -> Option<(u32, String)> {
        self.trap_hit.take()
    }

    fn take_writes(&mut self) -> Vec<(u32, usize)> {
        std::mem::take(&mut self.write_events)
    }
}

#[cfg(feature = "unicorn")]
fn guest_prot_to_unicorn(prot: u32) -> Prot {
    let mut out = Prot::NONE;
    if (prot & GUEST_PROT_READ) != 0 {
        out |= Prot::READ;
    }
    if (prot & GUEST_PROT_WRITE) != 0 {
        out |= Prot::WRITE;
    }
    if (prot & GUEST_PROT_EXEC) != 0 {
        out |= Prot::EXEC;
    }
    if out == Prot::NONE {
        Prot::READ
    } else {
        out
    }
}

#[cfg(feature = "unicorn")]
fn map_unicorn_error(ctx: &str, err: uc_error) -> CoreError {
    CoreError::Backend(format!("{ctx}: {err}"))
}

#[cfg(feature = "unicorn")]
#[derive(Debug)]
pub struct UnicornArm32Backend {
    emu: Unicorn<'static, UnicornHookState>,
    shadow: MemoryArm32Backend,
    hooks: Vec<UcHookId>,
    status: String,
    stop_reason: String,
    trace: Vec<String>,
    entry_bytes_present: bool,
    first_instruction_addr: Option<u32>,
    first_instruction: Option<u32>,
    first_instruction_thumb: bool,
    executed_instructions: u64,
    shadow_write_cursor: usize,
    shadow_trace_cursor: usize,
    hybrid_silent_steps: u64,
    hybrid_recent_pcs: VecDeque<u32>,
    hybrid_handoff_reason: Option<String>,
    hybrid_handoff_count: u32,
    native_steps: u64,
    shadow_steps: u64,
    shadow_trap_steps: u64,
    shadow_fallback_steps: u64,
    shadow_handoff_steps: u64,
    trap_dispatches: u32,
    fallback_dispatches: u32,
    trap_class_counts: HashMap<String, u32>,
    trap_stop_sites: HashMap<StopSiteKey, u32>,
    last_trap_class: Option<String>,
    last_trap_reason: Option<String>,
}

#[cfg(feature = "unicorn")]
impl UnicornArm32Backend {
    pub fn new(cfg: &CoreConfig) -> CoreResult<Self> {
        let mut emu = Unicorn::new_with_data(Arch::ARM, Mode::LITTLE_ENDIAN, UnicornHookState::default())
            .map_err(|err| map_unicorn_error("failed to create Unicorn ARM backend", err))?;
        let mut hooks = Vec::new();
        let code_hook = emu
            .add_code_hook(1, 0, |uc, addr, size| {
                let state = uc.get_data_mut();
                state.last_pc = Some((addr as u32) & !1);
                state.last_size = Some(size);
                if let Some(label) = state.trap_points.get(&((addr as u32) & !1)).cloned() {
                    state.trap_hit = Some(((addr as u32) & !1, label));
                    let _ = uc.emu_stop();
                }
            })
            .map_err(|err| map_unicorn_error("failed to install Unicorn code hook", err))?;
        hooks.push(code_hook);
        let write_hook = emu
            .add_mem_hook(HookType::MEM_WRITE, 1, 0, |uc, _ty: MemType, addr, size, _value| {
                let state = uc.get_data_mut();
                state.write_events.push(((addr as u32), size));
                true
            })
            .map_err(|err| map_unicorn_error("failed to install Unicorn write hook", err))?;
        hooks.push(write_hook);
        Ok(Self {
            emu,
            shadow: MemoryArm32Backend::with_config(cfg),
            hooks,
            status: "not-started".to_string(),
            stop_reason: "not-started".to_string(),
            trace: Vec::new(),
            entry_bytes_present: false,
            first_instruction_addr: None,
            first_instruction: None,
            first_instruction_thumb: false,
            executed_instructions: 0,
            shadow_write_cursor: 0,
            shadow_trace_cursor: 0,
            hybrid_silent_steps: 0,
            hybrid_recent_pcs: VecDeque::with_capacity(UNICORN_HYBRID_SPIN_WINDOW),
            hybrid_handoff_reason: None,
            hybrid_handoff_count: 0,
            native_steps: 0,
            shadow_steps: 0,
            shadow_trap_steps: 0,
            shadow_fallback_steps: 0,
            shadow_handoff_steps: 0,
            trap_dispatches: 0,
            fallback_dispatches: 0,
            trap_class_counts: HashMap::new(),
            trap_stop_sites: HashMap::new(),
            last_trap_class: None,
            last_trap_reason: None,
        })
    }

    fn read_unicorn_reg(&self, reg: RegisterARM) -> CoreResult<u32> {
        self.emu
            .reg_read(reg)
            .map(|value| value as u32)
            .map_err(|err| map_unicorn_error("failed to read Unicorn register", err))
    }

    fn write_unicorn_reg(&mut self, reg: RegisterARM, value: u32) -> CoreResult<()> {
        self.emu
            .reg_write(reg, value as u64)
            .map_err(|err| map_unicorn_error("failed to write Unicorn register", err))
    }

    fn read_unicorn_pc(&self) -> CoreResult<u32> {
        self.read_unicorn_reg(RegisterARM::PC).map(|value| value & !1)
    }

    fn read_unicorn_thumb(&self) -> CoreResult<bool> {
        self.read_unicorn_reg(RegisterARM::CPSR)
            .map(|cpsr| (cpsr & 0x20) != 0)
    }

    fn set_unicorn_thumb(&mut self, thumb: bool) -> CoreResult<()> {
        let mut cpsr = self.read_unicorn_reg(RegisterARM::CPSR).unwrap_or(0);
        if thumb {
            cpsr |= 0x20;
        } else {
            cpsr &= !0x20;
        }
        self.write_unicorn_reg(RegisterARM::CPSR, cpsr)
    }

    fn sync_regs_from_unicorn_to_shadow(&mut self) -> CoreResult<()> {
        self.shadow.cpu.regs[0] = self.read_unicorn_reg(RegisterARM::R0)?;
        self.shadow.cpu.regs[1] = self.read_unicorn_reg(RegisterARM::R1)?;
        self.shadow.cpu.regs[2] = self.read_unicorn_reg(RegisterARM::R2)?;
        self.shadow.cpu.regs[3] = self.read_unicorn_reg(RegisterARM::R3)?;
        self.shadow.cpu.regs[4] = self.read_unicorn_reg(RegisterARM::R4)?;
        self.shadow.cpu.regs[5] = self.read_unicorn_reg(RegisterARM::R5)?;
        self.shadow.cpu.regs[6] = self.read_unicorn_reg(RegisterARM::R6)?;
        self.shadow.cpu.regs[7] = self.read_unicorn_reg(RegisterARM::R7)?;
        self.shadow.cpu.regs[8] = self.read_unicorn_reg(RegisterARM::R8)?;
        self.shadow.cpu.regs[9] = self.read_unicorn_reg(RegisterARM::R9)?;
        self.shadow.cpu.regs[10] = self.read_unicorn_reg(RegisterARM::R10)?;
        self.shadow.cpu.regs[11] = self.read_unicorn_reg(RegisterARM::R11)?;
        self.shadow.cpu.regs[12] = self.read_unicorn_reg(RegisterARM::R12)?;
        self.shadow.cpu.regs[13] = self.read_unicorn_reg(RegisterARM::SP)?;
        self.shadow.cpu.regs[14] = self.read_unicorn_reg(RegisterARM::LR)?;
        self.shadow.cpu.regs[15] = self.read_unicorn_pc()?;
        let cpsr = self.read_unicorn_reg(RegisterARM::CPSR)?;
        self.shadow.cpu.thumb = (cpsr & 0x20) != 0;
        self.shadow.cpu.flags.n = (cpsr & (1 << 31)) != 0;
        self.shadow.cpu.flags.z = (cpsr & (1 << 30)) != 0;
        self.shadow.cpu.flags.c = (cpsr & (1 << 29)) != 0;
        self.shadow.cpu.flags.v = (cpsr & (1 << 28)) != 0;
        Ok(())
    }

    fn sync_regs_from_shadow_to_unicorn(&mut self) -> CoreResult<()> {
        self.write_unicorn_reg(RegisterARM::R0, self.shadow.cpu.regs[0])?;
        self.write_unicorn_reg(RegisterARM::R1, self.shadow.cpu.regs[1])?;
        self.write_unicorn_reg(RegisterARM::R2, self.shadow.cpu.regs[2])?;
        self.write_unicorn_reg(RegisterARM::R3, self.shadow.cpu.regs[3])?;
        self.write_unicorn_reg(RegisterARM::R4, self.shadow.cpu.regs[4])?;
        self.write_unicorn_reg(RegisterARM::R5, self.shadow.cpu.regs[5])?;
        self.write_unicorn_reg(RegisterARM::R6, self.shadow.cpu.regs[6])?;
        self.write_unicorn_reg(RegisterARM::R7, self.shadow.cpu.regs[7])?;
        self.write_unicorn_reg(RegisterARM::R8, self.shadow.cpu.regs[8])?;
        self.write_unicorn_reg(RegisterARM::R9, self.shadow.cpu.regs[9])?;
        self.write_unicorn_reg(RegisterARM::R10, self.shadow.cpu.regs[10])?;
        self.write_unicorn_reg(RegisterARM::R11, self.shadow.cpu.regs[11])?;
        self.write_unicorn_reg(RegisterARM::R12, self.shadow.cpu.regs[12])?;
        self.write_unicorn_reg(RegisterARM::SP, self.shadow.cpu.regs[13])?;
        self.write_unicorn_reg(RegisterARM::LR, self.shadow.cpu.regs[14])?;
        self.write_unicorn_reg(RegisterARM::PC, self.shadow.cpu.regs[15])?;
        let mut cpsr = 0u32;
        if self.shadow.cpu.flags.n { cpsr |= 1 << 31; }
        if self.shadow.cpu.flags.z { cpsr |= 1 << 30; }
        if self.shadow.cpu.flags.c { cpsr |= 1 << 29; }
        if self.shadow.cpu.flags.v { cpsr |= 1 << 28; }
        if self.shadow.cpu.thumb { cpsr |= 0x20; }
        self.write_unicorn_reg(RegisterARM::CPSR, cpsr)
    }

    fn append_shadow_trace_delta(&mut self) {
        if self.shadow_trace_cursor >= self.shadow.diag.trace.len() {
            return;
        }
        self.trace.extend(self.shadow.diag.trace[self.shadow_trace_cursor..].iter().cloned());
        self.shadow_trace_cursor = self.shadow.diag.trace.len();
    }

    fn mirror_shadow_writes_to_unicorn(&mut self) -> CoreResult<()> {
        while self.shadow_write_cursor < self.shadow.diag.writes.len() {
            let (addr, size) = self.shadow.diag.writes[self.shadow_write_cursor];
            self.shadow_write_cursor += 1;
            if size == 0 {
                continue;
            }
            let Some(region) = self.shadow.find_region(addr, size as u32) else {
                continue;
            };
            let offset = (addr - region.addr) as usize;
            let bytes = &region.data[offset..offset + size];
            self.emu
                .mem_write(addr as u64, bytes)
                .map_err(|err| map_unicorn_error(&format!("failed to mirror shadow write at 0x{addr:08x}"), err))?;
        }
        Ok(())
    }

    fn mirror_unicorn_writes_to_shadow(&mut self) -> CoreResult<()> {
        let writes = self.emu.get_data_mut().take_writes();
        for (addr, size) in writes {
            if size == 0 {
                continue;
            }
            let bytes = self
                .emu
                .mem_read_as_vec(addr as u64, size)
                .map_err(|err| map_unicorn_error(&format!("failed to read Unicorn write at 0x{addr:08x}"), err))?;
            self.shadow.write_bytes(addr, &bytes)?;
        }
        self.shadow_write_cursor = self.shadow.diag.writes.len();
        Ok(())
    }

    fn refresh_first_instruction(&mut self) -> CoreResult<()> {
        let pc = self.read_unicorn_pc()?;
        let thumb = self.read_unicorn_thumb()?;
        self.first_instruction_addr = Some(pc);
        self.first_instruction_thumb = thumb;
        let bytes = if thumb {
            self.emu.mem_read_as_vec(pc as u64, 2)
                .map_err(|err| map_unicorn_error("failed to fetch Thumb entry bytes", err))?
        } else {
            self.emu.mem_read_as_vec(pc as u64, 4)
                .map_err(|err| map_unicorn_error("failed to fetch ARM entry bytes", err))?
        };
        self.entry_bytes_present = !bytes.is_empty();
        self.first_instruction = if thumb && bytes.len() >= 2 {
            Some(u16::from_le_bytes([bytes[0], bytes[1]]) as u32)
        } else if !thumb && bytes.len() >= 4 {
            Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        } else {
            None
        };
        Ok(())
    }

    fn trace_current_step(&mut self, idx: u64, pc: u32, thumb: bool) -> CoreResult<()> {
        if thumb {
            let bytes = self
                .emu
                .mem_read_as_vec(pc as u64, 2)
                .map_err(|err| map_unicorn_error("failed to fetch Thumb trace bytes", err))?;
            if bytes.len() >= 2 {
                let hw = u16::from_le_bytes([bytes[0], bytes[1]]);
                self.trace.push(format!("uc[{idx:06}] thumb pc=0x{pc:08x} hw=0x{hw:04x} {}", format_thumb_halfword(hw)));
            }
        } else {
            let bytes = self
                .emu
                .mem_read_as_vec(pc as u64, 4)
                .map_err(|err| map_unicorn_error("failed to fetch ARM trace bytes", err))?;
            if bytes.len() >= 4 {
                let word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                self.trace.push(format!("uc[{idx:06}] arm   pc=0x{pc:08x} word=0x{word:08x} {}", format_arm_word(word)));
                if (0x0001_23dc..=0x0001_23ec).contains(&pc) {
                    let r5 = self.read_unicorn_reg(RegisterARM::R5).unwrap_or(0);
                    let r6 = self.read_unicorn_reg(RegisterARM::R6).unwrap_or(0);
                    let sp = self.read_unicorn_reg(RegisterARM::SP).unwrap_or(0);
                    let lr = self.read_unicorn_reg(RegisterARM::LR).unwrap_or(0);
                    let slot = if r6 != 0 {
                        self.emu
                            .mem_read_as_vec(r6 as u64, 4)
                            .ok()
                            .filter(|bytes| bytes.len() >= 4)
                            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                    } else {
                        None
                    };
                    let sp_word = if sp != 0 {
                        self.emu
                            .mem_read_as_vec(sp as u64, 4)
                            .ok()
                            .filter(|bytes| bytes.len() >= 4)
                            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                    } else {
                        None
                    };
                    self.trace.push(format!(
                        "     ↳ uc-audiofile-probe pc=0x{pc:08x} word=0x{word:08x} r5(out)=0x{r5:08x} r6(slot)=0x{r6:08x} *(r6)={} sp=0x{sp:08x} *(sp)={} lr=0x{lr:08x}",
                        slot
                            .map(|value| format!("0x{value:08x}"))
                            .unwrap_or_else(|| "<unreadable>".to_string()),
                        sp_word
                            .map(|value| format!("0x{value:08x}"))
                            .unwrap_or_else(|| "<unreadable>".to_string()),
                    ));
                }
            }
        }
        Ok(())
    }

    fn note_trap_class(&mut self, class: &str, reason: impl Into<String>) {
        *self.trap_class_counts.entry(class.to_string()).or_insert(0) += 1;
        self.last_trap_class = Some(class.to_string());
        self.last_trap_reason = Some(reason.into());
    }

    fn note_stop_site(
        &mut self,
        class: &str,
        pc: u32,
        thumb: bool,
        symbol: Option<&str>,
        reason: Option<String>,
    ) {
        let key = StopSiteKey {
            trap_class: class.to_string(),
            pc: pc & !1,
            thumb,
            symbol: symbol.map(str::to_string),
            reason,
        };
        *self.trap_stop_sites.entry(key).or_insert(0) += 1;
    }

    fn classify_semantics_area(site: &RuntimeBackendStopSite) -> &'static str {
        let symbol = site.symbol.as_deref().unwrap_or_default().to_ascii_lowercase();
        let reason = site.reason.as_deref().unwrap_or_default().to_ascii_lowercase();
        if site.trap_class == TRAP_CLASS_OBJC_UNRESOLVED_DISPATCH
            || symbol.contains("objc")
            || symbol.contains("nsclassfromstring")
            || symbol.contains("nsselectorfromstring")
            || reason.contains("objc")
        {
            "objc-runtime-dispatch"
        } else if site.trap_class == TRAP_CLASS_GRAPHICS_FALLBACK
            || symbol.starts_with("gl")
            || symbol.starts_with("cg")
            || symbol.contains("uigraphics")
            || symbol.contains("uiimage")
            || symbol.contains("eagl")
            || reason.contains("present")
            || reason.contains("graphics")
        {
            "graphics-present-path"
        } else if symbol.starts_with("cfreadstream")
            || symbol.starts_with("cfwritestream")
            || symbol.starts_with("cfsocket")
            || symbol.starts_with("cfstream")
            || symbol.starts_with("scnetwork")
            || symbol.starts_with("secitem")
            || symbol.contains("nsurl")
            || symbol.contains("nsconnection")
            || reason.contains("runloop")
            || reason.contains("stream")
            || reason.contains("socket")
            || reason.contains("network")
        {
            "cfnetwork-runloop-bridge"
        } else if site.trap_class == TRAP_CLASS_MEMORY_FAULT {
            "memory-mapping-bootstrap"
        } else if site.trap_class == TRAP_CLASS_UNSUPPORTED_INSTRUCTION
            || reason.contains("thumb")
            || reason.contains("arm")
            || reason.contains("vfp")
        {
            "arm-vfp-runtime-semantics"
        } else {
            "foundation-runtime-semantics"
        }
    }

    fn semantics_weight_for_site(site: &RuntimeBackendStopSite) -> u32 {
        let base: u32 = match site.trap_class.as_str() {
            TRAP_CLASS_MEMORY_FAULT => 7,
            TRAP_CLASS_OBJC_UNRESOLVED_DISPATCH => 6,
            TRAP_CLASS_GRAPHICS_FALLBACK => 5,
            TRAP_CLASS_UNSUPPORTED_INSTRUCTION => 4,
            TRAP_CLASS_UNSUPPORTED_SYSCALL_STUB => 3,
            _ => 2,
        };
        base.saturating_mul(site.count.max(1))
    }

    fn build_semantics_candidates(stop_sites: &[RuntimeBackendStopSite]) -> Vec<RuntimeBackendSemanticsCandidate> {
        let mut ranked: HashMap<String, (u32, u32, Vec<String>)> = HashMap::new();
        for site in stop_sites {
            let area = Self::classify_semantics_area(site).to_string();
            let entry = ranked.entry(area).or_insert_with(|| (0, 0, Vec::new()));
            entry.0 = entry.0.saturating_add(site.count);
            entry.1 = entry.1.saturating_add(Self::semantics_weight_for_site(site));
            if entry.2.len() < 4 {
                let mut snippet = format!("0x{:08x}", site.pc);
                if let Some(symbol) = site.symbol.as_deref() {
                    if !symbol.is_empty() {
                        snippet.push_str(": ");
                        snippet.push_str(symbol);
                    }
                } else if let Some(reason) = site.reason.as_deref() {
                    if !reason.is_empty() {
                        snippet.push_str(": ");
                        snippet.push_str(reason);
                    }
                }
                if !entry.2.iter().any(|existing| existing == &snippet) {
                    entry.2.push(snippet);
                }
            }
        }
        let mut out: Vec<_> = ranked
            .into_iter()
            .map(|(area, (total_hits, weighted_score, evidence))| RuntimeBackendSemanticsCandidate {
                area,
                total_hits,
                weighted_score,
                evidence,
            })
            .collect();
        out.sort_by(|a, b| {
            b.weighted_score
                .cmp(&a.weighted_score)
                .then_with(|| b.total_hits.cmp(&a.total_hits))
                .then_with(|| a.area.cmp(&b.area))
        });
        out.truncate(6);
        out
    }

    fn classify_stub_trap(label: &str) -> &'static str {
        let label = label.to_ascii_lowercase();
        if label.contains("objc_msgsend") {
            TRAP_CLASS_OBJC_UNRESOLVED_DISPATCH
        } else if label.starts_with("gl")
            || label.contains("eagl")
            || label.contains("uigraphics")
            || label.starts_with("cg")
            || label.starts_with("uiimage")
        {
            TRAP_CLASS_GRAPHICS_FALLBACK
        } else {
            TRAP_CLASS_UNSUPPORTED_SYSCALL_STUB
        }
    }

    fn classify_unicorn_error(err: &uc_error) -> &'static str {
        let lower = format!("{err:?} {err}").to_ascii_lowercase();
        if lower.contains("read_unmapped")
            || lower.contains("write_unmapped")
            || lower.contains("fetch_unmapped")
            || lower.contains("read_prot")
            || lower.contains("write_prot")
            || lower.contains("fetch_prot")
            || lower.contains("read_unaligned")
            || lower.contains("write_unaligned")
            || lower.contains("fetch_unaligned")
        {
            TRAP_CLASS_MEMORY_FAULT
        } else if lower.contains("insn_invalid") || lower.contains("exception") {
            TRAP_CLASS_UNSUPPORTED_INSTRUCTION
        } else {
            TRAP_CLASS_UNKNOWN
        }
    }

    fn classify_handoff_reason(reason: &str) -> &'static str {
        let lower = reason.to_ascii_lowercase();
        if lower.contains("present") || lower.contains("gl_") || lower.contains("graphics") {
            TRAP_CLASS_GRAPHICS_FALLBACK
        } else {
            TRAP_CLASS_UNKNOWN
        }
    }

    fn build_execution_summary(&self) -> RuntimeBackendExecutionSummary {
        let total_steps = self.native_steps.saturating_add(self.shadow_steps);
        let native_share_milli = if total_steps == 0 {
            0
        } else {
            ((self.native_steps.saturating_mul(1000)) / total_steps) as u32
        };
        let shadow_share_milli = if total_steps == 0 {
            0
        } else {
            ((self.shadow_steps.saturating_mul(1000)) / total_steps) as u32
        };
        let mut trap_classes: Vec<RuntimeCountEntry> = self
            .trap_class_counts
            .iter()
            .map(|(name, count)| RuntimeCountEntry {
                name: name.clone(),
                count: *count,
            })
            .collect();
        trap_classes.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        let mut top_stop_sites: Vec<RuntimeBackendStopSite> = self
            .trap_stop_sites
            .iter()
            .map(|(key, count)| RuntimeBackendStopSite {
                trap_class: key.trap_class.clone(),
                pc: key.pc,
                thumb: key.thumb,
                count: *count,
                symbol: key.symbol.clone(),
                reason: key.reason.clone(),
            })
            .collect();
        top_stop_sites.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.pc.cmp(&b.pc))
                .then_with(|| a.trap_class.cmp(&b.trap_class))
        });
        top_stop_sites.truncate(12);
        let semantics_candidates = Self::build_semantics_candidates(&top_stop_sites);
        RuntimeBackendExecutionSummary {
            backend_policy: "hybrid-execution".to_string(),
            total_steps,
            native_steps: self.native_steps,
            shadow_steps: self.shadow_steps,
            shadow_trap_steps: self.shadow_trap_steps,
            shadow_fallback_steps: self.shadow_fallback_steps,
            shadow_handoff_steps: self.shadow_handoff_steps,
            trap_dispatches: self.trap_dispatches,
            fallback_dispatches: self.fallback_dispatches,
            handoff_count: self.hybrid_handoff_count,
            native_share_milli,
            shadow_share_milli,
            trap_classes,
            top_stop_sites,
            semantics_candidates,
            last_trap_class: self.last_trap_class.clone(),
            last_trap_reason: self.last_trap_reason.clone(),
            last_handoff_reason: self.hybrid_handoff_reason.clone(),
        }
    }

    fn handle_trap_via_shadow(&mut self, idx: u64, pc: u32, label: &str) -> CoreResult<StepControl> {
        self.mirror_unicorn_writes_to_shadow()?;
        self.sync_regs_from_unicorn_to_shadow()?;
        self.trace.push(format!("uc[{idx:06}] trap  pc=0x{pc:08x} label={label}"));
        self.trap_dispatches = self.trap_dispatches.saturating_add(1);
        self.shadow_steps = self.shadow_steps.saturating_add(1);
        self.shadow_trap_steps = self.shadow_trap_steps.saturating_add(1);
        let class = Self::classify_stub_trap(label);
        self.note_trap_class(class, format!("stub:{label}"));
        self.note_stop_site(class, pc, self.shadow.cpu.thumb, Some(label), Some(format!("stub:{label}")));
        let control = self.shadow.handle_hle_stub(idx, pc)?.unwrap_or(StepControl::Continue);
        if matches!(control, StepControl::Continue) {
            self.shadow.process_runtime_post_step_hooks("unicorn:trap-shadow");
        }
        self.append_shadow_trace_delta();
        self.mirror_shadow_writes_to_unicorn()?;
        self.sync_regs_from_shadow_to_unicorn()?;
        Ok(control)
    }

    fn fallback_step_via_shadow(&mut self, idx: u64, pc: u32, thumb: bool, err: uc_error) -> CoreResult<StepControl> {
        self.mirror_unicorn_writes_to_shadow()?;
        self.sync_regs_from_unicorn_to_shadow()?;
        self.trace.push(format!(
            "uc[{idx:06}] fallback pc=0x{pc:08x} thumb={} reason={err}",
            if thumb { "yes" } else { "no" }
        ));
        self.fallback_dispatches = self.fallback_dispatches.saturating_add(1);
        self.shadow_steps = self.shadow_steps.saturating_add(1);
        self.shadow_fallback_steps = self.shadow_fallback_steps.saturating_add(1);
        let trap_class = Self::classify_unicorn_error(&err);
        self.note_trap_class(trap_class, format!("fallback:{err}"));
        self.note_stop_site(trap_class, pc, thumb, None, Some(err.to_string()));
        let control = if thumb {
            let halfword = self.shadow.read_u16_le(pc)?;
            self.shadow.exec.current_exec_pc = pc;
            self.shadow.exec.current_exec_word = halfword as u32;
            self.shadow.exec.current_exec_thumb = true;
            self.shadow.step_thumb(halfword, pc)?
        } else {
            let word = self.shadow.read_u32_le(pc)?;
            self.shadow.exec.current_exec_pc = pc;
            self.shadow.exec.current_exec_word = word;
            self.shadow.exec.current_exec_thumb = false;
            self.shadow.record_exact_epilogue_trace(pc, word);
            self.shadow.record_audiofile_probe_trace(pc, word);
            self.shadow.step_arm(word, pc)?
        };
        if matches!(control, StepControl::Continue) {
            self.shadow.process_runtime_post_step_hooks("unicorn:fallback-shadow");
        }
        self.append_shadow_trace_delta();
        self.mirror_shadow_writes_to_unicorn()?;
        self.sync_regs_from_shadow_to_unicorn()?;
        self.hybrid_reset_silent_window();
        Ok(control)
    }

    fn hybrid_reset_silent_window(&mut self) {
        self.hybrid_silent_steps = 0;
        self.hybrid_recent_pcs.clear();
    }

    fn hybrid_note_native_step(&mut self, pc: u32) {
        self.hybrid_silent_steps = self.hybrid_silent_steps.saturating_add(1);
        if self.hybrid_recent_pcs.len() >= UNICORN_HYBRID_SPIN_WINDOW {
            self.hybrid_recent_pcs.pop_front();
        }
        self.hybrid_recent_pcs.push_back(pc & !1);
    }

    fn hybrid_recent_pc_uniques(&self) -> usize {
        let mut unique = BTreeSet::new();
        for pc in self.hybrid_recent_pcs.iter().copied() {
            unique.insert(pc);
            if unique.len() > UNICORN_HYBRID_SPIN_UNIQUE_PCS {
                break;
            }
        }
        unique.len()
    }

    fn hybrid_bootstrap_reached(&self) -> bool {
        self.shadow.runtime.ui_runtime.window_visible
            || self.shadow.runtime.ui_graphics.graphics_layer_attached
            || self.shadow.runtime.ui_graphics.graphics_context_current
            || self.shadow.runtime.ui_cocos.opengl_view != 0
            || self.shadow.objc_real_msgsend_dispatches() > 0
    }

    fn hybrid_handoff_candidate_reason(&self) -> Option<String> {
        if matches!(self.shadow.tuning.runtime_mode, RuntimeMode::Strict) {
            return None;
        }
        if self.shadow.runtime.ui_graphics.graphics_presented {
            return None;
        }
        let silent = self.hybrid_silent_steps;
        let unique_pcs = self.hybrid_recent_pc_uniques();
        let bootstrapped = self.hybrid_bootstrap_reached();

        if bootstrapped && silent >= UNICORN_HYBRID_BOOTSTRAP_SILENT_STEPS {
            return Some(format!(
                "bootstrap stalled before first present (silent_steps={}, unique_pcs={}, objc_real={}, window_visible={}, gl_attached={}, gl_current={})",
                silent,
                unique_pcs,
                self.shadow.objc_real_msgsend_dispatches(),
                self.shadow.runtime.ui_runtime.window_visible,
                self.shadow.runtime.ui_graphics.graphics_layer_attached,
                self.shadow.runtime.ui_graphics.graphics_context_current,
            ));
        }

        if silent >= UNICORN_HYBRID_SPIN_SILENT_STEPS
            && self.hybrid_recent_pcs.len() >= UNICORN_HYBRID_SPIN_WINDOW / 2
            && unique_pcs > 0
            && unique_pcs <= UNICORN_HYBRID_SPIN_UNIQUE_PCS
        {
            return Some(format!(
                "probable guest spin-loop before first present (silent_steps={}, unique_pcs={})",
                silent, unique_pcs
            ));
        }

        if silent >= UNICORN_HYBRID_HARD_SILENT_STEPS {
            return Some(format!(
                "native execution made no HLE-visible progress for {} step(s) before first present",
                silent
            ));
        }

        None
    }

    fn handoff_remaining_via_shadow(&mut self, idx: u64, reason: &str, remaining_steps: u64) -> CoreResult<()> {
        self.sync_regs_from_unicorn_to_shadow()?;
        self.mirror_unicorn_writes_to_shadow()?;
        let shadow_before = self.shadow.diag.executed_instructions;
        self.trace.push(format!(
            "uc[{idx:06}] handoff reason={} remaining_steps={}",
            reason,
            remaining_steps
        ));
        self.hybrid_handoff_count = self.hybrid_handoff_count.saturating_add(1);
        self.hybrid_handoff_reason = Some(reason.to_string());
        let trap_class = Self::classify_handoff_reason(reason);
        self.note_trap_class(trap_class, format!("handoff:{reason}"));
        self.note_stop_site(trap_class, self.shadow.cpu.regs[15], self.shadow.cpu.thumb, None, Some(reason.to_string()));
        self.shadow.run(remaining_steps.max(1))?;
        let shadow_delta = self.shadow.diag.executed_instructions.saturating_sub(shadow_before);
        self.shadow_steps = self.shadow_steps.saturating_add(shadow_delta);
        self.shadow_handoff_steps = self.shadow_handoff_steps.saturating_add(shadow_delta);
        self.executed_instructions = self.executed_instructions.saturating_add(shadow_delta);
        self.append_shadow_trace_delta();
        self.stop_reason = format!("hybrid handoff -> {}", self.shadow.diag.stop_reason.clone());
        Ok(())
    }
}

#[cfg(feature = "unicorn")]
impl CpuBackend for UnicornArm32Backend {
    fn policy(&self) -> BackendPolicy {
        BackendPolicy::HybridExecution
    }

    fn map(&mut self, addr: u32, size: u32, prot: u32) -> CoreResult<()> {
        self.shadow.map(addr, size, prot)?;
        self.emu
            .mem_map(addr as u64, size as u64, guest_prot_to_unicorn(prot))
            .map_err(|err| map_unicorn_error(&format!("failed to map Unicorn memory 0x{addr:08x}..0x{:08x}", addr.saturating_add(size)), err))?;
        Ok(())
    }

    fn write_mem(&mut self, addr: u32, data: &[u8]) -> CoreResult<()> {
        self.shadow.write_mem(addr, data)?;
        self.emu
            .mem_write(addr as u64, data)
            .map_err(|err| map_unicorn_error(&format!("failed to write Unicorn memory at 0x{addr:08x}"), err))?;
        self.shadow_write_cursor = self.shadow.diag.writes.len();
        Ok(())
    }

    fn set_pc(&mut self, pc: u32, thumb: bool) -> CoreResult<()> {
        self.shadow.set_pc(pc, thumb)?;
        self.write_unicorn_reg(RegisterARM::PC, pc & !1)?;
        self.set_unicorn_thumb(thumb)
    }

    fn set_sp(&mut self, sp: u32) -> CoreResult<()> {
        self.shadow.set_sp(sp)?;
        self.write_unicorn_reg(RegisterARM::SP, sp)
    }

    fn set_initial_registers(&mut self, regs: &InitialRegisters) -> CoreResult<()> {
        self.shadow.set_initial_registers(regs)?;
        self.write_unicorn_reg(RegisterARM::R0, regs.r0)?;
        self.write_unicorn_reg(RegisterARM::R1, regs.r1)?;
        self.write_unicorn_reg(RegisterARM::R2, regs.r2)?;
        self.write_unicorn_reg(RegisterARM::R3, regs.r3)?;
        self.write_unicorn_reg(RegisterARM::LR, regs.lr)?;
        self.write_unicorn_reg(RegisterARM::SP, regs.sp)?;
        self.write_unicorn_reg(RegisterARM::PC, regs.pc & !1)?;
        self.set_unicorn_thumb(regs.thumb)?;
        self.shadow_write_cursor = self.shadow.diag.writes.len();
        self.shadow_trace_cursor = self.shadow.diag.trace.len();
        Ok(())
    }

    fn seed_objc_metadata_sections(&mut self, sections: &[SectionInfo]) {
        self.shadow.seed_objc_metadata_sections(sections);
    }

    fn run(&mut self, max_instructions: u64) -> CoreResult<()> {
        let requested_steps = max_instructions.max(1);
        self.refresh_first_instruction()?;
        let entry_pc = self.read_unicorn_pc()?;
        let entry_sp = self.read_unicorn_reg(RegisterARM::SP)?;
        if entry_pc == 0 {
            return Err(CoreError::Backend("unicorn backend run requested before PC was installed".into()));
        }
        if entry_sp == 0 {
            return Err(CoreError::Backend("unicorn backend run requested before SP was installed".into()));
        }
        self.stop_reason = "not-started".to_string();
        self.status = "running".to_string();
        self.trace.push(format!(
            "unicorn backend start pc=0x{entry_pc:08x} sp=0x{entry_sp:08x} requested_steps={requested_steps}"
        ));
        for idx in 0..requested_steps {
            if is_stop_requested() {
                self.stop_reason = "host shutdown requested".to_string();
                break;
            }
            let pc = self.read_unicorn_pc()?;
            let thumb = self.read_unicorn_thumb()?;
            self.trace_current_step(idx, pc, thumb)?;
            let result = self.emu.emu_start(pc as u64, u64::MAX, 0, 1);
            let trap = self.emu.get_data_mut().take_trap();
            if let Some((trap_pc, label)) = trap {
                self.executed_instructions = self.executed_instructions.saturating_add(1);
                match self.handle_trap_via_shadow(idx, trap_pc, &label)? {
                    StepControl::Continue => {
                        self.hybrid_reset_silent_window();
                        continue;
                    }
                    StepControl::Stop(reason) => {
                        self.stop_reason = reason;
                        break;
                    }
                }
            }
            match result {
                Ok(()) => {
                    self.executed_instructions = self.executed_instructions.saturating_add(1);
                    self.native_steps = self.native_steps.saturating_add(1);
                    self.mirror_unicorn_writes_to_shadow()?;
                    self.sync_regs_from_unicorn_to_shadow()?;
                    let fired_hooks = self.shadow.process_runtime_post_step_hooks("unicorn:native-step");
                    if fired_hooks != 0 {
                        self.append_shadow_trace_delta();
                        self.mirror_shadow_writes_to_unicorn()?;
                        self.sync_regs_from_shadow_to_unicorn()?;
                        self.hybrid_reset_silent_window();
                    }
                    self.hybrid_note_native_step(pc);
                    if let Some(reason) = self.hybrid_handoff_candidate_reason() {
                        let remaining_steps = requested_steps.saturating_sub(idx + 1);
                        self.handoff_remaining_via_shadow(idx, &reason, remaining_steps)?;
                        break;
                    }
                }
                Err(err) => {
                    self.executed_instructions = self.executed_instructions.saturating_add(1);
                    match self.fallback_step_via_shadow(idx, pc, thumb, err)? {
                        StepControl::Continue => continue,
                        StepControl::Stop(reason) => {
                            self.stop_reason = reason;
                            break;
                        }
                    }
                }
            }
        }
        if self.stop_reason == "not-started" {
            self.stop_reason = format!("step budget {} reached", requested_steps);
        }
        self.status = if let Some(reason) = self.hybrid_handoff_reason.as_ref() {
            format!(
                "unicorn-hybrid executed {} instruction(s); handoff={} count={}; stop: {}",
                self.executed_instructions,
                reason,
                self.hybrid_handoff_count,
                self.stop_reason
            )
        } else {
            format!(
                "unicorn-hybrid executed {} instruction(s); stop: {}",
                self.executed_instructions, self.stop_reason
            )
        };
        Ok(())
    }

    fn execution_summary(&self) -> RuntimeBackendExecutionSummary {
        self.build_execution_summary()
    }

    fn snapshot(&self) -> BackendSnapshot {
        let first_instruction_text = match self.first_instruction {
            Some(word) if self.first_instruction_thumb => Some(format_thumb_halfword(word as u16)),
            Some(word) => Some(format_arm_word(word)),
            None => None,
        };
        BackendSnapshot {
            backend: "unicorn".to_string(),
            status: self.status.clone(),
            stop_reason: self.stop_reason.clone(),
            first_instruction_addr: self.first_instruction_addr,
            first_instruction: self.first_instruction,
            first_instruction_text,
            entry_bytes_present: self.entry_bytes_present,
            executed_instructions: self.executed_instructions,
            final_pc: Some(self.shadow.cpu.regs[15]),
            final_sp: Some(self.shadow.cpu.regs[13]),
            final_lr: Some(self.shadow.cpu.regs[14]),
            trace: self.trace.clone(),
            runtime_state: Some(self.shadow.runtime_state_snapshot()),
            backend_execution: Some(self.build_execution_summary()),
        }
    }

    fn install_symbol_label(&mut self, addr: u32, label: &str) -> CoreResult<()> {
        self.shadow.install_symbol_label(addr, label)?;
        self.emu.get_data_mut().trap_points.insert(addr & !1, label.to_string());
        Ok(())
    }
}

#[cfg(not(feature = "unicorn"))]
#[derive(Debug, Default)]
pub struct UnicornArm32Backend;

#[cfg(not(feature = "unicorn"))]
impl UnicornArm32Backend {
    pub fn new(_cfg: &CoreConfig) -> CoreResult<Self> {
        Err(CoreError::Backend(
            "execution backend 'unicorn' requires building mkea-core with the 'unicorn' feature".to_string(),
        ))
    }
}

#[cfg(not(feature = "unicorn"))]
impl CpuBackend for UnicornArm32Backend {
    fn policy(&self) -> BackendPolicy {
        BackendPolicy::HybridExecution
    }

    fn map(&mut self, _addr: u32, _size: u32, _prot: u32) -> CoreResult<()> {
        Err(CoreError::Backend("unicorn backend unavailable without feature".to_string()))
    }
    fn write_mem(&mut self, _addr: u32, _data: &[u8]) -> CoreResult<()> {
        Err(CoreError::Backend("unicorn backend unavailable without feature".to_string()))
    }
    fn set_pc(&mut self, _pc: u32, _thumb: bool) -> CoreResult<()> {
        Err(CoreError::Backend("unicorn backend unavailable without feature".to_string()))
    }
    fn set_sp(&mut self, _sp: u32) -> CoreResult<()> {
        Err(CoreError::Backend("unicorn backend unavailable without feature".to_string()))
    }
    fn run(&mut self, _max_instructions: u64) -> CoreResult<()> {
        Err(CoreError::Backend("unicorn backend unavailable without feature".to_string()))
    }
    fn snapshot(&self) -> BackendSnapshot {
        BackendSnapshot {
            backend: "unicorn".to_string(),
            status: "unicorn feature missing".to_string(),
            stop_reason: "unicorn backend unavailable without feature".to_string(),
            first_instruction_addr: None,
            first_instruction: None,
            first_instruction_text: None,
            entry_bytes_present: false,
            executed_instructions: 0,
            final_pc: None,
            final_sp: None,
            final_lr: None,
            trace: vec!["build mkea-core with feature=unicorn to enable the UnicornArm32Backend".to_string()],
            runtime_state: None,
            backend_execution: None,
        }
    }

}
