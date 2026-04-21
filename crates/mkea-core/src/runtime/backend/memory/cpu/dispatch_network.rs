impl MemoryArm32Backend {
    fn maybe_handle_network_hle_stub(
        &mut self,
        index: u64,
        current_pc: u32,
        label: &str,
    ) -> CoreResult<Option<StepControl>> {
        match label {
            "CFNetworkCopySystemProxySettings" => {
                self.install_uikit_labels();
                let detail = format!(
                    "hle CFNetworkCopySystemProxySettings() -> {}",
                    self.describe_ptr(self.runtime.ui_network.proxy_settings),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
                self.cpu.regs[0] = self.runtime.ui_network.proxy_settings;
            }
            "SCNetworkReachabilityCreateWithName" => {
                self.install_uikit_labels();
                let host = self
                    .read_c_string(self.cpu.regs[1], 128)
                    .unwrap_or_else(|| "<synthetic-host>".to_string());
                let detail = format!(
                    "hle SCNetworkReachabilityCreateWithName(allocator={}, host='{}') -> {}",
                    self.describe_ptr(self.cpu.regs[0]),
                    host,
                    self.describe_ptr(self.runtime.ui_network.reachability),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
                self.cpu.regs[0] = self.runtime.ui_network.reachability;
            }
            "SCNetworkReachabilitySetCallback" => {
                self.runtime.ui_network.reachability_callback_set = true;
                self.bootstrap_synthetic_runloop();
                let detail = format!(
                    "hle SCNetworkReachabilitySetCallback(target={}, callback={}, context={}) -> YES",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
                self.cpu.regs[0] = 1;
            }
            "SCNetworkReachabilityScheduleWithRunLoop" => {
                self.bootstrap_synthetic_runloop();
                self.runtime.ui_network.reachability_scheduled = true;
                self.recalc_runloop_sources();
                let detail = format!(
                    "hle SCNetworkReachabilityScheduleWithRunLoop(target={}, runloop={}, mode={}) -> YES",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
                self.cpu.regs[0] = 1;
            }
            "SCNetworkReachabilityUnscheduleFromRunLoop" => {
                self.runtime.ui_network.reachability_scheduled = false;
                self.recalc_runloop_sources();
                let detail = format!(
                    "hle SCNetworkReachabilityUnscheduleFromRunLoop(target={}, runloop={}, mode={}) -> YES",
                    self.describe_ptr(self.cpu.regs[0]),
                    self.describe_ptr(self.cpu.regs[1]),
                    self.describe_ptr(self.cpu.regs[2]),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
                self.cpu.regs[0] = 1;
            }
            "SCNetworkReachabilityGetFlags" => {
                self.install_uikit_labels();
                let flags_ptr = self.cpu.regs[1];
                let flags = self.reachability_flags();
                if flags_ptr != 0 {
                    self.write_u32_le(flags_ptr, flags)?;
                }
                let detail = format!(
                    "hle SCNetworkReachabilityGetFlags(target={}, out=0x{:08x}) -> {}",
                    self.describe_ptr(self.cpu.regs[0]),
                    flags_ptr,
                    self.reachability_flags_label(),
                );
                self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
                self.cpu.regs[0] = 1;
            }
            "CFStreamCreatePairWithSocket" | "CFStreamCreatePairWithSocketToHost" => {
                self.bootstrap_synthetic_runloop();
                let (read_out, write_out, detail) = if label == "CFStreamCreatePairWithSocketToHost" {
                    let read_out = self.cpu.regs[3];
                    let write_out = self.read_u32_le(self.cpu.regs[13]).unwrap_or(0);
                    let host = self
                        .read_c_string(self.cpu.regs[1], 128)
                        .unwrap_or_else(|| self.network_host_string().to_string());
                    (
                        read_out,
                        write_out,
                        format!(
                            "hle {}(allocator={}, host='{}', port={}, outRead=0x{:08x}, outWrite=0x{:08x}) -> read={} write={}",
                            label,
                            self.describe_ptr(self.cpu.regs[0]),
                            host,
                            self.cpu.regs[2],
                            read_out,
                            write_out,
                            self.describe_ptr(self.runtime.ui_network.read_stream),
                            self.describe_ptr(self.runtime.ui_network.write_stream),
                        ),
                    )
                } else {
                    let read_out = self.cpu.regs[2];
                    let write_out = self.cpu.regs[3];
                    (
                        read_out,
                        write_out,
                        format!(
                            "hle CFStreamCreatePairWithSocket(allocator={}, socket={}, outRead=0x{:08x}, outWrite=0x{:08x}) -> read={} write={}",
                            self.describe_ptr(self.cpu.regs[0]),
                            self.cpu.regs[1],
                            read_out,
                            write_out,
                            self.describe_ptr(self.runtime.ui_network.read_stream),
                            self.describe_ptr(self.runtime.ui_network.write_stream),
                        ),
                    )
                };
                if read_out != 0 {
                    self.write_u32_le(read_out, self.runtime.ui_network.read_stream)?;
                }
                if write_out != 0 {
                    self.write_u32_le(write_out, self.runtime.ui_network.write_stream)?;
                }
                self.diag.trace.push(self.hle_trace_line(index, current_pc, label, &detail));
                self.cpu.regs[0] = 0;
            }
            _ => return Ok(None),
        }

        self.cpu.regs[15] = self.cpu.regs[14] & !1;
        self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
        Ok(Some(StepControl::Continue))
    }

    fn maybe_dispatch_network_objc_msgsend(
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
        let mut note: Option<String> = None;
        let result = match selector {
            "URLWithString:" => {
                if let Some(raw) = self.guest_string_value(arg2) {
                    if let Some(file_url) = self.create_synthetic_file_url_from_string_request(&raw, false) {
                        note = Some(format!(
                            "file-url <- {}, {}",
                            self.describe_ptr(file_url),
                            self.url_like_debug_summary(file_url, false)
                        ));
                        file_url
                    } else {
                        self.note_network_slot_touch(
                            "url",
                            selector,
                            receiver,
                            self.runtime.ui_network.network_url,
                            self.runtime.ui_network.network_request,
                            "synthetic NSURL returned from constructor",
                        );
                        note = Some(format!(
                            "url <- {}, absoluteString='{}'",
                            self.describe_ptr(self.runtime.ui_network.network_url),
                            self.network_url_string()
                        ));
                        self.runtime.ui_network.network_url
                    }
                } else {
                    self.note_network_slot_touch(
                        "url",
                        selector,
                        receiver,
                        self.runtime.ui_network.network_url,
                        self.runtime.ui_network.network_request,
                        "synthetic NSURL returned from constructor",
                    );
                    note = Some(format!(
                        "url <- {}, absoluteString='{}'",
                        self.describe_ptr(self.runtime.ui_network.network_url),
                        self.network_url_string()
                    ));
                    self.runtime.ui_network.network_url
                }
            }
            "fileURLWithPath:" | "initFileURLWithPath:" => {
                if let Some(raw) = self.guest_string_value(arg2) {
                    let file_url = self.create_synthetic_file_url_from_string_request(&raw, false).unwrap_or(0);
                    note = Some(format!(
                        "file-url <- {}, {}",
                        self.describe_ptr(file_url),
                        self.url_like_debug_summary(file_url, false)
                    ));
                    file_url
                } else {
                    note = Some(format!("file-url miss pathArg={}", self.describe_ptr(arg2)));
                    0
                }
            }
            "fileURLWithPath:isDirectory:" | "initFileURLWithPath:isDirectory:" => {
                if let Some(raw) = self.guest_string_value(arg2) {
                    let is_directory = arg3 != 0;
                    let file_url = self.create_synthetic_file_url_from_string_request(&raw, is_directory).unwrap_or(0);
                    note = Some(format!(
                        "file-url <- {}, {}",
                        self.describe_ptr(file_url),
                        self.url_like_debug_summary(file_url, is_directory)
                    ));
                    file_url
                } else {
                    note = Some(format!(
                        "file-url miss pathArg={} isDirectory={}",
                        self.describe_ptr(arg2),
                        if arg3 != 0 { "YES" } else { "NO" }
                    ));
                    0
                }
            }
            "URLByAppendingPathComponent:" => {
                if let Some(base) = self.resolve_path_from_url_like_value(receiver, self.synthetic_file_url_is_directory(receiver)) {
                    let component = self.guest_string_value(arg2).unwrap_or_default();
                    if component.is_empty() {
                        note = Some(format!("append-path noop base={} component=<empty>", self.describe_ptr(receiver)));
                        receiver
                    } else {
                        let next = base.join(component.trim_matches('/').trim_matches('\\'));
                        let next_is_directory = false;
                        let next_url = self
                            .create_synthetic_file_url_from_string_request(&next.display().to_string(), next_is_directory)
                            .unwrap_or(0);
                        note = Some(format!(
                            "append-path base={} component='{}' -> {}, {}",
                            self.describe_ptr(receiver),
                            component,
                            self.describe_ptr(next_url),
                            self.url_like_debug_summary(next_url, next_is_directory)
                        ));
                        next_url
                    }
                } else {
                    return Ok(None);
                }
            }
            "requestWithURL:" | "initWithURL:" => {
                self.note_network_slot_touch(
                    "request",
                    selector,
                    receiver,
                    self.runtime.ui_network.network_request,
                    self.runtime.ui_network.network_request,
                    "synthetic NSURLRequest returned from constructor",
                );
                note = Some(format!(
                    "request <- {}, method={}, url={}",
                    self.describe_ptr(self.runtime.ui_network.network_request),
                    self.network_http_method(),
                    self.describe_ptr(self.runtime.ui_network.network_url)
                ));
                self.runtime.ui_network.network_request
            }
            "connectionWithRequest:delegate:" | "initWithRequest:delegate:" | "initWithRequest:delegate:startImmediately:" => {
                self.note_network_slot_touch(
                    "connection",
                    selector,
                    receiver,
                    self.runtime.ui_network.network_connection,
                    arg2,
                    "synthetic NSURLConnection construction site",
                );
                if arg3 != 0 {
                    let previous_delegate = self.runtime.ui_network.network_delegate;
                    self.diag.object_labels
                        .entry(arg3)
                        .or_insert_with(|| "NSURLConnection.delegate(instance)".to_string());
                    self.note_network_connection_birth(selector, receiver, arg2, arg3, self.runtime.ui_network.network_connection);
                    self.note_objc_delegate_binding(selector, "NSURLConnection.delegate", receiver, arg3, arg2, previous_delegate);
                    self.assign_network_delegate_with_provenance(
                        selector,
                        receiver,
                        arg2,
                        arg3,
                        "connection constructor delegate assignment",
                    );
                }
                self.runtime.ui_network.network_completed = false;
                self.runtime.ui_network.network_armed = true;
                self.runtime.ui_network.network_stage = 0;
                self.runtime.ui_network.network_bytes_delivered = 0;
                self.runtime.ui_network.network_source_closed = false;
                self.runtime.ui_network.network_response_retained = false;
                self.runtime.ui_network.network_data_retained = false;
                self.runtime.ui_network.network_error_retained = false;
                self.runtime.ui_network.network_timeout_armed = false;
                self.runtime.ui_network.network_faulted = false;
                self.runtime.ui_network.network_cancelled = false;
                self.runtime.ui_network.network_fault_mode = 0;
                self.runtime.ui_network.network_fault_history.clear();
                self.runtime.ui_runtime.idle_ticks_after_completion = 0;
                self.reset_synthetic_stream_transport();
                self.refresh_network_object_labels();
                self.recalc_runloop_sources();
                let start_immediately = if selector == "initWithRequest:delegate:startImmediately:" {
                    self.peek_stack_u32(0).unwrap_or(0) != 0
                } else {
                    false
                };
                if start_immediately {
                    self.runtime.ui_network.network_armed = true;
                }
                note = Some(format!(
                    "connection <- {}, delegate <- {}, request={}, response={}, data={}, startImmediately={}",
                    self.describe_ptr(self.runtime.ui_network.network_connection),
                    self.describe_ptr(self.current_network_delegate()),
                    self.describe_ptr(self.runtime.ui_network.network_request),
                    self.describe_ptr(self.runtime.ui_network.network_response),
                    self.describe_ptr(self.runtime.ui_network.network_data),
                    if start_immediately { "YES" } else { "NO" }
                ));
                self.runtime.ui_network.network_connection
            }
            "scheduleInRunLoop:forMode:" => {
                self.bootstrap_synthetic_runloop();
                self.runtime.ui_network.network_armed = true;
                self.runtime.ui_network.network_source_closed = false;
                self.refresh_network_object_labels();
                self.recalc_runloop_sources();
                note = Some(format!(
                    "scheduled {} in {} for request {}",
                    self.describe_ptr(receiver),
                    self.describe_ptr(arg3),
                    self.describe_ptr(self.runtime.ui_network.network_request)
                ));
                receiver
            }
            "start" => {
                self.bootstrap_synthetic_runloop();
                self.runtime.ui_network.network_armed = true;
                self.runtime.ui_network.network_completed = false;
                self.runtime.ui_network.network_bytes_delivered = 0;
                self.runtime.ui_network.network_source_closed = false;
                self.runtime.ui_network.network_response_retained = false;
                self.runtime.ui_network.network_data_retained = false;
                self.runtime.ui_network.network_error_retained = false;
                self.runtime.ui_network.network_timeout_armed = false;
                self.runtime.ui_network.network_faulted = false;
                self.runtime.ui_network.network_cancelled = false;
                self.runtime.ui_network.network_fault_mode = 0;
                self.runtime.ui_network.network_fault_history.clear();
                self.runtime.ui_runtime.idle_ticks_after_completion = 0;
                if self.runtime.ui_network.network_stage > 2 {
                    self.runtime.ui_network.network_stage = 0;
                }
                self.refresh_network_object_labels();
                self.recalc_runloop_sources();
                note = Some(format!(
                    "network start {} url='{}' host='{}' method={}",
                    self.describe_ptr(receiver),
                    self.network_url_string(),
                    self.network_host_string(),
                    self.network_http_method()
                ));
                receiver
            }
            "cancel" => {
                self.runtime.ui_network.network_armed = false;
                self.runtime.ui_network.network_completed = false;
                self.runtime.ui_network.network_source_closed = true;
                self.runtime.ui_network.network_timeout_armed = false;
                self.runtime.ui_network.network_faulted = false;
                self.runtime.ui_network.network_cancelled = true;
                self.runtime.ui_network.network_fault_mode = 4;
                self.runtime.ui_network.network_error_retained = true;
                self.refresh_network_object_labels();
                self.recalc_runloop_sources();
                note = Some(format!(
                    "network cancel {} error={} code={} state=cancelled retry=NO",
                    self.describe_ptr(receiver),
                    self.describe_ptr(self.runtime.ui_network.network_error),
                    self.network_error_code()
                ));
                receiver
            }
            "isLoading" => {
                if self.runtime.ui_network.network_armed && !self.runtime.ui_network.network_completed { 1 } else { 0 }
            }
            "isCancelled" => if self.runtime.ui_network.network_cancelled { 1 } else { 0 },
            "hasError" => {
                if self.runtime.ui_network.network_faulted || self.runtime.ui_network.network_cancelled { 1 } else { 0 }
            }
            "shouldRetry" | "canReconnect" => if self.network_should_retry() { 1 } else { 0 },
            "connectionState" => self.network_connection_state_code(),
            "failureMode" => {
                if self.runtime.ui_network.network_faulted || self.runtime.ui_network.network_cancelled {
                    self.runtime.ui_network.network_error
                } else {
                    0
                }
            }
            "loadedData" | "receivedData" | "body" => {
                if receiver == self.runtime.ui_network.fault_connection {
                    if self.network_fault_has_data() { self.runtime.ui_network.network_data } else { 0 }
                } else {
                    self.runtime.ui_network.network_data
                }
            }
            "retainedResponse" => {
                if self.runtime.ui_network.network_response_retained { self.runtime.ui_network.network_response } else { 0 }
            }
            "retainedData" => {
                if self.runtime.ui_network.network_data_retained { self.runtime.ui_network.network_data } else { 0 }
            }
            "retainedError" => {
                if self.runtime.ui_network.network_error_retained { self.runtime.ui_network.network_error } else { 0 }
            }
            "proxySettings" => self.runtime.ui_network.proxy_settings,
            "connection:didReceiveResponse:" => {
                note = Some(format!(
                    "delegate callback response conn={} response={} status=200 mime=text/plain state=response-received",
                    self.describe_ptr(arg2),
                    self.describe_ptr(arg3)
                ));
                0
            }
            "connection:didReceiveData:" => {
                note = Some(format!(
                    "delegate callback data conn={} data={} bytes={} state=receiving delivered={}/{}",
                    self.describe_ptr(arg2),
                    self.describe_ptr(arg3),
                    self.network_payload_len(),
                    self.runtime.ui_network.network_bytes_delivered,
                    self.network_payload_len()
                ));
                0
            }
            "connectionDidFinishLoading:" => {
                note = Some(format!(
                    "delegate callback finish conn={} request={} response={} state=completed loading=NO",
                    self.describe_ptr(arg2),
                    self.describe_ptr(self.runtime.ui_network.network_request),
                    self.describe_ptr(self.runtime.ui_network.network_response)
                ));
                0
            }
            "connection:didFailWithError:" => {
                note = Some(format!(
                    "delegate callback error conn={} error={} domain={} code={} state={}",
                    self.describe_ptr(arg2),
                    self.describe_ptr(arg3),
                    self.network_error_domain(),
                    self.network_error_code(),
                    if self.runtime.ui_network.network_cancelled { "cancelled" } else { "failed-timeout" }
                ));
                0
            }
            _ => return Ok(None),
        };

        self.finish_objc_msgsend_hle_dispatch(
            index,
            current_pc,
            "objc_msgSend",
            receiver_desc,
            selector,
            arg2_desc,
            arg3_desc,
            result,
            note,
        )
    }
}
