use std::path::PathBuf;

fn resolve_surface_profile(loaded: &mkea_loader::LoadedIpa) -> (Option<mkea_loader::DisplayOrientation>, Option<(u32, u32)>) {
    let mut orientation = loaded.probe.manifest.plist_orientation_hint();
    let mut surface = loaded.probe.manifest.surface_size_hint();
    if (orientation.is_none() || surface.is_none()) && loaded.bundle_root.is_some() {
        if let Some(bundle_root) = loaded.bundle_root.as_ref() {
            let profile = mkea_loader::infer_bundle_display_profile(
                bundle_root,
                &loaded.probe.manifest.supported_interface_orientations,
            );
            if orientation.is_none() {
                orientation = profile.preferred_orientation;
            }
            if surface.is_none() {
                surface = profile.surface_size();
            }
        }
    }
    (orientation, surface)
}

#[derive(Debug, Clone, Default)]
pub struct CoreConfigOverrides {
    pub argv0: Option<String>,
    pub runtime_mode: Option<mkea_core::RuntimeMode>,
    pub backend: Option<mkea_core::ExecutionBackendKind>,
    pub synthetic_network_faults: bool,
    pub runloop_ticks: Option<u32>,
    pub dump_frames: Option<bool>,
    pub frame_dump_dir: Option<PathBuf>,
    pub dump_every: Option<u32>,
    pub dump_limit: Option<u32>,
    pub input_script: Option<PathBuf>,
    pub input_width: Option<u32>,
    pub input_height: Option<u32>,
    pub input_flip_y: bool,
    pub menu_probe_selector: Option<String>,
    pub menu_probe_after: Option<u32>,
    pub live_host_mode: bool,
    pub max_instructions_floor: Option<u64>,
}

