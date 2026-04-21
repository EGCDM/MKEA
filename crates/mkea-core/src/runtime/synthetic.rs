use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{CoreConfig, ExecutionBackendKind, RuntimeMode};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSyntheticConfigReport {
    pub runtime_mode: String,
    pub execution_backend: String,
    pub network_fault_probes: bool,
    pub runloop_tick_budget: u32,
    pub menu_probe_selector: Option<String>,
    pub menu_probe_after_ticks: u32,
    pub menu_probe_attempts: u32,
    pub menu_probe_fired: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct BackendTuning {
    pub synthetic_network_fault_probes: bool,
    pub synthetic_runloop_ticks: u32,
    pub synthetic_menu_probe_selector: Option<String>,
    pub synthetic_menu_probe_after_ticks: u32,
    pub live_host_mode: bool,
    pub host_input_script_path: Option<PathBuf>,
    pub host_input_width: u32,
    pub host_input_height: u32,
    pub host_input_flip_y: bool,
    pub dump_frames: bool,
    pub frame_dump_dir: PathBuf,
    pub dump_every: u32,
    pub dump_limit: u32,
    pub runtime_mode: RuntimeMode,
    pub execution_backend: ExecutionBackendKind,
}

impl BackendTuning {
    pub(crate) fn from_core_config(cfg: &CoreConfig) -> Self {
        let synthetic_runtime_enabled = cfg.runtime_mode.allows_synthetic_runtime();
        Self {
            synthetic_network_fault_probes: synthetic_runtime_enabled && cfg.synthetic_network_fault_probes,
            synthetic_runloop_ticks: if synthetic_runtime_enabled {
                cfg.synthetic_runloop_ticks.max(1)
            } else {
                0
            },
            synthetic_menu_probe_selector: if synthetic_runtime_enabled {
                cfg.synthetic_menu_probe_selector
                    .clone()
                    .filter(|value| !value.trim().is_empty())
            } else {
                None
            },
            synthetic_menu_probe_after_ticks: cfg.synthetic_menu_probe_after_ticks.max(1),
            live_host_mode: cfg.live_host_mode,
            host_input_script_path: cfg.input_script_path.as_ref().map(PathBuf::from),
            host_input_width: cfg.input_host_width,
            host_input_height: cfg.input_host_height,
            host_input_flip_y: cfg.input_flip_y,
            dump_frames: cfg.dump_frames,
            frame_dump_dir: PathBuf::from(cfg.frame_dump_dir.clone()),
            dump_every: cfg.dump_every.max(1),
            dump_limit: cfg.dump_limit,
            runtime_mode: cfg.runtime_mode,
            execution_backend: cfg.execution_backend,
        }
    }

    pub(crate) fn synthetic_runloop_enabled(&self) -> bool {
        self.synthetic_runloop_ticks > 0
    }
}

impl Default for BackendTuning {
    fn default() -> Self {
        Self::from_core_config(&CoreConfig::default())
    }
}
