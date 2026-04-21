use std::collections::BTreeSet;

use mkea_loader::{IndirectPointerKind, IpaProbe, SectionInfo, SegmentInfo};

use super::{
    backend::{BackendPolicy, BackendSnapshot, CpuBackend, DryRunArm32Backend, MemoryArm32Backend, UnicornArm32Backend},
    diagnostics::RuntimeReport,
    engine::{
        CoreRuntime, ARM_BX_LR, HLE_EXTERN_DATA_BASE,
        HLE_EXTERN_DATA_CGAFFINE_TRANSFORM_IDENTITY, HLE_EXTERN_DATA_CGPOINT_ZERO,
        HLE_EXTERN_DATA_CGRECT_ZERO, HLE_EXTERN_DATA_CGSIZE_ZERO,
        HLE_EXTERN_DATA_UIEDGEINSETS_ZERO, HLE_STUB_AUDIOQUEUE_CALLBACK_RETURN_ARM,
        HLE_STUB_AUDIOQUEUE_CALLBACK_RETURN_THUMB, HLE_STUB_UIAPPLICATION_POST_LAUNCH_ARM,
        HLE_STUB_UIAPPLICATION_POST_LAUNCH_THUMB,
    },
    memory::{align_down, align_up_checked, mach_prot_to_guest, prot_to_string, MemoryRegion, GUEST_PROT_EXEC, GUEST_PROT_READ, GUEST_PROT_WRITE},
};
use crate::{
    config::{CoreConfig, ExecutionBackendKind},
    error::{CoreError, CoreResult},
    types::{
        BootstrapPlan, EntryPoint, ImageLoadReport, InitialRegisters, MemoryWriteRecord,
        PlannedRegion, StackBootstrap,
    },
};

#[derive(Debug)]
enum PlannerBackend {
    Memory(MemoryArm32Backend),
    DryRun(DryRunArm32Backend),
    Unicorn(UnicornArm32Backend),
}

impl PlannerBackend {
    fn from_config(cfg: &CoreConfig) -> CoreResult<Self> {
        match cfg.execution_backend {
            ExecutionBackendKind::Memory => Ok(Self::Memory(MemoryArm32Backend::with_config(cfg))),
            ExecutionBackendKind::DryRun => Ok(Self::DryRun(DryRunArm32Backend::default())),
            ExecutionBackendKind::Unicorn => Ok(Self::Unicorn(UnicornArm32Backend::new(cfg)?)),
        }
    }
}

impl CpuBackend for PlannerBackend {
    fn policy(&self) -> BackendPolicy {
        match self {
            Self::Memory(inner) => inner.policy(),
            Self::DryRun(inner) => inner.policy(),
            Self::Unicorn(inner) => inner.policy(),
        }
    }

    fn map(&mut self, addr: u32, size: u32, prot: u32) -> CoreResult<()> {
        match self {
            Self::Memory(inner) => inner.map(addr, size, prot),
            Self::DryRun(inner) => inner.map(addr, size, prot),
            Self::Unicorn(inner) => inner.map(addr, size, prot),
        }
    }

    fn write_mem(&mut self, addr: u32, data: &[u8]) -> CoreResult<()> {
        match self {
            Self::Memory(inner) => inner.write_mem(addr, data),
            Self::DryRun(inner) => inner.write_mem(addr, data),
            Self::Unicorn(inner) => inner.write_mem(addr, data),
        }
    }

    fn set_pc(&mut self, pc: u32, thumb: bool) -> CoreResult<()> {
        match self {
            Self::Memory(inner) => inner.set_pc(pc, thumb),
            Self::DryRun(inner) => inner.set_pc(pc, thumb),
            Self::Unicorn(inner) => inner.set_pc(pc, thumb),
        }
    }

    fn set_sp(&mut self, sp: u32) -> CoreResult<()> {
        match self {
            Self::Memory(inner) => inner.set_sp(sp),
            Self::DryRun(inner) => inner.set_sp(sp),
            Self::Unicorn(inner) => inner.set_sp(sp),
        }
    }