pub fn build_core_config(
    loaded: &mkea_loader::LoadedIpa,
    overrides: CoreConfigOverrides,
) -> mkea_core::CoreConfig {
    let CoreConfigOverrides {
        argv0,
        runtime_mode,
        backend,
        synthetic_network_faults,
        runloop_ticks,
        dump_frames,
        frame_dump_dir,
        dump_every,
        dump_limit,
        input_script,
        input_width,
        input_height,
        input_flip_y,
        menu_probe_selector,
        menu_probe_after,
        live_host_mode,
        max_instructions_floor,
    } = overrides;

    let mut cfg = mkea_core::CoreConfig::default();
    cfg.argv0 = argv0.unwrap_or_else(|| loaded.probe.manifest.executable.clone());
    cfg.bundle_root = loaded.bundle_root.as_ref().map(|path| path.display().to_string());
    let (orientation_hint, surface_hint) = resolve_surface_profile(loaded);
    cfg.orientation_hint = orientation_hint.map(|value| value.as_str().to_string());
    if let Some((width, height)) = surface_hint {
        cfg.preferred_surface_width = width.max(1);
        cfg.preferred_surface_height = height.max(1);
    }
    if let Some(runtime_mode) = runtime_mode {
        cfg.runtime_mode = runtime_mode;
    }
    if let Some(backend) = backend {
        cfg.execution_backend = backend;
    }
    cfg.synthetic_network_fault_probes = synthetic_network_faults;
    if let Some(ticks) = runloop_ticks {
        cfg.synthetic_runloop_ticks = ticks.max(1);
    }
    cfg.live_host_mode = live_host_mode;

    cfg.dump_frames = dump_frames.unwrap_or(false);
    if let Some(dir) = frame_dump_dir {
        cfg.frame_dump_dir = dir.display().to_string();
    }
    if let Some(every) = dump_every {
        cfg.dump_every = every.max(1);
    }
    if let Some(limit) = dump_limit {
        cfg.dump_limit = limit;
    }

    let has_input_script = input_script.is_some();
    cfg.input_script_path = input_script.map(|path| path.display().to_string());
    if let Some(width) = input_width {
        cfg.input_host_width = width.max(1);
    }
    if let Some(height) = input_height {
        cfg.input_host_height = height.max(1);
    }
    cfg.input_flip_y = input_flip_y;
    cfg.synthetic_menu_probe_selector = menu_probe_selector.filter(|value| !value.trim().is_empty());
    if let Some(after) = menu_probe_after {
        cfg.synthetic_menu_probe_after_ticks = after.max(1);
    }

    let heuristic_floor = if cfg.dump_frames {
        65_536
    } else if has_input_script {
        131_072
    } else {
        16_384
    };
    cfg.max_instructions = cfg.max_instructions.max(heuristic_floor);
    if let Some(floor) = max_instructions_floor {
        cfg.max_instructions = cfg.max_instructions.max(floor);
    }

    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_loaded(bundle_root: Option<&str>) -> mkea_loader::LoadedIpa {
        mkea_loader::LoadedIpa {
            probe: mkea_loader::IpaProbe {
                manifest: mkea_loader::Manifest {
                    bundle_id: "com.example.game".to_string(),
                    bundle_name: "Example Game".to_string(),
                    minimum_ios_version: "3.0".to_string(),
                    executable: "ExampleGame".to_string(),
                    info_plist_path_in_ipa: "Payload/ExampleGame.app/Info.plist".to_string(),
                    executable_path_in_ipa: "Payload/ExampleGame.app/ExampleGame".to_string(),
                    chosen_arch: "armv6".to_string(),
                    supported_interface_orientations: Vec::new(),
                    preferred_orientation: None,
                    native_surface_width: None,
                    native_surface_height: None,
                    orientation_inference_source: None,
                },
                mach: mkea_loader::MachProbe {
                    arch: "armv6".to_string(),
                    endianness: "LE".to_string(),
                    ncmds: 0,
                    sizeofcmds: 0,
                    has_lc_main: false,
                    has_unixthread: true,
                    entryoff: None,
                    stacksize: None,
                    entry_pc: Some(0x1000),
                    initial_sp: Some(0x7000_0000),
                    encryption_cryptid: Some(0),
                    dylibs: Vec::new(),
                    undefined_symbols: Vec::new(),
                    segments: Vec::new(),
                    sections: Vec::new(),
                    indirect_pointers: Vec::new(),
                    external_relocations: Vec::new(),
                },
                notes: Vec::new(),
            },
            macho_slice: Vec::new(),
            bundle_root: bundle_root.map(PathBuf::from),
        }
    }

    #[test]
    fn builder_sets_bundle_root_and_runtime_knobs() {
        let loaded = fake_loaded(Some("Payload/ExampleGame.app"));
        let cfg = build_core_config(
            &loaded,
            CoreConfigOverrides {
                runtime_mode: Some(mkea_core::RuntimeMode::Hybrid),
                backend: Some(mkea_core::ExecutionBackendKind::DryRun),
                synthetic_network_faults: true,
                runloop_ticks: Some(0),
                ..CoreConfigOverrides::default()
            },
        );

        assert_eq!(cfg.argv0, "ExampleGame");
        assert_eq!(cfg.bundle_root.as_deref(), Some("Payload/ExampleGame.app"));
        assert!(matches!(cfg.runtime_mode, mkea_core::RuntimeMode::Hybrid));
        assert!(matches!(cfg.execution_backend, mkea_core::ExecutionBackendKind::DryRun));
        assert!(cfg.synthetic_network_fault_probes);
        assert_eq!(cfg.synthetic_runloop_ticks, 1);
    }

    #[test]
    fn builder_applies_input_and_instruction_floors() {
        let loaded = fake_loaded(None);
        let cfg = build_core_config(
            &loaded,
            CoreConfigOverrides {
                input_script: Some(PathBuf::from("touch.jsonl")),
                input_width: Some(640),
                input_height: Some(960),
                input_flip_y: true,
                max_instructions_floor: Some(200_000),
                ..CoreConfigOverrides::default()
            },
        );

        assert_eq!(cfg.input_script_path.as_deref(), Some("touch.jsonl"));
        assert_eq!(cfg.input_host_width, 640);
        assert_eq!(cfg.input_host_height, 960);
        assert!(cfg.input_flip_y);
        assert_eq!(cfg.max_instructions, 200_000);
    }
}
