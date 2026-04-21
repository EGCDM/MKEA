impl MemoryArm32Backend {
// Synthetic NSURLConnection reachability, fault probes, and runloop source driving.

    fn network_state_label(&self) -> String {
        if self.runtime.ui_network.network_cancelled {
            format!(
                "cancelled code={} retained(error={})",
                self.network_error_code(),
                Self::retained_flag(self.runtime.ui_network.network_error_retained),
            )
        } else if self.runtime.ui_network.network_faulted {
            format!(
                "failed {} code={} retained(error={})",
                self.network_error_kind(),
                self.network_error_code(),
                Self::retained_flag(self.runtime.ui_network.network_error_retained),
            )
        } else if self.runtime.ui_network.network_completed {
            if self.runtime.ui_network.network_source_closed {
                format!(
                    "idle-complete {}/{} retained(response={},data={})",
                    self.runtime.ui_network.network_bytes_delivered,
                    self.network_payload_len(),
                    Self::retained_flag(self.runtime.ui_network.network_response_retained),
                    Self::retained_flag(self.runtime.ui_network.network_data_retained),
                )
            } else {
                format!("completed {}/{}", self.runtime.ui_network.network_bytes_delivered, self.network_payload_len())
            }
        } else if self.runtime.ui_network.network_armed {
            match self.runtime.ui_network.network_stage {
                0 => "scheduled".to_string(),
                1 => format!("response-ready 0/{}", self.network_payload_len()),
                _ => format!("receiving {}/{}", self.runtime.ui_network.network_bytes_delivered, self.network_payload_len()),
            }
        } else {
            format!("idle {}/{}", self.runtime.ui_network.network_bytes_delivered, self.network_payload_len())
        }
    }




    fn network_fault_state_label(&self) -> String {
        if self.runtime.ui_network.network_cancelled {
            format!(
                "cancelled code={} retained(error={})",
                self.network_error_code(),
                Self::retained_flag(self.runtime.ui_network.network_error_retained),
            )
        } else if self.runtime.ui_network.network_faulted {
            format!(
                "failed {} code={} retained(error={})",
                self.network_error_kind(),
                self.network_error_code(),
                Self::retained_flag(self.runtime.ui_network.network_error_retained),
            )
        } else if self.runtime.ui_network.network_timeout_armed {
            format!("scheduled-{}", self.network_error_kind())
        } else {
            "idle".to_string()
        }
    }

    fn arm_synthetic_fault_probe(&mut self, mode: u32) {
        self.runtime.ui_network.network_fault_mode = mode;
        self.runtime.ui_network.network_timeout_armed = true;
        self.runtime.ui_network.network_faulted = false;
        self.runtime.ui_network.network_cancelled = false;
        self.runtime.ui_network.network_error_retained = false;
        self.runtime.ui_network.network_response_retained = false;
        self.runtime.ui_network.network_data_retained = false;
        self.runtime.ui_network.network_bytes_delivered = 0;
        self.refresh_network_object_labels();
        self.recalc_runloop_sources();
        self.diag.trace.push(format!(
            "     ↳ hle NSURLConnection.faultProbeArmed conn={} error={} mode={} url={}",
            self.describe_ptr(self.runtime.ui_network.fault_connection),
            self.describe_ptr(self.runtime.ui_network.network_error),
            self.network_error_kind(),
            self.describe_ptr(self.runtime.ui_network.network_url),
        ));
    }

    fn refresh_network_object_labels(&mut self) {
        self.note_network_seeded_slots("refresh_network_object_labels");
        self.sync_stream_transport_state();
        self.diag.object_labels.insert(
            self.runtime.ui_network.network_url,
            format!("NSURL.synthetic#0<'{}'>", self.network_url_string()),
        );
        self.diag.object_labels.insert(
            self.runtime.ui_network.network_request,
            format!(
                "NSURLRequest.synthetic#0<{} {}>",
                self.network_http_method(),
                self.network_path_string()
            ),
        );
        self.diag.object_labels.insert(
            self.runtime.ui_network.network_connection,
            format!(
                "NSURLConnection.synthetic#0<delegate={} state={}>",
                self.describe_ptr(self.current_network_delegate()),
                self.network_state_label(),
            ),
        );
        self.diag.object_labels.insert(
            self.runtime.ui_network.network_response,
            if self.runtime.ui_network.network_response_retained {
                if self.runtime.ui_network.network_faulted || self.runtime.ui_network.network_cancelled {
                    format!(
                        "NSHTTPURLResponse.synthetic#0<200 text/plain {} bytes retained after {}>",
                        self.network_payload_len(),
                        self.network_error_kind()
                    )
                } else {
                    format!(
                        "NSHTTPURLResponse.synthetic#0<200 text/plain {} bytes retained>",
                        self.network_payload_len()
                    )
                }
            } else {
                format!(
                    "NSHTTPURLResponse.synthetic#0<200 text/plain {} bytes>",
                    self.network_payload_len()
                )
            },
        );
        self.diag.object_labels.insert(
            self.runtime.ui_network.network_data,
            if self.runtime.ui_network.network_data_retained {
                if self.runtime.ui_network.network_faulted || self.runtime.ui_network.network_cancelled {
                    format!(
                        "NSData.synthetic#0<{} / {} bytes retained after {}>",
                        self.runtime.ui_network.network_bytes_delivered,
                        self.network_payload_len(),
                        self.network_error_kind()
                    )
                } else {
                    format!(
                        "NSData.synthetic#0<{} / {} bytes retained>",
                        self.runtime.ui_network.network_bytes_delivered,
                        self.network_payload_len()
                    )
                }
            } else {
                format!(
                    "NSData.synthetic#0<{} / {} bytes>",
                    self.runtime.ui_network.network_bytes_delivered,
                    self.network_payload_len()
                )
            },
        );
        self.diag.object_labels.insert(
            self.runtime.ui_network.proxy_settings,
            "CFProxySettings.synthetic#0<direct/no-proxy>".to_string(),
        );
        self.diag.object_labels.insert(
            self.runtime.ui_network.fault_connection,
            format!(
                "NSURLConnection.synthetic#fault<delegate={} state={}>",
                self.describe_ptr(self.current_network_delegate()),
                self.network_fault_state_label(),
            ),
        );
        self.diag.object_labels.insert(
            self.runtime.ui_network.network_error,
            if self.runtime.ui_network.network_error_retained {
                format!(
                    "NSError.synthetic#0<{} {} {} retained>",
                    self.network_error_domain(),
                    self.network_error_code(),
                    self.network_error_kind()
                )
            } else {
                format!(
                    "NSError.synthetic#0<{} {} {}>",
                    self.network_error_domain(),
                    self.network_error_code(),
                    self.network_error_kind()
                )
            },
        );
        self.diag.object_labels.insert(
            self.runtime.ui_network.read_stream,
            format!(
                "CFReadStream.synthetic#0<state={} read={}/{} scheduled={} open={}>",
                Self::stream_status_name(self.runtime.ui_network.read_stream_status),
                self.runtime.ui_network.read_stream_bytes_consumed,
                self.synthetic_payload_bytes().len(),
                if self.runtime.ui_network.read_stream_scheduled { "YES" } else { "NO" },
                if self.runtime.ui_network.read_stream_open { "YES" } else { "NO" },
            ),
        );
        self.diag.object_labels.insert(
            self.runtime.ui_network.write_stream,
            format!(
                "CFWriteStream.synthetic#0<state={} written={} scheduled={} open={}>",
                Self::stream_status_name(self.runtime.ui_network.write_stream_status),
                self.runtime.ui_network.write_stream_bytes_written,
                if self.runtime.ui_network.write_stream_scheduled { "YES" } else { "NO" },
                if self.runtime.ui_network.write_stream_open { "YES" } else { "NO" },
            ),
        );
        self.diag.object_labels.insert(
            self.runtime.ui_network.reachability,
            format!(
                "SCNetworkReachability.synthetic#0<flags={} scheduled={} callback={}>",
                self.reachability_flags_label(),
                if self.runtime.ui_network.reachability_scheduled { "YES" } else { "NO" },
                if self.runtime.ui_network.reachability_callback_set { "YES" } else { "NO" },
            ),
        );
        self.refresh_graphics_object_labels();
        let _ = self.refresh_foundation_backing_objects();
    }

    fn current_network_delegate(&self) -> u32 {
        if self.runtime.ui_network.network_delegate != 0 {
            self.runtime.ui_network.network_delegate
        } else {
            self.runtime.ui_objects.delegate
        }
    }

    fn emit_network_delegate_callback(
        &mut self,
        selector: &str,
        arg2: u32,
        arg3: u32,
        result: &str,
        detail: String,
    ) {
        self.runtime.ui_network.delegate_callbacks = self.runtime.ui_network.delegate_callbacks.saturating_add(1);
        let target = self.current_network_delegate();
        let target_class = self.objc_receiver_class_name_hint(target).unwrap_or_else(|| "<unknown>".to_string());
        let observed_candidates = self.objc_observed_receiver_candidates_summary(selector);
        let created_candidates = self.objc_created_receiver_candidates_summary(selector);
        self.push_callback_trace(format!(
            "network.dispatch tick={} sel={} target={} targetClass={} conn={} request={} response={} data={} observed={} created={}",
            self.runtime.ui_runtime.runloop_ticks,
            selector,
            self.describe_ptr(target),
            target_class,
            self.describe_ptr(self.runtime.ui_network.network_connection),
            self.describe_ptr(self.runtime.ui_network.network_request),
            self.describe_ptr(self.runtime.ui_network.network_response),
            self.describe_ptr(self.runtime.ui_network.network_data),
            observed_candidates,
            created_candidates,
        ));
        if self.runtime.ui_network.network_connection_birth_trace.is_empty()
            || self.runtime.ui_network.network_delegate_binding_trace.is_empty()
        {
            self.push_callback_trace(format!(
                "network.provenance-gap tick={} sel={} target={} targetClass={} birthTrace={} delegateBindings={} ownerHint={} firstAppBind={} lastSlot={}",
                self.runtime.ui_runtime.runloop_ticks,
                selector,
                self.describe_ptr(target),
                target_class,
                self.runtime.ui_network.network_connection_birth_trace.len(),
                self.runtime.ui_network.network_delegate_binding_trace.len(),
                self.runtime.ui_network.network_last_owner_candidate.clone().unwrap_or_else(|| "<none>".to_string()),
                self.runtime.ui_network.first_app_delegate_binding.clone().unwrap_or_else(|| "<none>".to_string()),
                self.runtime.ui_network.network_last_slot_event.clone().unwrap_or_else(|| "<none>".to_string()),
            ));
        }
        let invoked = if target != 0 {
            self.invoke_objc_selector_now_resolved(
                target,
                selector,
                arg2,
                arg3,
                120_000,
                "synthetic-network-callback",
            )
        } else {
            false
        };
        if selector == "connectionDidFinishLoading:" && !invoked {
            self.push_callback_trace(format!(
                "network.finish-miss tick={} target={} targetClass={} observed={} created={} ownerHint={} firstAppBind={} lastSlot={}",
                self.runtime.ui_runtime.runloop_ticks,
                self.describe_ptr(target),
                target_class,
                observed_candidates,
                created_candidates,
                self.runtime.ui_network.network_last_owner_candidate.clone().unwrap_or_else(|| "<none>".to_string()),
                self.runtime.ui_network.first_app_delegate_binding.clone().unwrap_or_else(|| "<none>".to_string()),
                self.runtime.ui_network.network_last_slot_event.clone().unwrap_or_else(|| "<none>".to_string()),
            ));
        }
        self.diag.trace.push(format!(
            "     ↳ hle delegate objc_msgSend(receiver={}, sel={}, arg2={}, arg3={}, result={}, invoked={}, detail={})",
            self.describe_ptr(target),
            selector,
            self.describe_ptr(arg2),
            self.describe_ptr(arg3),
            result,
            if invoked { "YES" } else { "NO" },
            detail,
        ));
    }

    fn install_uikit_labels(&mut self) {
        self.diag.object_labels
            .entry(self.runtime.ui_objects.app)
            .or_insert_with(|| "UIApplication.sharedApplication".to_string());
        self.diag.object_labels
            .entry(self.runtime.ui_objects.delegate)
            .or_insert_with(|| "UIApplication.delegate".to_string());
        if self.runtime.ui_network.network_delegate != 0 {
            self.diag.object_labels
                .entry(self.runtime.ui_network.network_delegate)
                .or_insert_with(|| "NSURLConnection.delegate".to_string());
        }
        self.diag.object_labels
            .entry(self.runtime.ui_objects.window)
            .or_insert_with(|| "UIWindow.main".to_string());
        self.diag.object_labels
            .entry(self.runtime.ui_objects.root_controller)
            .or_insert_with(|| "UIViewController.root".to_string());
        self.diag.object_labels
            .entry(self.runtime.ui_objects.screen)
            .or_insert_with(|| "UIScreen.mainScreen".to_string());
        self.diag.object_labels
            .entry(self.runtime.ui_objects.main_runloop)
            .or_insert_with(|| "NSRunLoop.mainRunLoop".to_string());
        self.diag.object_labels
            .entry(self.runtime.ui_objects.default_mode)
            .or_insert_with(|| "kCFRunLoopDefaultMode".to_string());
        self.diag.object_labels
            .entry(self.runtime.ui_objects.synthetic_timer)
            .or_insert_with(|| "NSTimer.synthetic#0".to_string());
        self.diag.object_labels
            .entry(self.runtime.ui_cocos.synthetic_display_link)
            .or_insert_with(|| "CADisplayLink.synthetic#0".to_string());
        self.diag.object_labels
            .entry(self.runtime.ui_network.reachability)
            .or_insert_with(|| "SCNetworkReachability.synthetic#0".to_string());
        self.diag.object_labels
            .entry(self.runtime.ui_graphics.eagl_context)
            .or_insert_with(|| "EAGLContext.synthetic#0".to_string());
        self.diag.object_labels
            .entry(self.runtime.ui_graphics.eagl_layer)
            .or_insert_with(|| "CAEAGLLayer.synthetic#0".to_string());
        self.diag.object_labels
            .entry(self.runtime.ui_graphics.gl_framebuffer)
            .or_insert_with(|| "GLFramebuffer.synthetic#0".to_string());
        self.diag.object_labels
            .entry(self.runtime.ui_graphics.gl_renderbuffer)
            .or_insert_with(|| "GLRenderbuffer.synthetic#0".to_string());
        if self.runtime.ui_cocos.cocos_director != 0 {
            self.diag.object_labels
                .entry(self.runtime.ui_cocos.cocos_director)
                .or_insert_with(|| "CCDirector.synthetic#0".to_string());
        }
        if self.runtime.ui_cocos.opengl_view != 0 {
            self.diag.object_labels
                .entry(self.runtime.ui_cocos.opengl_view)
                .or_insert_with(|| "EAGLView.synthetic#0".to_string());
        }
        if self.runtime.ui_cocos.running_scene != 0 {
            self.diag.object_labels
                .entry(self.runtime.ui_cocos.running_scene)
                .or_insert_with(|| "CCScene.running".to_string());
        }
        self.refresh_network_object_labels();
        if self.runtime.ui_objects.first_responder != 0 {
            self.diag.object_labels
                .entry(self.runtime.ui_objects.first_responder)
                .or_insert_with(|| "UIResponder.firstResponder".to_string());
        }
    }


    fn drive_runloop_reachability_source(&mut self) {
        if !self.runtime.ui_network.reachability_scheduled || !self.runtime.ui_network.reachability_callback_set {
            return;
        }

        let target = self.current_network_delegate();
        let target_class = self
            .objc_receiver_class_name_hint(target)
            .unwrap_or_else(|| "<unknown>".to_string());
        let has_real_selector = target != 0
            && self.objc_lookup_imp_for_receiver(target, "reachabilityChanged:").is_some();

        if !has_real_selector {
            self.diag.trace.push(format!(
                "     ↳ hle SCNetworkReachability callback suppressed target={} class={} flags={} reason=no-real-reachabilityChanged-selector",
                self.describe_ptr(target),
                target_class,
                self.reachability_flags_label(),
            ));
            self.push_callback_trace(format!(
                "reachability.suppressed tick={} target={} targetClass={} flags={} callbackSet={} scheduled={} reason=no-real-selector",
                self.runtime.ui_runtime.runloop_ticks,
                self.describe_ptr(target),
                target_class,
                self.reachability_flags_label(),
                if self.runtime.ui_network.reachability_callback_set { "YES" } else { "NO" },
                if self.runtime.ui_network.reachability_scheduled { "YES" } else { "NO" },
            ));
            return;
        }

        self.runtime.ui_network.network_events = self.runtime.ui_network.network_events.saturating_add(1);
        self.diag.trace.push(format!(
            "     ↳ hle SCNetworkReachability callback {} flags={} target={} class={}",
            self.describe_ptr(self.runtime.ui_network.reachability),
            self.reachability_flags_label(),
            self.describe_ptr(target),
            target_class,
        ));
        self.emit_network_delegate_callback(
            "reachabilityChanged:",
            self.runtime.ui_network.reachability,
            0,
            "notified",
            format!(
                "target={} flags={} state=reachable",
                self.describe_ptr(self.runtime.ui_network.reachability),
                self.reachability_flags_label(),
            ),
        );
    }

    fn poll_runloop_stream_sources(&mut self) {
        if self.runtime.ui_network.read_stream_scheduled && self.runtime.ui_network.read_stream_open {
            self.runtime.ui_network.read_stream_events = self.runtime.ui_network.read_stream_events.saturating_add(1);
            self.diag.trace.push(format!(
                "     ↳ hle CFReadStream.poll {} status={} available={} consumed={}/{}",
                self.describe_ptr(self.runtime.ui_network.read_stream),
                Self::stream_status_name(self.runtime.ui_network.read_stream_status),
                if self.read_stream_has_bytes_available() { "YES" } else { "NO" },
                self.runtime.ui_network.read_stream_bytes_consumed,
                self.synthetic_payload_bytes().len(),
            ));
            if self.runtime.ui_network.read_stream_client_set {
                self.push_callback_trace(format!(
                    "stream.client kind=read tick={} stream={} status={} available={} callback={} context={} flags=0x{:08x} watchScene={} watchOrigin={}",
                    self.runtime.ui_runtime.runloop_ticks,
                    self.describe_ptr(self.runtime.ui_network.read_stream),
                    Self::stream_status_name(self.runtime.ui_network.read_stream_status),
                    if self.read_stream_has_bytes_available() { "YES" } else { "NO" },
                    self.describe_ptr(self.runtime.ui_network.read_stream_client_callback),
                    self.describe_ptr(self.runtime.ui_network.read_stream_client_context),
                    self.runtime.ui_network.read_stream_client_flags,
                    self.describe_ptr(self.scheduler_trace_watch_scene()),
                    self.runtime.scheduler
                        .trace.window_origin
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                ));
            }
        }
        if self.runtime.ui_network.write_stream_scheduled && self.runtime.ui_network.write_stream_open {
            self.runtime.ui_network.write_stream_events = self.runtime.ui_network.write_stream_events.saturating_add(1);
            self.diag.trace.push(format!(
                "     ↳ hle CFWriteStream.poll {} status={} canAccept={} written={}",
                self.describe_ptr(self.runtime.ui_network.write_stream),
                Self::stream_status_name(self.runtime.ui_network.write_stream_status),
                if self.write_stream_can_accept_bytes() { "YES" } else { "NO" },
                self.runtime.ui_network.write_stream_bytes_written,
            ));
            if self.runtime.ui_network.write_stream_client_set {
                self.push_callback_trace(format!(
                    "stream.client kind=write tick={} stream={} status={} canAccept={} callback={} context={} flags=0x{:08x} watchScene={} watchOrigin={}",
                    self.runtime.ui_runtime.runloop_ticks,
                    self.describe_ptr(self.runtime.ui_network.write_stream),
                    Self::stream_status_name(self.runtime.ui_network.write_stream_status),
                    if self.write_stream_can_accept_bytes() { "YES" } else { "NO" },
                    self.describe_ptr(self.runtime.ui_network.write_stream_client_callback),
                    self.describe_ptr(self.runtime.ui_network.write_stream_client_context),
                    self.runtime.ui_network.write_stream_client_flags,
                    self.describe_ptr(self.scheduler_trace_watch_scene()),
                    self.runtime.scheduler
                        .trace.window_origin
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                ));
            }
        }
    }

    fn drive_runloop_network_sources(&mut self) {
        if self.runtime.ui_network.network_timeout_armed {
            self.handle_network_timeout_tick();
        }
        if self.runtime.ui_network.network_armed && !self.runtime.ui_network.network_completed {
            self.handle_active_network_tick();
        } else if self.runtime.ui_network.network_completed && self.runtime.ui_network.network_source_closed {
            self.handle_completed_network_idle_tick();
        }
    }

    fn handle_network_timeout_tick(&mut self) {
        self.runtime.ui_network.network_events = self.runtime.ui_network.network_events.saturating_add(1);
        self.runtime.ui_network.network_fault_events = self.runtime.ui_network.network_fault_events.saturating_add(1);
        self.runtime.ui_network.network_timeout_armed = false;
        self.runtime.ui_network.network_error_retained = true;
        self.runtime.ui_network.network_source_closed = true;
        self.runtime.ui_network.network_armed = false;
        match self.runtime.ui_network.network_fault_mode {
            3 => {
                self.runtime.ui_network.network_faulted = true;
                self.runtime.ui_network.network_cancelled = false;
                self.runtime.ui_network.network_response_retained = true;
                self.runtime.ui_network.network_data_retained = true;
                self.runtime.ui_network.network_bytes_delivered = self.network_payload_len() / 2;
            }
            4 => {
                self.runtime.ui_network.network_cancelled = true;
                self.runtime.ui_network.network_faulted = false;
                self.runtime.ui_network.network_response_retained = false;
                self.runtime.ui_network.network_data_retained = false;
                self.runtime.ui_network.network_bytes_delivered = 0;
            }
            _ => {
                self.runtime.ui_network.network_faulted = true;
                self.runtime.ui_network.network_cancelled = false;
                self.runtime.ui_network.network_response_retained = false;
                self.runtime.ui_network.network_data_retained = false;
                self.runtime.ui_network.network_bytes_delivered = 0;
            }
        }
        let failure_state = if self.runtime.ui_network.network_fault_mode == 4 {
            "cancelled".to_string()
        } else {
            format!("failed-{}", self.network_error_kind())
        };
        let kind = self.network_error_kind().to_string();
        if !self.runtime.ui_network.network_fault_history.iter().any(|k| k == &kind) {
            self.runtime.ui_network.network_fault_history.push(kind);
        }
        self.refresh_network_object_labels();
        self.diag.trace.push(format!(
            "     ↳ hle NSURLConnection.didFailWithError conn={} error={} domain={} code={} detail={} state={} response={} data={} shouldRetry={}",
            self.describe_ptr(self.runtime.ui_network.fault_connection),
            self.describe_ptr(self.runtime.ui_network.network_error),
            self.network_error_domain(),
            self.network_error_code(),
            self.network_error_description(),
            failure_state,
            if self.network_fault_has_response() {
                self.describe_ptr(self.runtime.ui_network.network_response)
            } else {
                "nil".to_string()
            },
            if self.network_fault_has_data() {
                self.describe_ptr(self.runtime.ui_network.network_data)
            } else {
                "nil".to_string()
            },
            if self.network_should_retry() { "YES" } else { "NO" },
        ));
        self.diag.trace.push(format!(
            "     ↳ hle NSURLConnection.sourceClosed conn={} runloop={} mode={} loading=NO state={}",
            self.describe_ptr(self.runtime.ui_network.fault_connection),
            self.describe_ptr(self.runtime.ui_objects.main_runloop),
            self.describe_ptr(self.runtime.ui_objects.default_mode),
            failure_state,
        ));
        self.emit_network_delegate_callback(
            "connection:didFailWithError:",
            self.runtime.ui_network.fault_connection,
            self.runtime.ui_network.network_error,
            self.network_failure_result(),
            format!(
                "conn={} request={} error={} domain={} code={} loading=NO state={} source=closed retainedError={} response={} data={} shouldRetry={}",
                self.describe_ptr(self.runtime.ui_network.fault_connection),
                self.describe_ptr(self.runtime.ui_network.network_request),
                self.describe_ptr(self.runtime.ui_network.network_error),
                self.network_error_domain(),
                self.network_error_code(),
                failure_state,
                Self::retained_flag(self.runtime.ui_network.network_error_retained),
                if self.network_fault_has_response() {
                    self.describe_ptr(self.runtime.ui_network.network_response)
                } else {
                    "nil".to_string()
                },
                if self.network_fault_has_data() {
                    self.describe_ptr(self.runtime.ui_network.network_data)
                } else {
                    "nil".to_string()
                },
                if self.network_should_retry() { "YES" } else { "NO" },
            ),
        );
        self.recalc_runloop_sources();
    }

    fn handle_active_network_tick(&mut self) {
        self.runtime.ui_network.network_events = self.runtime.ui_network.network_events.saturating_add(1);
        match self.runtime.ui_network.network_stage {
            0 => {
                self.diag.trace.push(format!(
                    "     ↳ hle NSURLConnection.didReceiveResponse conn={} response={} status=200 mime=text/plain state=response-received",
                    self.describe_ptr(self.runtime.ui_network.network_connection),
                    self.describe_ptr(self.runtime.ui_network.network_response),
                ));
                self.emit_network_delegate_callback(
                    "connection:didReceiveResponse:",
                    self.runtime.ui_network.network_connection,
                    self.runtime.ui_network.network_response,
                    "continue-loading",
                    format!(
                        "conn={} response={} url={} status=200 mime=text/plain state=response-received",
                        self.describe_ptr(self.runtime.ui_network.network_connection),
                        self.describe_ptr(self.runtime.ui_network.network_response),
                        self.describe_ptr(self.runtime.ui_network.network_url),
                    ),
                );
            }
            1 => {
                self.runtime.ui_network.network_bytes_delivered = self.network_payload_len();
                self.refresh_network_object_labels();
                self.diag.trace.push(format!(
                    "     ↳ hle NSURLConnection.didReceiveData conn={} data={} bytes={} state=receiving",
                    self.describe_ptr(self.runtime.ui_network.network_connection),
                    self.describe_ptr(self.runtime.ui_network.network_data),
                    self.network_payload_len(),
                ));
                self.emit_network_delegate_callback(
                    "connection:didReceiveData:",
                    self.runtime.ui_network.network_connection,
                    self.runtime.ui_network.network_data,
                    "append-data",
                    format!(
                        "conn={} data={} bytes={} request={} state=receiving delivered={}/{}",
                        self.describe_ptr(self.runtime.ui_network.network_connection),
                        self.describe_ptr(self.runtime.ui_network.network_data),
                        self.network_payload_len(),
                        self.describe_ptr(self.runtime.ui_network.network_request),
                        self.runtime.ui_network.network_bytes_delivered,
                        self.network_payload_len(),
                    ),
                );
            }
            _ => {
                self.diag.trace.push(format!(
                    "     ↳ hle NSURLConnectionDidFinishLoading conn={} delegate={} state=completed",
                    self.describe_ptr(self.runtime.ui_network.network_connection),
                    self.describe_ptr(self.current_network_delegate()),
                ));
                self.runtime.ui_network.network_completed = true;
                self.runtime.ui_network.network_armed = false;
                self.runtime.ui_network.network_source_closed = true;
                self.runtime.ui_network.network_response_retained = true;
                self.runtime.ui_network.network_data_retained = true;
                self.refresh_network_object_labels();
                self.diag.trace.push(format!(
                    "     ↳ hle NSURLConnection.sourceClosed conn={} runloop={} mode={} loading=NO state=idle-complete",
                    self.describe_ptr(self.runtime.ui_network.network_connection),
                    self.describe_ptr(self.runtime.ui_objects.main_runloop),
                    self.describe_ptr(self.runtime.ui_objects.default_mode),
                ));
                self.diag.trace.push(format!(
                    "     ↳ hle delegate retain response={} data={} owner={} retainedResponse={} retainedData={}",
                    self.describe_ptr(self.runtime.ui_network.network_response),
                    self.describe_ptr(self.runtime.ui_network.network_data),
                    self.describe_ptr(self.current_network_delegate()),
                    Self::retained_flag(self.runtime.ui_network.network_response_retained),
                    Self::retained_flag(self.runtime.ui_network.network_data_retained),
                ));
                self.emit_network_delegate_callback(
                    "connectionDidFinishLoading:",
                    self.runtime.ui_network.network_connection,
                    0,
                    "completed",
                    format!(
                        "conn={} request={} response={} data={} bytes={} delegateCallbacks={} state=completed loading=NO retainedResponse={} retainedData={} source=closed",
                        self.describe_ptr(self.runtime.ui_network.network_connection),
                        self.describe_ptr(self.runtime.ui_network.network_request),
                        self.describe_ptr(self.runtime.ui_network.network_response),
                        self.describe_ptr(self.runtime.ui_network.network_data),
                        self.network_payload_len(),
                        self.runtime.ui_network.delegate_callbacks.saturating_add(1),
                        Self::retained_flag(self.runtime.ui_network.network_response_retained),
                        Self::retained_flag(self.runtime.ui_network.network_data_retained),
                    ),
                );
            }
        }
        self.runtime.ui_network.network_stage = self.runtime.ui_network.network_stage.saturating_add(1);
        self.recalc_runloop_sources();
    }

    fn handle_completed_network_idle_tick(&mut self) {
        self.runtime.ui_runtime.idle_ticks_after_completion =
            self.runtime.ui_runtime.idle_ticks_after_completion.saturating_add(1);
        self.diag.trace.push(format!(
            "     ↳ hle NSURLConnection.idle conn={} response={} data={} loading=NO retainedResponse={} retainedData={} idleTick={} state=idle-complete",
            self.describe_ptr(self.runtime.ui_network.network_connection),
            self.describe_ptr(self.runtime.ui_network.network_response),
            self.describe_ptr(self.runtime.ui_network.network_data),
            Self::retained_flag(self.runtime.ui_network.network_response_retained),
            Self::retained_flag(self.runtime.ui_network.network_data_retained),
            self.runtime.ui_runtime.idle_ticks_after_completion,
        ));
        let next_fault_mode = match self.runtime.ui_runtime.idle_ticks_after_completion {
            1 if self.runtime.ui_network.network_fault_events == 0 => Some(1),
            2 if self.runtime.ui_network.network_fault_events == 1 => Some(2),
            3 if self.runtime.ui_network.network_fault_events == 2 => Some(3),
            4 if self.runtime.ui_network.network_fault_events == 3 => Some(4),
            _ => None,
        };
        if self.tuning.synthetic_network_fault_probes {
            if let Some(mode) = next_fault_mode {
                self.runtime.ui_network.network_faulted = false;
                self.runtime.ui_network.network_cancelled = false;
                self.arm_synthetic_fault_probe(mode);
            }
        }
    }

}