    fn set_initial_registers(&mut self, regs: &InitialRegisters) -> CoreResult<()> {
        match self {
            Self::Memory(inner) => inner.set_initial_registers(regs),
            Self::DryRun(inner) => inner.set_initial_registers(regs),
            Self::Unicorn(inner) => inner.set_initial_registers(regs),
        }
    }

    fn seed_objc_metadata_sections(&mut self, sections: &[SectionInfo]) {
        match self {
            Self::Memory(inner) => inner.seed_objc_metadata_sections(sections),
            Self::DryRun(inner) => inner.seed_objc_metadata_sections(sections),
            Self::Unicorn(inner) => inner.seed_objc_metadata_sections(sections),
        }
    }

    fn run(&mut self, max_instructions: u64) -> CoreResult<()> {
        match self {
            Self::Memory(inner) => inner.run(max_instructions),
            Self::DryRun(inner) => inner.run(max_instructions),
            Self::Unicorn(inner) => inner.run(max_instructions),
        }
    }

    fn snapshot(&self) -> BackendSnapshot {
        match self {
            Self::Memory(inner) => inner.snapshot(),
            Self::DryRun(inner) => inner.snapshot(),
            Self::Unicorn(inner) => inner.snapshot(),
        }
    }

    fn install_symbol_label(&mut self, addr: u32, label: &str) -> CoreResult<()> {
        match self {
            Self::Memory(inner) => inner.install_symbol_label(addr, label),
            Self::DryRun(inner) => inner.install_symbol_label(addr, label),
            Self::Unicorn(inner) => inner.install_symbol_label(addr, label),
        }
    }
}

pub struct BootstrapPlanner {
    cfg: CoreConfig,
}

impl BootstrapPlanner {
    pub fn new(cfg: CoreConfig) -> Self {
        Self { cfg }
    }

