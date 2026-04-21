use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMode {
    BringUp,
    Hybrid,
    Strict,
}

impl RuntimeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BringUp => "bring_up",
            Self::Hybrid => "hybrid",
            Self::Strict => "strict",
        }
    }

    pub fn allows_synthetic_runtime(self) -> bool {
        !matches!(self, Self::Strict)
    }
}

impl Default for RuntimeMode {
    fn default() -> Self {
        Self::Strict
    }
}

impl fmt::Display for RuntimeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RuntimeMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bringup" | "bring_up" | "bring-up" => Ok(Self::BringUp),
            "hybrid" => Ok(Self::Hybrid),
            "strict" => Ok(Self::Strict),
            other => Err(format!("unknown runtime mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackendKind {
    Memory,
    DryRun,
    Unicorn,
}

impl ExecutionBackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::DryRun => "dry_run",
            Self::Unicorn => "unicorn",
        }
    }
}

impl Default for ExecutionBackendKind {
    fn default() -> Self {
        Self::Memory
    }
}

impl fmt::Display for ExecutionBackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ExecutionBackendKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "memory" => Ok(Self::Memory),
            "dryrun" | "dry_run" | "dry-run" => Ok(Self::DryRun),
            "unicorn" => Ok(Self::Unicorn),
            other => Err(format!("unknown execution backend: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    pub page_size: u32,
    pub max_instructions: u64,
    pub break_on_uimain: bool,
    pub enable_trace: bool,
    pub enable_objc: bool,
    pub enable_uikit: bool,
    pub enable_soft_gles: bool,
    pub runtime_mode: RuntimeMode,
    pub execution_backend: ExecutionBackendKind,
    pub synthetic_network_fault_probes: bool,
    pub synthetic_runloop_ticks: u32,
    pub dump_frames: bool,
    pub dump_every: u32,
    pub dump_limit: u32,
    pub frame_dump_dir: String,
    pub input_script_path: Option<String>,
    pub input_host_width: u32,
    pub input_host_height: u32,
    pub input_flip_y: bool,
    pub synthetic_menu_probe_selector: Option<String>,
    pub synthetic_menu_probe_after_ticks: u32,
    pub live_host_mode: bool,
    pub bundle_root: Option<String>,
    pub orientation_hint: Option<String>,
    pub preferred_surface_width: u32,
    pub preferred_surface_height: u32,
    pub argv0: String,
    pub stack_base: u32,
    pub stack_size: u32,
    pub heap_base: u32,
    pub heap_size: u32,
    pub selector_pool_base: u32,
    pub selector_pool_size: u32,
    pub trampoline_addr: u32,
}

impl CoreConfig {
    pub fn stack_top(&self) -> u32 {
        self.stack_base.saturating_add(self.stack_size)
    }

    pub fn trampoline_size(&self) -> u32 {
        self.page_size
    }
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            page_size: 0x1000,
            max_instructions: 5_000_000,
            break_on_uimain: true,
            enable_trace: true,
            enable_objc: true,
            enable_uikit: true,
            enable_soft_gles: false,
            runtime_mode: RuntimeMode::Strict,
            execution_backend: ExecutionBackendKind::Memory,
            synthetic_network_fault_probes: false,
            synthetic_runloop_ticks: 24,
            dump_frames: false,
            dump_every: 1,
            dump_limit: 0,
            frame_dump_dir: "frame_dumps".to_string(),
            input_script_path: None,
            input_host_width: 0,
            input_host_height: 0,
            input_flip_y: false,
            synthetic_menu_probe_selector: None,
            synthetic_menu_probe_after_ticks: 4,
            live_host_mode: false,
            bundle_root: None,
            orientation_hint: None,
            preferred_surface_width: 0,
            preferred_surface_height: 0,
            argv0: String::new(),
            stack_base: 0x7000_0000,
            stack_size: 8 * 1024 * 1024 + 0x10000,
            heap_base: 0x6000_0000,
            heap_size: 64 * 1024 * 1024,
            selector_pool_base: 0x6FE0_0000,
            selector_pool_size: 1 * 1024 * 1024,
            trampoline_addr: 0x6FFF_0000,
        }
    }
}
