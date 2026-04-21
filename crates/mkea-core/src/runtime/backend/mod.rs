pub mod dryrun;
pub mod unicorn;

use mkea_loader::SectionInfo;
use serde::{Deserialize, Serialize};

use crate::{
    error::CoreResult,
    types::InitialRegisters,
};

use super::diagnostics::{RuntimeBackendExecutionSummary, RuntimeStateReport};

pub use dryrun::DryRunArm32Backend;
pub use super::engine::MemoryArm32Backend;
pub use unicorn::UnicornArm32Backend;

/// Fixed backend responsibility boundary for Stage A:
/// - DryRun materializes memory/register state only.
/// - Memory owns synthetic/HLE execution and diagnostics generation.
/// - Unicorn owns native execution but delegates unsupported behavior and diagnostics
///   to the shared shadow memory backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendPolicy {
    ProbeOnly,
    SyntheticRuntime,
    HybridExecution,
}

pub trait CpuBackend {
    fn policy(&self) -> BackendPolicy;
    fn map(&mut self, addr: u32, size: u32, prot: u32) -> CoreResult<()>;
    fn write_mem(&mut self, addr: u32, data: &[u8]) -> CoreResult<()>;
    fn set_pc(&mut self, pc: u32, thumb: bool) -> CoreResult<()>;
    fn set_sp(&mut self, sp: u32) -> CoreResult<()>;
    fn set_initial_registers(&mut self, regs: &InitialRegisters) -> CoreResult<()> {
        self.set_pc(regs.pc, regs.thumb)?;
        self.set_sp(regs.sp)?;
        Ok(())
    }
    fn seed_objc_metadata_sections(&mut self, _sections: &[SectionInfo]) {}
    fn run(&mut self, max_instructions: u64) -> CoreResult<()>;
    fn snapshot(&self) -> BackendSnapshot;
    fn install_symbol_label(&mut self, _addr: u32, _label: &str) -> CoreResult<()> {
        Ok(())
    }
    fn execution_summary(&self) -> RuntimeBackendExecutionSummary {
        let backend_policy = match self.policy() {
            BackendPolicy::ProbeOnly => "probe-only",
            BackendPolicy::SyntheticRuntime => "synthetic-runtime",
            BackendPolicy::HybridExecution => "hybrid-execution",
        };
        RuntimeBackendExecutionSummary {
            backend_policy: backend_policy.to_string(),
            total_steps: 0,
            native_steps: 0,
            shadow_steps: 0,
            shadow_trap_steps: 0,
            shadow_fallback_steps: 0,
            shadow_handoff_steps: 0,
            trap_dispatches: 0,
            fallback_dispatches: 0,
            handoff_count: 0,
            native_share_milli: 0,
            shadow_share_milli: 0,
            trap_classes: Vec::new(),
            top_stop_sites: Vec::new(),
            semantics_candidates: Vec::new(),
            last_trap_class: None,
            last_trap_reason: None,
            last_handoff_reason: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendSnapshot {
    pub backend: String,
    pub status: String,
    pub stop_reason: String,
    pub first_instruction_addr: Option<u32>,
    pub first_instruction: Option<u32>,
    pub first_instruction_text: Option<String>,
    pub entry_bytes_present: bool,
    pub executed_instructions: u64,
    pub final_pc: Option<u32>,
    pub final_sp: Option<u32>,
    pub final_lr: Option<u32>,
    pub trace: Vec<String>,
    pub runtime_state: Option<RuntimeStateReport>,
    pub backend_execution: Option<RuntimeBackendExecutionSummary>,
}