    pub fn plan(&self, probe: &IpaProbe, macho_slice: &[u8]) -> CoreResult<(BootstrapPlan, RuntimeReport)> {
        let mut backend = PlannerBackend::from_config(&self.cfg)?;
        backend.seed_objc_metadata_sections(&probe.mach.sections);
        let mut runtime = CoreRuntime::new(self.cfg.clone(), backend);
        let mut mapped_regions = Vec::new();
        let mut warnings = Vec::new();
        match self.cfg.execution_backend {
            ExecutionBackendKind::DryRun => warnings.push(
                "execution backend=dry_run: the guest image will be materialized, but no instructions will execute".to_string(),
            ),
            ExecutionBackendKind::Unicorn => warnings.push(
                "execution backend=unicorn: real CPU execution is attempted first, then unsupported trap points/instructions fall back into the synthetic Rust shadow backend".to_string(),
            ),
            ExecutionBackendKind::Memory => {}
        }

        for seg in &probe.mach.segments {
            if seg.name == "__PAGEZERO" {
                warnings.push("__PAGEZERO intentionally left unmapped as a guard region".to_string());
                continue;
            }
            if seg.vmsize == 0 {
                warnings.push(format!("segment {} has zero vmsize and was skipped", seg.name));
                continue;
            }

            let region = build_image_region(seg, self.cfg.page_size)?;
            runtime.map_region(MemoryRegion {
                name: region.name.clone(),
                kind: region.kind.clone(),
                addr: region.start,
                size: region.size,
                prot: mach_prot_to_guest(seg.initprot),
            })?;
            mapped_regions.push(region);
        }

        for region in self.synthetic_regions()? {
            let prot_bits = match region.kind.as_str() {
                "trampoline" => GUEST_PROT_READ | GUEST_PROT_EXEC,
                _ => GUEST_PROT_READ | GUEST_PROT_WRITE,
            };
            runtime.map_region(MemoryRegion {
                name: region.name.clone(),
                kind: region.kind.clone(),
                addr: region.start,
                size: region.size,
                prot: prot_bits,
            })?;
            mapped_regions.push(region);
        }

        runtime
            .stubs_mut()
            .seed_trampoline(self.cfg.trampoline_addr, self.cfg.trampoline_size())?;
        runtime
            .stubs_mut()
            .insert(HLE_STUB_AUDIOQUEUE_CALLBACK_RETURN_ARM, "__audioqueue_callback_return_arm");
        runtime
            .stubs_mut()
            .insert(HLE_STUB_AUDIOQUEUE_CALLBACK_RETURN_THUMB, "__audioqueue_callback_return_thumb");
        runtime
            .stubs_mut()
            .insert(HLE_STUB_UIAPPLICATION_POST_LAUNCH_ARM, "__uimain_post_launch_arm");
        runtime
            .stubs_mut()
            .insert(HLE_STUB_UIAPPLICATION_POST_LAUNCH_THUMB, "__uimain_post_launch_thumb");

        let mut load_report = ImageLoadReport {
            segments_written: 0,
            bytes_written: 0,
            stack_writes: 0,
            trampoline_writes: 0,
            stubbed_symbols: 0,
            indirect_pointer_writes: 0,
            external_relocation_writes: 0,
            unresolved_symbols: Vec::new(),
            write_records: Vec::new(),
        };

        self.materialize_trampoline_page(&mut runtime, &mut load_report)?;
        self.materialize_segments(&mut runtime, probe, macho_slice, &mut load_report)?;

        let argv0 = if self.cfg.argv0.is_empty() {
            probe.manifest.executable.clone()
        } else {
            self.cfg.argv0.clone()
        };
        let stack = build_initial_stack_layout(&self.cfg, &argv0)?;
        self.materialize_initial_stack(&mut runtime, &stack, &mut load_report)?;
        self.materialize_data_externals(&mut runtime, &mut load_report)?;

        self.bind_imports(&mut runtime, probe, &mut load_report, &mut warnings)?;

        let raw_pc = probe.mach.entry_pc.ok_or_else(|| {
            CoreError::InvalidImage("Mach-O does not expose LC_MAIN or LC_UNIXTHREAD entry PC".into())
        })?;
        let entry = EntryPoint {
            pc: raw_pc & !1,
            sp: stack.sp,
            thumb: (raw_pc & 1) != 0,
        };

        if !mapped_regions.iter().any(|r| entry.pc >= r.start && entry.pc < r.end) {
            warnings.push(format!(
                "entry PC 0x{:08x} does not land inside any mapped image region",
                entry.pc
            ));
        }

        if !load_report.unresolved_symbols.is_empty() {
            warnings.push(format!(
                "{} symbols could not be bound inside the current trampoline page",
                load_report.unresolved_symbols.len()
            ));
        }

        runtime.sync_stub_symbols_to_backend()?;

        let registers = InitialRegisters {
            r0: stack.argc,
            r1: stack.argv_ptr,
            r2: 0,
            r3: 0,
            lr: 0,
            sp: entry.sp,
            pc: entry.pc,
            thumb: entry.thumb,
        };

        runtime.install_initial_registers(&registers)?;

        let plan = BootstrapPlan {
            app: probe.manifest.bundle_name.clone(),
            bundle_id: probe.manifest.bundle_id.clone(),
            arch: probe.mach.arch.clone(),
            minimum_ios_version: probe.manifest.minimum_ios_version.clone(),
            page_size: self.cfg.page_size,
            entry,
            stack,
            registers,
            mapped_regions,
            image_load: load_report,
            warnings,
        };

        let report = runtime.run(entry)?;
        Ok((plan, report))
    }

