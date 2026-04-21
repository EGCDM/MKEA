mod backend;
mod bootstrap;
mod diagnostics;
mod profiles;
mod control;
mod engine;
mod framebus;
mod inputbus;
mod memory;
mod render;
mod stubs;
mod synthetic;
mod telemetry;

pub use control::{clear_stop_request, is_stop_requested, request_stop};
pub use framebus::{install_live_frame_sink, live_frame_queue_depth, take_live_frame, LiveFramePacket};
pub use inputbus::{clear_live_input, drain_live_input, enqueue_live_input, LiveInputPacket};
pub use bootstrap::{plan_bootstrap, BootstrapPlanner};
pub use backend::{BackendPolicy, BackendSnapshot, CpuBackend, DryRunArm32Backend, MemoryArm32Backend, UnicornArm32Backend};
pub use diagnostics::RuntimeReport;
pub use engine::CoreRuntime;
pub use memory::{
    align_down, align_up_checked, mach_prot_to_guest, prot_to_string, GuestMemory, MemoryRegion,
    GUEST_PROT_EXEC, GUEST_PROT_READ, GUEST_PROT_WRITE,
};
pub use stubs::{StubBinding, StubRegistry};
pub use synthetic::RuntimeSyntheticConfigReport;
pub use telemetry::{
    clear_live_runtime_telemetry, note_live_input_event, snapshot_live_runtime_telemetry,
    InputEventTelemetry, LivePresentTelemetry, LiveRuntimeTelemetry, RunloopTickTelemetry,
};
