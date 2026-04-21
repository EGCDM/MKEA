use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use mkea_cli::{
    args::{BackendArg, RuntimeModeArg},
    config_builder::{build_core_config, CoreConfigOverrides},
    live, phase,
};

#[derive(Debug, Parser)]
#[command(name = "mkea")]
#[command(about = "MKEA iPhoneOS 3.0/3.1 HLE rewrite CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Probe {
        ipa: PathBuf,
        #[arg(long, default_value = "armv6")]
        prefer_arch: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Build {
        ipa: PathBuf,
        #[arg(long, default_value = "armv6")]
        prefer_arch: String,
        #[arg(long, default_value = "build")]
        out_dir: PathBuf,
    },
    Bootstrap {
        ipa: PathBuf,
        #[arg(long, default_value = "armv6")]
        prefer_arch: String,
        #[arg(long)]
        argv0: Option<String>,
        #[arg(long, value_enum, default_value_t = RuntimeModeArg::Strict)]
        runtime_mode: RuntimeModeArg,
        #[arg(long, value_enum, default_value_t = BackendArg::Unicorn)]
        backend: BackendArg,
        #[arg(long)]
        synthetic_network_faults: bool,
        #[arg(long)]
        runloop_ticks: Option<u32>,
        #[arg(long)]
        dump_frames: bool,
        #[arg(long)]
        frame_dump_dir: Option<PathBuf>,
        #[arg(long)]
        dump_every: Option<u32>,
        #[arg(long)]
        dump_limit: Option<u32>,
        #[arg(long)]
        input_script: Option<PathBuf>,
        #[arg(long)]
        input_width: Option<u32>,
        #[arg(long)]
        input_height: Option<u32>,
        #[arg(long)]
        input_flip_y: bool,
        #[arg(long)]
        menu_probe_selector: Option<String>,
        #[arg(long)]
        menu_probe_after: Option<u32>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    BootstrapManifest {
        manifest: PathBuf,
        #[arg(long)]
        argv0: Option<String>,
        #[arg(long, value_enum, default_value_t = RuntimeModeArg::Strict)]
        runtime_mode: RuntimeModeArg,
        #[arg(long, value_enum, default_value_t = BackendArg::Unicorn)]
        backend: BackendArg,
        #[arg(long)]
        synthetic_network_faults: bool,
        #[arg(long)]
        runloop_ticks: Option<u32>,
        #[arg(long)]
        dump_frames: bool,
        #[arg(long)]
        frame_dump_dir: Option<PathBuf>,
        #[arg(long)]
        dump_every: Option<u32>,
        #[arg(long)]
        dump_limit: Option<u32>,
        #[arg(long)]
        input_script: Option<PathBuf>,
        #[arg(long)]
        input_width: Option<u32>,
        #[arg(long)]
        input_height: Option<u32>,
        #[arg(long)]
        input_flip_y: bool,
        #[arg(long)]
        menu_probe_selector: Option<String>,
        #[arg(long)]
        menu_probe_after: Option<u32>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    RunLive {
        manifest: PathBuf,
        #[arg(long)]
        argv0: Option<String>,
        #[arg(long, value_enum, default_value_t = RuntimeModeArg::Strict)]
        runtime_mode: RuntimeModeArg,
        #[arg(long, value_enum, default_value_t = BackendArg::Unicorn)]
        backend: BackendArg,
        #[arg(long)]
        synthetic_network_faults: bool,
        #[arg(long)]
        runloop_ticks: Option<u32>,
        #[arg(long)]
        input_script: Option<PathBuf>,
        #[arg(long)]
        input_flip_y: bool,
        #[arg(long)]
        menu_probe_selector: Option<String>,
        #[arg(long)]
        menu_probe_after: Option<u32>,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value = "mkEA live")]
        title: String,
        #[arg(long, default_value_t = 640)]
        window_width: u32,
        #[arg(long, default_value_t = 960)]
        window_height: u32,
        #[arg(long, default_value_t = 150_000_000)]
        max_instructions: u64,
        #[arg(long)]
        frame_dump_dir: Option<PathBuf>,
        #[arg(long)]
        close_when_finished: bool,
    },
    Audit {
        phase: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Diff {
        baseline: PathBuf,
        candidate: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Explain {
        manifest: Option<PathBuf>,
    },
}

fn run_bootstrap(
    loaded: mkea_loader::LoadedIpa,
    cfg: mkea_core::CoreConfig,
    out: Option<PathBuf>,
    context_label: &str,
) -> anyhow::Result<()> {
    let (plan, runtime) = mkea_core::plan_bootstrap(&loaded.probe, &loaded.macho_slice, cfg)
        .with_context(|| format!("failed to bootstrap image from {context_label}"))?;

    let out_json = serde_json::json!({
        "probe": loaded.probe,
        "bootstrap": plan,
        "runtime": runtime,
    });
    emit_json(&out_json, out.as_deref())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Probe {
            ipa,
            prefer_arch,
            out,
        } => {
            let loaded = mkea_loader::load_ipa_with_arch(&ipa, &prefer_arch)
                .with_context(|| format!("failed to inspect ipa: {}", ipa.display()))?;
            let json = serde_json::to_value(&loaded.probe)?;
            emit_json(&json, out.as_deref())?;
        }
        Command::Build {
            ipa,
            prefer_arch,
            out_dir,
        } => {
            let artifact = mkea_loader::write_build_artifacts(&ipa, &prefer_arch, &out_dir)
                .with_context(|| format!("failed to build loader artifacts from {}", ipa.display()))?;
            println!("wrote {}", out_dir.join("manifest.json").display());
            println!("selected slice: {}", artifact.binary_slice_path);
        }
        Command::Bootstrap {
            ipa,
            prefer_arch,
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
            out,
        } => {
            let loaded = mkea_loader::load_ipa_with_arch(&ipa, &prefer_arch)
                .with_context(|| format!("failed to inspect ipa: {}", ipa.display()))?;
            let cfg = build_core_config(
                &loaded,
                CoreConfigOverrides {
                    argv0,
                    runtime_mode: Some(runtime_mode.into()),
                    backend: Some(backend.into()),
                    synthetic_network_faults,
                    runloop_ticks,
                    dump_frames: Some(dump_frames),
                    frame_dump_dir,
                    dump_every,
                    dump_limit,
                    input_script,
                    input_width,
                    input_height,
                    input_flip_y,
                    menu_probe_selector,
                    menu_probe_after,
                    ..CoreConfigOverrides::default()
                },
            );
            run_bootstrap(loaded, cfg, out, &ipa.display().to_string())?;
        }
        Command::BootstrapManifest {
            manifest,
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
            out,
        } => {
            let loaded = mkea_loader::load_build_artifact(&manifest)
                .with_context(|| format!("failed to load build manifest: {}", manifest.display()))?;
            let cfg = build_core_config(
                &loaded,
                CoreConfigOverrides {
                    argv0,
                    runtime_mode: Some(runtime_mode.into()),
                    backend: Some(backend.into()),
                    synthetic_network_faults,
                    runloop_ticks,
                    dump_frames: Some(dump_frames),
                    frame_dump_dir,
                    dump_every,
                    dump_limit,
                    input_script,
                    input_width,
                    input_height,
                    input_flip_y,
                    menu_probe_selector,
                    menu_probe_after,
                    ..CoreConfigOverrides::default()
                },
            );
            run_bootstrap(loaded, cfg, out, &manifest.display().to_string())?;
        }
        Command::RunLive {
            manifest,
            argv0,
            runtime_mode,
            backend,
            synthetic_network_faults,
            runloop_ticks,
            input_script,
            input_flip_y,
            menu_probe_selector,
            menu_probe_after,
            out,
            title,
            window_width,
            window_height,
            max_instructions,
            frame_dump_dir,
            close_when_finished,
        } => {
            live::run_live(live::LiveRunRequest {
                manifest,
                argv0,
                runtime_mode: runtime_mode.into(),
                backend: backend.into(),
                synthetic_network_faults,
                runloop_ticks,
                input_script,
                input_flip_y,
                menu_probe_selector,
                menu_probe_after,
                out,
                title,
                window_width,
                window_height,
                max_instructions,
                frame_dump_dir,
                close_when_finished,
            })?;
        }
        Command::Audit { phase, out } => {
            let value = phase::load_json(&phase)?;
            let audited = phase::audit_phase(&value);
            emit_json(&audited, out.as_deref())?;
        }
        Command::Diff {
            baseline,
            candidate,
            out,
        } => {
            let baseline_json = phase::load_json(&baseline)?;
            let candidate_json = phase::load_json(&candidate)?;
            let diff = phase::diff_phase(&baseline_json, &candidate_json);
            emit_json(&diff, out.as_deref())?;
        }
        Command::Explain { manifest } => {
            if let Some(path) = manifest {
                println!("manifest path: {}", path.display());
            }
            println!("Current Rust phase reaches post-launch UIKit/Cocos bootstrap, indexes the extracted bundle, and emits auditable runtime summaries for ObjC bridge, filesystem, network, graphics, and input.");
            println!("Workflow: build <ipa> --out-dir build -> bootstrap-manifest build/manifest.json --out phase.json -> audit phase.json / diff baseline.json candidate.json.");
            println!("Scripted input: bootstrap-manifest build/manifest.json --runtime-mode strict|hybrid|bring-up --backend memory|dry-run|unicorn --input-script touch.jsonl [--input-width 1280 --input-height 720 --input-flip-y].");
            println!("Live host window: run-live build/manifest.json [--runtime-mode strict|hybrid|bring-up --backend memory|dry-run|unicorn --window-width 640 --window-height 960 --input-script live.jsonl --out live_phase.json].");
        }
    }

    Ok(())
}

fn emit_json(value: &serde_json::Value, out: Option<&std::path::Path>) -> anyhow::Result<()> {
    if let Some(path) = out {
        phase::save_json(path, value)?;
        println!("wrote {}", path.display());
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}
