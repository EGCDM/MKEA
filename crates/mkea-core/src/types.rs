use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GuestRange {
    pub start: u32,
    pub size: u32,
}

impl GuestRange {
    pub fn end(&self) -> u32 {
        self.start.saturating_add(self.size)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EntryPoint {
    pub pc: u32,
    pub sp: u32,
    pub thumb: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedRegion {
    pub name: String,
    pub kind: String,
    pub start: u32,
    pub end: u32,
    pub size: u32,
    pub prot: String,
    pub fileoff: Option<u32>,
    pub filesize: Option<u32>,
    pub loaded_bytes: Option<u32>,
    pub zero_fill_bytes: Option<u32>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackBootstrap {
    pub argv0: String,
    pub argv0_addr: u32,
    pub argv_ptr: u32,
    pub argc: u32,
    pub sp: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitialRegisters {
    pub r0: u32,
    pub r1: u32,
    pub r2: u32,
    pub r3: u32,
    pub lr: u32,
    pub sp: u32,
    pub pc: u32,
    pub thumb: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryWriteRecord {
    pub address: u32,
    pub size: u32,
    pub kind: String,
    pub symbol: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageLoadReport {
    pub segments_written: usize,
    pub bytes_written: u64,
    pub stack_writes: usize,
    pub trampoline_writes: usize,
    pub stubbed_symbols: usize,
    pub indirect_pointer_writes: usize,
    pub external_relocation_writes: usize,
    pub unresolved_symbols: Vec<String>,
    pub write_records: Vec<MemoryWriteRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapPlan {
    pub app: String,
    pub bundle_id: String,
    pub arch: String,
    pub minimum_ios_version: String,
    pub page_size: u32,
    pub entry: EntryPoint,
    pub stack: StackBootstrap,
    pub registers: InitialRegisters,
    pub mapped_regions: Vec<PlannedRegion>,
    pub image_load: ImageLoadReport,
    pub warnings: Vec<String>,
}
