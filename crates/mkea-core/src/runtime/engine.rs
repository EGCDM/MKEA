use std::{collections::{HashMap, HashSet, VecDeque}, fs, io::{Cursor, Read}, path::{Path, PathBuf}};

use flate2::{read::{DeflateDecoder, ZlibDecoder}, write::ZlibEncoder as FlateZlibEncoder, Compression};
use png::{BitDepth, ColorType, Encoder};
use serde::Deserialize;

use mkea_loader::SectionInfo;

use super::{backend::{BackendSnapshot, CpuBackend}, diagnostics::*, drain_live_input, framebus::publish_live_frame, framebus::LiveFramePacket, inputbus::LiveInputPacket, is_stop_requested, profiles, render::decide_frame_source, render::RenderFrameSource, synthetic::{BackendTuning, RuntimeSyntheticConfigReport}, telemetry::note_runloop_tick};

use crate::{
    config::{CoreConfig, RuntimeMode},
    error::{CoreError, CoreResult},
    runtime::{
        align_down, align_up_checked, mach_prot_to_guest, prot_to_string, GuestMemory,
        MemoryRegion, StubRegistry, GUEST_PROT_EXEC, GUEST_PROT_READ,
        GUEST_PROT_WRITE,
    },
    types::{
        EntryPoint, InitialRegisters,
    },
};


pub(super) const ARM_BX_LR: [u8; 4] = [0x1e, 0xff, 0x2f, 0xe1];
const SYNTHETIC_HEAP_GUARD_BYTES: u32 = 16;
const HLE_FAKE_UIAPPLICATION: u32 = 0x6fff03a0;
const HLE_FAKE_APP_DELEGATE: u32 = 0x6fff03c0;
const HLE_FAKE_UIWINDOW: u32 = 0x6fff03e0;
const HLE_FAKE_ROOT_CONTROLLER: u32 = 0x6fff0400;
const HLE_FAKE_MAIN_SCREEN: u32 = 0x6fff0420;
const HLE_FAKE_MAIN_RUNLOOP: u32 = 0x6fff0440;
const HLE_FAKE_DEFAULT_MODE: u32 = 0x6fff0460;
const HLE_FAKE_SYNTH_TIMER: u32 = 0x6fff0480;
const HLE_FAKE_SYNTH_DISPLAYLINK: u32 = 0x6fff04a0;
const HLE_FAKE_REACHABILITY: u32 = 0x6fff04c0;
const HLE_FAKE_URL: u32 = 0x6fff04e0;
const HLE_FAKE_URL_REQUEST: u32 = 0x6fff0500;
const HLE_FAKE_URL_CONNECTION: u32 = 0x6fff0520;
const HLE_FAKE_HTTP_RESPONSE: u32 = 0x6fff0540;
const HLE_FAKE_DATA: u32 = 0x6fff0560;
const HLE_FAKE_PROXY_SETTINGS: u32 = 0x6fff0580;
const HLE_FAKE_FAULT_CONNECTION: u32 = 0x6fff05a0;
const HLE_FAKE_NETWORK_ERROR: u32 = 0x6fff05c0;
const HLE_FAKE_NSSTRING_URL_ABSOLUTE: u32 = 0x6fff05e0;
const HLE_FAKE_NSSTRING_URL_HOST: u32 = 0x6fff0600;
const HLE_FAKE_NSSTRING_URL_PATH: u32 = 0x6fff0620;
const HLE_FAKE_NSSTRING_HTTP_METHOD: u32 = 0x6fff0640;
const HLE_FAKE_NSSTRING_MIME_TYPE: u32 = 0x6fff0660;
const HLE_FAKE_NSSTRING_ERROR_DOMAIN: u32 = 0x6fff0680;
const HLE_FAKE_NSSTRING_ERROR_DESCRIPTION: u32 = 0x6fff06a0;
const HLE_FAKE_READ_STREAM: u32 = 0x6fff06c0;
const HLE_FAKE_WRITE_STREAM: u32 = 0x6fff06e0;
const HLE_FAKE_EAGL_CONTEXT: u32 = 0x6fff0700;
const HLE_FAKE_CAEAGL_LAYER: u32 = 0x6fff0720;
const HLE_FAKE_GL_FRAMEBUFFER: u32 = 0x6fff0740;
const HLE_FAKE_GL_RENDERBUFFER: u32 = 0x6fff0760;
const HLE_FAKE_UIGRAPHICS_CONTEXT: u32 = 0x6fff0780;
const HLE_FAKE_UIIMAGE: u32 = 0x6fff07a0;
const HLE_FAKE_MAIN_BUNDLE: u32 = 0x6fff07c0;
pub(super) const HLE_STUB_AUDIOQUEUE_CALLBACK_RETURN_ARM: u32 = 0x6fff0fc0;
pub(super) const HLE_STUB_AUDIOQUEUE_CALLBACK_RETURN_THUMB: u32 = 0x6fff0fd0;
pub(super) const HLE_STUB_UIAPPLICATION_POST_LAUNCH_ARM: u32 = 0x6fff0fe0;
pub(super) const HLE_STUB_UIAPPLICATION_POST_LAUNCH_THUMB: u32 = 0x6fff0ff0;

pub(super) const HLE_EXTERN_DATA_BASE: u32 = 0x6ffef000;
pub(super) const HLE_EXTERN_DATA_CGPOINT_ZERO: u32 = HLE_EXTERN_DATA_BASE + 0x000;
pub(super) const HLE_EXTERN_DATA_CGSIZE_ZERO: u32 = HLE_EXTERN_DATA_BASE + 0x010;
pub(super) const HLE_EXTERN_DATA_CGRECT_ZERO: u32 = HLE_EXTERN_DATA_BASE + 0x020;
pub(super) const HLE_EXTERN_DATA_CGAFFINE_TRANSFORM_IDENTITY: u32 = HLE_EXTERN_DATA_BASE + 0x040;
pub(super) const HLE_EXTERN_DATA_UIEDGEINSETS_ZERO: u32 = HLE_EXTERN_DATA_BASE + 0x060;

const GL_VERTEX_ARRAY: u32 = 0x8074;
const GL_COLOR_ARRAY: u32 = 0x8076;
const GL_TEXTURE_COORD_ARRAY: u32 = 0x8078;
const GL_MODELVIEW: u32 = 0x1700;
const GL_PROJECTION: u32 = 0x1701;
const GL_TEXTURE: u32 = 0x1702;