    fn synthetic_regions(&self) -> CoreResult<Vec<PlannedRegion>> {
        Ok(vec![
            simple_region(
                "stack",
                "stack",
                self.cfg.stack_base,
                self.cfg.stack_size,
                GUEST_PROT_READ | GUEST_PROT_WRITE,
                Some("argc/argv bootstrap lives near the top of this mapping".to_string()),
            )?,
            simple_region(
                "heap",
                "heap",
                self.cfg.heap_base,
                self.cfg.heap_size,
                GUEST_PROT_READ | GUEST_PROT_WRITE,
                Some("future malloc arena / CoreFoundation allocations".to_string()),
            )?,
            simple_region(
                "selector_pool",
                "selector_pool",
                self.cfg.selector_pool_base,
                self.cfg.selector_pool_size,
                GUEST_PROT_READ | GUEST_PROT_WRITE,
                Some("future ObjC selector/string pool".to_string()),
            )?,
            simple_region(
                "extern_data",
                "extern_data",
                HLE_EXTERN_DATA_BASE,
                self.cfg.page_size,
                GUEST_PROT_READ | GUEST_PROT_WRITE,
                Some("synthetic data extern page for imported CoreGraphics/UIEdgeInsets constants".to_string()),
            )?,
            simple_region(
                "trampoline",
                "trampoline",
                self.cfg.trampoline_addr,
                self.cfg.trampoline_size(),
                GUEST_PROT_READ | GUEST_PROT_EXEC,
                Some("stub return/trap page; each unresolved import gets a tiny bx lr slot".to_string()),
            )?,
        ])
    }

    fn materialize_trampoline_page<B: CpuBackend>(
        &self,
        runtime: &mut CoreRuntime<B>,
        report: &mut ImageLoadReport,
    ) -> CoreResult<()> {
        let page_len = self.cfg.trampoline_size() as usize;
        let mut page = vec![0u8; page_len];
        for chunk in page.chunks_exact_mut(4) {
            chunk.copy_from_slice(&ARM_BX_LR);
        }
        for thumb_stub_addr in [
            HLE_STUB_AUDIOQUEUE_CALLBACK_RETURN_THUMB,
            HLE_STUB_UIAPPLICATION_POST_LAUNCH_THUMB,
        ] {
            let thumb_stub_offset = thumb_stub_addr
                .saturating_sub(self.cfg.trampoline_addr) as usize;
            if thumb_stub_offset + 4 <= page.len() {
                page[thumb_stub_offset..thumb_stub_offset + 4].copy_from_slice(&[0x70, 0x47, 0x70, 0x47]);
            }
        }
        runtime.write_guest(self.cfg.trampoline_addr, &page, "trampoline_page")?;
        report.trampoline_writes += 1;
        report.bytes_written += page.len() as u64;
        report.write_records.push(MemoryWriteRecord {
            address: self.cfg.trampoline_addr,
            size: page.len() as u32,
            kind: "trampoline_page".to_string(),
            symbol: None,
            note: Some("filled with ARM bx lr stubs".to_string()),
        });
        Ok(())
    }

    fn materialize_segments<B: CpuBackend>(
        &self,
        runtime: &mut CoreRuntime<B>,
        probe: &IpaProbe,
        macho_slice: &[u8],
        report: &mut ImageLoadReport,
    ) -> CoreResult<()> {
        for seg in &probe.mach.segments {
            if seg.name == "__PAGEZERO" || seg.vmsize == 0 || seg.filesize == 0 {
                continue;
            }
            let data = read_segment_bytes(macho_slice, seg)?;
            runtime.write_guest(seg.vmaddr, &data, format!("segment:{}", seg.name))?;
            report.segments_written += 1;
            report.bytes_written += data.len() as u64;
            report.write_records.push(MemoryWriteRecord {
                address: seg.vmaddr,
                size: data.len() as u32,
                kind: "segment_bytes".to_string(),
                symbol: None,
                note: Some(seg.name.clone()),
            });
        }
        Ok(())
    }

