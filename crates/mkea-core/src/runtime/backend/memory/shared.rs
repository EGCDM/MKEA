const ALC_FALSE: u32 = 0;
const ALC_TRUE: u32 = 1;
const ALC_NO_ERROR: u32 = 0;
const ALC_INVALID_DEVICE: u32 = 0xA001;
const ALC_INVALID_CONTEXT: u32 = 0xA002;
const ALC_INVALID_ENUM: u32 = 0xA003;
const ALC_INVALID_VALUE: u32 = 0xA004;
const ALC_OUT_OF_MEMORY: u32 = 0xA005;

const AL_FALSE: u32 = 0;
const AL_TRUE: u32 = 1;
const AL_NONE: u32 = 0;
const AL_PITCH: u32 = 0x1003;
const AL_POSITION: u32 = 0x1004;
const AL_FORMAT_MONO8: u32 = 0x1100;
const AL_FORMAT_MONO16: u32 = 0x1101;
const AL_FORMAT_STEREO8: u32 = 0x1102;
const AL_FORMAT_STEREO16: u32 = 0x1103;
const AL_VELOCITY: u32 = 0x1006;
const AL_LOOPING: u32 = 0x1007;
const AL_BUFFER: u32 = 0x1009;
const AL_GAIN: u32 = 0x100A;
const AL_ORIENTATION: u32 = 0x100F;
const AL_SOURCE_STATE: u32 = 0x1010;
const AL_INITIAL: u32 = 0x1011;
const AL_PLAYING: u32 = 0x1012;
const AL_PAUSED: u32 = 0x1013;
const AL_STOPPED: u32 = 0x1014;
const AL_BUFFERS_QUEUED: u32 = 0x1015;
const AL_BUFFERS_PROCESSED: u32 = 0x1016;
const AL_NO_ERROR: u32 = 0;
const AL_INVALID_NAME: u32 = 0xA001;
const AL_INVALID_ENUM: u32 = 0xA002;
const AL_INVALID_VALUE: u32 = 0xA003;
const AL_INVALID_OPERATION: u32 = 0xA004;
const AL_OUT_OF_MEMORY: u32 = 0xA005;

const AUDIOQUEUE_NO_ERR: u32 = 0;
const AUDIOQUEUE_PARAM_ERR: u32 = (-50i32) as u32;
const AUDIOQUEUE_BUFFER_STRUCT_SIZE: u32 = 28;
const AUDIOQUEUE_PACKET_DESC_SIZE: u32 = 16;

impl MemoryArm32Backend {
// Shared low-level helpers: guest memory access, heap, streams, and foundation backing.

    fn find_region_mut(&mut self, addr: u32, size: u32) -> Option<&mut BackendRegion> {
        self.address_space.mapped.iter_mut().find(|region| region.contains_range(addr, size))
    }

    pub(crate) fn find_region(&self, addr: u32, size: u32) -> Option<&BackendRegion> {
        self.address_space.mapped.iter().find(|region| region.contains_range(addr, size))
    }

    pub(crate) fn read_u32_le(&self, addr: u32) -> CoreResult<u32> {
        let region = self
            .find_region(addr, 4)
            .ok_or_else(|| CoreError::Backend(self.format_backend_read_error(addr, 4)))?;
        let offset = (addr - region.addr) as usize;
        let bytes = &region.data[offset..offset + 4];
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn read_u16_le(&self, addr: u32) -> CoreResult<u16> {
        let region = self
            .find_region(addr, 2)
            .ok_or_else(|| CoreError::Backend(self.format_backend_read_error(addr, 2)))?;
        let offset = (addr - region.addr) as usize;
        let bytes = &region.data[offset..offset + 2];
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn write_u16_le(&mut self, addr: u32, value: u16) -> CoreResult<()> {
        let region = self
            .find_region_mut(addr, 2)
            .ok_or_else(|| CoreError::Backend(format!("backend cannot write 2 bytes at 0x{addr:08x}")))?;
        let offset = (addr - region.addr) as usize;
        let bytes = value.to_le_bytes();
        region.data[offset..offset + 2].copy_from_slice(&bytes);
        self.diag.writes.push((addr, 2));
        Ok(())
    }

    fn read_u8(&self, addr: u32) -> CoreResult<u8> {
        let region = self
            .find_region(addr, 1)
            .ok_or_else(|| CoreError::Backend(self.format_backend_read_error(addr, 1)))?;
        let offset = (addr - region.addr) as usize;
        Ok(region.data[offset])
    }

    fn format_backend_read_error(&self, addr: u32, size: u32) -> String {
        let exec_pc = self.exec.current_exec_pc;
        let exec_word = self.exec.current_exec_word;
        let exec_thumb = self.exec.current_exec_thumb || self.cpu.thumb;
        let sp = self.cpu.regs[13];
        let lr = self.cpu.regs[14];
        let region_hint = self
            .address_space
            .mapped
            .iter()
            .find(|region| addr >= region.addr && addr < region.end())
            .map(|region| {
                format!(
                    " containing_region=0x{:08x}-0x{:08x}/prot=0x{:x}",
                    region.addr,
                    region.end(),
                    region.prot,
                )
            })
            .unwrap_or_else(|| " no_region_contains_addr".to_string());
        format!(
            "backend cannot read {size} bytes at 0x{addr:08x} (pc=0x{exec_pc:08x} word=0x{exec_word:08x} thumb={} sp=0x{sp:08x} lr=0x{lr:08x} r0=0x{:08x} r1=0x{:08x} r2=0x{:08x} r3=0x{:08x} r5=0x{:08x} r6=0x{:08x} r7=0x{:08x}{region_hint})",
            if exec_thumb { "yes" } else { "no" },
            self.cpu.regs[0],
            self.cpu.regs[1],
            self.cpu.regs[2],
            self.cpu.regs[3],
            self.cpu.regs[5],
            self.cpu.regs[6],
            self.cpu.regs[7],
        )
    }

    fn write_u8(&mut self, addr: u32, value: u8) -> CoreResult<()> {
        let region = self
            .find_region_mut(addr, 1)
            .ok_or_else(|| CoreError::Backend(format!("backend cannot write 1 byte at 0x{addr:08x}")))?;
        let offset = (addr - region.addr) as usize;
        region.data[offset] = value;
        self.diag.writes.push((addr, 1));
        Ok(())
    }

    pub(crate) fn write_bytes(&mut self, addr: u32, bytes: &[u8]) -> CoreResult<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        let size = bytes.len() as u32;
        let region_hint = self
            .address_space
            .mapped
            .iter()
            .find(|region| addr >= region.addr && addr < region.addr.saturating_add(region.size))
            .map(|region| format!(" region_start=0x{:08x} region_size=0x{:x}", region.addr, region.size))
            .unwrap_or_else(|| " no_region_contains_addr".to_string());

        let Some(region) = self.find_region_mut(addr, size) else {
            return Err(CoreError::Backend(format!(
                "backend cannot write {} bytes at 0x{addr:08x}{}",
                bytes.len(),
                region_hint
            )));
        };

        let offset = (addr - region.addr) as usize;
        region.data[offset..offset + bytes.len()].copy_from_slice(bytes);
        self.diag.writes.push((addr, bytes.len()));
        Ok(())
    }


    fn read_c_string(&self, addr: u32, max_len: usize) -> Option<String> {
        if addr == 0 {
            return None;
        }
        let mut bytes = Vec::new();
        for i in 0..max_len {
            let b = self.read_u8(addr.wrapping_add(i as u32)).ok()?;
            if b == 0 {
                break;
            }
            if b.is_ascii_graphic() || b == b' ' || b == b':' || b == b'_' {
                bytes.push(b);
            } else {
                return None;
            }
        }
        if bytes.is_empty() {
            None
        } else {
            String::from_utf8(bytes).ok()
        }
    }

    fn read_guest_bytes(&self, addr: u32, size: u32) -> CoreResult<Vec<u8>> {
        if size == 0 {
            return Ok(Vec::new());
        }
        let region = self
            .find_region(addr, size)
            .ok_or_else(|| CoreError::Backend(format!("backend cannot read {} bytes at 0x{addr:08x}", size)))?;
        let offset = (addr - region.addr) as usize;
        let end = offset.saturating_add(size as usize);
        Ok(region.data[offset..end].to_vec())
    }

    fn read_guest_string_bytes(&self, addr: u32, size: usize) -> Option<String> {
        if addr == 0 || size == 0 || size > 1024 {
            return None;
        }
        let bytes = self.read_guest_bytes(addr, size as u32).ok()?;
        if bytes.iter().any(|byte| *byte == 0) {
            return None;
        }
        String::from_utf8(bytes).ok()
    }

    fn try_decode_cfstring_at(&self, addr: u32) -> Option<String> {
        if addr == 0 {
            return None;
        }
        if let Some(range) = self.runtime.objc.objc_section_cfstring {
            if !range.contains(addr) {
                return None;
            }
        }
        let cstr_ptr = self.read_u32_le(addr.wrapping_add(8)).ok()?;
        let len = self.read_u32_le(addr.wrapping_add(12)).ok()? as usize;
        self.read_guest_string_bytes(cstr_ptr, len)
    }

    fn alloc_selector_c_string(&mut self, text: &str) -> CoreResult<u32> {
        let Some(existing) = self.runtime.objc.selector_string_pool.get(text).copied() else {
            let align = 4u32;
            let start = (self.runtime.objc.selector_pool_cursor + (align - 1)) & !(align - 1);
            let mut bytes = text.as_bytes().to_vec();
            bytes.push(0);
            let end = start
                .checked_add(bytes.len() as u32)
                .ok_or_else(|| CoreError::Backend("selector pool overflow".to_string()))?;
            if end > self.runtime.objc.selector_pool_end {
                return Err(CoreError::Backend(format!(
                    "selector pool exhausted while interning '{}'",
                    text
                )));
            }
            self.write_bytes(start, &bytes)?;
            self.runtime.objc.selector_pool_cursor = end;
            self.runtime.objc.selector_string_pool.insert(text.to_string(), start);
            return Ok(start);
        };
        Ok(existing)
    }

    fn objc_read_selector_name(&self, sel_ptr: u32) -> Option<String> {
        self.read_c_string(sel_ptr, 160).or_else(|| {
            let nested = self.read_u32_le(sel_ptr).ok()?;
            self.read_c_string(nested, 160)
        })
    }

    fn write_u32_le(&mut self, addr: u32, value: u32) -> CoreResult<()> {
        self.maybe_trace_watched_sprite_write(addr, value, 4, "u32");
        let bytes = value.to_le_bytes();
        let region = self
            .find_region_mut(addr, 4)
            .ok_or_else(|| CoreError::Backend(format!("backend cannot write 4 bytes at 0x{addr:08x}")))?;
        let offset = (addr - region.addr) as usize;
        region.data[offset..offset + 4].copy_from_slice(&bytes);
        self.diag.writes.push((addr, 4));
        Ok(())
    }

    fn read_u64_le(&self, addr: u32) -> CoreResult<u64> {
        let bytes = self.read_guest_bytes(addr, 8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn write_u64_le(&mut self, addr: u32, value: u64) -> CoreResult<()> {
        self.write_bytes(addr, &value.to_le_bytes())
    }

    fn vfp_get_s(&self, reg: usize) -> u32 {
        let d_index = reg / 2;
        let word_index = reg % 2;
        let raw = self.exec.vfp_d_regs[d_index].to_le_bytes();
        let base = word_index * 4;
        u32::from_le_bytes([raw[base], raw[base + 1], raw[base + 2], raw[base + 3]])
    }

    fn vfp_set_s(&mut self, reg: usize, value: u32) {
        let d_index = reg / 2;
        let word_index = reg % 2;
        let mut raw = self.exec.vfp_d_regs[d_index].to_le_bytes();
        let bytes = value.to_le_bytes();
        let base = word_index * 4;
        raw[base..base + 4].copy_from_slice(&bytes);
        self.exec.vfp_d_regs[d_index] = u64::from_le_bytes(raw);
    }

    fn vfp_get_s_f32(&self, reg: usize) -> f32 {
        f32::from_bits(self.vfp_get_s(reg))
    }

    fn vfp_set_s_f32(&mut self, reg: usize, value: f32) {
        self.vfp_set_s(reg, value.to_bits());
    }

    fn vfp_get_d_f64(&self, reg: usize) -> f64 {
        f64::from_bits(self.exec.vfp_d_regs[reg])
    }

    fn vfp_set_d_f64(&mut self, reg: usize, value: f64) {
        self.exec.vfp_d_regs[reg] = value.to_bits();
    }

    fn vfp_set_cmp_flags_f32(&mut self, lhs: f32, rhs: f32) {
        if lhs.is_nan() || rhs.is_nan() {
            self.cpu.flags.n = false;
            self.cpu.flags.z = false;
            self.cpu.flags.c = true;
            self.cpu.flags.v = true;
            return;
        }
        if lhs == rhs {
            self.cpu.flags.n = false;
            self.cpu.flags.z = true;
            self.cpu.flags.c = true;
            self.cpu.flags.v = false;
        } else if lhs < rhs {
            self.cpu.flags.n = true;
            self.cpu.flags.z = false;
            self.cpu.flags.c = false;
            self.cpu.flags.v = false;
        } else {
            self.cpu.flags.n = false;
            self.cpu.flags.z = false;
            self.cpu.flags.c = true;
            self.cpu.flags.v = false;
        }
    }

    fn vfp_set_cmp_flags_f64(&mut self, lhs: f64, rhs: f64) {
        if lhs.is_nan() || rhs.is_nan() {
            self.cpu.flags.n = false;
            self.cpu.flags.z = false;
            self.cpu.flags.c = true;
            self.cpu.flags.v = true;
            return;
        }
        if lhs == rhs {
            self.cpu.flags.n = false;
            self.cpu.flags.z = true;
            self.cpu.flags.c = true;
            self.cpu.flags.v = false;
        } else if lhs < rhs {
            self.cpu.flags.n = true;
            self.cpu.flags.z = false;
            self.cpu.flags.c = false;
            self.cpu.flags.v = false;
        } else {
            self.cpu.flags.n = false;
            self.cpu.flags.z = false;
            self.cpu.flags.c = true;
            self.cpu.flags.v = false;
        }
    }

    fn reg_operand(&self, reg: usize, current_pc: u32) -> u32 {
        if reg == 15 {
            current_pc.wrapping_add(8)
        } else {
            self.cpu.regs[reg]
        }
    }

    fn set_reg_branch_aware(&mut self, reg: usize, value: u32) -> StepControl {
        if reg == 15 {
            self.cpu.thumb = (value & 1) != 0;
            self.cpu.regs[15] = value & !1;
            StepControl::Continue
        } else {
            self.cpu.regs[reg] = value;
            StepControl::Continue
        }
    }

    fn symbol_label(&self, addr: u32) -> Option<&str> {
        self.diag.symbol_labels.get(&addr).map(String::as_str)
    }

    fn object_label(&self, addr: u32) -> Option<&str> {
        self.diag.object_labels.get(&addr).map(String::as_str)
    }

    fn describe_ptr(&self, addr: u32) -> String {
        if addr == 0 {
            "nil".to_string()
        } else if let Some(label) = self.object_label(addr) {
            format!("0x{addr:08x}<{}>", label)
        } else if let Some(text) = self.read_c_string(addr, 96) {
            format!("0x{addr:08x}:'{}'", text)
        } else {
            format!("0x{addr:08x}")
        }
    }

    fn read_argv0(&self, argv: u32) -> Option<String> {
        if argv == 0 {
            return None;
        }
        let ptr = self.read_u32_le(argv).ok()?;
        self.read_c_string(ptr, 256)
    }

    fn network_url_string(&self) -> &'static str {
        self.active_profile().synthetic_network_profile().url
    }

    fn network_host_string(&self) -> &'static str {
        self.active_profile().synthetic_network_profile().host
    }

    fn network_path_string(&self) -> &'static str {
        self.active_profile().synthetic_network_profile().path
    }

