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

    fn trace_startup_runtime_interest(
        &mut self,
        receiver: u32,
        selector: &str,
        arg2: u32,
        arg3: u32,
        origin: &str,
    ) {
        let interesting = matches!(
            selector,
            "applicationDidFinishLaunching:"
                | "application:didFinishLaunchingWithOptions:"
                | "applicationDidBecomeActive:"
                | "applicationWillResignActive:"
                | "applicationWillEnterForeground:"
                | "video_play"
                | "defaultCenter"
                | "addObserver:selector:name:object:"
                | "removeObserver:"
                | "removeObserver:name:object:"
                | "initWithContentURL:"
                | "setShouldAutoplay:"
                | "prepareToPlay"
                | "play"
                | "pause"
                | "stop"
                | "view"
                | "runWithScene:"
                | "replaceScene:"
                | "popScene"
                | "setNextScene"
                | "addChild:"
                | "addChild:z:"
                | "addChild:z:tag:"
                | "onEnter"
        );
        if !interesting {
            return;
        }
        let receiver_class = self.objc_receiver_class_name_hint(receiver).unwrap_or_default();
        let arg2_class = self.objc_receiver_class_name_hint(arg2).unwrap_or_default();
        let stack0 = self.peek_stack_u32(0).unwrap_or(0);
        let stack1 = self.peek_stack_u32(1).unwrap_or(0);
        let detail = match selector {
            "addObserver:selector:name:object:" => {
                let observed_selector = self
                    .objc_read_selector_name(arg3)
                    .unwrap_or_else(|| format!("0x{arg3:08x}"));
                format!(
                    "observer={} observedSelector={} name={} object={} moviePlayers={} observers={} running={} next={}",
                    self.describe_ptr(arg2),
                    observed_selector,
                    self.synthetic_notification_name_desc(stack0),
                    self.describe_ptr(stack1),
                    self.runtime.ui_runtime.movie_players.len(),
                    self.runtime.ui_runtime.notification_observers.values().map(|items| items.len()).sum::<usize>(),
                    self.describe_ptr(self.runtime.ui_cocos.running_scene),
                    self.describe_ptr(self.runtime.ui_cocos.next_scene),
                )
            }
            "removeObserver:name:object:" => format!(
                "observer={} name={} object={} observers={} running={} next={}",
                self.describe_ptr(arg2),
                self.synthetic_notification_name_desc(stack0),
                self.describe_ptr(stack1),
                self.runtime.ui_runtime.notification_observers.values().map(|items| items.len()).sum::<usize>(),
                self.describe_ptr(self.runtime.ui_cocos.running_scene),
                self.describe_ptr(self.runtime.ui_cocos.next_scene),
            ),
            "removeObserver:" => format!(
                "observer={} observers={} running={} next={}",
                self.describe_ptr(arg2),
                self.runtime.ui_runtime.notification_observers.values().map(|items| items.len()).sum::<usize>(),
                self.describe_ptr(self.runtime.ui_cocos.running_scene),
                self.describe_ptr(self.runtime.ui_cocos.next_scene),
            ),
            "initWithContentURL:" => {
                let url_desc = self
                    .guest_string_value(arg2)
                    .or_else(|| self.host_path_from_string_value(arg2).map(|path| path.display().to_string()))
                    .unwrap_or_else(|| self.describe_ptr(arg2));
                format!(
                    "contentURL={} window={} visible={} running={} next={}",
                    url_desc,
                    self.describe_ptr(self.runtime.ui_objects.window),
                    if self.runtime.ui_runtime.window_visible { "YES" } else { "NO" },
                    self.describe_ptr(self.runtime.ui_cocos.running_scene),
                    self.describe_ptr(self.runtime.ui_cocos.next_scene),
                )
            }
            "setShouldAutoplay:" | "prepareToPlay" | "play" | "pause" | "stop" | "view" => {
                let movie_state = self.runtime.ui_runtime.movie_players.get(&receiver).cloned().unwrap_or_default();
                let url_desc = if movie_state.content_url != 0 {
                    self.guest_string_value(movie_state.content_url)
                        .or_else(|| self.host_path_from_string_value(movie_state.content_url).map(|path| path.display().to_string()))
                        .unwrap_or_else(|| self.describe_ptr(movie_state.content_url))
                } else {
                    "<none>".to_string()
                };
                format!(
                    "contentURL={} shouldAutoplay={} view={} plays={} pauses={} stops={} observers={} running={} next={}",
                    url_desc,
                    if movie_state.should_autoplay { "YES" } else { "NO" },
                    self.describe_ptr(movie_state.synthetic_view),
                    movie_state.play_count,
                    movie_state.pause_count,
                    movie_state.stop_count,
                    self.runtime.ui_runtime.notification_observers.values().map(|items| items.len()).sum::<usize>(),
                    self.describe_ptr(self.runtime.ui_cocos.running_scene),
                    self.describe_ptr(self.runtime.ui_cocos.next_scene),
                )
            }
            "runWithScene:" | "replaceScene:" | "popScene" | "setNextScene" | "addChild:" | "addChild:z:" | "addChild:z:tag:" | "onEnter" => format!(
                "sceneArg={} running={} next={} effect={} pendingSelector={} pendingDest={}",
                self.describe_ptr(arg2),
                self.describe_ptr(self.runtime.ui_cocos.running_scene),
                self.describe_ptr(self.runtime.ui_cocos.next_scene),
                self.describe_ptr(self.runtime.ui_cocos.effect_scene),
                self.runtime.ui_cocos.pending_scene_route_selector.clone().unwrap_or_else(|| "<none>".to_string()),
                self.describe_ptr(self.runtime.ui_cocos.pending_scene_route_destination),
            ),
            _ => format!(
                "window={} visible={} active={} running={} next={} moviePlayers={} observers={}",
                self.describe_ptr(self.runtime.ui_objects.window),
                if self.runtime.ui_runtime.window_visible { "YES" } else { "NO" },
                if self.runtime.ui_runtime.app_active { "YES" } else { "NO" },
                self.describe_ptr(self.runtime.ui_cocos.running_scene),
                self.describe_ptr(self.runtime.ui_cocos.next_scene),
                self.runtime.ui_runtime.movie_players.len(),
                self.runtime.ui_runtime.notification_observers.values().map(|items| items.len()).sum::<usize>(),
            ),
        };
        self.push_callback_trace(format!(
            "startup.trace origin={} sel={} recv={} recvClass={} arg2={} arg2Class={} arg3={} stack0={} stack1={} {}",
            origin,
            selector,
            self.describe_ptr(receiver),
            if receiver_class.is_empty() { "<unknown>" } else { &receiver_class },
            self.describe_ptr(arg2),
            if arg2_class.is_empty() { "<unknown>" } else { &arg2_class },
            self.describe_ptr(arg3),
            self.describe_ptr(stack0),
            self.describe_ptr(stack1),
            detail,
        ));
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

        if receiver_is_gl_view {
            self.ui_adopt_cocos_opengl_view(receiver, &format!("uikit-msgsend:{}", selector));
        }
        if arg2_is_gl_view {
            self.ui_adopt_cocos_opengl_view(arg2, &format!("uikit-arg:{}", selector));
        }

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
                        let previous_delegate = self.runtime.ui_network.network_delegate;
                        self.diag.object_labels
                            .entry(arg2)
                            .or_insert_with(|| "NSURLConnection.delegate".to_string());
                        self.note_objc_delegate_binding("setDelegate:", "NSURLConnection.delegate", receiver, arg2, self.runtime.ui_network.network_request, previous_delegate);
                        self.assign_network_delegate_with_provenance(
                            "setDelegate:",
                            receiver,
                            self.runtime.ui_network.network_request,
                            arg2,
                            "message send delegate setter",
                        );
                    } else {
                        let previous_delegate = self.runtime.ui_objects.delegate;
                        self.diag.object_labels
                            .entry(arg2)
                            .or_insert_with(|| "UIApplication.delegate".to_string());
                        self.note_objc_delegate_binding("setDelegate:", "UIApplication.delegate", receiver, arg2, self.runtime.ui_network.network_request, previous_delegate);
                        self.runtime.ui_objects.delegate = arg2;
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
                if self.runtime.ui_cocos.opengl_view != 0 {
                    self.runtime.ui_objects.first_responder = self.runtime.ui_cocos.opengl_view;
                } else if self.runtime.ui_objects.first_responder == 0 {
                    self.runtime.ui_objects.first_responder = self.runtime.ui_objects.root_controller;
                }
                note = Some(format!(
                    "window became key+visible, firstResponder={}",
                    self.describe_ptr(self.runtime.ui_objects.first_responder)
                ));
                receiver
            }
            "nextResponder" => {
                let next = self.ui_next_responder(receiver);
                note = Some(format!(
                    "nextResponder {} -> {}",
                    self.describe_ptr(receiver),
                    self.describe_ptr(next)
                ));
                next
            }
            "pointInside:withEvent:" => {
                if let Some((bits, source)) = self.read_msgsend_point_arg(arg2, arg3) {
                    let inside = self.ui_view_contains_local_point(
                        receiver,
                        Self::f32_from_bits(bits[0]),
                        Self::f32_from_bits(bits[1]),
                    );
                    note = Some(format!(
                        "pointInside point=({:.3},{:.3}) via {} -> {}",
                        Self::f32_from_bits(bits[0]),
                        Self::f32_from_bits(bits[1]),
                        source,
                        if inside { "YES" } else { "NO" }
                    ));
                    if inside { 1 } else { 0 }
                } else {
                    note = Some("pointInside decode failed".to_string());
                    0
                }
            }
            "hitTest:withEvent:" => {
                if let Some((bits, source)) = self.read_msgsend_point_arg(arg2, arg3) {
                    let hit = self.ui_hit_test_view_subtree(
                        receiver,
                        Self::f32_from_bits(bits[0]),
                        Self::f32_from_bits(bits[1]),
                    ).unwrap_or(0);
                    note = Some(format!(
                        "hitTest point=({:.3},{:.3}) via {} -> {}",
                        Self::f32_from_bits(bits[0]),
                        Self::f32_from_bits(bits[1]),
                        source,
                        self.describe_ptr(hit),
                    ));
                    hit
                } else {
                    note = Some("hitTest decode failed".to_string());
                    0
                }
            }
            "sendEvent:" => {
                if arg2 != 0 {
                    if let Some(phase) = self.synthetic_phase_name_for_event(arg2) {
                        if let Some((dispatch_target, hit_view, dispatched)) =
                            self.dispatch_uikit_event_via_window_send_event(phase, arg2, "uikit-sendEvent")
                        {
                            note = Some(format!(
                                "sendEvent routed phase={} dispatchTarget={} hitView={} selector={}",
                                phase,
                                self.describe_ptr(dispatch_target),
                                self.describe_ptr(hit_view),
                                dispatched,
                            ));
                        } else {
                            note = Some(format!(
                                "sendEvent phase={} event={} routed=NO",
                                phase,
                                self.describe_ptr(arg2),
                            ));
                        }
                    } else {
                        note = Some(format!("sendEvent ignored non-synthetic event={}", self.describe_ptr(arg2)));
                    }
                }
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

    fn observe_real_uikit_objc_msgsend(&mut self, receiver: u32, selector: &str, arg2: u32, _arg3: u32) {
        let receiver_class = self.objc_receiver_class_name_hint(receiver).unwrap_or_default();
        let receiver_label = self.diag.object_labels.get(&receiver).cloned().unwrap_or_default();
        let arg2_class = self.objc_receiver_class_name_hint(arg2).unwrap_or_default();
        let arg2_label = self.diag.object_labels.get(&arg2).cloned().unwrap_or_default();
        let receiver_is_gl_view = receiver == self.runtime.ui_cocos.opengl_view
            || receiver_class.to_ascii_lowercase().contains("eagl")
            || receiver_class.to_ascii_lowercase().contains("glview")
            || receiver_label.contains("EAGLView")
            || receiver_label.contains("GLView");
        let arg2_is_gl_view = arg2 == self.runtime.ui_cocos.opengl_view
            || arg2_class.to_ascii_lowercase().contains("eagl")
            || arg2_class.to_ascii_lowercase().contains("glview")
            || arg2_label.contains("EAGLView")
            || arg2_label.contains("GLView");

        self.trace_startup_runtime_interest(receiver, selector, arg2, _arg3, "real-objc-dispatch");

        if receiver_is_gl_view {
            self.ui_adopt_cocos_opengl_view(receiver, &format!("real-objc:{}", selector));
        }
        if arg2_is_gl_view {
            self.ui_adopt_cocos_opengl_view(arg2, &format!("real-objc-arg:{}", selector));
        }

        match selector {
            "runWithScene:" | "pushScene:" | "replaceScene:" | "replaceScene:byTarget:selector:" => {
                self.record_scene_route_request(selector, receiver, arg2, _arg3, "real-objc-dispatch");
                let _ = self.maybe_commit_pending_scene_route("real-objc-dispatch");
            }
            "setEffectScene:" => {
                self.set_effect_scene(arg2, "real-objc-dispatch");
            }
            "addSubview:" if receiver != 0 && arg2 != 0 && self.ui_object_is_view_like(receiver) => {
                self.runtime.ui_objects.view_superviews.insert(arg2, receiver);
                let children = self.runtime.ui_objects.view_subviews.entry(receiver).or_default();
                if !children.contains(&arg2) {
                    children.push(arg2);
                }
                if receiver == self.runtime.ui_objects.window {
                    self.runtime.ui_runtime.window_visible = true;
                }
            }
            "becomeFirstResponder" if receiver != 0 => {
                self.runtime.ui_objects.first_responder = receiver;
            }
            "makeKeyAndVisible" => {
                self.runtime.ui_runtime.window_visible = true;
                if self.runtime.ui_cocos.opengl_view != 0 {
                    self.runtime.ui_objects.first_responder = self.runtime.ui_cocos.opengl_view;
                }
            }
            _ => {}
        }
    }
}