    fn materialize_initial_stack<B: CpuBackend>(
        &self,
        runtime: &mut CoreRuntime<B>,
        stack: &StackBootstrap,
        report: &mut ImageLoadReport,
    ) -> CoreResult<()> {
        let argv0_bytes = stack.argv0.as_bytes();
        let mut string = argv0_bytes.to_vec();
        string.push(0);
        let padded_len = align4(string.len() as u32) as usize;
        string.resize(padded_len, 0);
        runtime.write_guest(stack.argv0_addr, &string, "stack_argv0")?;
        report.stack_writes += 1;
        report.bytes_written += string.len() as u64;
        report.write_records.push(MemoryWriteRecord {
            address: stack.argv0_addr,
            size: string.len() as u32,
            kind: "stack_argv0".to_string(),
            symbol: None,
            note: Some(stack.argv0.clone()),
        });

        let mut argv = Vec::with_capacity(8);
        argv.extend_from_slice(&stack.argv0_addr.to_le_bytes());
        argv.extend_from_slice(&0u32.to_le_bytes());
        runtime.write_guest(stack.argv_ptr, &argv, "stack_argv")?;
        report.stack_writes += 1;
        report.bytes_written += argv.len() as u64;
        report.write_records.push(MemoryWriteRecord {
            address: stack.argv_ptr,
            size: argv.len() as u32,
            kind: "stack_argv".to_string(),
            symbol: None,
            note: Some("argv[0], argv[1]=NULL".to_string()),
        });

        runtime.write_u32(stack.sp, stack.argc, "stack_argc")?;
        report.stack_writes += 1;
        report.bytes_written += 4;
        report.write_records.push(MemoryWriteRecord {
            address: stack.sp,
            size: 4,
            kind: "stack_argc".to_string(),
            symbol: None,
            note: Some("argc".to_string()),
        });
        Ok(())
    }

    fn data_external_binding(symbol: &str) -> Option<(u32, Vec<u8>, &'static str)> {
        let push_f32 = |out: &mut Vec<u8>, value: f32| {
            out.extend_from_slice(&value.to_bits().to_le_bytes());
        };
        let mut bytes = Vec::new();
        let (addr, note) = match symbol {
            "CGPointZero" => {
                push_f32(&mut bytes, 0.0);
                push_f32(&mut bytes, 0.0);
                (HLE_EXTERN_DATA_CGPOINT_ZERO, "CGPoint{0,0}")
            }
            "CGSizeZero" => {
                push_f32(&mut bytes, 0.0);
                push_f32(&mut bytes, 0.0);
                (HLE_EXTERN_DATA_CGSIZE_ZERO, "CGSize{0,0}")
            }
            "CGRectZero" => {
                push_f32(&mut bytes, 0.0);
                push_f32(&mut bytes, 0.0);
                push_f32(&mut bytes, 0.0);
                push_f32(&mut bytes, 0.0);
                (HLE_EXTERN_DATA_CGRECT_ZERO, "CGRect{{0,0},{0,0}}")
            }
            "CGAffineTransformIdentity" => {
                push_f32(&mut bytes, 1.0);
                push_f32(&mut bytes, 0.0);
                push_f32(&mut bytes, 0.0);
                push_f32(&mut bytes, 1.0);
                push_f32(&mut bytes, 0.0);
                push_f32(&mut bytes, 0.0);
                (HLE_EXTERN_DATA_CGAFFINE_TRANSFORM_IDENTITY, "CGAffineTransformIdentity")
            }
            "UIEdgeInsetsZero" => {
                push_f32(&mut bytes, 0.0);
                push_f32(&mut bytes, 0.0);
                push_f32(&mut bytes, 0.0);
                push_f32(&mut bytes, 0.0);
                (HLE_EXTERN_DATA_UIEDGEINSETS_ZERO, "UIEdgeInsets{0,0,0,0}")
            }
            _ => return None,
        };
        Some((addr, bytes, note))
    }

