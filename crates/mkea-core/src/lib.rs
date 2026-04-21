pub mod config;
pub mod error;
pub mod runtime;
pub mod types;

pub use config::{CoreConfig, ExecutionBackendKind, RuntimeMode};
pub use error::{CoreError, CoreResult};
pub use runtime::{
    clear_live_input, clear_live_runtime_telemetry, clear_stop_request, drain_live_input, enqueue_live_input,
    install_live_frame_sink, is_stop_requested, live_frame_queue_depth, note_live_input_event, plan_bootstrap,
    request_stop, snapshot_live_runtime_telemetry, take_live_frame, BackendPolicy, BackendSnapshot, BootstrapPlanner, CoreRuntime,
    CpuBackend, DryRunArm32Backend, InputEventTelemetry, LiveFramePacket, LiveInputPacket,
    LivePresentTelemetry, LiveRuntimeTelemetry, MemoryArm32Backend, RunloopTickTelemetry, RuntimeReport,
    RuntimeSyntheticConfigReport, UnicornArm32Backend,
};
pub use types::{
    BootstrapPlan, EntryPoint, ImageLoadReport, InitialRegisters, MemoryWriteRecord, PlannedRegion,
    StackBootstrap,
};