const GL_BYTE: u32 = 0x1400;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_SHORT: u32 = 0x1402;
const GL_UNSIGNED_SHORT: u32 = 0x1403;
const GL_FLOAT: u32 = 0x1406;
const GL_FIXED: u32 = 0x140C;
const GL_UNSIGNED_SHORT_4_4_4_4: u32 = 0x8033;
const GL_UNSIGNED_SHORT_5_5_5_1: u32 = 0x8034;
const GL_UNSIGNED_SHORT_5_6_5: u32 = 0x8363;

const GL_POINTS: u32 = 0x0000;
const GL_LINES: u32 = 0x0001;
const GL_LINE_LOOP: u32 = 0x0002;
const GL_LINE_STRIP: u32 = 0x0003;
const GL_TRIANGLES: u32 = 0x0004;
const GL_TRIANGLE_STRIP: u32 = 0x0005;
const GL_TRIANGLE_FAN: u32 = 0x0006;

const GL_ZERO: u32 = 0;
const GL_ONE: u32 = 1;
const GL_SRC_ALPHA: u32 = 0x0302;
const GL_ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_BLEND: u32 = 0x0BE2;
const GL_ALPHA: u32 = 0x1906;
const GL_RGB: u32 = 0x1907;
const GL_RGBA: u32 = 0x1908;
const GL_LUMINANCE: u32 = 0x1909;
const GL_LUMINANCE_ALPHA: u32 = 0x190A;
const GL_TEXTURE_ENV: u32 = 0x2300;
const GL_TEXTURE_ENV_MODE: u32 = 0x2200;
const GL_MODULATE: u32 = 0x2100;
const GL_REPLACE: u32 = 0x1E01;
const GL_NEAREST: u32 = 0x2600;
const GL_LINEAR: u32 = 0x2601;
const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_TEXTURE_WRAP_S: u32 = 0x2802;
const GL_TEXTURE_WRAP_T: u32 = 0x2803;
const GL_CLAMP: u32 = 0x2900;
const GL_REPEAT: u32 = 0x2901;
const GL_CLAMP_TO_EDGE: u32 = 0x812F;


fn gl_identity_mat4() -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum GraphicsMatrixMode {
    #[default]
    ModelView,
    Projection,
    Texture,
}