    fn materialize_data_externals<B: CpuBackend>(
        &self,
        runtime: &mut CoreRuntime<B>,
        report: &mut ImageLoadReport,
    ) -> CoreResult<()> {
        for symbol in [
            "CGPointZero",
            "CGSizeZero",
            "CGRectZero",
            "CGAffineTransformIdentity",
            "UIEdgeInsetsZero",
        ] {
            let Some((addr, bytes, note)) = Self::data_external_binding(symbol) else {
                continue;
            };
            runtime.write_guest(addr, &bytes, format!("extern_data:{symbol}"))?;
            runtime.stubs_mut().insert(addr, symbol.to_string());
            report.bytes_written += bytes.len() as u64;
            report.write_records.push(MemoryWriteRecord {
                address: addr,
                size: bytes.len() as u32,
                kind: "extern_data".to_string(),
                symbol: Some(symbol.to_string()),
                note: Some(note.to_string()),
            });
        }
        Ok(())
    }

    fn bind_imports<B: CpuBackend>(
        &self,
        runtime: &mut CoreRuntime<B>,
        probe: &IpaProbe,
        report: &mut ImageLoadReport,
        warnings: &mut Vec<String>,
    ) -> CoreResult<()> {
        let mut requested = BTreeSet::new();
        for symbol in &probe.mach.undefined_symbols {
            requested.insert(symbol.clone());
        }
        for ptr in &probe.mach.indirect_pointers {
            requested.insert(ptr.symbol.clone());
        }
        for reloc in &probe.mach.external_relocations {
            requested.insert(reloc.symbol.clone());
        }

        for symbol in requested {
            if let Some((addr, _, _)) = Self::data_external_binding(&symbol) {
                runtime.stubs_mut().insert(addr, symbol.clone());
                continue;
            }
            if let Err(err) = runtime.stubs_mut().ensure_symbol(&symbol) {
                report.unresolved_symbols.push(symbol.clone());
                warnings.push(format!("failed to assign stub for {symbol}: {err}"));
            }
        }
        report.stubbed_symbols = runtime.stubs_mut().len();

        for ptr in &probe.mach.indirect_pointers {
            match ptr.kind {
                IndirectPointerKind::Stub => continue,
                IndirectPointerKind::Lazy | IndirectPointerKind::NonLazy => {}
            }
            let Some(target) = runtime.stubs_mut().lookup_symbol(&ptr.symbol) else {
                continue;
            };
            match runtime.write_u32(ptr.address, target, format!("bind:{}", ptr.symbol)) {
                Ok(()) => {
                    report.indirect_pointer_writes += 1;
                    report.bytes_written += 4;
                    report.write_records.push(MemoryWriteRecord {
                        address: ptr.address,
                        size: 4,
                        kind: "indirect_pointer".to_string(),
                        symbol: Some(ptr.symbol.clone()),
                        note: Some(match ptr.kind {
                            IndirectPointerKind::Lazy => "lazy symbol pointer".to_string(),
                            IndirectPointerKind::NonLazy => "non-lazy symbol pointer".to_string(),
                            IndirectPointerKind::Stub => "symbol stub".to_string(),
                        }),
                    });
                }
                Err(err) => warnings.push(format!(
                    "failed to write indirect pointer for {} at 0x{:08x}: {}",
                    ptr.symbol, ptr.address, err
                )),
            }
        }

        for reloc in &probe.mach.external_relocations {
            let Some(target) = runtime.stubs_mut().lookup_symbol(&reloc.symbol) else {
                continue;
            };
            match runtime.write_u32(reloc.address, target, format!("extrel:{}", reloc.symbol)) {
                Ok(()) => {
                    report.external_relocation_writes += 1;
                    report.bytes_written += 4;
                    report.write_records.push(MemoryWriteRecord {
                        address: reloc.address,
                        size: 4,
                        kind: "external_relocation".to_string(),
                        symbol: Some(reloc.symbol.clone()),
                        note: Some(format!(
                            "pcrel={} length={} type={}",
                            reloc.pcrel, reloc.length, reloc.rtype
                        )),
                    });
                }
                Err(err) => warnings.push(format!(
                    "failed to apply external relocation for {} at 0x{:08x}: {}",
                    reloc.symbol, reloc.address, err
                )),
            }
        }

        Ok(())
    }
}

