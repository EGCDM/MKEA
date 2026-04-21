use clap::ValueEnum;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RuntimeModeArg {
    #[value(alias = "bring_up", alias = "bringup")]
    BringUp,
    Hybrid,
    Strict,
}

impl From<RuntimeModeArg> for mkea_core::RuntimeMode {
    fn from(value: RuntimeModeArg) -> Self {
        match value {
            RuntimeModeArg::BringUp => Self::BringUp,
            RuntimeModeArg::Hybrid => Self::Hybrid,
            RuntimeModeArg::Strict => Self::Strict,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum BackendArg {
    Memory,
    #[value(alias = "dry_run", alias = "dryrun")]
    DryRun,
    Unicorn,
}

impl From<BackendArg> for mkea_core::ExecutionBackendKind {
    fn from(value: BackendArg) -> Self {
        match value {
            BackendArg::Memory => Self::Memory,
            BackendArg::DryRun => Self::DryRun,
            BackendArg::Unicorn => Self::Unicorn,
        }
    }
}
