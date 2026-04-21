impl MemoryArm32Backend {
    fn maybe_dispatch_cocos_objc_msgsend(
        &mut self,
        index: u64,
        current_pc: u32,
        receiver: u32,
        selector: &str,
        arg2: u32,
        arg3: u32,
        receiver_desc: &str,
        arg2_desc: &str,
        arg3_desc: &str,
    ) -> CoreResult<Option<StepControl>> {
        let Some((result, note)) = self.maybe_handle_cocos_fastpath(selector, receiver, arg2, arg3) else {
            return Ok(None);
        };

        let mut detail = format!(
            "hle/fastpath objc_msgSend(receiver={}, sel={}, arg2={}, arg3={}, result={})",
            receiver_desc,
            selector,
            arg2_desc,
            arg3_desc,
            self.describe_ptr(result),
        );
        if !note.is_empty() {
            detail.push_str(&format!(", note={}", note));
        }
        self.diag.trace
            .push(self.hle_trace_line(index, current_pc, "objc_msgSend", &detail));
        self.cpu.regs[0] = result;
        self.cpu.regs[15] = self.cpu.regs[14] & !1;
        self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
        Ok(Some(StepControl::Continue))
    }
}