pub fn plan_bootstrap(
    probe: &IpaProbe,
    macho_slice: &[u8],
    cfg: CoreConfig,
) -> CoreResult<(BootstrapPlan, RuntimeReport)> {
    BootstrapPlanner::new(cfg).plan(probe, macho_slice)
}

fn build_image_region(seg: &SegmentInfo, page_size: u32) -> CoreResult<PlannedRegion> {
    let seg_end = seg
        .vmaddr
        .checked_add(seg.vmsize)
        .ok_or_else(|| CoreError::Memory(format!("segment {} overflows guest address space", seg.name)))?;
    let start = align_down(seg.vmaddr, page_size);
    let end = align_up_checked(seg_end, page_size)?;
    let size = end
        .checked_sub(start)
        .ok_or_else(|| CoreError::Memory(format!("segment {} produced negative size", seg.name)))?;
    let loaded_bytes = seg.filesize.min(seg.vmsize);
    let zero_fill_bytes = seg.vmsize.saturating_sub(seg.filesize);

    Ok(PlannedRegion {
        name: seg.name.clone(),
        kind: "image".to_string(),
        start,
        end,
        size,
        prot: prot_to_string(mach_prot_to_guest(seg.initprot)),
        fileoff: Some(seg.fileoff),
        filesize: Some(seg.filesize),
        loaded_bytes: Some(loaded_bytes),
        zero_fill_bytes: Some(zero_fill_bytes),
        note: None,
    })
}

fn simple_region(
    name: &str,
    kind: &str,
    start: u32,
    size: u32,
    prot: u32,
    note: Option<String>,
) -> CoreResult<PlannedRegion> {
    let end = start
        .checked_add(size)
        .ok_or_else(|| CoreError::Memory(format!("synthetic region {name} overflows guest address space")))?;
    Ok(PlannedRegion {
        name: name.to_string(),
        kind: kind.to_string(),
        start,
        end,
        size,
        prot: prot_to_string(prot),
        fileoff: None,
        filesize: None,
        loaded_bytes: None,
        zero_fill_bytes: None,
        note,
    })
}

fn build_initial_stack_layout(cfg: &CoreConfig, argv0: &str) -> CoreResult<StackBootstrap> {
    let top = cfg
        .stack_base
        .checked_add(cfg.stack_size)
        .ok_or_else(|| CoreError::Memory("stack top overflow".into()))?;
    let mut p = top
        .checked_sub(0x1000)
        .ok_or_else(|| CoreError::Memory("stack reservation underflow".into()))?;

    let string_len = argv0.len() as u32 + 1;
    p = p
        .checked_sub(align4(string_len))
        .ok_or_else(|| CoreError::Memory("argv0 placement underflow".into()))?;
    let argv0_addr = p;

    p = p
        .checked_sub(8)
        .ok_or_else(|| CoreError::Memory("argv vector underflow".into()))?;
    let argv_ptr = p;

    p = p
        .checked_sub(4)
        .ok_or_else(|| CoreError::Memory("argc slot underflow".into()))?;
    let sp = p;

    Ok(StackBootstrap {
        argv0: argv0.to_string(),
        argv0_addr,
        argv_ptr,
        argc: 1,
        sp,
    })
}

fn read_segment_bytes(macho_slice: &[u8], seg: &SegmentInfo) -> CoreResult<Vec<u8>> {
    let fileoff = seg.fileoff as usize;
    let filesize = seg.filesize as usize;
    let end = fileoff
        .checked_add(filesize)
        .ok_or_else(|| CoreError::InvalidImage(format!("segment {} file range overflow", seg.name)))?;
    if end > macho_slice.len() {
        return Err(CoreError::InvalidImage(format!(
            "segment {} points outside the selected Mach-O slice (off=0x{:x} size=0x{:x} len=0x{:x})",
            seg.name,
            fileoff,
            filesize,
            macho_slice.len()
        )));
    }
    Ok(macho_slice[fileoff..end].to_vec())
}

fn align4(x: u32) -> u32 {
    (x + 3) & !3
}
