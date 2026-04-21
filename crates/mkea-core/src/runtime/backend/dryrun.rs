use crate::{
    error::CoreResult,
    types::InitialRegisters,
};

use super::{BackendPolicy, BackendSnapshot, CpuBackend};

#[derive(Debug, Default, Clone)]
pub struct DryRunArm32Backend {
    pub mapped: Vec<(u32, u32, u32)>,
    pub writes: Vec<(u32, usize)>,
    pub regs: [u32; 16],
    pub thumb: bool,
    pub entry_installed: bool,
}

impl CpuBackend for DryRunArm32Backend {
    fn policy(&self) -> BackendPolicy {
        BackendPolicy::ProbeOnly
    }

    fn map(&mut self, addr: u32, size: u32, prot: u32) -> CoreResult<()> {
        self.mapped.push((addr, size, prot));
        Ok(())
    }

    fn write_mem(&mut self, addr: u32, data: &[u8]) -> CoreResult<()> {
        self.writes.push((addr, data.len()));
        Ok(())
    }

    fn set_pc(&mut self, pc: u32, thumb: bool) -> CoreResult<()> {
        self.regs[15] = pc & !1;
        self.thumb = thumb;
        self.entry_installed = true;
        Ok(())
    }

    fn set_sp(&mut self, sp: u32) -> CoreResult<()> {
        self.regs[13] = sp;
        self.entry_installed = true;
        Ok(())
    }

    fn set_initial_registers(&mut self, regs: &InitialRegisters) -> CoreResult<()> {
        self.regs[0] = regs.r0;
        self.regs[1] = regs.r1;
        self.regs[2] = regs.r2;
        self.regs[3] = regs.r3;
        self.regs[13] = regs.sp;
        self.regs[14] = regs.lr;
        self.regs[15] = regs.pc & !1;
        self.thumb = regs.thumb;
        self.entry_installed = true;
        Ok(())
    }

    fn run(&mut self, _max_instructions: u64) -> CoreResult<()> {
        Ok(())
    }

    fn snapshot(&self) -> BackendSnapshot {
        BackendSnapshot {
            backend: "dryrun".to_string(),
            status: if self.entry_installed {
                "dry-run only; image materialized and guest registers were seeded".to_string()
            } else {
                "dry-run only; no memory-backed entry fetch happened".to_string()
            },
            stop_reason: "dry-run backend does not execute or trace instructions".to_string(),
            first_instruction_addr: None,
            first_instruction: None,
            first_instruction_text: None,
            entry_bytes_present: false,
            executed_instructions: 0,
            final_pc: Some(self.regs[15]),
            final_sp: Some(self.regs[13]),
            final_lr: Some(self.regs[14]),
            trace: Vec::new(),
            runtime_state: None,
            backend_execution: None,
        }
    }
}
