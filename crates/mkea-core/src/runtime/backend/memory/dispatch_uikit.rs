impl MemoryArm32Backend {
    fn finish_objc_msgsend_hle_dispatch(
        &mut self,
        index: u64,
        current_pc: u32,
        label: &str,
        receiver_desc: &str,
        selector: &str,
        arg2_desc: &str,
        arg3_desc: &str,
        result: u32,
        note: Option<String>,
    ) -> CoreResult<Option<StepControl>> {
        let result = if selector.starts_with("init")
            && result == 0
            && self.runtime.objc.objc_classes_by_ptr.contains_key(&self.cpu.regs[0])
        {
            self.objc_hle_alloc_like(self.cpu.regs[0], 0, "init-fallback")
        } else {
            result
        };
        if selector.starts_with("init") {
            self.objc_note_init_result(self.cpu.regs[0], result);
        }
        let mut detail = format!(
            "hle objc_msgSend(receiver={}, sel={}, arg2={}, arg3={}, result={})",
            receiver_desc,
            selector,
            arg2_desc,
            arg3_desc,
            self.describe_ptr(result),
        );
        if let Some(extra) = note {
            detail.push_str(&format!(", note={extra}"));
        }
        self.diag.trace
            .push(self.hle_trace_line(index, current_pc, label, &detail));
        self.cpu.regs[0] = result;
        self.cpu.regs[15] = self.cpu.regs[14] & !1;
        self.cpu.thumb = (self.cpu.regs[14] & 1) != 0;
        Ok(Some(StepControl::Continue))
    }

    fn maybe_dispatch_uikit_objc_msgsend(
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
        let receiver_class = self.objc_receiver_class_name_hint(receiver).unwrap_or_default();
        let receiver_label = self.diag.object_labels.get(&receiver).cloned().unwrap_or_default();
        let arg2_class = self.objc_receiver_class_name_hint(arg2).unwrap_or_default();
        let arg2_label = self.diag.object_labels.get(&arg2).cloned().unwrap_or_default();
        let receiver_is_networkish = receiver == self.runtime.ui_network.network_connection
            || receiver == self.runtime.ui_network.fault_connection
            || receiver_class.contains("NSURLConnection")
            || receiver_label.contains("NSURLConnection")
            || receiver_label.contains("CFReadStream")
            || receiver_label.contains("NSURLResponse");
        let receiver_is_gl_view = receiver == self.runtime.ui_cocos.opengl_view
            || receiver_class.to_ascii_lowercase().contains("eagl")
            || receiver_class.to_ascii_lowercase().contains("glview")
            || receiver_label.contains("EAGLView")
            || receiver_label.contains("GLView");
        let receiver_is_view_like = self.ui_object_is_view_like(receiver) || receiver_is_gl_view;
        let arg2_is_gl_view = arg2 == self.runtime.ui_cocos.opengl_view
            || arg2_class.to_ascii_lowercase().contains("eagl")
            || arg2_class.to_ascii_lowercase().contains("glview")
            || arg2_label.contains("EAGLView")
            || arg2_label.contains("GLView");
        let result = match selector {
            "sharedApplication" => self.runtime.ui_objects.app,
            "window" | "keyWindow" => self.runtime.ui_objects.window,
            "rootViewController" => self.runtime.ui_objects.root_controller,
            "mainScreen" => self.runtime.ui_objects.screen,
            "mainRunLoop" | "currentRunLoop" => self.runtime.ui_objects.main_runloop,
            "defaultMode" => self.runtime.ui_objects.default_mode,
            "isFirstResponder" => {
                if receiver != 0 && receiver == self.runtime.ui_objects.first_responder { 1 } else { 0 }
            }
            "becomeFirstResponder" => {
                if receiver != 0 {
                    self.runtime.ui_objects.first_responder = receiver;
                }
                note = Some(format!("firstResponder <- {}", self.describe_ptr(self.runtime.ui_objects.first_responder)));
                1
            }
            "setDelegate:" => {
                if arg2 != 0 {
                    if receiver_is_networkish {
                        self.runtime.ui_network.network_delegate = arg2;
                        self.diag.object_labels
                            .entry(arg2)
                            .or_insert_with(|| "NSURLConnection.delegate".to_string());
                    } else {
                        self.runtime.ui_objects.delegate = arg2;
                        self.diag.object_labels
                            .entry(arg2)
                            .or_insert_with(|| "UIApplication.delegate".to_string());
                    }
                }
                self.runtime.ui_runtime.delegate_set = true;
                note = Some(format!(
                    "delegate app={} network={}",
                    self.describe_ptr(self.runtime.ui_objects.delegate),
                    self.describe_ptr(self.current_network_delegate())
                ));
                receiver
            }
            "setWindow:" => {
                if arg2 != 0 {
                    self.runtime.ui_objects.window = arg2;
                    self.diag.object_labels
                        .entry(arg2)
                        .or_insert_with(|| "UIWindow.main".to_string());
                    self.ui_set_frame_bits(arg2, self.ui_surface_rect_bits());
                    self.ui_set_bounds_bits(arg2, Self::ui_rect_size_bits(self.ui_surface_rect_bits()));
                }
                note = Some(format!("window <- {}", self.describe_ptr(self.runtime.ui_objects.window)));
                receiver
            }
            "setRootViewController:" => {
                if arg2 != 0 {
                    self.runtime.ui_objects.root_controller = arg2;
                    self.runtime.ui_objects.first_responder = arg2;
                    self.diag.object_labels
                        .entry(arg2)
                        .or_insert_with(|| "UIViewController.root".to_string());
                }
                note = Some(format!(
                    "root <- {}, firstResponder <- {}",
                    self.describe_ptr(self.runtime.ui_objects.root_controller),
                    self.describe_ptr(self.runtime.ui_objects.first_responder)
                ));
                receiver
            }
            "addSubview:" => {
                if receiver != 0 && arg2 != 0 {
                    self.runtime.ui_objects.view_superviews.insert(arg2, receiver);
                    let entry = self.runtime.ui_objects.view_subviews.entry(receiver).or_default();
                    if !entry.contains(&arg2) {
                        entry.push(arg2);
                    }
                    if receiver == self.runtime.ui_objects.window && arg2_is_gl_view {
                        self.runtime.ui_cocos.opengl_view = arg2;
                        self.diag.object_labels
                            .entry(arg2)
                            .or_insert_with(|| "EAGLView.synthetic#0".to_string());
                        if self.runtime.ui_objects.first_responder == 0 || self.runtime.ui_objects.first_responder == self.runtime.ui_objects.root_controller {
                            self.runtime.ui_objects.first_responder = arg2;
                        }
                    }
                    if receiver == self.runtime.ui_objects.window {
                        self.runtime.ui_runtime.window_visible = true;
                    }
                    let parent_frame = self.ui_frame_bits_for_object(receiver);
                    let parent_bounds = self.ui_bounds_bits_for_object(receiver);
                    self.runtime.ui_objects.view_frames_bits.entry(arg2).or_insert(parent_frame);
                    self.runtime.ui_objects.view_bounds_bits.entry(arg2).or_insert(Self::ui_rect_size_bits(parent_bounds));
                }
                note = Some(format!(
                    "hierarchy {} <- {}",
                    self.describe_ptr(receiver),
                    self.describe_ptr(arg2)
                ));
                receiver
            }
            "removeFromSuperview" => {
                if receiver != 0 {
                    if let Some(parent) = self.runtime.ui_objects.view_superviews.remove(&receiver) {
                        if let Some(children) = self.runtime.ui_objects.view_subviews.get_mut(&parent) {
                            children.retain(|child| *child != receiver);
                        }
                    }
                }
                note = Some(format!("hierarchy detach {}", self.describe_ptr(receiver)));
                receiver
            }
            "superview" => {
                let mut parent = self.runtime.ui_objects.view_superviews.get(&receiver).copied().unwrap_or(0);
                if parent == receiver {
                    parent = 0;
                }
                if parent == 0 && receiver_is_gl_view {
                    parent = self.runtime.ui_objects.window;
                }
                if parent == 0 && receiver == self.runtime.ui_objects.root_controller {
                    parent = self.runtime.ui_objects.window;
                }
                note = Some(format!("superview {} -> {}", self.describe_ptr(receiver), self.describe_ptr(parent)));
                parent
            }
            "setFrame:" => {
                if receiver_is_view_like {
                    if let Some((bits, source)) = self.read_msgsend_rect_arg() {
                        self.ui_set_frame_bits(receiver, bits);
                        note = Some(format!("frame <- {} via {}", Self::ui_rect_bits_to_string(bits), source));
                    } else {
                        note = Some("frame decode failed".to_string());
                    }
                }
                receiver
            }
            "setBounds:" => {
                if receiver_is_view_like {
                    if let Some((bits, source)) = self.read_msgsend_rect_arg() {
                        self.ui_set_bounds_bits(receiver, bits);
                        note = Some(format!("bounds <- {} via {}", Self::ui_rect_bits_to_string(bits), source));
                    } else {
                        note = Some("bounds decode failed".to_string());
                    }
                }
                receiver
            }
            "setContentScaleFactor:" => {
                if receiver_is_view_like || self.ui_object_is_layer_like(receiver) {
                    self.ui_set_content_scale_bits(receiver, arg2);
                    note = Some(format!(
                        "contentScaleFactor <- {:.3}",
                        Self::ui_content_scale_value_from_bits(self.ui_content_scale_bits_for_object(receiver)),
                    ));
                }
                receiver
            }
            "contentScaleFactor" => {
                let bits = self.ui_content_scale_bits_for_object(receiver);
                note = Some(format!("contentScaleFactor -> {:.3}", Self::ui_content_scale_value_from_bits(bits)));
                bits
            }
            "addTimer:forMode:" => {
                self.bootstrap_synthetic_runloop();
                let attached = self.attach_foundation_timer(arg2, selector);
                self.runtime.ui_runtime.timer_armed = true;
                self.recalc_runloop_sources();
                note = Some(format!(
                    "timer source attached runLoop={} timer={} mode={} attached={}",
                    self.describe_ptr(receiver),
                    self.describe_ptr(arg2),
                    self.describe_ptr(arg3),
                    if attached { "YES" } else { "NO" }
                ));
                receiver
            }
            "invalidate" => {
                let foundation_invalidated = self.invalidate_foundation_timer(receiver, selector);
                if receiver == self.runtime.ui_objects.synthetic_timer {
                    self.runtime.ui_runtime.timer_armed = false;
                }
                if receiver == self.runtime.ui_cocos.synthetic_display_link {
                    self.runtime.ui_cocos.display_link_armed = false;
                }
                self.recalc_runloop_sources();
                note = Some(format!(
                    "invalidated {} foundationTimer={}",
                    self.describe_ptr(receiver),
                    if foundation_invalidated { "YES" } else { "NO" }
                ));
                receiver
            }
            "fire" => {
                if !self.fire_foundation_timer_now(receiver, selector) {
                    self.push_synthetic_runloop_tick("NSTimer.fire", true);
                }
                note = Some(format!("timer fire {}", self.describe_ptr(receiver)));
                receiver
            }
            "makeKeyAndVisible" => {
                self.runtime.ui_runtime.window_visible = true;
                if self.runtime.ui_objects.first_responder == 0 {
                    self.runtime.ui_objects.first_responder = self.runtime.ui_objects.root_controller;
                }
                note = Some(format!(
                    "window became key+visible, firstResponder={}",
                    self.describe_ptr(self.runtime.ui_objects.first_responder)
                ));
                receiver
            }
            "application:didFinishLaunchingWithOptions:" | "applicationDidFinishLaunching:" => {
                self.runtime.ui_runtime.launched = true;
                self.bootstrap_synthetic_runloop();
                note = Some(format!("delegate launch on {}", self.describe_ptr(receiver)));
                1
            }
            "applicationDidBecomeActive:" | "applicationWillEnterForeground:" => {
                self.bootstrap_synthetic_runloop();
                note = Some(format!(
                    "app active callback on {} state=active",
                    self.describe_ptr(receiver)
                ));
                0
            }
            "runMode:beforeDate:" => {
                self.bootstrap_synthetic_runloop();
                self.push_synthetic_runloop_tick(
                    "NSRunLoop.runMode:beforeDate:",
                    self.runtime.ui_runtime.runloop_sources > 0,
                );
                note = Some(format!("runloop advanced on {}", self.describe_ptr(receiver)));
                1
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