    fn network_http_method(&self) -> &'static str {
        self.active_profile().synthetic_network_profile().method
    }

    fn reachability_flags(&self) -> u32 {
        self.runtime.ui_network.reachability_flags
    }

    fn reachability_flags_label(&self) -> String {
        let mut parts = Vec::new();
        let flags = self.reachability_flags();
        if (flags & 0x0000_0001) != 0 {
            parts.push("Reachable");
        }
        if (flags & 0x0000_0002) != 0 {
            parts.push("TransientConnection");
        }
        if (flags & 0x0000_0004) != 0 {
            parts.push("WWAN");
        }
        if parts.is_empty() {
            "None".to_string()
        } else {
            parts.join("|")
        }
    }

    fn stream_status_name(code: u32) -> &'static str {
        match code {
            1 => "opening",
            2 => "open",
            3 => "reading",
            4 => "writing",
            5 => "at-end",
            6 => "closed",
            7 => "error",
            _ => "not-open",
        }
    }

    fn read_stream_has_bytes_available(&self) -> bool {
        self.runtime.ui_network.read_stream_open && self.runtime.ui_network.read_stream_bytes_consumed < self.synthetic_payload_bytes().len() as u32
    }

    fn write_stream_can_accept_bytes(&self) -> bool {
        self.runtime.ui_network.write_stream_open && !self.runtime.ui_network.network_cancelled && !self.runtime.ui_network.network_faulted
    }

    fn sync_stream_transport_state(&mut self) {
        let delivered = self.synthetic_payload_bytes().len() as u32;
        if self.runtime.ui_network.read_stream_open {
            self.runtime.ui_network.read_stream_status = if self.runtime.ui_network.network_cancelled || self.runtime.ui_network.network_faulted {
                7
            } else if delivered == 0 {
                2
            } else if self.runtime.ui_network.read_stream_bytes_consumed >= delivered {
                if self.runtime.ui_network.network_source_closed { 5 } else { 2 }
            } else if self.runtime.ui_network.read_stream_bytes_consumed > 0 {
                3
            } else {
                2
            };
        } else if self.runtime.ui_network.read_stream_status != 6 {
            self.runtime.ui_network.read_stream_status = 0;
        }

        if self.runtime.ui_network.write_stream_open {
            self.runtime.ui_network.write_stream_status = if self.runtime.ui_network.network_cancelled || self.runtime.ui_network.network_faulted {
                7
            } else if self.runtime.ui_network.write_stream_bytes_written > 0 {
                4
            } else {
                2
            };
        } else if self.runtime.ui_network.write_stream_status != 6 {
            self.runtime.ui_network.write_stream_status = 0;
        }
    }

    fn reset_synthetic_stream_transport(&mut self) {
        self.runtime.ui_network.read_stream_bytes_consumed = 0;
        self.runtime.ui_network.write_stream_bytes_written = 0;
        self.runtime.ui_network.read_stream_events = 0;
        self.runtime.ui_network.write_stream_events = 0;
        if !self.runtime.ui_network.read_stream_open {
            self.runtime.ui_network.read_stream_status = 0;
        }
        if !self.runtime.ui_network.write_stream_open {
            self.runtime.ui_network.write_stream_status = 0;
        }
        self.sync_stream_transport_state();
    }

    fn network_payload_len(&self) -> u32 {
        128
    }

    fn network_error_domain(&self) -> &'static str {
        "NSURLErrorDomain"
    }

    fn network_error_code(&self) -> i32 {
        match self.runtime.ui_network.network_fault_mode {
            2 => -1004,
            3 => -1005,
            4 => -999,
            _ => -1001,
        }
    }

    fn network_error_kind(&self) -> &'static str {
        match self.runtime.ui_network.network_fault_mode {
            2 => "cannot-connect",
            3 => "lost-connection",
            4 => "cancelled",
            _ => "timed-out",
        }
    }

    fn network_error_description(&self) -> &'static str {
        match self.runtime.ui_network.network_fault_mode {
            2 => "The connection could not be established.",
            3 => "The network connection was lost.",
            4 => "The connection was cancelled.",
            _ => "The request timed out.",
        }
    }

    fn network_failure_result(&self) -> &'static str {
        match self.runtime.ui_network.network_fault_mode {
            2 => "failed-cannot-connect",
            3 => "failed-lost-connection",
            4 => "cancelled",
            _ => "failed-timeout",
        }
    }

    fn network_should_retry(&self) -> bool {
        !self.runtime.ui_network.network_cancelled && matches!(self.runtime.ui_network.network_fault_mode, 1 | 2 | 3)
    }

    fn network_last_error_summary(&self) -> String {
        if self.runtime.ui_network.network_cancelled || self.runtime.ui_network.network_faulted || self.runtime.ui_network.network_timeout_armed {
            format!("{} {}", self.network_error_code(), self.network_error_kind())
        } else {
            "none".to_string()
        }
    }

    fn network_connection_state_code(&self) -> u32 {
        if self.runtime.ui_network.network_cancelled {
            5
        } else if self.runtime.ui_network.network_faulted {
            4
        } else if self.runtime.ui_network.network_completed {
            3
        } else if self.runtime.ui_network.network_armed && self.runtime.ui_network.network_stage > 0 {
            2
        } else if self.runtime.ui_network.network_armed {
            1
        } else {
            0
        }
    }

    fn network_connection_state_name(&self) -> &'static str {
        match self.network_connection_state_code() {
            1 => "scheduled",
            2 => "receiving",
            3 => "completed",
            4 => "faulted",
            5 => "cancelled",
            _ => "idle",
        }
    }

    fn network_fault_has_response(&self) -> bool {
        self.runtime.ui_network.network_fault_mode == 3 && self.runtime.ui_network.network_response_retained
    }

    fn network_fault_has_data(&self) -> bool {
        self.runtime.ui_network.network_fault_mode == 3 && self.runtime.ui_network.network_data_retained
    }

    fn retained_flag(value: bool) -> &'static str {
        if value { "YES" } else { "NO" }
    }

    fn synthetic_payload_bytes(&self) -> Vec<u8> {
        let delivered = if self.runtime.ui_network.network_cancelled {
            0usize
        } else if self.runtime.ui_network.network_bytes_delivered > 0 {
            self.runtime.ui_network.network_bytes_delivered as usize
        } else if self.runtime.ui_network.network_completed {
            self.network_payload_len() as usize
        } else {
            0usize
        };
        self.active_profile().synthetic_payload(
            self.network_connection_state_name(),
            self.network_should_retry(),
            delivered,
        )
    }

    fn synthetic_payload_preview(&self) -> String {
        let bytes = self.synthetic_payload_bytes();
        if bytes.is_empty() {
            return String::new();
        }
        let text = String::from_utf8_lossy(&bytes);
        let mut preview = text.replace('\0', "");
        if preview.len() > 96 {
            preview.truncate(96);
        }
        preview
    }

    fn synthetic_heap_bounds_error(&self, size: u32) -> CoreError {
        CoreError::Backend(format!(
            "synthetic heap exhausted: need {} bytes in [0x{:08x}, 0x{:08x})",
            size, self.runtime.heap.synthetic_heap_cursor, self.runtime.heap.synthetic_heap_end
        ))
    }

    fn reserve_synthetic_heap_block(&mut self, requested_size: u32, align: u32, guard_bytes: u32) -> CoreResult<(u32, u32)> {
        let align = align.max(1);
        let requested_size = requested_size.max(1);
        let start = self.runtime.heap
            .synthetic_heap_cursor
            .checked_add(align - 1)
            .map(|value| value & !(align - 1))
            .ok_or_else(|| CoreError::Backend("synthetic heap overflow".to_string()))?;
        let reserved_size = requested_size
            .checked_add(guard_bytes)
            .ok_or_else(|| CoreError::Backend("synthetic heap overflow".to_string()))?;
        let end = start
            .checked_add(reserved_size)
            .ok_or_else(|| CoreError::Backend("synthetic heap overflow".to_string()))?;
        if end > self.runtime.heap.synthetic_heap_end {
            return Err(self.synthetic_heap_bounds_error(reserved_size));
        }
        Ok((start, reserved_size))
    }

    fn alloc_synthetic_heap_block(&mut self, requested_size: u32, zero_fill: bool, tag: impl Into<String>) -> CoreResult<u32> {
        let tag = tag.into();
        let requested_size = requested_size.max(1);
        let (start, reserved_size) = self.reserve_synthetic_heap_block(requested_size, 16, SYNTHETIC_HEAP_GUARD_BYTES)?;
        let region = self
            .find_region_mut(start, reserved_size)
            .ok_or_else(|| CoreError::Backend(format!("backend cannot materialize synthetic heap block at 0x{start:08x}")))?;
        let offset = (start - region.addr) as usize;
        let end = offset + reserved_size as usize;
        if zero_fill {
            region.data[offset..end].fill(0);
        } else if reserved_size > requested_size {
            region.data[offset + requested_size as usize..end].fill(0xCD);
        }
        self.diag.writes.push((start, reserved_size as usize));
        self.runtime.heap.synthetic_heap_cursor = start.saturating_add(reserved_size);
        self.runtime.heap.synthetic_heap_allocations.insert(
            start,
            SyntheticHeapAllocation {
                ptr: start,
                size: requested_size,
                reserved_size,
                freed: false,
                tag,
            },
        );
        self.runtime.heap.synthetic_heap_allocations_total = self.runtime.heap.synthetic_heap_allocations_total.saturating_add(1);
        self.runtime.heap.synthetic_heap_bytes_active = self.runtime.heap.synthetic_heap_bytes_active.saturating_add(requested_size);
        self.runtime.heap.synthetic_heap_bytes_peak = self.runtime.heap.synthetic_heap_bytes_peak.max(self.runtime.heap.synthetic_heap_bytes_active);
        self.runtime.heap.synthetic_heap_last_alloc_ptr = Some(start);
        self.runtime.heap.synthetic_heap_last_alloc_size = Some(requested_size);
        self.runtime.heap.synthetic_heap_last_error = None;
        Ok(start)
    }

    fn free_synthetic_heap_block(&mut self, ptr: u32) -> bool {
        if ptr == 0 {
            self.runtime.heap.synthetic_heap_last_freed_ptr = Some(0);
            self.runtime.heap.synthetic_heap_last_error = None;
            return true;
        }
        let mut scribble: Option<(u32, u32)> = None;
        let result = if let Some(block) = self.runtime.heap.synthetic_heap_allocations.get_mut(&ptr) {
            if block.freed {
                self.runtime.heap.synthetic_heap_last_error = Some(format!("double free at 0x{ptr:08x}"));
                false
            } else {
                block.freed = true;
                self.runtime.heap.synthetic_heap_frees = self.runtime.heap.synthetic_heap_frees.saturating_add(1);
                self.runtime.heap.synthetic_heap_bytes_active = self.runtime.heap.synthetic_heap_bytes_active.saturating_sub(block.size);
                self.runtime.heap.synthetic_heap_last_freed_ptr = Some(ptr);
                self.runtime.heap.synthetic_heap_last_error = None;
                scribble = Some((block.ptr, block.reserved_size));
                true
            }
        } else {
            self.runtime.heap.synthetic_heap_last_error = Some(format!("free on unknown pointer 0x{ptr:08x}"));
            false
        };
        if let Some((base, size)) = scribble {
            if let Some(region) = self.find_region_mut(base, size) {
                let offset = (base - region.addr) as usize;
                region.data[offset..offset + size as usize].fill(0xDD);
                self.diag.writes.push((base, size as usize));
            }
        }
        result
    }

    fn alloc_synthetic_guest_bytes(&mut self, bytes: &[u8], zero_terminated: bool) -> CoreResult<u32> {
        let extra = if zero_terminated { 1usize } else { 0usize };
        let size = bytes.len().saturating_add(extra) as u32;
        let ptr = self.alloc_synthetic_heap_block(size, false, "synthetic-bytes")?;
        self.write_bytes(ptr, bytes)?;
        if zero_terminated {
            self.write_u8(ptr.wrapping_add(bytes.len() as u32), 0)?;
        }
        Ok(ptr)
    }

    fn handle_guest_malloc(&mut self, size: u32) -> u32 {
        match self.alloc_synthetic_heap_block(size, false, format!("malloc({size})")) {
            Ok(ptr) => ptr,
            Err(err) => {
                self.runtime.heap.synthetic_heap_last_error = Some(err.to_string());
                0
            }
        }
    }

    fn handle_guest_calloc(&mut self, count: u32, size: u32) -> u32 {
        let Some(total) = count.checked_mul(size) else {
            self.runtime.heap.synthetic_heap_last_error = Some(format!("calloc overflow count={} size={}", count, size));
            return 0;
        };
        match self.alloc_synthetic_heap_block(total, true, format!("calloc({count},{size})")) {
            Ok(ptr) => ptr,
            Err(err) => {
                self.runtime.heap.synthetic_heap_last_error = Some(err.to_string());
                0
            }
        }
    }

    fn handle_guest_realloc(&mut self, ptr: u32, size: u32) -> u32 {
        if ptr == 0 {
            return self.handle_guest_malloc(size);
        }
        if size == 0 {
            let _ = self.free_synthetic_heap_block(ptr);
            self.runtime.heap.synthetic_heap_reallocs = self.runtime.heap.synthetic_heap_reallocs.saturating_add(1);
            self.runtime.heap.synthetic_heap_last_realloc_old_ptr = Some(ptr);
            self.runtime.heap.synthetic_heap_last_realloc_new_ptr = Some(0);
            self.runtime.heap.synthetic_heap_last_realloc_size = Some(0);
            return 0;
        }
        let Some(existing) = self.runtime.heap.synthetic_heap_allocations.get(&ptr).cloned() else {
            self.runtime.heap.synthetic_heap_last_error = Some(format!("realloc on unknown pointer 0x{ptr:08x}"));
            return 0;
        };
        if existing.freed {
            self.runtime.heap.synthetic_heap_last_error = Some(format!("realloc on freed pointer 0x{ptr:08x}"));
            return 0;
        }
        let new_ptr = match self.alloc_synthetic_heap_block(size, false, format!("realloc(0x{ptr:08x},{size})")) {
            Ok(ptr2) => ptr2,
            Err(err) => {
                self.runtime.heap.synthetic_heap_last_error = Some(err.to_string());
                return 0;
            }
        };
        let copy_len = existing.size.min(size.max(1));
        if copy_len > 0 {
            if let Ok(bytes) = self.read_guest_bytes(ptr, copy_len) {
                let _ = self.write_bytes(new_ptr, &bytes);
            }
        }
        let _ = self.free_synthetic_heap_block(ptr);
        self.runtime.heap.synthetic_heap_reallocs = self.runtime.heap.synthetic_heap_reallocs.saturating_add(1);
        self.runtime.heap.synthetic_heap_last_realloc_old_ptr = Some(ptr);
        self.runtime.heap.synthetic_heap_last_realloc_new_ptr = Some(new_ptr);
        self.runtime.heap.synthetic_heap_last_realloc_size = Some(size);
        self.runtime.heap.synthetic_heap_last_error = None;
        new_ptr
    }


    fn audio_is_objc_player_class(class_name: &str) -> bool {
        matches!(class_name, "AVAudioPlayer")
    }

    fn audio_is_objc_engine_class(class_name: &str) -> bool {
        matches!(class_name, "SimpleAudioEngine" | "CDAudioManager" | "CDSoundEngine" | "OALSimpleAudio")
    }

    fn audio_is_objc_audio_class(class_name: &str) -> bool {
        Self::audio_is_objc_player_class(class_name) || Self::audio_is_objc_engine_class(class_name)
    }

    fn audio_is_objc_audio_selector(selector: &str) -> bool {
        matches!(
            selector,
            "sharedEngine"
                | "sharedManager"
                | "soundEngine"
                | "asynchLoadProgress"
                | "playSound:channelGroupId:pitch:pan:gain:loop:"
                | "initWithContentsOfURL:error:"
                | "initWithData:error:"
                | "prepareToPlay"
                | "play"
                | "pause"
                | "stop"
                | "playing"
                | "setVolume:"
                | "volume"
                | "setNumberOfLoops:"
                | "numberOfLoops"
                | "preloadBackgroundMusic:"
                | "playBackgroundMusic:"
                | "playBackgroundMusic:loop:"
                | "stopBackgroundMusic"
                | "pauseBackgroundMusic"
                | "resumeBackgroundMusic"
                | "setBackgroundMusicVolume:"
                | "preloadEffect:"
                | "playEffect:"
                | "playEffect:loop:"
                | "playEffect:pitch:pan:gain:"
                | "stopEffect:"
                | "unloadEffect:"
                | "setEffectsVolume:"
        )
    }

    fn audio_trace_note_objc_audio_selector(
        &mut self,
        class_name: &str,
        selector: &str,
        resource: Option<String>,
        fallback: bool,
    ) {
        self.runtime.audio_trace.objc_audio_last_class = Some(class_name.to_string());
        self.runtime.audio_trace.objc_audio_last_selector = Some(selector.to_string());
        self.runtime.audio_trace.objc_audio_last_resource = resource.clone();
        match selector {
            "alloc" | "allocWithZone:" | "new" if Self::audio_is_objc_player_class(class_name) => {
                self.runtime.audio_trace.objc_audio_player_alloc_calls = self.runtime.audio_trace.objc_audio_player_alloc_calls.saturating_add(1);
            }
            "initWithContentsOfURL:error:" => {
                self.runtime.audio_trace.objc_audio_player_init_url_calls = self.runtime.audio_trace.objc_audio_player_init_url_calls.saturating_add(1);
            }
            "initWithData:error:" => {
                self.runtime.audio_trace.objc_audio_player_init_data_calls = self.runtime.audio_trace.objc_audio_player_init_data_calls.saturating_add(1);
            }
            "prepareToPlay" => {
                self.runtime.audio_trace.objc_audio_player_prepare_calls = self.runtime.audio_trace.objc_audio_player_prepare_calls.saturating_add(1);
            }
            "play" if Self::audio_is_objc_player_class(class_name) => {
                self.runtime.audio_trace.objc_audio_player_play_calls = self.runtime.audio_trace.objc_audio_player_play_calls.saturating_add(1);
            }
            "pause" => {
                self.runtime.audio_trace.objc_audio_player_pause_calls = self.runtime.audio_trace.objc_audio_player_pause_calls.saturating_add(1);
            }
            "stop" if Self::audio_is_objc_player_class(class_name) => {
                self.runtime.audio_trace.objc_audio_player_stop_calls = self.runtime.audio_trace.objc_audio_player_stop_calls.saturating_add(1);
            }
            "setVolume:" => {
                self.runtime.audio_trace.objc_audio_player_set_volume_calls = self.runtime.audio_trace.objc_audio_player_set_volume_calls.saturating_add(1);
            }
            "setNumberOfLoops:" => {
                self.runtime.audio_trace.objc_audio_player_set_loops_calls = self.runtime.audio_trace.objc_audio_player_set_loops_calls.saturating_add(1);
            }
            "sharedEngine" | "sharedManager" => {
                self.runtime.audio_trace.objc_audio_engine_shared_calls = self.runtime.audio_trace.objc_audio_engine_shared_calls.saturating_add(1);
                if class_name.contains("CDAudioManager") {
                    self.runtime.audio_trace.objc_audio_manager_shared_calls = self.runtime.audio_trace.objc_audio_manager_shared_calls.saturating_add(1);
                }
            }
            "soundEngine" => {
                self.runtime.audio_trace.objc_audio_manager_soundengine_calls = self.runtime.audio_trace.objc_audio_manager_soundengine_calls.saturating_add(1);
            }
            "preloadBackgroundMusic:" => {
                self.runtime.audio_trace.objc_audio_engine_preload_calls = self.runtime.audio_trace.objc_audio_engine_preload_calls.saturating_add(1);
                self.runtime.audio_trace.objc_audio_bgm_preload_calls = self.runtime.audio_trace.objc_audio_bgm_preload_calls.saturating_add(1);
            }
            "preloadEffect:" => {
                self.runtime.audio_trace.objc_audio_engine_preload_calls = self.runtime.audio_trace.objc_audio_engine_preload_calls.saturating_add(1);
            }
            "playBackgroundMusic:" | "playBackgroundMusic:loop:" => {
                self.runtime.audio_trace.objc_audio_engine_play_calls = self.runtime.audio_trace.objc_audio_engine_play_calls.saturating_add(1);
                self.runtime.audio_trace.objc_audio_bgm_play_calls = self.runtime.audio_trace.objc_audio_bgm_play_calls.saturating_add(1);
            }
            "stopBackgroundMusic" | "pauseBackgroundMusic" | "resumeBackgroundMusic" => {
                self.runtime.audio_trace.objc_audio_engine_stop_calls = self.runtime.audio_trace.objc_audio_engine_stop_calls.saturating_add(1);
            }
            "playEffect:" | "playEffect:loop:" | "playEffect:pitch:pan:gain:" | "stopEffect:" | "unloadEffect:" | "setEffectsVolume:" => {
                self.runtime.audio_trace.objc_audio_engine_effect_calls = self.runtime.audio_trace.objc_audio_engine_effect_calls.saturating_add(1);
            }
            "asynchLoadProgress" => {
                self.runtime.audio_trace.objc_audio_engine_async_load_progress_calls = self.runtime.audio_trace.objc_audio_engine_async_load_progress_calls.saturating_add(1);
                if class_name.is_empty() {
                    self.runtime.audio_trace.objc_audio_engine_async_load_progress_nil_receivers = self.runtime.audio_trace.objc_audio_engine_async_load_progress_nil_receivers.saturating_add(1);
                }
            }
            "playSound:channelGroupId:pitch:pan:gain:loop:" => {
                self.runtime.audio_trace.objc_audio_engine_playsound_calls = self.runtime.audio_trace.objc_audio_engine_playsound_calls.saturating_add(1);
                if class_name.is_empty() {
                    self.runtime.audio_trace.objc_audio_engine_playsound_nil_receivers = self.runtime.audio_trace.objc_audio_engine_playsound_nil_receivers.saturating_add(1);
                }
            }
            _ => {}
        }
        if fallback {
            self.runtime.audio_trace.objc_audio_fallback_dispatches = self.runtime.audio_trace.objc_audio_fallback_dispatches.saturating_add(1);
        }
        let resource_desc = resource.unwrap_or_else(|| "<none>".to_string());
        let tag = if fallback { "objc.audio.fallback" } else { "objc.audio" };
        self.audio_trace_push_event(format!(
            "{} class={} selector={} resource={}",
            tag,
            if class_name.is_empty() { "<unknown>" } else { class_name },
            selector,
            resource_desc,
        ));
    }

    fn audio_trace_push_event(&mut self, message: impl Into<String>) {
        let message = message.into();
        let trace = &mut self.runtime.audio_trace.recent_events;
        trace.push(message);
        if trace.len() > 96 {
            let drop_count = trace.len().saturating_sub(96);
            trace.drain(0..drop_count);
        }
    }

    fn audio_hex_preview(bytes: &[u8]) -> String {
        if bytes.is_empty() {
            return String::new();
        }
        bytes.iter()
            .take(24)
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn audio_ascii_preview(bytes: &[u8]) -> String {
        bytes.iter()
            .take(48)
            .map(|byte| {
                if byte.is_ascii_graphic() || *byte == b' ' {
                    *byte as char
                } else {
                    '.'
                }
            })
            .collect()
    }

    fn openal_format_name(format: u32) -> &'static str {
        match format {
            AL_FORMAT_MONO8 => "AL_FORMAT_MONO8",
            AL_FORMAT_MONO16 => "AL_FORMAT_MONO16",
            AL_FORMAT_STEREO8 => "AL_FORMAT_STEREO8",
            AL_FORMAT_STEREO16 => "AL_FORMAT_STEREO16",
            _ => "AL_FORMAT_UNKNOWN",
        }
    }

    fn openal_state_name(state: u32) -> &'static str {
        match state {
            AL_INITIAL => "AL_INITIAL",
            AL_PLAYING => "AL_PLAYING",
            AL_PAUSED => "AL_PAUSED",
            AL_STOPPED => "AL_STOPPED",
            _ => "AL_STATE_UNKNOWN",
        }
    }

    fn openal_set_al_error(&mut self, err: u32) {
        if err != AL_NO_ERROR {
            self.runtime.openal.last_al_error = err;
        }
    }

    fn openal_take_al_error(&mut self) -> u32 {
        let err = self.runtime.openal.last_al_error;
        self.runtime.openal.last_al_error = AL_NO_ERROR;
        err
    }

    fn openal_set_alc_error(&mut self, err: u32) {
        if err != ALC_NO_ERROR {
            self.runtime.openal.last_alc_error = err;
        }
    }

    fn openal_take_alc_error(&mut self) -> u32 {
        let err = self.runtime.openal.last_alc_error;
        self.runtime.openal.last_alc_error = ALC_NO_ERROR;
        err
    }

    fn openal_alloc_handle(&mut self, tag: &str) -> CoreResult<u32> {
        self.alloc_synthetic_heap_block(32, true, format!("openal.{tag}"))
    }

    fn openal_device_handle(&mut self) -> CoreResult<u32> {
        self.runtime.audio_trace.openal_device_open_calls = self.runtime.audio_trace.openal_device_open_calls.saturating_add(1);
        if self.runtime.openal.device_ptr == 0 {
            let ptr = self.openal_alloc_handle("device")?;
            self.runtime.openal.device_ptr = ptr;
            self.diag.object_labels.insert(ptr, "OpenAL.device.synthetic#0".to_string());
            self.runtime.openal.last_alc_error = ALC_NO_ERROR;
            self.audio_trace_push_event(format!("openal.device.open ptr={}", self.describe_ptr(ptr)));
        } else {
            self.audio_trace_push_event(format!("openal.device.reuse ptr={}", self.describe_ptr(self.runtime.openal.device_ptr)));
        }
        Ok(self.runtime.openal.device_ptr)
    }

    fn openal_context_handle(&mut self) -> CoreResult<u32> {
        self.runtime.audio_trace.openal_context_create_calls = self.runtime.audio_trace.openal_context_create_calls.saturating_add(1);
        if self.runtime.openal.context_ptr == 0 {
            let ptr = self.openal_alloc_handle("context")?;
            self.runtime.openal.context_ptr = ptr;
            self.diag.object_labels.insert(ptr, "OpenAL.context.synthetic#0".to_string());
            self.runtime.openal.last_alc_error = ALC_NO_ERROR;
            self.audio_trace_push_event(format!("openal.context.create ptr={}", self.describe_ptr(ptr)));
        } else {
            self.audio_trace_push_event(format!("openal.context.reuse ptr={}", self.describe_ptr(self.runtime.openal.context_ptr)));
        }
        Ok(self.runtime.openal.context_ptr)
    }

    fn openal_promote_processed(source: &mut BackendOpenAlSourceState) {
        if source.state == AL_STOPPED {
            while source.processed_buffers.len() < source.queued_buffers.len() {
                if let Some(id) = source.queued_buffers.get(source.processed_buffers.len()).copied() {
                    source.processed_buffers.push_back(id);
                } else {
                    break;
                }
            }
            return;
        }
        if source.state == AL_PLAYING && source.processed_buffers.len() < source.queued_buffers.len() {
            if let Some(id) = source.queued_buffers.get(source.processed_buffers.len()).copied() {
                source.processed_buffers.push_back(id);
            }
        }
    }

    fn openal_gen_buffers(&mut self, count: u32, out_ptr: u32) -> CoreResult<Vec<u32>> {
        self.runtime.audio_trace.openal_buffers_generated = self.runtime.audio_trace.openal_buffers_generated.saturating_add(count);
        let mut ids = Vec::with_capacity(count as usize);
        {
            let openal = &mut self.runtime.openal;
            for _ in 0..count {
                let id = openal.next_buffer_id.max(1);
                openal.next_buffer_id = id.saturating_add(1);
                openal.buffers.insert(id, BackendOpenAlBufferState::default());
                ids.push(id);
            }
        }
        for (index, id) in ids.iter().enumerate() {
            self.write_u32_le(out_ptr.wrapping_add((index as u32) * 4), *id)?;
        }
        self.runtime.openal.last_al_error = AL_NO_ERROR;
        self.audio_trace_push_event(format!("openal.gen_buffers count={} ids={:?}", count, ids));
        Ok(ids)
    }

    fn openal_gen_sources(&mut self, count: u32, out_ptr: u32) -> CoreResult<Vec<u32>> {
        self.runtime.audio_trace.openal_sources_generated = self.runtime.audio_trace.openal_sources_generated.saturating_add(count);
        let mut ids = Vec::with_capacity(count as usize);
        {
            let openal = &mut self.runtime.openal;
            for _ in 0..count {
                let id = openal.next_source_id.max(1);
                openal.next_source_id = id.saturating_add(1);
                let mut source = BackendOpenAlSourceState::default();
                source.floats.insert(AL_GAIN, 1.0);
                source.floats.insert(AL_PITCH, 1.0);
                source.vectors.insert(AL_POSITION, vec![0.0, 0.0, 0.0]);
                source.vectors.insert(AL_VELOCITY, vec![0.0, 0.0, 0.0]);
                openal.sources.insert(id, source);
                ids.push(id);
            }
        }
        for (index, id) in ids.iter().enumerate() {
            self.write_u32_le(out_ptr.wrapping_add((index as u32) * 4), *id)?;
        }
        self.runtime.openal.last_al_error = AL_NO_ERROR;
        self.audio_trace_push_event(format!("openal.gen_sources count={} ids={:?}", count, ids));
        Ok(ids)
    }

    fn openal_queue_buffers(&mut self, source_id: u32, ids: &[u32]) -> bool {
        self.runtime.audio_trace.openal_queue_calls = self.runtime.audio_trace.openal_queue_calls.saturating_add(1);
        let (queued_len, state) = {
            let Some(source) = self.runtime.openal.sources.get_mut(&source_id) else {
                return false;
            };
            for id in ids {
                source.queued_buffers.push_back(*id);
            }
            if source.state == AL_INITIAL && !source.queued_buffers.is_empty() {
                source.state = AL_STOPPED;
            }
            (source.queued_buffers.len(), source.state)
        };
        self.audio_trace_push_event(format!("openal.queue source={} ids={:?} queued={} state={}", source_id, ids, queued_len, Self::openal_state_name(state)));
        true
    }

    fn openal_unqueue_buffers(&mut self, source_id: u32, count: u32) -> Option<Vec<u32>> {
        self.runtime.audio_trace.openal_unqueue_calls = self.runtime.audio_trace.openal_unqueue_calls.saturating_add(1);
        let (ids, remaining, state) = {
            let source = self.runtime.openal.sources.get_mut(&source_id)?;
            Self::openal_promote_processed(source);
            let mut ids = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let Some(id) = source.processed_buffers.pop_front() else {
                    break;
                };
                let front = source.queued_buffers.pop_front();
                if front != Some(id) {
                    break;
                }
                ids.push(id);
            }
            if source.queued_buffers.is_empty() && source.state == AL_PLAYING {
                source.state = AL_STOPPED;
            }
            (ids, source.queued_buffers.len(), source.state)
        };
        self.audio_trace_push_event(format!("openal.unqueue source={} requested={} returned={:?} remaining={} state={}", source_id, count, ids, remaining, Self::openal_state_name(state)));
        Some(ids)
    }

    fn openal_read_f32_slice(&self, ptr: u32, count: usize) -> CoreResult<Vec<f32>> {
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            let bits = self.read_u32_le(ptr.wrapping_add((index as u32) * 4))?;
            values.push(f32::from_bits(bits));
        }
        Ok(values)
    }

    fn audioqueue_read_asbd(&self, ptr: u32) -> Option<AudioStreamBasicDescriptionState> {
        if ptr == 0 {
            return None;
        }
        Some(AudioStreamBasicDescriptionState {
            sample_rate: self.read_u64_le(ptr).ok().map(f64::from_bits)?,
            format_id: self.read_u32_le(ptr.wrapping_add(8)).ok()?,
            format_flags: self.read_u32_le(ptr.wrapping_add(12)).ok()?,
            bytes_per_packet: self.read_u32_le(ptr.wrapping_add(16)).ok()?,
            frames_per_packet: self.read_u32_le(ptr.wrapping_add(20)).ok()?,
            bytes_per_frame: self.read_u32_le(ptr.wrapping_add(24)).ok()?,
            channels_per_frame: self.read_u32_le(ptr.wrapping_add(28)).ok()?,
            bits_per_channel: self.read_u32_le(ptr.wrapping_add(32)).ok()?,
            reserved: self.read_u32_le(ptr.wrapping_add(36)).ok()?,
        })
    }

    fn audioqueue_asbd_bytes(format: &AudioStreamBasicDescriptionState) -> Vec<u8> {
        let mut bytes = vec![0u8; 40];
        bytes[0..8].copy_from_slice(&format.sample_rate.to_le_bytes());
        bytes[8..12].copy_from_slice(&format.format_id.to_le_bytes());
        bytes[12..16].copy_from_slice(&format.format_flags.to_le_bytes());
        bytes[16..20].copy_from_slice(&format.bytes_per_packet.to_le_bytes());
        bytes[20..24].copy_from_slice(&format.frames_per_packet.to_le_bytes());
        bytes[24..28].copy_from_slice(&format.bytes_per_frame.to_le_bytes());
        bytes[28..32].copy_from_slice(&format.channels_per_frame.to_le_bytes());
        bytes[32..36].copy_from_slice(&format.bits_per_channel.to_le_bytes());
        bytes[36..40].copy_from_slice(&format.reserved.to_le_bytes());
        bytes
    }

    fn audioqueue_format_summary(&self, format: Option<&AudioStreamBasicDescriptionState>) -> String {
        let Some(format) = format else {
            return "<unknown-asbd>".to_string();
        };
        let format_id = format.format_id.to_le_bytes();
        let format_tag = if format_id.iter().all(|b| b.is_ascii_graphic()) {
            String::from_utf8_lossy(&format_id).to_string()
        } else {
            format!("0x{:08x}", format.format_id)
        };
        format!(
            "sr={:.2} fmt={} flags=0x{:08x} bpp={} fpp={} bpf={} ch={} bits={}",
            format.sample_rate,
            format_tag,
            format.format_flags,
            format.bytes_per_packet,
            format.frames_per_packet,
            format.bytes_per_frame,
            format.channels_per_frame,
            format.bits_per_channel,
        )
    }

    fn audioqueue_write_buffer_guest_struct(&mut self, buffer_ptr: u32) -> CoreResult<()> {
        let Some(buffer) = self.runtime.audio_queue.buffers.get(&buffer_ptr).cloned() else {
            return Err(CoreError::Backend(format!("AudioQueueBuffer {} not found", self.describe_ptr(buffer_ptr))));
        };
        self.write_u32_le(buffer.buffer_ptr, buffer.audio_data_capacity)?;
        self.write_u32_le(buffer.buffer_ptr.wrapping_add(4), buffer.audio_data_ptr)?;
        self.write_u32_le(buffer.buffer_ptr.wrapping_add(8), buffer.last_byte_size)?;
        self.write_u32_le(buffer.buffer_ptr.wrapping_add(12), buffer.user_data_ptr)?;
        self.write_u32_le(buffer.buffer_ptr.wrapping_add(16), buffer.packet_desc_capacity)?;
        self.write_u32_le(buffer.buffer_ptr.wrapping_add(20), buffer.packet_descs_ptr)?;
        self.write_u32_le(buffer.buffer_ptr.wrapping_add(24), buffer.packet_desc_count)?;
        Ok(())
    }

    fn audioqueue_sync_buffer_from_guest(&mut self, buffer_ptr: u32) -> CoreResult<()> {
        let byte_size = self.read_u32_le(buffer_ptr.wrapping_add(8)).unwrap_or(0);
        let user_data_ptr = self.read_u32_le(buffer_ptr.wrapping_add(12)).unwrap_or(0);
        let packet_descs_ptr = self.read_u32_le(buffer_ptr.wrapping_add(20)).unwrap_or(0);
        let packet_desc_count = self.read_u32_le(buffer_ptr.wrapping_add(24)).unwrap_or(0);
        if let Some(buffer) = self.runtime.audio_queue.buffers.get_mut(&buffer_ptr) {
            buffer.last_byte_size = byte_size.min(buffer.audio_data_capacity);
            buffer.user_data_ptr = user_data_ptr;
            if packet_descs_ptr != 0 {
                buffer.packet_descs_ptr = packet_descs_ptr;
            }
            buffer.packet_desc_count = packet_desc_count.min(buffer.packet_desc_capacity);
        }
        Ok(())
    }

    fn audioqueue_next_serial_label(&mut self, kind: &str) -> String {
        match kind {
            "queue" => {
                let serial = self.runtime.audio_queue.next_queue_serial.max(1);
                self.runtime.audio_queue.next_queue_serial = serial.saturating_add(1);
                format!("AudioQueue.synthetic#{}", serial)
            }
            _ => {
                let serial = self.runtime.audio_queue.next_buffer_serial.max(1);
                self.runtime.audio_queue.next_buffer_serial = serial.saturating_add(1);
                format!("AudioQueueBuffer.synthetic#{}", serial)
            }
        }
    }

    fn audioqueue_create_output(
        &mut self,
        format_ptr: u32,
        callback_ptr: u32,
        user_data_ptr: u32,
        callback_runloop: u32,
        callback_runloop_mode: u32,
        flags: u32,
    ) -> CoreResult<u32> {
        self.runtime.audio_trace.audioqueue_create_calls = self.runtime.audio_trace.audioqueue_create_calls.saturating_add(1);
        let handle_ptr = self.alloc_synthetic_heap_block(32, true, "audioqueue.handle")?;
        let label = self.audioqueue_next_serial_label("queue");
        let format = self.audioqueue_read_asbd(format_ptr);
        let format_summary = self.audioqueue_format_summary(format.as_ref());
        self.runtime.audio_trace.audioqueue_last_format = Some(format_summary.clone());
        self.diag.object_labels.insert(handle_ptr, format!("{}<{}>", label, format_summary));
        self.runtime.audio_queue.queues.insert(
            handle_ptr,
            BackendAudioQueueHandleState {
                handle_ptr,
                callback_ptr,
                user_data_ptr,
                callback_runloop,
                callback_runloop_mode,
                flags,
                format,
                ..Default::default()
            },
        );
        self.audio_trace_push_event(format!("audioqueue.create queue={} callback=0x{:08x} format={}", self.describe_ptr(handle_ptr), callback_ptr, format_summary));
        Ok(handle_ptr)
    }

    fn audioqueue_allocate_buffer(&mut self, queue_ptr: u32, capacity: u32) -> CoreResult<u32> {
        self.runtime.audio_trace.audioqueue_allocate_calls = self.runtime.audio_trace.audioqueue_allocate_calls.saturating_add(1);
        if !self.runtime.audio_queue.queues.contains_key(&queue_ptr) {
            return Err(CoreError::Backend(format!("AudioQueue {} does not exist", self.describe_ptr(queue_ptr))));
        }
        let buffer_ptr = self.alloc_synthetic_heap_block(AUDIOQUEUE_BUFFER_STRUCT_SIZE, true, "audioqueue.buffer")?;
        let audio_data_ptr = self.alloc_synthetic_heap_block(capacity.max(1), true, "audioqueue.buffer.data")?;
        let label = self.audioqueue_next_serial_label("buffer");
        self.diag.object_labels.insert(buffer_ptr, label.clone());
        self.diag.object_labels.insert(audio_data_ptr, format!("{}.mAudioData", label));
        self.runtime.audio_queue.buffers.insert(
            buffer_ptr,
            BackendAudioQueueBufferState {
                queue_ptr,
                buffer_ptr,
                audio_data_ptr,
                audio_data_capacity: capacity,
                ..Default::default()
            },
        );
        if let Some(queue) = self.runtime.audio_queue.queues.get_mut(&queue_ptr) {
            queue.allocated_buffers.push(buffer_ptr);
        }
        self.audioqueue_write_buffer_guest_struct(buffer_ptr)?;
        self.audio_trace_push_event(format!("audioqueue.allocate queue={} buffer={} capacity={} audioData={}", self.describe_ptr(queue_ptr), self.describe_ptr(buffer_ptr), capacity, self.describe_ptr(audio_data_ptr)));
        Ok(buffer_ptr)
    }

    fn audioqueue_free_packet_descs(&mut self, buffer_ptr: u32) {
        let packet_descs_ptr = self
            .runtime
            .audio_queue
            .buffers
            .get(&buffer_ptr)
            .map(|buffer| buffer.packet_descs_ptr)
            .unwrap_or(0);
        if packet_descs_ptr != 0 {
            let _ = self.free_synthetic_heap_block(packet_descs_ptr);
            if let Some(buffer) = self.runtime.audio_queue.buffers.get_mut(&buffer_ptr) {
                buffer.packet_descs_ptr = 0;
                buffer.packet_desc_capacity = 0;
                buffer.packet_desc_count = 0;
            }
        }
    }

    fn audioqueue_free_buffer(&mut self, queue_ptr: u32, buffer_ptr: u32) -> bool {
        let Some(buffer) = self.runtime.audio_queue.buffers.get(&buffer_ptr).cloned() else {
            return false;
        };
        if buffer.queue_ptr != queue_ptr || buffer.freed {
            return false;
        }
        self.audioqueue_free_packet_descs(buffer_ptr);
        let _ = self.free_synthetic_heap_block(buffer.audio_data_ptr);
        let _ = self.free_synthetic_heap_block(buffer_ptr);
        self.diag.object_labels.remove(&buffer.audio_data_ptr);
        self.diag.object_labels.remove(&buffer_ptr);
        if let Some(queue) = self.runtime.audio_queue.queues.get_mut(&queue_ptr) {
            queue.allocated_buffers.retain(|ptr| *ptr != buffer_ptr);
            queue.queued_buffers.retain(|ptr| *ptr != buffer_ptr);
        }
        if let Some(buffer_state) = self.runtime.audio_queue.buffers.get_mut(&buffer_ptr) {
            buffer_state.freed = true;
            buffer_state.enqueued = false;
            buffer_state.callback_inflight = false;
        }
        self.runtime.audio_queue.buffers.remove(&buffer_ptr);
        true
    }

    fn audioqueue_enqueue_buffer(&mut self, queue_ptr: u32, buffer_ptr: u32, packet_desc_count: u32, packet_descs_ptr: u32) -> CoreResult<(u32, u32)> {
        self.runtime.audio_trace.audioqueue_enqueue_calls = self.runtime.audio_trace.audioqueue_enqueue_calls.saturating_add(1);
        if !self.runtime.audio_queue.queues.contains_key(&queue_ptr) {
            return Err(CoreError::Backend(format!("AudioQueue {} does not exist", self.describe_ptr(queue_ptr))));
        }
        self.audioqueue_sync_buffer_from_guest(buffer_ptr)?;
        let mut last_byte_size = 0;
        let mut capacity = 0;
        {
            let Some(buffer) = self.runtime.audio_queue.buffers.get(&buffer_ptr).cloned() else {
                return Err(CoreError::Backend(format!("AudioQueueBuffer {} does not exist", self.describe_ptr(buffer_ptr))));
            };
            if buffer.queue_ptr != queue_ptr || buffer.freed {
                return Err(CoreError::Backend(format!("AudioQueueBuffer {} does not belong to {}", self.describe_ptr(buffer_ptr), self.describe_ptr(queue_ptr))));
            }
            last_byte_size = buffer.last_byte_size.min(buffer.audio_data_capacity);
            capacity = buffer.audio_data_capacity;
        }
        self.audioqueue_free_packet_descs(buffer_ptr);
        if packet_desc_count != 0 && packet_descs_ptr != 0 {
            let desc_bytes = packet_desc_count.saturating_mul(AUDIOQUEUE_PACKET_DESC_SIZE);
            let payload = self.read_guest_bytes(packet_descs_ptr, desc_bytes)?;
            let guest_copy = self.alloc_synthetic_heap_block(desc_bytes.max(1), true, "audioqueue.packet_descs")?;
            self.write_bytes(guest_copy, &payload)?;
            if let Some(buffer) = self.runtime.audio_queue.buffers.get_mut(&buffer_ptr) {
                buffer.packet_descs_ptr = guest_copy;
                buffer.packet_desc_capacity = packet_desc_count;
                buffer.packet_desc_count = packet_desc_count;
            }
        }
        if let Some(buffer) = self.runtime.audio_queue.buffers.get_mut(&buffer_ptr) {
            buffer.enqueued = true;
            buffer.callback_inflight = false;
            buffer.last_byte_size = last_byte_size.min(capacity);
        }
        if let Some(queue) = self.runtime.audio_queue.queues.get_mut(&queue_ptr) {
            if !queue.queued_buffers.iter().any(|ptr| *ptr == buffer_ptr) {
                queue.queued_buffers.push_back(buffer_ptr);
            }
        }
        self.audioqueue_write_buffer_guest_struct(buffer_ptr)?;
        self.runtime.audio_trace.audioqueue_enqueued_bytes = self.runtime.audio_trace.audioqueue_enqueued_bytes.saturating_add(last_byte_size.min(capacity) as u64);
        self.runtime.audio_trace.audioqueue_last_queue = Some(queue_ptr);
        self.runtime.audio_trace.audioqueue_last_buffer = Some(buffer_ptr);
        if let Some(buffer) = self.runtime.audio_queue.buffers.get(&buffer_ptr).cloned() {
            let preview_len = buffer.last_byte_size.min(48);
            let preview = if preview_len != 0 { self.read_guest_bytes(buffer.audio_data_ptr, preview_len).unwrap_or_default() } else { Vec::new() };
            self.runtime.audio_trace.audioqueue_last_buffer_preview_hex = Some(Self::audio_hex_preview(&preview));
            self.runtime.audio_trace.audioqueue_last_buffer_preview_ascii = Some(Self::audio_ascii_preview(&preview));
            self.audio_trace_push_event(format!(
                "audioqueue.enqueue queue={} buffer={} bytes={} capacity={} packetDescCount={} previewHex=[{}] previewAscii='{}'",
                self.describe_ptr(queue_ptr),
                self.describe_ptr(buffer_ptr),
                last_byte_size.min(capacity),
                capacity,
                packet_desc_count,
                Self::audio_hex_preview(&preview),
                Self::audio_ascii_preview(&preview),
            ));
        }
        Ok((last_byte_size.min(capacity), capacity))
    }

    fn audioqueue_set_parameter(&mut self, queue_ptr: u32, parameter_id: u32, value: f32) -> bool {
        let Some(queue) = self.runtime.audio_queue.queues.get_mut(&queue_ptr) else {
            return false;
        };
        queue.parameters.insert(parameter_id, value);
        true
    }

    fn audioqueue_set_property(&mut self, queue_ptr: u32, property_id: u32, data_ptr: u32, data_size: u32) -> CoreResult<bool> {
        let payload = if data_ptr == 0 || data_size == 0 {
            Vec::new()
        } else {
            self.read_guest_bytes(data_ptr, data_size)?
        };
        let Some(queue) = self.runtime.audio_queue.queues.get_mut(&queue_ptr) else {
            return Ok(false);
        };
        queue.properties.insert(property_id, payload);
        Ok(true)
    }

    fn audioqueue_get_property(&mut self, queue_ptr: u32, property_id: u32, out_data_ptr: u32, io_size_ptr: u32) -> CoreResult<(bool, u32)> {
        let Some(queue) = self.runtime.audio_queue.queues.get(&queue_ptr).cloned() else {
            if io_size_ptr != 0 {
                self.write_u32_le(io_size_ptr, 0)?;
            }
            return Ok((false, 0));
        };
        let requested_size = if io_size_ptr != 0 {
            self.read_u32_le(io_size_ptr).unwrap_or(0)
        } else {
            0
        };
        let payload = if let Some(blob) = queue.properties.get(&property_id) {
            blob.clone()
        } else if queue.property_listeners.iter().any(|listener| listener.property_id == property_id) {
            (if queue.is_running { 1u32 } else { 0u32 }).to_le_bytes().to_vec()
        } else if let Some(format) = queue.format.as_ref() {
            let bytes = Self::audioqueue_asbd_bytes(format);
            if requested_size >= bytes.len() as u32 {
                bytes
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        if payload.is_empty() {
            if io_size_ptr != 0 {
                self.write_u32_le(io_size_ptr, 0)?;
            }
            return Ok((false, 0));
        }
        let actual_size = payload.len().min(u32::MAX as usize) as u32;
        if out_data_ptr != 0 {
            self.write_bytes(out_data_ptr, &payload[..actual_size as usize])?;
        }
        if io_size_ptr != 0 {
            self.write_u32_le(io_size_ptr, actual_size)?;
        }
        Ok((true, actual_size))
    }

    fn audioqueue_add_property_listener(&mut self, queue_ptr: u32, property_id: u32, callback_ptr: u32, user_data_ptr: u32) -> bool {
        let Some(queue) = self.runtime.audio_queue.queues.get_mut(&queue_ptr) else {
            return false;
        };
        if queue.property_listeners.iter().any(|listener| {
            listener.property_id == property_id && listener.callback_ptr == callback_ptr && listener.user_data_ptr == user_data_ptr
        }) {
            return true;
        }
        queue.property_listeners.push(BackendAudioQueuePropertyListenerState {
            property_id,
            callback_ptr,
            user_data_ptr,
        });
        true
    }

    fn audioqueue_collect_state_change_callbacks(
        &mut self,
        queue_ptr: u32,
        notify_listeners: bool,
        request_output_callbacks: bool,
    ) -> VecDeque<BackendAudioQueuePendingInvocation> {
        let mut pending = VecDeque::new();
        let Some(queue) = self.runtime.audio_queue.queues.get(&queue_ptr).cloned() else {
            return pending;
        };
        if request_output_callbacks && queue.callback_ptr != 0 {
            for buffer_ptr in queue.allocated_buffers {
                if let Some(buffer) = self.runtime.audio_queue.buffers.get_mut(&buffer_ptr) {
                    if !buffer.freed && !buffer.enqueued && !buffer.callback_inflight {
                        buffer.callback_inflight = true;
                        pending.push_back(BackendAudioQueuePendingInvocation::OutputCallback { queue_ptr, buffer_ptr });
                    }
                }
            }
        }
        if notify_listeners {
            for listener in queue.property_listeners {
                if listener.callback_ptr != 0 {
                    pending.push_back(BackendAudioQueuePendingInvocation::PropertyListener {
                        queue_ptr,
                        property_id: listener.property_id,
                        callback_ptr: listener.callback_ptr,
                        user_data_ptr: listener.user_data_ptr,
                    });
                }
            }
        }
        pending
    }

    fn audioqueue_set_running(&mut self, queue_ptr: u32, running: bool) -> bool {
        let Some(queue) = self.runtime.audio_queue.queues.get_mut(&queue_ptr) else {
            return false;
        };
        queue.is_running = running;
        true
    }

    fn audioqueue_begin_pending_callbacks(
        &mut self,
        origin_label: &str,
        resume_lr: u32,
        return_status: u32,
        pending: VecDeque<BackendAudioQueuePendingInvocation>,
    ) -> CoreResult<bool> {
        if pending.is_empty() {
            return Ok(false);
        }
        if self.runtime.audio_queue.callback_resume.is_some() {
            return Ok(false);
        }
        self.runtime.audio_queue.callback_resume = Some(BackendAudioQueueCallbackResumeState {
            origin_label: origin_label.to_string(),
            resume_lr,
            return_status,
            current: None,
            pending,
        });
        self.audioqueue_dispatch_next_pending_callback()?;
        Ok(true)
    }

    fn audioqueue_dispatch_next_pending_callback(&mut self) -> CoreResult<()> {
        loop {
            let (origin_label, next) = {
                let Some(resume) = self.runtime.audio_queue.callback_resume.as_mut() else {
                    return Ok(());
                };
                let Some(next) = resume.pending.pop_front() else {
                    return Ok(());
                };
                resume.current = Some(next.clone());
                (resume.origin_label.clone(), next)
            };
            match next {
                BackendAudioQueuePendingInvocation::OutputCallback { queue_ptr, buffer_ptr } => {
                    self.runtime.audio_trace.audioqueue_output_callback_dispatches = self.runtime.audio_trace.audioqueue_output_callback_dispatches.saturating_add(1);
                    let Some(queue) = self.runtime.audio_queue.queues.get(&queue_ptr).cloned() else {
                        if let Some(resume) = self.runtime.audio_queue.callback_resume.as_mut() {
                            resume.current = None;
                        }
                        continue;
                    };
                    if queue.callback_ptr == 0 {
                        if let Some(resume) = self.runtime.audio_queue.callback_resume.as_mut() {
                            resume.current = None;
                        }
                        continue;
                    }
                    let return_stub = if (queue.callback_ptr & 1) != 0 {
                        HLE_STUB_AUDIOQUEUE_CALLBACK_RETURN_THUMB | 1
                    } else {
                        HLE_STUB_AUDIOQUEUE_CALLBACK_RETURN_ARM
                    };
                    let user_data_desc = self.describe_ptr(queue.user_data_ptr);
                    let queue_desc = self.describe_ptr(queue_ptr);
                    let buffer_desc = self.describe_ptr(buffer_ptr);
                    self.push_callback_trace(format!(
                        "audioqueue.output origin={} callback=0x{:08x} userData={} queue={} buffer={}",
                        origin_label,
                        queue.callback_ptr,
                        user_data_desc,
                        queue_desc,
                        buffer_desc,
                    ));
                    self.audio_trace_push_event(format!(
                        "audioqueue.callback.output origin={} callback=0x{:08x} queue={} buffer={} userData={}",
                        origin_label,
                        queue.callback_ptr,
                        queue_desc,
                        buffer_desc,
                        user_data_desc,
                    ));
                    self.cpu.regs[0] = queue.user_data_ptr;
                    self.cpu.regs[1] = queue_ptr;
                    self.cpu.regs[2] = buffer_ptr;
                    self.cpu.regs[3] = 0;
                    self.cpu.regs[14] = return_stub;
                    self.cpu.regs[15] = queue.callback_ptr & !1;
                    self.cpu.thumb = (queue.callback_ptr & 1) != 0;
                    return Ok(());
                }
                BackendAudioQueuePendingInvocation::PropertyListener {
                    queue_ptr,
                    property_id,
                    callback_ptr,
                    user_data_ptr,
                } => {
                    self.runtime.audio_trace.audioqueue_property_callback_dispatches = self.runtime.audio_trace.audioqueue_property_callback_dispatches.saturating_add(1);
                    if callback_ptr == 0 {
                        if let Some(resume) = self.runtime.audio_queue.callback_resume.as_mut() {
                            resume.current = None;
                        }
                        continue;
                    }
                    let return_stub = if (callback_ptr & 1) != 0 {
                        HLE_STUB_AUDIOQUEUE_CALLBACK_RETURN_THUMB | 1
                    } else {
                        HLE_STUB_AUDIOQUEUE_CALLBACK_RETURN_ARM
                    };
                    let user_data_desc = self.describe_ptr(user_data_ptr);
                    let queue_desc = self.describe_ptr(queue_ptr);
                    self.push_callback_trace(format!(
                        "audioqueue.property origin={} callback=0x{:08x} userData={} queue={} property=0x{:08x}",
                        origin_label,
                        callback_ptr,
                        user_data_desc,
                        queue_desc,
                        property_id,
                    ));
                    self.audio_trace_push_event(format!(
                        "audioqueue.callback.property origin={} callback=0x{:08x} queue={} property=0x{:08x} userData={}",
                        origin_label,
                        callback_ptr,
                        queue_desc,
                        property_id,
                        user_data_desc,
                    ));
                    self.cpu.regs[0] = user_data_ptr;
                    self.cpu.regs[1] = queue_ptr;
                    self.cpu.regs[2] = property_id;
                    self.cpu.regs[3] = 0;
                    self.cpu.regs[14] = return_stub;
                    self.cpu.regs[15] = callback_ptr & !1;
                    self.cpu.thumb = (callback_ptr & 1) != 0;
                    return Ok(());
                }
            }
        }
    }

    fn audioqueue_complete_current_callback(&mut self) {
        let current = self
            .runtime
            .audio_queue
            .callback_resume
            .as_ref()
            .and_then(|resume| resume.current.clone());
        if let Some(BackendAudioQueuePendingInvocation::OutputCallback { buffer_ptr, .. }) = current {
            if let Some(buffer) = self.runtime.audio_queue.buffers.get_mut(&buffer_ptr) {
                buffer.callback_inflight = false;
            }
        }
        if let Some(resume) = self.runtime.audio_queue.callback_resume.as_mut() {
            resume.current = None;
        }
    }

    fn audioqueue_resume_after_callback_return(&mut self) -> CoreResult<()> {
        self.audioqueue_complete_current_callback();
        let should_continue = self
            .runtime
            .audio_queue
            .callback_resume
            .as_ref()
            .map(|resume| !resume.pending.is_empty())
            .unwrap_or(false);
        if should_continue {
            self.audioqueue_dispatch_next_pending_callback()?;
            return Ok(());
        }
        let Some(resume) = self.runtime.audio_queue.callback_resume.take() else {
            return Ok(());
        };
        self.cpu.regs[0] = resume.return_status;
        self.cpu.regs[15] = resume.resume_lr & !1;
        self.cpu.thumb = (resume.resume_lr & 1) != 0;
        self.push_callback_trace(format!(
            "audioqueue.resume origin={} returnStatus={} resumeLR=0x{:08x}",
            resume.origin_label,
            resume.return_status as i32,
            resume.resume_lr,
        ));
        Ok(())
    }

    fn active_synthetic_heap_allocations(&self) -> u32 {
        self.runtime.heap.synthetic_heap_allocations
            .values()
            .filter(|alloc| !alloc.freed)
            .count() as u32
    }

    fn synthetic_heap_reserved_bytes(&self) -> u32 {
        self.runtime.heap.synthetic_heap_allocations
            .values()
            .filter(|alloc| !alloc.freed)
            .map(|alloc| alloc.reserved_size)
            .sum()
    }

    fn ensure_string_backing(&mut self, object: u32, label: String, text: &str) -> CoreResult<()> {
        let ptr = self.alloc_synthetic_guest_bytes(text.as_bytes(), true)?;
        self.runtime.heap.synthetic_string_backing.insert(
            object,
            SyntheticStringBacking {
                ptr,
                len: text.chars().count() as u32,
                text: text.to_string(),
                font_name: None,
                font_size_bits: 0,
                font_size_explicit: false,
            },
        );
        self.diag.object_labels.insert(object, label);
        Ok(())
    }

    fn ensure_blob_backing(&mut self, object: u32, label: String, bytes: &[u8]) -> CoreResult<()> {
        let ptr = if bytes.is_empty() {
            0
        } else {
            self.alloc_synthetic_guest_bytes(bytes, false)?
        };
        self.runtime.heap.synthetic_blob_backing.insert(
            object,
            SyntheticBlobBacking {
                ptr,
                len: bytes.len() as u32,
                preview_ascii: String::from_utf8_lossy(bytes).replace('\0', ""),
            },
        );
        self.diag.object_labels.insert(object, label);
        Ok(())
    }

    fn refresh_foundation_backing_objects(&mut self) -> CoreResult<()> {
        self.ensure_string_backing(
            HLE_FAKE_NSSTRING_URL_ABSOLUTE,
            format!("NSString.synthetic.url.absoluteString<'{}'>", self.network_url_string()),
            self.network_url_string(),
        )?;
        self.ensure_string_backing(
            HLE_FAKE_NSSTRING_URL_HOST,
            format!("NSString.synthetic.url.host<'{}'>", self.network_host_string()),
            self.network_host_string(),
        )?;
        self.ensure_string_backing(
            HLE_FAKE_NSSTRING_URL_PATH,
            format!("NSString.synthetic.url.path<'{}'>", self.network_path_string()),
            self.network_path_string(),
        )?;
        self.ensure_string_backing(
            HLE_FAKE_NSSTRING_HTTP_METHOD,
            format!("NSString.synthetic.request.method<'{}'>", self.network_http_method()),
            self.network_http_method(),
        )?;
        self.ensure_string_backing(
            HLE_FAKE_NSSTRING_MIME_TYPE,
            "NSString.synthetic.response.mimeType<'text/plain'>".to_string(),
            "text/plain",
        )?;
        self.ensure_string_backing(
            HLE_FAKE_NSSTRING_ERROR_DOMAIN,
            format!("NSString.synthetic.error.domain<'{}'>", self.network_error_domain()),
            self.network_error_domain(),
        )?;
        self.ensure_string_backing(
            HLE_FAKE_NSSTRING_ERROR_DESCRIPTION,
            format!(
                "NSString.synthetic.error.localizedDescription<'{}'>",
                self.network_error_description()
            ),
            self.network_error_description(),
        )?;

        let payload = self.synthetic_payload_bytes();
        let data_label = if self.runtime.ui_network.network_data_retained {
            format!(
                "NSData.synthetic#0<{} / {} bytes retained>",
                payload.len(),
                self.network_payload_len()
            )
        } else {
            format!(
                "NSData.synthetic#0<{} / {} bytes>",
                payload.len(),
                self.network_payload_len()
            )
        };
        self.ensure_blob_backing(self.runtime.ui_network.network_data, data_label, &payload)?;
        Ok(())
    }

    fn string_backing(&self, object: u32) -> Option<&SyntheticStringBacking> {
        self.runtime.heap.synthetic_string_backing.get(&object)
    }

    fn ensure_string_backing_for_value(
        &mut self,
        object: u32,
        default_label: &str,
    ) -> Option<SyntheticStringBacking> {
        if object == 0 {
            return None;
        }
        if let Some(existing) = self.string_backing(object) {
            return Some(existing.clone());
        }
        let text = self.guest_string_value(object)?;
        let mut label = self
            .diag
            .object_labels
            .get(&object)
            .cloned()
            .unwrap_or_else(|| default_label.to_string());
        if label.is_empty() || label.starts_with("0x") {
            let snippet = text.chars().take(48).collect::<String>().replace('\n', "\\n");
            label = format!("{}<'{}'>", default_label, snippet);
        }
        self.ensure_string_backing(object, label, &text).ok()?;
        self.string_backing(object).cloned()
    }

    fn blob_backing(&self, object: u32) -> Option<&SyntheticBlobBacking> {
        self.runtime.heap.synthetic_blob_backing.get(&object)
    }

    fn foundation_string_backing_ready(&self) -> bool {
        [
            HLE_FAKE_NSSTRING_URL_ABSOLUTE,
            HLE_FAKE_NSSTRING_URL_HOST,
            HLE_FAKE_NSSTRING_URL_PATH,
            HLE_FAKE_NSSTRING_HTTP_METHOD,
            HLE_FAKE_NSSTRING_MIME_TYPE,
            HLE_FAKE_NSSTRING_ERROR_DOMAIN,
            HLE_FAKE_NSSTRING_ERROR_DESCRIPTION,
        ]
        .iter()
        .all(|obj| self.runtime.heap.synthetic_string_backing.contains_key(obj))
    }

    fn foundation_data_backing_ready(&self) -> bool {
        self.runtime.heap.synthetic_blob_backing.contains_key(&self.runtime.ui_network.network_data)
    }

    fn index_bundle_resources(bundle_root: &Path) -> HashMap<String, PathBuf> {
        let mut index = HashMap::new();
        Self::index_bundle_resources_recursive(bundle_root, bundle_root, &mut index);
        index
    }

    fn index_bundle_resources_recursive(bundle_root: &Path, current: &Path, out: &mut HashMap<String, PathBuf>) {
        let Ok(entries) = fs::read_dir(current) else { return; };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                Self::index_bundle_resources_recursive(bundle_root, &path, out);
                continue;
            }
            let Ok(rel) = path.strip_prefix(bundle_root) else { continue; };
            let rel_norm = rel.to_string_lossy().replace('\\', "/");
            let rel_lower = rel_norm.to_ascii_lowercase();
            out.entry(rel_lower).or_insert_with(|| path.clone());
            if let Some(name) = path.file_name().and_then(|v| v.to_str()) {
                out.entry(name.to_ascii_lowercase()).or_insert_with(|| path.clone());
            }
            if let Some(stem) = path.file_stem().and_then(|v| v.to_str()) {
                out.entry(stem.to_ascii_lowercase()).or_insert_with(|| path.clone());
            }
        }
    }

    fn materialize_host_string_object(&mut self, label: &str, text: &str) -> u32 {
        let obj = self.alloc_synthetic_ui_object(label.to_string());
        if self.ensure_string_backing(obj, label.to_string(), text).is_ok() {
            obj
        } else {
            0
        }
    }

    fn guest_string_value(&self, value: u32) -> Option<String> {
        if value == 0 {
            return None;
        }
        if let Some(backing) = self.string_backing(value) {
            return Some(backing.text.clone());
        }
        if self
            .runtime.objc.objc_section_cfstring
            .map(|range| range.contains(value))
            .unwrap_or(false)
        {
            return self
                .try_decode_cfstring_at(value)
                .or_else(|| self.read_c_string(value, 260));
        }
        self.read_c_string(value, 260)
            .or_else(|| self.try_decode_cfstring_at(value))
    }
}