impl GraphicsMatrixMode {
    pub(crate) fn from_gl(value: u32) -> Option<Self> {
        match value {
            GL_MODELVIEW => Some(Self::ModelView),
            GL_PROJECTION => Some(Self::Projection),
            GL_TEXTURE => Some(Self::Texture),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ModelView => "modelview",
            Self::Projection => "projection",
            Self::Texture => "texture",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GraphicsMatrixStackState {
    pub(crate) current_mode: GraphicsMatrixMode,
    pub(crate) modelview_stack: Vec<[f32; 16]>,
    pub(crate) projection_stack: Vec<[f32; 16]>,
    pub(crate) texture_stack: Vec<[f32; 16]>,
    pub(crate) modelview_touched: bool,
    pub(crate) projection_touched: bool,
    pub(crate) texture_touched: bool,
    pub(crate) op_count: u32,
}

impl Default for GraphicsMatrixStackState {
    fn default() -> Self {
        Self {
            current_mode: GraphicsMatrixMode::ModelView,
            modelview_stack: vec![gl_identity_mat4()],
            projection_stack: vec![gl_identity_mat4()],
            texture_stack: vec![gl_identity_mat4()],
            modelview_touched: false,
            projection_touched: false,
            texture_touched: false,
            op_count: 0,
        }
    }
}

include!("backend/memory/uikit_state.rs");


#[derive(Debug, Clone)]
struct SyntheticStringBacking {
    ptr: u32,
    len: u32,
    text: String,
    font_name: Option<String>,
    font_size_bits: u32,
    font_size_explicit: bool,
}

#[derive(Debug, Clone)]
struct SyntheticBlobBacking {
    ptr: u32,
    len: u32,
    preview_ascii: String,
}

#[derive(Debug, Clone)]
struct SyntheticHeapAllocation {
    ptr: u32,
    size: u32,
    reserved_size: u32,
    freed: bool,
    tag: String,
}

#[derive(Debug, Clone, Copy)]
struct ObjcSectionRange {
    addr: u32,
    size: u32,
}

impl ObjcSectionRange {
    fn contains(&self, addr: u32) -> bool {
        let end = self.addr.saturating_add(self.size);
        addr >= self.addr && addr < end
    }
}

#[derive(Debug, Clone)]
struct ObjcClassInfo {
    cls: u32,
    isa: u32,
    superclass: u32,
    ro: u32,
    name: String,
    instance_size: u32,
    methods: HashMap<String, u32>,
    meta_methods: HashMap<String, u32>,
    ivars: HashMap<String, u32>,
}

#[derive(Debug, Clone)]
struct UimainBridgePlan {
    receiver: u32,
    selector_name: String,
    selector_ptr: u32,
    imp: u32,
    return_stub: u32,
    delegate_name: Option<String>,
    delegate_class_name: String,
}


pub struct CoreRuntime<B: CpuBackend> {
    cfg: CoreConfig,
    backend: B,
    memory: GuestMemory,
    stubs: StubRegistry,
}

impl<B: CpuBackend> CoreRuntime<B> {
    pub fn new(cfg: CoreConfig, backend: B) -> Self {
        Self {
            cfg,
            backend,
            memory: GuestMemory::default(),
            stubs: StubRegistry::default(),
        }
    }

    pub fn memory_mut(&mut self) -> &mut GuestMemory {
        &mut self.memory
    }


    pub fn stubs_mut(&mut self) -> &mut StubRegistry {
        &mut self.stubs
    }

    pub fn map_region(&mut self, region: MemoryRegion) -> CoreResult<()> {
        let addr = region.addr;
        let size = region.size;
        let prot = region.prot;
        self.memory.register_region(region)?;
        self.backend.map(addr, size, prot)?;
        Ok(())
    }

    pub fn write_guest(&mut self, addr: u32, data: &[u8], kind: impl Into<String>) -> CoreResult<()> {
        self.memory.write_bytes(addr, data.len() as u32, kind)?;
        self.backend.write_mem(addr, data)?;
        Ok(())
    }

    pub fn write_u32(&mut self, addr: u32, value: u32, kind: impl Into<String>) -> CoreResult<()> {
        self.write_guest(addr, &value.to_le_bytes(), kind)
    }

    pub fn install_entry(&mut self, pc: u32, sp: u32, thumb: bool) -> CoreResult<()> {
        self.backend.set_pc(pc, thumb)?;
        self.backend.set_sp(sp)?;
        Ok(())
    }

    pub fn install_initial_registers(&mut self, regs: &InitialRegisters) -> CoreResult<()> {
        self.backend.set_initial_registers(regs)
    }

    pub fn sync_stub_symbols_to_backend(&mut self) -> CoreResult<()> {
        for binding in self.stubs.bindings() {
            self.backend.install_symbol_label(binding.address, &binding.symbol)?;
        }
        Ok(())
    }

    pub fn run(mut self, entry: EntryPoint) -> CoreResult<RuntimeReport> {
        self.backend.run(self.cfg.max_instructions)?;
        let snapshot = self.backend.snapshot();
        Ok(RuntimeReport {
            backend: snapshot.backend,
            mapped_regions: self.memory.region_count(),
            registered_stubs: self.stubs.len(),
            entry_pc: entry.pc,
            initial_sp: entry.sp,
            memory_writes: self.memory.writes().len(),
            first_instruction_addr: snapshot.first_instruction_addr,
            first_instruction: snapshot.first_instruction,
            first_instruction_text: snapshot.first_instruction_text,
            entry_bytes_present: snapshot.entry_bytes_present,
            executed_instructions: snapshot.executed_instructions,
            final_pc: snapshot.final_pc,
            final_sp: snapshot.final_sp,
            final_lr: snapshot.final_lr,
            stop_reason: snapshot.stop_reason,
            trace: snapshot.trace,
            status: snapshot.status,
            runtime_state: snapshot.runtime_state,
            backend_execution: snapshot.backend_execution,
        })
    }
}


#[derive(Debug, Clone)]
pub(crate) struct BackendRegion {
    pub(crate) addr: u32,
    pub(crate) size: u32,
    pub(crate) prot: u32,
    pub(crate) data: Vec<u8>,
}

impl BackendRegion {
    fn end(&self) -> u32 {
        self.addr.saturating_add(self.size)
    }

    fn contains_range(&self, addr: u32, size: u32) -> bool {
        let Some(end) = addr.checked_add(size) else {
            return false;
        };
        addr >= self.addr && end <= self.end()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ArmFlags {
    pub(crate) n: bool,
    pub(crate) z: bool,
    pub(crate) c: bool,
    pub(crate) v: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct GlClientArrayState {
    enabled: bool,
    size: u32,
    ty: u32,
    stride: u32,
    ptr: u32,
}

impl GlClientArrayState {
    fn configured(&self) -> bool {
        self.ptr != 0 && self.size != 0
    }

    fn element_stride_bytes(&self) -> u32 {
        let elem_size = self.size.saturating_mul(MemoryArm32Backend::gl_type_size(self.ty));
        if self.stride == 0 { elem_size } else { self.stride.max(elem_size) }
    }
}

#[derive(Debug, Clone)]
struct SyntheticBitmapContext {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    fill_rgba: [u8; 4],
}

#[derive(Debug, Clone)]
struct SyntheticImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn sample_rgba_fingerprint(src: &[u8], width: u32, height: u32) -> String {
    if src.len() < 4 || width == 0 || height == 0 {
        return "empty".to_string();
    }
    let coords = [
        (0u32, 0u32, "tl"),
        (width / 2, height / 2, "c"),
        (width.saturating_sub(1), height.saturating_sub(1), "br"),
        (width / 4, height / 4, "q1"),
        ((width.saturating_mul(3)) / 4, (height.saturating_mul(3)) / 4, "q3"),
    ];
    let mut parts = Vec::new();
    for (x, y, tag) in coords {
        let idx = ((y as usize)
            .saturating_mul(width as usize)
            .saturating_add(x as usize))
            .saturating_mul(4);
        if idx + 3 >= src.len() {
            continue;
        }
        parts.push(format!(
            "{}={}/{}/{}/{}",
            tag, src[idx], src[idx + 1], src[idx + 2], src[idx + 3]
        ));
    }
    let mut sum_r = 0u64;
    let mut sum_g = 0u64;
    let mut sum_b = 0u64;
    let mut sum_a = 0u64;
    let mut count = 0u64;
    for px in src.chunks_exact(4) {
        sum_r = sum_r.saturating_add(px[0] as u64);
        sum_g = sum_g.saturating_add(px[1] as u64);
        sum_b = sum_b.saturating_add(px[2] as u64);
        sum_a = sum_a.saturating_add(px[3] as u64);
        count = count.saturating_add(1);
    }
    if count != 0 {
        parts.push(format!(
            "avg={:.1}/{:.1}/{:.1}/{:.1}",
            sum_r as f64 / count as f64,
            sum_g as f64 / count as f64,
            sum_b as f64 / count as f64,
            sum_a as f64 / count as f64,
        ));
    }
    parts.join(" ")
}

fn sample_framebuffer_region_fingerprint(
    src: &[u8],
    buffer_w: u32,
    buffer_h: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> String {
    if src.len() < 4 || buffer_w == 0 || buffer_h == 0 || width == 0 || height == 0 {
        return "empty".to_string();
    }
    let start_x = x.max(0) as u32;
    let start_y = y.max(0) as u32;
    let end_x = (x.saturating_add(width as i32)).max(0) as u32;
    let end_y = (y.saturating_add(height as i32)).max(0) as u32;
    let clamped_end_x = end_x.min(buffer_w);
    let clamped_end_y = end_y.min(buffer_h);
    if start_x >= clamped_end_x || start_y >= clamped_end_y {
        return "empty".to_string();
    }
    let region_w = clamped_end_x - start_x;
    let region_h = clamped_end_y - start_y;
    let mut cropped = vec![0u8; region_w as usize * region_h as usize * 4];
    for row in 0..region_h {
        let src_idx = (((start_y + row) * buffer_w + start_x) * 4) as usize;
        let dst_idx = (row * region_w * 4) as usize;
        let span = (region_w * 4) as usize;
        cropped[dst_idx..dst_idx + span].copy_from_slice(&src[src_idx..src_idx + span]);
    }
    sample_rgba_fingerprint(&cropped, region_w.max(1), region_h.max(1))
}

#[derive(Debug, Clone, Default)]
struct SyntheticDictionary {
    entries: HashMap<String, u32>,
}

#[derive(Debug, Clone, Default)]
struct SyntheticArray {
    items: Vec<u32>,
    mutation_count: u32,
}

#[derive(Debug, Clone)]
struct SyntheticTexture {
    width: u32,
    height: u32,
    gl_name: u32,
    has_premultiplied_alpha: bool,
    image: u32,
    source_key: String,
    source_path: String,
    cache_key: String,
}

#[derive(Debug, Clone)]
struct GuestGlTextureObject {
    target: u32,
    width: u32,
    height: u32,
    internal_format: u32,
    format: u32,
    ty: u32,
    min_filter: u32,
    mag_filter: u32,
    wrap_s: u32,
    wrap_t: u32,
    pixels_rgba: Vec<u8>,
    upload_count: u32,
}

impl Default for GuestGlTextureObject {
    fn default() -> Self {
        Self {
            target: GL_TEXTURE_2D,
            width: 0,
            height: 0,
            internal_format: GL_RGBA,
            format: GL_RGBA,
            ty: GL_UNSIGNED_BYTE,
            min_filter: GL_LINEAR,
            mag_filter: GL_LINEAR,
            wrap_s: GL_CLAMP_TO_EDGE,
            wrap_t: GL_CLAMP_TO_EDGE,
            pixels_rgba: Vec::new(),
            upload_count: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct SyntheticTextureAtlasState {
    texture: u32,
    capacity: u32,
    total_quads: u32,
    quad_buffer_ptr: u32,
    quad_stride: u32,
    update_count: u32,
    invalid_update_count: u32,
    last_index: Option<u32>,
    max_index_seen: Option<u32>,
}

#[derive(Debug, Clone)]
struct SyntheticSpriteState {
    texture: u32,
    width: u32,
    height: u32,
    anchor_x_bits: u32,
    anchor_y_bits: u32,
    anchor_explicit: bool,
    anchor_pixels_x_bits: u32,
    anchor_pixels_y_bits: u32,
    anchor_pixels_explicit: bool,
    position_x_bits: u32,
    position_y_bits: u32,
    position_bl_x_bits: u32,
    position_bl_y_bits: u32,
    position_bl_explicit: bool,
    scale_x_bits: u32,
    scale_y_bits: u32,
    scale_explicit: bool,
    last_guest_scale_tick: u32,
    texture_rect_x_bits: u32,
    texture_rect_y_bits: u32,
    texture_rect_w_bits: u32,
    texture_rect_h_bits: u32,
    texture_rect_explicit: bool,
    untrimmed_w_bits: u32,
    untrimmed_h_bits: u32,
    untrimmed_explicit: bool,
    offset_x_bits: u32,
    offset_y_bits: u32,
    offset_explicit: bool,
    flip_x: bool,
    flip_y: bool,
    fill_rgba: [u8; 4],
    fill_rgba_explicit: bool,
    relative_anchor_point: bool,
    relative_anchor_point_explicit: bool,
    parent: u32,
    children: u32,
    z_order: i32,
    tag: u32,
    visible: bool,
    touch_enabled: bool,
    entered: bool,
    callback_target: u32,
    callback_selector: u32,
    animation_dictionary: u32,
    last_display_frame_key: u32,
    last_display_frame_index: u32,
    guest_graph_observed: bool,
    content_revision: u32,
}

impl Default for SyntheticSpriteState {
    fn default() -> Self {
        Self {
            texture: 0,
            width: 0,
            height: 0,
            anchor_x_bits: 0,
            anchor_y_bits: 0,
            anchor_explicit: false,
            anchor_pixels_x_bits: 0,
            anchor_pixels_y_bits: 0,
            anchor_pixels_explicit: false,
            position_x_bits: 0,
            position_y_bits: 0,
            position_bl_x_bits: 0,
            position_bl_y_bits: 0,
            position_bl_explicit: false,
            scale_x_bits: 0,
            scale_y_bits: 0,
            scale_explicit: false,
            last_guest_scale_tick: 0,
            texture_rect_x_bits: 0,
            texture_rect_y_bits: 0,
            texture_rect_w_bits: 0,
            texture_rect_h_bits: 0,
            texture_rect_explicit: false,
            untrimmed_w_bits: 0,
            untrimmed_h_bits: 0,
            untrimmed_explicit: false,
            offset_x_bits: 0,
            offset_y_bits: 0,
            offset_explicit: false,
            flip_x: false,
            flip_y: false,
            fill_rgba: [0, 0, 0, 0],
            fill_rgba_explicit: false,
            relative_anchor_point: false,
            relative_anchor_point_explicit: false,
            parent: 0,
            children: 0,
            z_order: 0,
            tag: 0,
            visible: true,
            touch_enabled: false,
            entered: false,
            callback_target: 0,
            callback_selector: 0,
            animation_dictionary: 0,
            last_display_frame_key: 0,
            last_display_frame_index: 0,
            guest_graph_observed: false,
            content_revision: 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ScriptedPointerEvent {
    phase: String,
    px: f32,
    py: f32,
    #[serde(default)]
    pointer_id: u32,
    #[serde(default)]
    button: u32,
    #[serde(default)]
    host_width: Option<u32>,
    #[serde(default)]
    host_height: Option<u32>,
    #[serde(default)]
    flip_y: Option<bool>,
    #[serde(default)]
    source: Option<String>,
}

impl From<LiveInputPacket> for ScriptedPointerEvent {
    fn from(packet: LiveInputPacket) -> Self {
        Self {
            phase: packet.phase,
            px: packet.px,
            py: packet.py,
            pointer_id: packet.pointer_id,
            button: packet.button.unwrap_or(0),
            host_width: packet.host_width,
            host_height: packet.host_height,
            flip_y: packet.flip_y,
            source: packet.source,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyntheticUiTouchPhase {
    Began,
    Moved,
    Stationary,
    Ended,
    Cancelled,
}

impl SyntheticUiTouchPhase {
    fn from_host_phase(phase: &str) -> Self {
        match phase {
            "down" => Self::Began,
            "move" => Self::Moved,
            "up" => Self::Ended,
            _ => Self::Stationary,
        }
    }

    fn as_uikit_value(self) -> u32 {
        match self {
            Self::Began => 0,
            Self::Moved => 1,
            Self::Stationary => 2,
            Self::Ended => 3,
            Self::Cancelled => 4,
        }
    }
}

#[derive(Debug, Clone)]
struct SyntheticUiTouchState {
    pointer_id: u32,
    phase: SyntheticUiTouchPhase,
    tap_count: u32,
    timestamp_secs: f64,
    window: u32,
    view: u32,
    hit_view: u32,
    began_x: f32,
    began_y: f32,
    previous_x: f32,
    previous_y: f32,
    current_x: f32,
    current_y: f32,
}

#[derive(Debug, Clone, Default)]
struct SyntheticUiEventState {
    touch_set: u32,
    primary_touch: u32,
    event_type: u32,
    event_subtype: u32,
}

#[derive(Debug, Clone, Default)]
struct SyntheticSetState {
    items: Vec<u32>,
    mutation_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivePointerDispatchKind {
    UIKit,
    Cocos,
    Hybrid,
}

impl ActivePointerDispatchKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::UIKit => "ui",
            Self::Cocos => "cocos",
            Self::Hybrid => "hybrid",
        }
    }
}

#[derive(Debug, Clone)]
struct ActivePointerTouch {
    pointer_id: u32,
    target: u32,
    callback_target: u32,
    callback_selector: u32,
    dispatch_kind: ActivePointerDispatchKind,
    touch_dispatch_target: u32,
    touch_hit_view: u32,
    ui_dispatch_target: u32,
    ui_hit_view: u32,
    cocos_dispatch_target: u32,
    touch_object: u32,
    touch_set: u32,
    event_object: u32,
    last_x: f32,
    last_y: f32,
    source: String,
}

#[derive(Debug, Clone, Default)]
struct AutoSceneVisitStats {
    nodes_seen: u32,
    nodes_drawn: u32,
    entered_no: u32,
    visible_no: u32,
    container_skip: u32,
    no_texture: u32,
    zero_size: u32,
    missing: u32,
    max_depth: u32,
}

#[derive(Debug, Clone)]
struct HostFileHandle {
    path: String,
    mode: String,
    data: Vec<u8>,
    pos: usize,
    eof: bool,
    error: bool,
}


#[derive(Debug, Clone)]
pub(crate) enum StepControl {
    Continue,
    Stop(String),
}

#[path = "backend/memory.rs"]
mod memory_backend;
pub use memory_backend::MemoryArm32Backend;

fn format_vfp_reg_list(start: u32, count: u32, double_precision: bool) -> String {
    if count == 0 {
        return String::new();
    }
    let prefix = if double_precision { 'd' } else { 's' };
    if count == 1 {
        format!("{{{}{}}}", prefix, start)
    } else {
        format!("{{{}{}-{}{} }}", prefix, start, prefix, start + count - 1).replace(" }", "}")
    }
}


fn format_exact_vfp_opcode_override(word: u32, c: &str) -> Option<String> {
    match word {
        0xed9f6a2c => Some(format!("vldr{c} {{s12}}, [pc, #176]")),
        0xedd37a00 => Some(format!("vldr{c} {{s15}}, [r3]")),
        _ => None,
    }
}

fn format_vfp_literal_single_transfer(word: u32, c: &str) -> Option<String> {
    let masked = word & 0xFF30_0E00;
    if masked != 0xED10_0A00 && masked != 0xED00_0A00 {
        return None;
    }
    let p = ((word >> 24) & 1) != 0;
    let u = ((word >> 23) & 1) != 0;
    let d = ((word >> 22) & 1) != 0;
    let w = ((word >> 21) & 1) != 0;
    let l = ((word >> 20) & 1) != 0;
    let rn = (word >> 16) & 0xF;
    let vd = (word >> 12) & 0xF;
    let double_precision = ((word >> 8) & 1) != 0;
    let imm8 = word & 0xFF;
    if !p || w {
        return None;
    }

    let reg_text = if double_precision {
        format!("{{d{}}}", ((d as u32) << 4) | vd)
    } else {
        format!("{{s{}}}", (vd << 1) | (d as u32))
    };
    let op = if l { "vldr" } else { "vstr" };
    let sign = if u { "" } else { "-" };
    let base = if rn == 15 { "pc".to_string() } else { format!("r{}", rn) };
    Some(format!("{op}{c} {reg_text}, [{base}, #{sign}{}]", imm8 * 4))
}

fn format_vfp_scalar_data_processing(word: u32, c: &str) -> Option<String> {
    let masked = word & 0x0FB0_0F50;
    let op = match masked {
        0x0E00_0A00 => "vmla.f32",
        0x0E20_0A00 => "vmul.f32",
        0x0E30_0A00 => "vadd.f32",
        0x0E30_0A40 => "vsub.f32",
        0x0E80_0A00 => "vdiv.f32",
        _ => return None,
    };
    if ((word >> 8) & 1) != 0 {
        return None;
    }
    let d = (word >> 22) & 1;
    let vd = (word >> 12) & 0xF;
    let n = (word >> 7) & 1;
    let vn = (word >> 16) & 0xF;
    let m = (word >> 5) & 1;
    let vm = word & 0xF;
    let sd = (vd << 1) | d;
    let sn = (vn << 1) | n;
    let sm = (vm << 1) | m;
    Some(format!("{}{c} s{}, s{}, s{}", op, sd, sn, sm))
}

fn format_vfp_vmov_arm_sreg(word: u32, c: &str) -> Option<String> {
    if (word & 0x0DA0_0E70) != 0x0C00_0A10 {
        return None;
    }
    let l = ((word >> 20) & 1) != 0;
    let n = (word >> 7) & 1;
    let vn = (word >> 16) & 0xF;
    let rt = (word >> 12) & 0xF;
    let sreg = (vn << 1) | n;
    if l {
        Some(format!("vmov{c} r{}, s{}", rt, sreg))
    } else {
        Some(format!("vmov{c} s{}, r{}", sreg, rt))
    }
}

fn format_vfp_vmov_scalar(word: u32, c: &str) -> Option<String> {
    if (word & 0x0FBF_0FD0) != 0x0EB0_0A40 {
        return None;
    }
    let d = (word >> 22) & 1;
    let vd = (word >> 12) & 0xF;
    let m = (word >> 5) & 1;
    let vm = word & 0xF;
    let sd = (vd << 1) | d;
    let sm = (vm << 1) | m;
    Some(format!("vmov.f32{c} s{}, s{}", sd, sm))
}

fn format_vfp_convert_between_float_int(word: u32, c: &str) -> Option<String> {
    let masked = word & 0x0FBF_0FD0;
    let op = match masked {
        0x0EB8_0A40 => "vcvt.f32.u32",
        0x0EB8_0AC0 => "vcvt.f32.s32",
        0x0EBC_0AC0 => "vcvt.u32.f32",
        0x0EBD_0AC0 => "vcvt.s32.f32",
        _ => return None,
    };
    if ((word >> 8) & 1) != 0 {
        return None;
    }
    let d = (word >> 22) & 1;
    let vd = (word >> 12) & 0xF;
    let m = (word >> 5) & 1;
    let vm = word & 0xF;
    let sd = (vd << 1) | d;
    let sm = (vm << 1) | m;
    Some(format!("{}{c} s{}, s{}", op, sd, sm))
}

fn format_vfp_compare(word: u32, c: &str) -> Option<String> {
    let masked = word & 0x0FBE_0FD0;
    let op = match masked {
        0x0EB4_0A40 => "vcmp.f32",
        0x0EB4_0AC0 => "vcmpe.f32",
        _ => return None,
    };
    let d = (word >> 22) & 1;
    let vd = (word >> 12) & 0xF;
    let m = (word >> 5) & 1;
    let vm = word & 0xF;
    let sd = (vd << 1) | d;
    if (word & 0x6F) == 0x40 {
        Some(format!("{}{c} s{}, #0", op, sd))
    } else {
        let sm = (vm << 1) | m;
        Some(format!("{}{c} s{}, s{}", op, sd, sm))
    }
}

fn format_vfp_vmrs(word: u32, c: &str) -> Option<String> {
    if word == 0xEEF1_FA10 {
        Some(format!("vmrs{c} APSR_nzcv, fpscr"))
    } else {
        None
    }
}

fn format_vfp_load_store_multiple(word: u32, c: &str) -> Option<String> {
    if (word & 0x0E00_0E00) != 0x0C00_0A00 {
        return None;
    }
    let p = ((word >> 24) & 1) != 0;
    let u = ((word >> 23) & 1) != 0;
    let d = ((word >> 22) & 1) != 0;
    let w = ((word >> 21) & 1) != 0;
    let l = ((word >> 20) & 1) != 0;
    let rn = (word >> 16) & 0xF;
    let vd = (word >> 12) & 0xF;
    let double_precision = ((word >> 8) & 1) != 0;
    let imm8 = word & 0xFF;

    let (start_reg, reg_count) = if double_precision {
        (((d as u32) << 4) | vd, imm8 / 2)
    } else {
        (((vd << 1) | (d as u32)), imm8)
    };
    let list = format_vfp_reg_list(start_reg, reg_count, double_precision);

    if !l && rn == 13 && p && !u && w {
        Some(format!("vpush{} {}", c, list))
    } else if l && rn == 13 && !p && u && w {
        Some(format!("vpop{} {}", c, list))
    } else {
        let op = if l { "vldm" } else { "vstm" };
        let mode = match (p, u) {
            (false, true) => "ia",
            (true, true) => "ib",
            (false, false) => "da",
            (true, false) => "db",
        };
        let bang = if w { "!" } else { "" };
        Some(format!("{}{mode}{} r{}{bang}, {}", op, c, rn, list))
    }
}

fn format_reg_list(reg_list: u32) -> String {
    let mut regs = Vec::new();
    let mut i = 0u32;
    while i < 16 {
        if (reg_list & (1 << i)) == 0 {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        while end + 1 < 16 && (reg_list & (1 << (end + 1))) != 0 {
            end += 1;
        }
        if start == end {
            regs.push(format!("r{}", start));
        } else {
            regs.push(format!("r{}-r{}", start, end));
        }
        i = end + 1;
    }
    regs.join(", ")
}


pub(crate) fn format_thumb_halfword(halfword: u16) -> String {
    if (halfword & 0xFF87) == 0x4700 {
        let rm = (halfword >> 3) & 0xF;
        format!("bx r{rm} [0x{halfword:04x}]")
    } else if (halfword & 0xFF87) == 0x4780 {
        let rm = (halfword >> 3) & 0xF;
        format!("blx r{rm} [0x{halfword:04x}]")
    } else if (halfword & 0xFE00) == 0xB400 {
        let r = ((halfword >> 8) & 1) != 0;
        let mut regs = String::new();
        let list = (halfword & 0xFF) as u32;
        regs.push_str(&format_reg_list(list));
        if r {
            if !regs.is_empty() {
                regs.push_str(", ");
            }
            regs.push_str("r14");
        }
        format!("push {{{}}} [0x{halfword:04x}]", regs)
    } else if (halfword & 0xFE00) == 0xBC00 {
        let p = ((halfword >> 8) & 1) != 0;
        let mut regs = String::new();
        let list = (halfword & 0xFF) as u32;
        regs.push_str(&format_reg_list(list));
        if p {
            if !regs.is_empty() {
                regs.push_str(", ");
            }
            regs.push_str("r15");
        }
        format!("pop {{{}}} [0x{halfword:04x}]", regs)
    } else if (halfword & 0xF800) == 0x2000 {
        let rd = (halfword >> 8) & 0x7;
        let imm = halfword & 0xFF;
        format!("movs r{rd}, #0x{imm:x} [0x{halfword:04x}]")
    } else if (halfword & 0xF800) == 0x2800 {
        let rn = (halfword >> 8) & 0x7;
        let imm = halfword & 0xFF;
        format!("cmp r{rn}, #0x{imm:x} [0x{halfword:04x}]")
    } else if (halfword & 0xF800) == 0x3000 {
        let rd = (halfword >> 8) & 0x7;
        let imm = halfword & 0xFF;
        format!("adds r{rd}, #0x{imm:x} [0x{halfword:04x}]")
    } else if (halfword & 0xF800) == 0x3800 {
        let rd = (halfword >> 8) & 0x7;
        let imm = halfword & 0xFF;
        format!("subs r{rd}, #0x{imm:x} [0x{halfword:04x}]")
    } else if (halfword & 0xF800) == 0x4800 {
        let rd = (halfword >> 8) & 0x7;
        let imm = halfword & 0xFF;
        format!("ldr r{rd}, [pc, #0x{:x}] [0x{halfword:04x}]", imm << 2)
    } else if (halfword & 0xFF80) == 0xB000 {
        let imm = (halfword & 0x7F) << 2;
        format!("add sp, #0x{imm:x} [0x{halfword:04x}]")
    } else if (halfword & 0xFF80) == 0xB080 {
        let imm = (halfword & 0x7F) << 2;
        format!("sub sp, #0x{imm:x} [0x{halfword:04x}]")
    } else if (halfword & 0xF800) == 0xE000 {
        let imm = halfword & 0x7FF;
        format!("b 0x{imm:03x} [0x{halfword:04x}]")
    } else {
        format!("thumb 0x{halfword:04x}")
    }
}

fn format_extra_load_store(word: u32, c: &str) -> Option<String> {
    if ((word >> 25) & 0x7) != 0 || (word & (1 << 7)) == 0 || (word & (1 << 4)) == 0 || ((word >> 5) & 0x3) == 0 {
        return None;
    }
    let p = ((word >> 24) & 1) != 0;
    let u = ((word >> 23) & 1) != 0;
    let i = ((word >> 22) & 1) != 0;
    let w = ((word >> 21) & 1) != 0;
    let l = ((word >> 20) & 1) != 0;
    let rn = (word >> 16) & 0xF;
    let rd = (word >> 12) & 0xF;
    let sh = (word >> 5) & 0x3;

    let op = match (l, sh) {
        (false, 0x1) => "strh",
        (true, 0x1) => "ldrh",
        (true, 0x2) => "ldrsb",
        (true, 0x3) => "ldrsh",
        _ => return None,
    };
    let off_txt = if i {
        let imm = (((word >> 8) & 0xF) << 4) | (word & 0xF);
        format!("#0x{:x}", imm)
    } else {
        format!("r{}", word & 0xF)
    };
    let sign = if u { "" } else { "-" };
    let bang = if w { "!" } else { "" };
    Some(if p {
        format!("{op}{c} r{rd}, [r{rn}, {sign}{off_txt}]{bang}")
    } else {
        format!("{op}{c} r{rd}, [r{rn}], {sign}{off_txt}")
    })
}

fn format_reg_shift_operand2(word: u32) -> Option<String> {
    if (word & (1 << 25)) != 0 || (word & (1 << 4)) == 0 {
        return None;
    }
    let rm = word & 0xF;
    let rs = (word >> 8) & 0xF;
    let shift_type = (word >> 5) & 0x3;
    let shift_name = match shift_type {
        0 => "lsl",
        1 => "lsr",
        2 => "asr",
        3 => "ror",
        _ => return None,
    };
    Some(format!("r{rm}, {shift_name} r{rs}"))
}

pub(crate) fn format_arm_word(word: u32) -> String {
    let cond = match (word >> 28) & 0xF {
        0x0 => "eq",
        0x1 => "ne",
        0x2 => "cs",
        0x3 => "cc",
        0x4 => "mi",
        0x5 => "pl",
        0x6 => "vs",
        0x7 => "vc",
        0x8 => "hi",
        0x9 => "ls",
        0xA => "ge",
        0xB => "lt",
        0xC => "gt",
        0xD => "le",
        0xE => "al",
        _ => "nv",
    };
    let c = if cond == "al" { "" } else { cond };

    let text = if (word & 0x0FFF_FFF0) == 0x012F_FF10 {
        let rm = word & 0xF;
        format!("bx{c} r{rm}")
    } else if (word & 0x0FFF_FFF0) == 0x012F_FF30 {
        let rm = word & 0xF;
        format!("blx{c} r{rm}")
    } else if (word & 0x0F00_0000) == 0x0B00_0000 {
        let imm24 = word & 0x00FF_FFFF;
        format!("bl{c} 0x{imm24:06x} (imm24)")
    } else if (word & 0x0F00_0000) == 0x0A00_0000 {
        let imm24 = word & 0x00FF_FFFF;
        format!("b{c} 0x{imm24:06x} (imm24)")
    } else if (word & 0x0FFF_03F0) == 0x06AF_0070 {
        let rd = (word >> 12) & 0xF;
        let rm = word & 0xF;
        let rotate = ((word >> 10) & 0x3) * 8;
        let suffix = if rotate == 0 {
            String::new()
        } else {
            format!(", ror #{}", rotate)
        };
        format!("sxtb{c} r{rd}, r{rm}{suffix}")
    } else if (word & 0x0FFF_03F0) == 0x06BF_0070 {
        let rd = (word >> 12) & 0xF;
        let rm = word & 0xF;
        let rotate = ((word >> 10) & 0x3) * 8;
        let suffix = if rotate == 0 {
            String::new()
        } else {
            format!(", ror #{}", rotate)
        };
        format!("sxth{c} r{rd}, r{rm}{suffix}")
    } else if (word & 0x0FFF_03F0) == 0x06EF_0070 {
        let rd = (word >> 12) & 0xF;
        let rm = word & 0xF;
        let rotate = ((word >> 10) & 0x3) * 8;
        let suffix = if rotate == 0 {
            String::new()
        } else {
            format!(", ror #{}", rotate)
        };
        format!("uxtb{c} r{rd}, r{rm}{suffix}")
    } else if (word & 0x0FFF_03F0) == 0x06FF_0070 {
        let rd = (word >> 12) & 0xF;
        let rm = word & 0xF;
        let rotate = ((word >> 10) & 0x3) * 8;
        let suffix = if rotate == 0 {
            String::new()
        } else {
            format!(", ror #{}", rotate)
        };
        format!("uxth{c} r{rd}, r{rm}{suffix}")
    } else if let Some(vfp_text) = format_exact_vfp_opcode_override(word, c) {
        vfp_text
    } else if let Some(vfp_text) = format_vfp_literal_single_transfer(word, c) {
        vfp_text
    } else if let Some(vfp_text) = format_vfp_scalar_data_processing(word, c) {
        vfp_text
    } else if let Some(vfp_text) = format_vfp_vmov_arm_sreg(word, c) {
        vfp_text
    } else if let Some(vfp_text) = format_vfp_vmov_scalar(word, c) {
        vfp_text
    } else if let Some(vfp_text) = format_vfp_convert_between_float_int(word, c) {
        vfp_text
    } else if let Some(vfp_text) = format_vfp_compare(word, c) {
        vfp_text
    } else if let Some(vfp_text) = format_vfp_vmrs(word, c) {
        vfp_text
    } else if let Some(vfp_text) = format_vfp_load_store_multiple(word, c) {
        vfp_text
    } else if let Some(extra_text) = format_extra_load_store(word, c) {
        extra_text
    } else if (word & 0x0E00_0000) == 0x0800_0000 {
        let p = ((word >> 24) & 1) != 0;
        let u = ((word >> 23) & 1) != 0;
        let w = ((word >> 21) & 1) != 0;
        let l = ((word >> 20) & 1) != 0;
        let rn = (word >> 16) & 0xF;
        let reg_list = word & 0xFFFF;
        let op = if l { "ldm" } else { "stm" };
        let mode = match (p, u) {
            (false, true) => "ia",
            (true, true) => "ib",
            (false, false) => "da",
            (true, false) => "db",
        };
        if !l && rn == 13 && p && !u && w {
            format!("push{c} {{{}}}", format_reg_list(reg_list))
        } else if l && rn == 13 && !p && u && w {
            format!("pop{c} {{{}}}", format_reg_list(reg_list))
        } else {
            let bang = if w { "!" } else { "" };
            format!("{op}{mode}{c} r{rn}{bang}, {{{}}}", format_reg_list(reg_list))
        }
    } else if (word & 0x0E50_0000) == 0x0410_0000 {
        let rn = (word >> 16) & 0xF;
        let rd = (word >> 12) & 0xF;
        let off = word & 0xFFF;
        if (word & (1 << 24)) == 0 {
            format!("ldr{c} r{rd}, [r{rn}], #0x{off:x}")
        } else {
            format!("ldr{c} r{rd}, [r{rn}, #0x{off:x}]")
        }
    } else if (word & 0x0E50_0000) == 0x0400_0000 {
        let rn = (word >> 16) & 0xF;
        let rd = (word >> 12) & 0xF;
        let off = word & 0xFFF;
        if (word & (1 << 24)) == 0 {
            format!("str{c} r{rd}, [r{rn}], #0x{off:x}")
        } else {
            format!("str{c} r{rd}, [r{rn}, #0x{off:x}]")
        }
    } else if (word & 0x0E50_0010) == 0x0610_0000 {
        let rn = (word >> 16) & 0xF;
        let rd = (word >> 12) & 0xF;
        let rm = word & 0xF;
        format!("ldr{c} r{rd}, [r{rn}, r{rm}]")
    } else if (word & 0x0FE0_0FF0) == 0x01A0_0000 {
        let rd = (word >> 12) & 0xF;
        let rm = word & 0xF;
        format!("mov{c} r{rd}, r{rm}")
    } else if (word & 0x0FE0_0010) == 0x0080_0010 {
        let rn = (word >> 16) & 0xF;
        let rd = (word >> 12) & 0xF;
        let rm = word & 0xF;
        let shift = (word >> 7) & 0x1F;
        let stype = (word >> 5) & 0x3;
        let shift_txt = match stype {
            0 => format!(", lsl #{}", shift),
            1 => format!(", lsr #{}", shift),
            2 => format!(", asr #{}", shift),
            3 => format!(", ror #{}", shift),
            _ => String::new(),
        };
        format!("add{c} r{rd}, r{rn}, r{rm}{shift_txt}")
    } else if (word & 0x0C00_0000) == 0x0000_0000 && (word & 0x0000_0010) != 0 {
        let opcode = (word >> 21) & 0xF;
        let rn = (word >> 16) & 0xF;
        let rd = (word >> 12) & 0xF;
        let op = match opcode {
            0x0 => "and",
            0x1 => "eor",
            0x2 => "sub",
            0x3 => "rsb",
            0x4 => "add",
            0x5 => "adc",
            0x6 => "sbc",
            0x7 => "rsc",
            0x8 => "tst",
            0x9 => "teq",
            0xA => "cmp",
            0xB => "cmn",
            0xC => "orr",
            0xD => "mov",
            0xE => "bic",
            0xF => "mvn",
            _ => "dpreg",
        };
        let op2 = format_reg_shift_operand2(word).unwrap_or_else(|| format!("r{}", word & 0xF));
        if opcode == 0xD || opcode == 0xF {
            format!("{op}{c} r{rd}, {op2}")
        } else if opcode == 0x8 || opcode == 0x9 || opcode == 0xA || opcode == 0xB {
            format!("{op}{c} r{rn}, {op2}")
        } else {
            format!("{op}{c} r{rd}, r{rn}, {op2}")
        }
    } else if (word & 0x0E00_0000) == 0x0200_0000 {
        let opcode = (word >> 21) & 0xF;
        let rn = (word >> 16) & 0xF;
        let rd = (word >> 12) & 0xF;
        let rotate = ((word >> 8) & 0xF) * 2;
        let imm8 = word & 0xFF;
        let imm = imm8.rotate_right(rotate);
        let op = match opcode {
            0x0 => "and",
            0x1 => "eor",
            0x2 => "sub",
            0x4 => "add",
            0xA => "cmp",
            0xC => "orr",
            0xD => "mov",
            0xE => "bic",
            _ => "dpimm",
        };
        if opcode == 0xD {
            format!("{op}{c} r{rd}, #0x{imm:x}")
        } else if opcode == 0xA {
            format!("{op}{c} r{rn}, #0x{imm:x}")
        } else {
            format!("{op}{c} r{rd}, r{rn}, #0x{imm:x}")
        }
    } else {
        format!("arm 0x{word:08x}")
    };
    format!("{text} [0x{word:08x}]")
}
