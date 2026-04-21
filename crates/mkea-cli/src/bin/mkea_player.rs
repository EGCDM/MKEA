#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use mkea_cli::{
    args::{BackendArg, RuntimeModeArg},
    live::{run_live, LiveRunRequest},
};

#[derive(Debug, Parser)]
#[command(name = "mkea-player")]
#[command(about = "Standalone host app window for mkEA runtime")]
struct Args {
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
    #[arg(long, default_value = "mkEA player")]
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
}

fn main() -> Result<()> {
    let args = Args::parse();
    run_live(LiveRunRequest {
        manifest: args.manifest,
        argv0: args.argv0,
        runtime_mode: args.runtime_mode.into(),
        backend: args.backend.into(),
        synthetic_network_faults: args.synthetic_network_faults,
        runloop_ticks: args.runloop_ticks,
        input_script: args.input_script,
        input_flip_y: args.input_flip_y,
        menu_probe_selector: args.menu_probe_selector,
        menu_probe_after: args.menu_probe_after,
        out: args.out,
        title: args.title,
        window_width: args.window_width,
        window_height: args.window_height,
        max_instructions: args.max_instructions,
        frame_dump_dir: args.frame_dump_dir,
        close_when_finished: args.close_when_finished,
    })
}
