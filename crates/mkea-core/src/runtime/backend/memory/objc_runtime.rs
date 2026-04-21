impl MemoryArm32Backend {
// Objective-C metadata indexing, allocation, dispatch, and UIApplication bridging.

    fn install_objc_metadata_sections(&mut self, sections: &[SectionInfo]) {
        self.runtime.objc.objc_section_classlist = sections
            .iter()
            .find(|section| section.sectname == "__objc_classlist")
            .map(|section| ObjcSectionRange { addr: section.addr, size: section.size });
        self.runtime.objc.objc_section_catlist = sections
            .iter()
            .find(|section| section.sectname == "__objc_catlist")
            .map(|section| ObjcSectionRange { addr: section.addr, size: section.size });
        self.runtime.objc.objc_section_cfstring = sections
            .iter()
            .find(|section| section.sectname == "__cfstring")
            .map(|section| ObjcSectionRange { addr: section.addr, size: section.size });
        self.runtime.objc.objc_section_const = sections
            .iter()
            .find(|section| section.sectname == "__objc_const")
            .map(|section| ObjcSectionRange { addr: section.addr, size: section.size });
    }

    fn objc_parse_method_list(&self, ml_ptr: u32) -> HashMap<String, u32> {
        if ml_ptr == 0 {
            return HashMap::new();
        }
        let entsize_flags = match self.read_u32_le(ml_ptr) {
            Ok(value) => value,
            Err(_) => return HashMap::new(),
        };
        let count = match self.read_u32_le(ml_ptr.wrapping_add(4)) {
            Ok(value) => value.min(1024),
            Err(_) => return HashMap::new(),
        };
        let mut entsize = entsize_flags & 0xFFFF_FFFC;
        if !(12..=64).contains(&entsize) {
            entsize = 12;
        }
        let mut out = HashMap::new();
        let base = ml_ptr.wrapping_add(8);
        for index in 0..count {
            let p = base.wrapping_add(index.saturating_mul(entsize));
            let sel_ptr = match self.read_u32_le(p) {
                Ok(value) => value,
                Err(_) => break,
            };
            let imp = match self.read_u32_le(p.wrapping_add(8)) {
                Ok(value) => value,
                Err(_) => break,
            };
            if imp == 0 {
                continue;
            }
            if let Some(name) = self.objc_read_selector_name(sel_ptr) {
                out.insert(name, imp);
            }
        }
        out
    }

    fn objc_parse_ivar_list(&self, ivars_ptr: u32) -> HashMap<String, u32> {
        if ivars_ptr == 0 {
            return HashMap::new();
        }
        let entsize_flags = match self.read_u32_le(ivars_ptr) {
            Ok(value) => value,
            Err(_) => return HashMap::new(),
        };
        let count = match self.read_u32_le(ivars_ptr.wrapping_add(4)) {
            Ok(value) => value.min(1024),
            Err(_) => return HashMap::new(),
        };
        let mut entsize = entsize_flags & 0xFFFF_FFFC;
        if !(20..=64).contains(&entsize) {
            entsize = 20;
        }
        let base = ivars_ptr.wrapping_add(8);
        let mut out = HashMap::new();
        for index in 0..count {
            let p = base.wrapping_add(index.saturating_mul(entsize));
            let Ok(offset_ptr) = self.read_u32_le(p) else { break; };
            let Ok(name_ptr) = self.read_u32_le(p.wrapping_add(4)) else { break; };
            let Some(name) = self.read_c_string(name_ptr, 256) else { continue; };
            let offset = if offset_ptr != 0 {
                self.read_u32_le(offset_ptr).ok().or_else(|| self.read_u32_le(offset_ptr & !0x3).ok())
            } else {
                None
            };
            let Some(offset) = offset else { continue; };
            out.insert(name, offset);
        }
        out
    }

    fn objc_parse_class_ro(&self, ro_ptr: u32) -> Option<(String, u32, u32, u32)> {
        let instance_size = self.read_u32_le(ro_ptr.wrapping_add(8)).ok()?;
        for (name_off, methods_off) in [(20u32, 24u32), (16u32, 20u32), (24u32, 28u32)] {
            let Ok(name_ptr) = self.read_u32_le(ro_ptr.wrapping_add(name_off)) else { continue; };
            let Ok(methods_ptr) = self.read_u32_le(ro_ptr.wrapping_add(methods_off)) else { continue; };
            if let Some(name) = self.read_c_string(name_ptr, 256) {
                let ivars_ptr = self.read_u32_le(ro_ptr.wrapping_add(methods_off.wrapping_add(8))).unwrap_or(0);
                return Some((name, instance_size, methods_ptr, ivars_ptr));
            }
        }
        None
    }

    fn objc_parse_class_info(&self, cls_ptr: u32) -> Option<ObjcClassInfo> {
        let isa = self.read_u32_le(cls_ptr).ok()?;
        let superclass = self.read_u32_le(cls_ptr.wrapping_add(4)).ok()?;
        let mut data_candidates = Vec::new();
        for off in [16u32, 20u32, 24u32] {
            if let Ok(value) = self.read_u32_le(cls_ptr.wrapping_add(off)) {
                if value != 0 {
                    data_candidates.push(value);
                }
            }
        }
        let mut ro_ptr = 0u32;
        let mut name: Option<String> = None;
        let mut instance_size = 0u32;
        let mut methods_ptr = 0u32;
        let mut ivars_ptr = 0u32;
        'outer: for bits in data_candidates {
            for mask in [0u32, 0x3, 0x7] {
                let candidate = bits & !mask;
                if let Some((candidate_name, candidate_size, candidate_methods_ptr, candidate_ivars_ptr)) = self.objc_parse_class_ro(candidate) {
                    ro_ptr = candidate;
                    name = Some(candidate_name);
                    instance_size = candidate_size;
                    methods_ptr = candidate_methods_ptr;
                    ivars_ptr = candidate_ivars_ptr;
                    break 'outer;
                }
            }
        }
        let name = name?;
        let methods = self.objc_parse_method_list(methods_ptr);
        let ivars = self.objc_parse_ivar_list(ivars_ptr);
        let mut meta_methods = HashMap::new();
        if isa != 0 {
            let mut meta_candidates = Vec::new();
            for off in [16u32, 20u32, 24u32] {
                if let Ok(value) = self.read_u32_le(isa.wrapping_add(off)) {
                    if value != 0 {
                        meta_candidates.push(value);
                    }
                }
            }
            'meta: for bits in meta_candidates {
                for mask in [0u32, 0x3, 0x7] {
                    let candidate = bits & !mask;
                    if let Some((_meta_name, _meta_size, meta_methods_ptr, _meta_ivars_ptr)) = self.objc_parse_class_ro(candidate) {
                        meta_methods = self.objc_parse_method_list(meta_methods_ptr);
                        break 'meta;
                    }
                }
            }
        }
        Some(ObjcClassInfo {
            cls: cls_ptr,
            isa,
            superclass,
            ro: ro_ptr,
            name,
            instance_size,
            methods,
            meta_methods,
            ivars,
        })
    }

    fn objc_parse_category_info(&self, category_ptr: u32) -> Option<(u32, HashMap<String, u32>, HashMap<String, u32>)> {
        if category_ptr == 0 {
            return None;
        }
        let class_ptr = self.read_u32_le(category_ptr.wrapping_add(4)).ok()?;
        if class_ptr == 0 {
            return None;
        }
        let instance_methods_ptr = self.read_u32_le(category_ptr.wrapping_add(8)).unwrap_or(0);
        let class_methods_ptr = self.read_u32_le(category_ptr.wrapping_add(12)).unwrap_or(0);
        Some((
            class_ptr,
            self.objc_parse_method_list(instance_methods_ptr),
            self.objc_parse_method_list(class_methods_ptr),
        ))
    }

    fn objc_merge_category_methods(&mut self) {
        let Some(range) = self.runtime.objc.objc_section_catlist else { return; };
        let count = (range.size / 4).min(4096);
        for index in 0..count {
            let addr = range.addr.wrapping_add(index.saturating_mul(4));
            let Ok(category_ptr) = self.read_u32_le(addr) else { continue; };
            let Some((class_ptr, methods, meta_methods)) = self.objc_parse_category_info(category_ptr) else {
                continue;
            };
            self.ensure_objc_class_hierarchy_indexed(class_ptr);
            let Some(info) = self.runtime.objc.objc_classes_by_ptr.get_mut(&class_ptr) else {
                continue;
            };
            for (selector, imp) in methods {
                info.methods.insert(selector, imp);
            }
            for (selector, imp) in meta_methods {
                info.meta_methods.insert(selector, imp);
            }
        }
    }

    fn objc_lookup_ivar_offset_in_class_chain(&mut self, receiver: u32, ivar_name: &str) -> Option<u32> {
        if receiver == 0 || ivar_name.is_empty() {
            return None;
        }
        let mut current = self.objc_class_ptr_for_receiver(receiver)?;
        for _ in 0..64 {
            self.ensure_objc_class_hierarchy_indexed(current);
            let info = self.runtime.objc.objc_classes_by_ptr.get(&current)?;
            if let Some(offset) = info.ivars.get(ivar_name).copied() {
                return Some(offset);
            }
            if info.superclass == 0 || info.superclass == current {
                break;
            }
            current = info.superclass;
        }
        None
    }

    fn ensure_objc_class_indexed_recursive(&mut self, class_ptr: u32, depth: u32) {
        if class_ptr == 0 || depth == 0 || self.runtime.objc.objc_classes_by_ptr.contains_key(&class_ptr) {
            return;
        }
        let Some(info) = self.objc_parse_class_info(class_ptr) else { return; };
        let superclass = info.superclass;
        let name = info.name.clone();
        self.runtime.objc.objc_classes_by_name.insert(name, class_ptr);
        self.runtime.objc.objc_classes_by_ptr.insert(class_ptr, info);
        if superclass != 0 && superclass != class_ptr {
            self.ensure_objc_class_indexed_recursive(superclass, depth.saturating_sub(1));
        }
    }

    fn ensure_objc_class_hierarchy_indexed(&mut self, class_ptr: u32) {
        self.ensure_objc_class_indexed_recursive(class_ptr, 32);
    }

    fn ensure_objc_metadata_indexed(&mut self) {
        if self.runtime.objc.objc_metadata_indexed {
            return;
        }
        self.runtime.objc.objc_metadata_indexed = true;
        let Some(range) = self.runtime.objc.objc_section_classlist else { return; };
        let count = (range.size / 4).min(4096);
        for index in 0..count {
            let addr = range.addr.wrapping_add(index.saturating_mul(4));
            let Ok(class_ptr) = self.read_u32_le(addr) else { continue; };
            if class_ptr == 0 {
                continue;
            }
            self.ensure_objc_class_hierarchy_indexed(class_ptr);
        }
        self.objc_merge_category_methods();
    }

    fn objc_class_name_for_ptr(&self, class_ptr: u32) -> Option<String> {
        self.runtime.objc.objc_classes_by_ptr.get(&class_ptr).map(|info| info.name.clone())
    }

    fn objc_class_name_for_receiver(&self, receiver: u32) -> Option<String> {
        if let Some(class_ptr) = self.runtime.objc.objc_instance_isa_overrides.get(&receiver).copied() {
            return self.objc_class_name_for_ptr(class_ptr);
        }
        if self.runtime.objc.objc_classes_by_ptr.contains_key(&receiver) {
            return self.objc_class_name_for_ptr(receiver);
        }
        let isa = self.read_u32_le(receiver).ok()?;
        if let Some(name) = self.objc_class_name_for_ptr(isa) {
            return Some(name);
        }
        self.objc_parse_class_info(isa).map(|info| info.name)
    }

    fn objc_lookup_class_by_name(&mut self, name: &str) -> Option<u32> {
        self.ensure_objc_metadata_indexed();
        self.runtime.objc.objc_classes_by_name.get(name).copied()
    }

    fn ensure_objc_singleton_object(&mut self, class_ptr: u32, class_name: &str, reason: &str) -> u32 {
        if let Some(existing) = self.runtime.objc.objc_singletons_by_class.get(&class_ptr).copied().filter(|ptr| *ptr != 0) {
            return existing;
        }
        let tag = format!("singleton-{reason}");
        let obj = self
            .objc_materialize_instance_with_extra(class_ptr, class_name, 0, &tag)
            .or_else(|| self.objc_materialize_instance(class_ptr, class_name))
            .unwrap_or_else(|| self.objc_hle_alloc_like(class_ptr, 0, reason));
        if obj != 0 {
            self.objc_attach_receiver_class(obj, class_ptr, class_name);
            self.diag.object_labels
                .insert(obj, format!("{}.singleton(synth)", class_name));
            self.runtime.objc.objc_singletons_by_class.insert(class_ptr, obj);
        }
        obj
    }

    fn objc_should_prefer_real_singleton_dispatch(&mut self, class_ptr: u32, class_name: &str, selector: &str) -> bool {
        if !Self::audio_is_objc_audio_class(class_name) {
            return false;
        }
        self.ensure_objc_class_hierarchy_indexed(class_ptr);
        self.objc_lookup_imp_in_class_chain(class_ptr, selector, true).is_some()
    }

    fn objc_register_guest_singleton_object(&mut self, class_ptr: u32, class_name: &str, obj: u32, reason: &str) {
        if class_ptr == 0 || obj == 0 {
            return;
        }
        self.objc_attach_receiver_class(obj, class_ptr, class_name);
        self.runtime.objc.objc_singletons_by_class.insert(class_ptr, obj);
        self.diag.object_labels
            .entry(obj)
            .or_insert_with(|| format!("{}.singleton(guest)<{}>", class_name, reason));
        match class_name {
            "CDAudioManager" => {
                self.runtime.graphics.cocos_audio_manager_object = obj;
            }
            "CDSoundEngine" => {
                self.runtime.graphics.cocos_sound_engine_object = obj;
            }
            _ => {}
        }
    }

    fn maybe_handle_objc_singleton_fastpath(&mut self, selector: &str, receiver: u32) -> Option<(u32, String)> {
        if receiver == 0 {
            return None;
        }
        let class_ptr = self.objc_class_ptr_for_receiver(receiver)?;
        let class_name = self.objc_class_name_for_ptr(class_ptr)?;
        let handled = match selector {
            "sharedManager" => class_name == "DataManager" || class_name == "MissionManager" || class_name == "CDAudioManager",
            "sharedEngine" => class_name == "SimpleAudioEngine" || class_name == "OALSimpleAudio",
            "sharedGameControl" => class_name == "GameControl",
            "sharedGUIController" => class_name == "GUIController",
            "sharedUserProfileMgr" => class_name == "UserProfileMgr",
            _ => false,
        };
        if !handled {
            return None;
        }
        if self.objc_should_prefer_real_singleton_dispatch(class_ptr, &class_name, selector) {
            if Self::audio_is_objc_audio_class(&class_name) {
                self.audio_trace_note_objc_audio_selector(&class_name, selector, None, false);
                self.audio_trace_push_event(format!(
                    "objc.audio.singleton-prefer-real class={} selector={} receiver={}",
                    class_name,
                    selector,
                    self.describe_ptr(receiver),
                ));
            }
            return None;
        }
        let action = if self.runtime.objc.objc_singletons_by_class.contains_key(&class_ptr) {
            "reused"
        } else {
            "materialized"
        };
        let obj = self.ensure_objc_singleton_object(class_ptr, &class_name, selector);
        if matches!(selector, "sharedEngine" | "sharedManager" | "sharedGameControl" | "sharedGUIController" | "sharedUserProfileMgr") {
            self.objc_attach_receiver_class(obj, class_ptr, &class_name);
        }
        if Self::audio_is_objc_audio_class(&class_name) {
            self.audio_trace_note_objc_audio_selector(&class_name, selector, None, false);
        }
        Some((
            obj,
            format!(
                "objc singleton repair selector={} class={} action={} -> {}",
                selector,
                class_name,
                action,
                self.describe_ptr(obj),
            ),
        ))
    }

    fn objc_lookup_imp_in_class_chain(&self, class_ptr: u32, selector: &str, class_method: bool) -> Option<u32> {
        let mut current = class_ptr;
        for _ in 0..64 {
            let info = self.runtime.objc.objc_classes_by_ptr.get(&current)?;
            let methods = if class_method { &info.meta_methods } else { &info.methods };
            if let Some(imp) = methods.get(selector).copied() {
                return Some(imp);
            }
            if info.superclass == 0 || info.superclass == current {
                break;
            }
            current = info.superclass;
        }
        None
    }

    fn objc_lookup_imp_for_receiver(&mut self, receiver: u32, selector: &str) -> Option<u32> {
        if receiver == 0 {
            return None;
        }
        self.ensure_objc_metadata_indexed();
        if self.runtime.objc.objc_classes_by_ptr.contains_key(&receiver) {
            self.ensure_objc_class_hierarchy_indexed(receiver);
            return self.objc_lookup_imp_in_class_chain(receiver, selector, true);
        }
        if let Some(class_ptr) = self.runtime.objc.objc_instance_isa_overrides.get(&receiver).copied() {
            self.ensure_objc_class_hierarchy_indexed(class_ptr);
            return self.objc_lookup_imp_in_class_chain(class_ptr, selector, false);
        }
        let isa = self.read_u32_le(receiver).ok()?;
        self.ensure_objc_class_hierarchy_indexed(isa);
        if self.runtime.objc.objc_classes_by_ptr.contains_key(&isa) {
            return self.objc_lookup_imp_in_class_chain(isa, selector, false);
        }
        None
    }

    fn objc_selector_unique_class_matches(&mut self, selector: &str) -> Vec<(u32, String)> {
        self.ensure_objc_metadata_indexed();
        let mut matches = Vec::new();
        for (&class_ptr, info) in &self.runtime.objc.objc_classes_by_ptr {
            if self.objc_lookup_imp_in_class_chain(class_ptr, selector, false).is_some() {
                matches.push((class_ptr, info.name.clone()));
            }
        }
        matches.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        matches
    }

    fn objc_selector_is_delegate_callback(selector: &str) -> bool {
        matches!(
            selector,
            "applicationDidBecomeActive:"
                | "applicationWillEnterForeground:"
                | "reachabilityChanged:"
                | "connectionDidFinishLoading:"
                | "connection:didReceiveData:"
                | "connection:didReceiveResponse:"
                | "connection:didFailWithError:"
        )
    }

    fn objc_bootstrap_signal_selectors() -> &'static [&'static str] {
        &[
            "connectionDidFinishLoading:",
            "connection:didReceiveData:",
            "connection:didReceiveResponse:",
            "connection:didFailWithError:",
            "parserDidEndDocument:",
            "parser:didStartElement:namespaceURI:qualifiedName:attributes:",
            "parser:didEndElement:namespaceURI:qualifiedName:",
            "parse",
            "startGame",
            "loadScene",
        ]
    }

    fn objc_bootstrap_signals_for_class(&mut self, class_ptr: u32) -> Vec<String> {
        if class_ptr == 0 {
            return Vec::new();
        }
        self.ensure_objc_metadata_indexed();
        self.ensure_objc_class_hierarchy_indexed(class_ptr);
        let mut out = Vec::new();
        for selector in Self::objc_bootstrap_signal_selectors() {
            if self.objc_lookup_imp_in_class_chain(class_ptr, selector, false).is_some() {
                out.push((*selector).to_string());
            }
        }
        out
    }

    fn objc_is_bootstrap_consumer_class(&mut self, class_ptr: u32) -> bool {
        !self.objc_bootstrap_signals_for_class(class_ptr).is_empty()
    }

    fn objc_note_created_instance(&mut self, receiver: u32, class_ptr: u32, class_name: &str, origin: &str) {
        if receiver == 0 || class_ptr == 0 {
            return;
        }
        let created_len = {
            let created = self
                .runtime
                .objc
                .objc_created_instances_by_class
                .entry(class_ptr)
                .or_default();
            if let Some(index) = created.iter().position(|value| *value == receiver) {
                created.remove(index);
            }
            created.push(receiver);
            if created.len() > 16 {
                let overflow = created.len().saturating_sub(16);
                created.drain(0..overflow);
            }
            created.len()
        };
        self.runtime.objc.objc_recent_created_receivers.push(ObjcCreatedReceiver {
            receiver,
            class_ptr,
            class_name: class_name.to_string(),
            origin: origin.to_string(),
            tick: self.runtime.ui_runtime.runloop_ticks,
        });
        if self.runtime.objc.objc_recent_created_receivers.len() > 96 {
            let overflow = self
                .runtime
                .objc
                .objc_recent_created_receivers
                .len()
                .saturating_sub(96);
            self.runtime.objc.objc_recent_created_receivers.drain(0..overflow);
        }
        let observed_count = self
            .runtime
            .objc
            .objc_observed_instances_by_class
            .get(&class_ptr)
            .map(|items| items.len())
            .unwrap_or(0);
        if self.objc_is_bootstrap_consumer_class(class_ptr) {
            let signals = self.objc_bootstrap_signals_for_class(class_ptr);
            self.push_callback_trace(format!(
                "objc.create tick={} obj={} class={} origin={} signals={} created={} observed={}",
                self.runtime.ui_runtime.runloop_ticks,
                self.describe_ptr(receiver),
                class_name,
                origin,
                if signals.is_empty() { "<none>".to_string() } else { signals.join("|") },
                created_len,
                observed_count,
            ));
        }
        if self.objc_class_is_network_owner_candidate(class_ptr, class_name) {
            let detail = format!("origin={} created={} observed={}", origin, created_len, observed_count);
            self.note_network_owner_candidate("objc.create", receiver, class_ptr, class_name, None, &detail);
        }
    }

    fn objc_created_receiver_candidates_for_selector(
        &mut self,
        selector: &str,
    ) -> Vec<(u32, u32, String, u32, String)> {
        let unique_matches = self.objc_selector_unique_class_matches(selector);
        if unique_matches.is_empty() {
            return Vec::new();
        }
        let allowed: HashMap<u32, String> = unique_matches.into_iter().collect();
        let recent = self.runtime.objc.objc_recent_created_receivers.clone();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for entry in recent.iter().rev() {
            let Some(class_name) = allowed.get(&entry.class_ptr) else { continue; };
            if !seen.insert(entry.receiver) {
                continue;
            }
            let Some(current_class_ptr) = self.objc_class_ptr_for_receiver(entry.receiver) else {
                continue;
            };
            if current_class_ptr != entry.class_ptr {
                continue;
            }
            out.push((
                entry.receiver,
                entry.class_ptr,
                class_name.clone(),
                entry.tick,
                entry.origin.clone(),
            ));
            if out.len() >= 8 {
                break;
            }
        }
        out
    }

    fn objc_created_receiver_candidates_summary(&mut self, selector: &str) -> String {
        let candidates = self.objc_created_receiver_candidates_for_selector(selector);
        if candidates.is_empty() {
            return "<none>".to_string();
        }
        candidates
            .into_iter()
            .take(4)
            .map(|(receiver, _class_ptr, class_name, tick, origin)| {
                format!(
                    "{}<{}>@tick{} origin={}",
                    self.describe_ptr(receiver),
                    class_name,
                    tick,
                    origin,
                )
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn objc_receiver_bootstrap_signal_summary(&mut self, receiver: u32) -> String {
        let Some(class_ptr) = self.objc_class_ptr_for_receiver(receiver) else {
            return "<none>".to_string();
        };
        let signals = self.objc_bootstrap_signals_for_class(class_ptr);
        if signals.is_empty() {
            "<none>".to_string()
        } else {
            signals.join("|")
        }
    }

    fn push_network_delegate_binding_summary(&mut self, event: String) {
        const MAX_EVENTS: usize = 24;
        self.runtime.ui_network.network_last_delegate_binding = Some(event.clone());
        self.runtime.ui_network.network_delegate_binding_trace.push(event);
        if self.runtime.ui_network.network_delegate_binding_trace.len() > MAX_EVENTS {
            let overflow = self.runtime.ui_network.network_delegate_binding_trace.len().saturating_sub(MAX_EVENTS);
            self.runtime.ui_network.network_delegate_binding_trace.drain(0..overflow);
        }
    }

    fn push_network_connection_birth_summary(&mut self, event: String) {
        const MAX_EVENTS: usize = 24;
        self.runtime.ui_network.network_last_connection_birth = Some(event.clone());
        self.runtime.ui_network.network_connection_birth_trace.push(event);
        if self.runtime.ui_network.network_connection_birth_trace.len() > MAX_EVENTS {
            let overflow = self.runtime.ui_network.network_connection_birth_trace.len().saturating_sub(MAX_EVENTS);
            self.runtime.ui_network.network_connection_birth_trace.drain(0..overflow);
        }
    }

    fn push_network_slot_summary(&mut self, event: String) {
        const MAX_EVENTS: usize = 64;
        self.runtime.ui_network.network_last_slot_event = Some(event.clone());
        self.runtime.ui_network.network_slot_trace.push(event);
        if self.runtime.ui_network.network_slot_trace.len() > MAX_EVENTS {
            let overflow = self.runtime.ui_network.network_slot_trace.len().saturating_sub(MAX_EVENTS);
            self.runtime.ui_network.network_slot_trace.drain(0..overflow);
        }
    }

    fn push_network_owner_candidate_summary(&mut self, event: String) {
        const MAX_EVENTS: usize = 24;
        self.runtime.ui_network.network_last_owner_candidate = Some(event.clone());
        self.runtime.ui_network.network_owner_candidate_trace.push(event);
        if self.runtime.ui_network.network_owner_candidate_trace.len() > MAX_EVENTS {
            let overflow = self.runtime.ui_network.network_owner_candidate_trace.len().saturating_sub(MAX_EVENTS);
            self.runtime.ui_network.network_owner_candidate_trace.drain(0..overflow);
        }
    }

    fn objc_class_is_network_owner_candidate(&mut self, class_ptr: u32, class_name: &str) -> bool {
        if class_ptr == 0 {
            return false;
        }
        if matches!(class_name, "CLScoreServerPost" | "CLScoreServerRequest") {
            return true;
        }
        let signals = self.objc_bootstrap_signals_for_class(class_ptr);
        signals.iter().any(|signal| matches!(signal.as_str(),
            "connectionDidFinishLoading:" |
            "connection:didReceiveData:" |
            "connection:didReceiveResponse:" |
            "connection:didFailWithError:" |
            "parse"
        ))
    }

    fn note_network_owner_candidate(
        &mut self,
        source: &str,
        receiver: u32,
        class_ptr: u32,
        class_name: &str,
        selector: Option<&str>,
        detail: &str,
    ) {
        if receiver == 0 || class_ptr == 0 {
            return;
        }
        let pc = self.cpu.regs[15];
        let lr = self.cpu.regs[14];
        let signals = self.objc_bootstrap_signals_for_class(class_ptr);
        let event = format!(
            "tick={} source={} obj={} class={} selector={} signals={} conn={} delegate={} request={} url={} method={} pc=0x{:08x}({}) lr=0x{:08x}({}) detail={}",
            self.runtime.ui_runtime.runloop_ticks,
            source,
            self.describe_ptr(receiver),
            class_name,
            selector.unwrap_or("<none>"),
            if signals.is_empty() { "<none>".to_string() } else { signals.join("|") },
            self.describe_ptr(self.runtime.ui_network.network_connection),
            self.describe_ptr(self.current_network_delegate()),
            self.describe_ptr(self.runtime.ui_network.network_request),
            self.network_url_string(),
            self.network_http_method(),
            pc,
            self.symbol_or_addr(pc),
            lr,
            self.symbol_or_addr(lr),
            detail,
        );
        self.push_callback_trace(format!("network.owner {}", event));
        self.push_network_owner_candidate_summary(event);
    }

    fn symbol_or_addr(&self, addr: u32) -> String {
        self.symbol_label(addr & !1)
            .map(|label| label.to_string())
            .unwrap_or_else(|| format!("0x{:08x}", addr))
    }

    fn note_network_seeded_slots(&mut self, source: &str) {
        if self.runtime.ui_network.network_seed_trace_emitted {
            return;
        }
        self.runtime.ui_network.network_seed_trace_emitted = true;
        let owner = self.runtime.ui_objects.app;
        let request = self.runtime.ui_network.network_request;
        for (slot, value, reason) in [
            ("delegate", self.runtime.ui_network.network_delegate, "preseed-default"),
            ("url", self.runtime.ui_network.network_url, "preseed-default"),
            ("request", self.runtime.ui_network.network_request, "preseed-default"),
            ("connection", self.runtime.ui_network.network_connection, "preseed-default"),
            ("response", self.runtime.ui_network.network_response, "preseed-default"),
            ("data", self.runtime.ui_network.network_data, "preseed-default"),
            ("error", self.runtime.ui_network.network_error, "preseed-default"),
            ("readStream", self.runtime.ui_network.read_stream, "preseed-default"),
            ("writeStream", self.runtime.ui_network.write_stream, "preseed-default"),
            ("reachability", self.runtime.ui_network.reachability, "preseed-default"),
        ] {
            self.note_network_slot_assignment(slot, source, 0, value, owner, request, reason);
        }
    }

    fn note_network_slot_assignment(
        &mut self,
        slot: &str,
        source: &str,
        old_value: u32,
        new_value: u32,
        owner: u32,
        request: u32,
        reason: &str,
    ) {
        let pc = self.cpu.regs[15];
        let lr = self.cpu.regs[14];
        let owner_class = self.objc_receiver_class_name_hint(owner).unwrap_or_else(|| "<unknown>".to_string());
        let request_class = self.objc_receiver_class_name_hint(request).unwrap_or_else(|| "<unknown>".to_string());
        let old_class = self.objc_receiver_class_name_hint(old_value).unwrap_or_else(|| "<unknown>".to_string());
        let new_class = self.objc_receiver_class_name_hint(new_value).unwrap_or_else(|| "<unknown>".to_string());
        let delegate = self.current_network_delegate();
        let delegate_class = self.objc_receiver_class_name_hint(delegate).unwrap_or_else(|| "<unknown>".to_string());
        let detail = format!(
            "tick={} source={} slot={} old={} oldClass={} new={} newClass={} owner={} ownerClass={} request={} requestClass={} conn={} delegate={} delegateClass={} reason={} pc=0x{:08x}({}) lr=0x{:08x}({})",
            self.runtime.ui_runtime.runloop_ticks,
            source,
            slot,
            self.describe_ptr(old_value),
            old_class,
            self.describe_ptr(new_value),
            new_class,
            self.describe_ptr(owner),
            owner_class,
            self.describe_ptr(request),
            request_class,
            self.describe_ptr(self.runtime.ui_network.network_connection),
            self.describe_ptr(delegate),
            delegate_class,
            reason,
            pc,
            self.symbol_or_addr(pc),
            lr,
            self.symbol_or_addr(lr),
        );
        if self.runtime.ui_network.first_app_delegate_binding.is_none()
            && (slot == "delegate" || slot == "UIApplication.delegate" || slot == "NSURLConnection.delegate")
            && (new_class.contains("AppDelegate") || self.describe_ptr(new_value).contains("AppDelegate"))
        {
            self.runtime.ui_network.first_app_delegate_binding = Some(detail.clone());
        }
        self.push_callback_trace(format!("network.slot {}", detail));
        self.push_network_slot_summary(detail);
    }

    fn assign_network_delegate_with_provenance(
        &mut self,
        source: &str,
        owner: u32,
        request: u32,
        new_delegate: u32,
        reason: &str,
    ) {
        self.note_network_seeded_slots(source);
        let old_delegate = self.runtime.ui_network.network_delegate;
        self.note_network_slot_assignment("delegate", source, old_delegate, new_delegate, owner, request, reason);
        self.runtime.ui_network.network_delegate = new_delegate;
    }

    fn note_network_slot_touch(
        &mut self,
        slot: &str,
        source: &str,
        owner: u32,
        value: u32,
        request: u32,
        reason: &str,
    ) {
        self.note_network_seeded_slots(source);
        self.note_network_slot_assignment(slot, source, value, value, owner, request, reason);
    }

    fn note_network_connection_birth(
        &mut self,
        source: &str,
        owner: u32,
        request: u32,
        delegate: u32,
        connection: u32,
    ) {
        let pc = self.cpu.regs[15];
        let lr = self.cpu.regs[14];
        let owner_class = self.objc_receiver_class_name_hint(owner).unwrap_or_else(|| "<unknown>".to_string());
        let request_class = self.objc_receiver_class_name_hint(request).unwrap_or_else(|| "<unknown>".to_string());
        let delegate_class = self.objc_receiver_class_name_hint(delegate).unwrap_or_else(|| "<unknown>".to_string());
        let tick = self.runtime.ui_runtime.runloop_ticks;
        let delegate_signals = self.objc_receiver_bootstrap_signal_summary(delegate);
        let url = self.network_url_string();
        let method = self.network_http_method();
        let detail = format!(
            "tick={} source={} conn={} owner={} ownerClass={} request={} requestClass={} delegate={} delegateClass={} delegateSignals={} pc=0x{:08x} lr=0x{:08x} url={} method={}",
            tick,
            source,
            self.describe_ptr(connection),
            self.describe_ptr(owner),
            owner_class,
            self.describe_ptr(request),
            request_class,
            self.describe_ptr(delegate),
            delegate_class,
            delegate_signals,
            pc,
            lr,
            url,
            method,
        );
        self.push_callback_trace(format!("connection.birth {}", detail));
        self.push_network_connection_birth_summary(detail);
    }

    fn note_objc_delegate_binding(&mut self, source: &str, role: &str, owner: u32, delegate: u32, request: u32, previous_delegate: u32) {
        let owner_class = self.objc_receiver_class_name_hint(owner).unwrap_or_else(|| "<unknown>".to_string());
        let delegate_class = self.objc_receiver_class_name_hint(delegate).unwrap_or_else(|| "<unknown>".to_string());
        let request_class = self.objc_receiver_class_name_hint(request).unwrap_or_else(|| "<unknown>".to_string());
        let delegate_signals = if let Some(class_ptr) = self.objc_class_ptr_for_receiver(delegate) {
            let signals = self.objc_bootstrap_signals_for_class(class_ptr);
            if signals.is_empty() { "<none>".to_string() } else { signals.join("|") }
        } else {
            "<none>".to_string()
        };
        let pc = self.cpu.regs[15];
        let lr = self.cpu.regs[14];
        let detail = format!(
            "tick={} source={} role={} owner={} ownerClass={} delegate={} delegateClass={} delegateSignals={} previousDelegate={} request={} requestClass={} conn={} pc=0x{:08x} lr=0x{:08x} url={} method={}",
            self.runtime.ui_runtime.runloop_ticks,
            source,
            role,
            self.describe_ptr(owner),
            owner_class,
            self.describe_ptr(delegate),
            delegate_class,
            delegate_signals,
            self.describe_ptr(previous_delegate),
            self.describe_ptr(request),
            request_class,
            self.describe_ptr(self.runtime.ui_network.network_connection),
            pc,
            lr,
            self.network_url_string(),
            self.network_http_method(),
        );
        if self.runtime.ui_network.first_app_delegate_binding.is_none()
            && delegate_class.contains("AppDelegate")
        {
            self.runtime.ui_network.first_app_delegate_binding = Some(detail.clone());
        }
        self.push_callback_trace(format!("delegate.bind {}", detail));
        self.push_network_delegate_binding_summary(detail);
    }

    fn objc_receiver_is_networkish(&mut self, receiver: u32) -> bool {
        if receiver == 0 {
            return false;
        }
        let receiver_class = self.objc_receiver_class_name_hint(receiver).unwrap_or_default();
        let receiver_label = self.diag.object_labels.get(&receiver).cloned().unwrap_or_default();
        receiver == self.runtime.ui_network.network_connection
            || receiver == self.runtime.ui_network.fault_connection
            || receiver == self.runtime.ui_network.read_stream
            || receiver == self.runtime.ui_network.write_stream
            || receiver_class.contains("NSURLConnection")
            || receiver_class.contains("NSURLRequest")
            || receiver_class.contains("NSMutableURLRequest")
            || receiver_class.contains("NSURLResponse")
            || receiver_class.contains("NSURL")
            || receiver_label.contains("NSURLConnection")
            || receiver_label.contains("NSURLRequest")
            || receiver_label.contains("NSURLResponse")
            || receiver_label.contains("NSURL")
            || receiver_label.contains("CFReadStream")
            || receiver_label.contains("CFWriteStream")
    }

    fn objc_should_prefer_real_meta_alloc(&mut self, class_ptr: u32, class_name: &str, selector: &str) -> bool {
        if class_ptr == 0 {
            return false;
        }
        if class_name == "TouchController" {
            return true;
        }
        let has_custom_meta_alloc = self
            .runtime
            .objc
            .objc_classes_by_ptr
            .get(&class_ptr)
            .and_then(|info| info.meta_methods.get(selector).copied())
            .unwrap_or(0)
            != 0;
        if !has_custom_meta_alloc {
            return false;
        }
        self.objc_class_is_network_owner_candidate(class_ptr, class_name)
    }

    fn objc_should_trace_audio_receiver_ivars(selector: &str, receiver_class: &str) -> bool {
        matches!(selector,
            "configure:channelGroupDefinitions:channelGroupTotal:"
                | "init:channelGroupDefinitions:channelGroupTotal:"
                | "preloadBackgroundMusic:"
                | "playBackgroundMusic:"
                | "playBackgroundMusic:loop:"
                | "backgroundMusic"
                | "soundEngine"
        ) && receiver_class.contains("CDAudioManager")
    }

    fn objc_audio_is_relevant_ivar_name(name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        [
            "audio",
            "sound",
            "engine",
            "music",
            "bgm",
            "background",
            "player",
            "source",
            "channel",
            "buffer",
            "file",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }

    fn objc_collect_audio_receiver_ivar_snapshot(&mut self, receiver: u32) -> Vec<PendingAudioIvarSnapshot> {
        if receiver == 0 {
            return Vec::new();
        }
        let mut current = match self.objc_class_ptr_for_receiver(receiver) {
            Some(class_ptr) => class_ptr,
            None => return Vec::new(),
        };
        let mut all = Vec::new();
        for _ in 0..64 {
            self.ensure_objc_class_hierarchy_indexed(current);
            let Some(info) = self.runtime.objc.objc_classes_by_ptr.get(&current).cloned() else {
                break;
            };
            for (name, offset) in info.ivars.iter() {
                if all.iter().any(|existing: &PendingAudioIvarSnapshot| existing.name == *name && existing.offset == *offset) {
                    continue;
                }
                let value = self.read_u32_le(receiver.wrapping_add(*offset)).unwrap_or(0);
                let value_class = self.objc_receiver_class_name_hint(value);
                let value_desc = if let Some(class_name) = value_class.as_deref() {
                    format!("{}<{}>", self.describe_ptr(value), class_name)
                } else {
                    self.describe_ptr(value)
                };
                all.push(PendingAudioIvarSnapshot {
                    owner_class: info.name.clone(),
                    name: name.clone(),
                    offset: *offset,
                    value,
                    value_desc,
                    value_class,
                });
            }
            if info.superclass == 0 || info.superclass == current {
                break;
            }
            current = info.superclass;
        }
        all.sort_by_key(|entry| entry.offset);
        let mut relevant: Vec<_> = all
            .iter()
            .filter(|entry| Self::objc_audio_is_relevant_ivar_name(&entry.name))
            .cloned()
            .collect();
        if relevant.is_empty() {
            relevant = all.into_iter().take(16).collect();
        }
        relevant
    }

    fn objc_format_audio_receiver_ivar_snapshot(snapshot: &[PendingAudioIvarSnapshot]) -> String {
        if snapshot.is_empty() {
            return "<none>".to_string();
        }
        snapshot
            .iter()
            .map(|entry| {
                let class_suffix = entry
                    .value_class
                    .as_deref()
                    .map(|class_name| format!("/{}", class_name))
                    .unwrap_or_default();
                format!(
                    "{}::{}@0x{:x}={}{}",
                    entry.owner_class,
                    entry.name,
                    entry.offset,
                    entry.value_desc,
                    class_suffix,
                )
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn objc_format_audio_receiver_ivar_diff(
        before: &[PendingAudioIvarSnapshot],
        after: &[PendingAudioIvarSnapshot],
    ) -> String {
        let mut diffs = Vec::new();
        for entry in after {
            let previous = before
                .iter()
                .find(|candidate| candidate.name == entry.name && candidate.offset == entry.offset);
            if let Some(previous) = previous {
                if previous.value == entry.value {
                    continue;
                }
                diffs.push(format!(
                    "{}::{}@0x{:x}: {} -> {}",
                    entry.owner_class,
                    entry.name,
                    entry.offset,
                    previous.value_desc,
                    entry.value_desc,
                ));
            } else {
                diffs.push(format!(
                    "{}::{}@0x{:x}: <new> -> {}",
                    entry.owner_class,
                    entry.name,
                    entry.offset,
                    entry.value_desc,
                ));
            }
        }
        if diffs.is_empty() {
            "<none>".to_string()
        } else {
            diffs.join(" | ")
        }
    }

    fn objc_audio_nil_result_inference(
        &self,
        selector: &str,
        snapshot: &[PendingAudioIvarSnapshot],
    ) -> Option<String> {
        let needles: &[&str] = match selector {
            "soundEngine" => &["sound", "engine"],
            "backgroundMusic" => &["background", "music", "bgm"],
            _ => return None,
        };
        let candidates: Vec<_> = snapshot
            .iter()
            .filter(|entry| {
                let lower = entry.name.to_ascii_lowercase();
                needles.iter().any(|needle| lower.contains(needle))
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        Some(
            candidates
                .into_iter()
                .map(|entry| format!("{}@0x{:x}={}", entry.name, entry.offset, entry.value_desc))
                .collect::<Vec<_>>()
                .join(" | "),
        )
    }

    fn objc_audio_snapshot_indicates_live_cdaudio_manager(snapshot: &[PendingAudioIvarSnapshot]) -> bool {
        snapshot.iter().any(|entry| {
            if entry.value == 0 {
                return false;
            }
            let lower = entry.name.to_ascii_lowercase();
            lower.contains("soundengine")
                || lower.contains("lastbackgroundmusicfilepath")
                || (lower.contains("backgroundmusic")
                    && !lower.contains("completion")
                    && !lower.contains("listener"))
        })
    }

    fn objc_maybe_register_live_cdaudio_manager_receiver(
        &mut self,
        receiver: u32,
        snapshot: &[PendingAudioIvarSnapshot],
        reason: &str,
    ) -> Option<String> {
        if receiver == 0 || !Self::objc_audio_snapshot_indicates_live_cdaudio_manager(snapshot) {
            return None;
        }
        let class_ptr = self.objc_lookup_class_by_name("CDAudioManager")?;
        let previous = self.runtime.graphics.cocos_audio_manager_object;
        if previous == receiver
            && self.runtime.objc.objc_singletons_by_class.get(&class_ptr).copied() == Some(receiver)
        {
            return None;
        }
        self.objc_register_guest_singleton_object(class_ptr, "CDAudioManager", receiver, reason);
        Some(format!(
            "canonical-singleton:CDAudioManager prev={} -> {}",
            self.describe_ptr(previous),
            self.describe_ptr(receiver),
        ))
    }

    fn objc_should_watch_real_audio_selector_return(
        &mut self,
        selector: &str,
        receiver: u32,
        receiver_class: &str,
    ) -> bool {
        if Self::audio_is_objc_audio_class(receiver_class) {
            return true;
        }
        if Self::audio_is_objc_audio_selector(selector) {
            return true;
        }
        if receiver != 0 {
            let class_name = self.objc_receiver_class_name_hint(receiver).unwrap_or_default();
            if Self::audio_is_objc_audio_class(&class_name) {
                return true;
            }
        }
        false
    }

    fn arm_real_audio_selector_return_watch(
        &mut self,
        selector: &str,
        receiver: u32,
        receiver_class: &str,
        arg2: u32,
        _arg3: u32,
        imp: u32,
        current_pc: u32,
        return_lr: u32,
    ) {
        if !self.objc_should_watch_real_audio_selector_return(selector, receiver, receiver_class) {
            return;
        }
        let return_pc = return_lr & !1;
        if return_pc == 0 {
            return;
        }
        let return_thumb = (return_lr & 1) != 0;
        let resource = self
            .resolve_path_from_url_like_value(arg2, false)
            .map(|path| path.display().to_string())
            .or_else(|| self.guest_string_value(arg2));
        self.audio_trace_note_objc_audio_selector(receiver_class, selector, resource.clone(), false);
        self.audio_trace_push_event(format!(
            "objc.audio.real-dispatch class={} selector={} receiver={} imp=0x{:08x} resource={}",
            if receiver_class.is_empty() { "<unknown>" } else { receiver_class },
            selector,
            self.describe_ptr(receiver),
            imp,
            resource.clone().unwrap_or_else(|| "<none>".to_string()),
        ));
        let receiver_ivars_before = if Self::objc_should_trace_audio_receiver_ivars(selector, receiver_class) {
            self.objc_collect_audio_receiver_ivar_snapshot(receiver)
        } else {
            Vec::new()
        };
        if !receiver_ivars_before.is_empty() {
            self.audio_trace_push_event(format!(
                "objc.audio.ivars.before class={} selector={} receiver={} ivars={}",
                if receiver_class.is_empty() { "<unknown>" } else { receiver_class },
                selector,
                self.describe_ptr(receiver),
                Self::objc_format_audio_receiver_ivar_snapshot(&receiver_ivars_before),
            ));
        }
        let watch = PendingAudioSelectorReturn {
            selector: selector.to_string(),
            receiver,
            receiver_class: receiver_class.to_string(),
            resource,
            imp,
            return_pc,
            return_thumb,
            dispatch_pc: current_pc,
            receiver_ivars_before,
        };
        self.runtime.audio_trace.pending_selector_returns.push(watch);
        self.diag.trace.push(format!(
            "     ↳ audio.return-watch arm selector={} receiver={} receiverClass={} imp=0x{:08x} return=0x{:08x}({}) dispatchPc=0x{:08x} depth={}",
            selector,
            self.describe_ptr(receiver),
            if receiver_class.is_empty() { "<unknown>" } else { receiver_class },
            imp,
            return_pc,
            if return_thumb { "thumb" } else { "arm" },
            current_pc,
            self.runtime.audio_trace.pending_selector_returns.len(),
        ));
    }

    fn process_real_audio_selector_return_watches(&mut self, origin: &str) -> usize {
        let mut fired = 0usize;
        loop {
            let Some(top) = self.runtime.audio_trace.pending_selector_returns.last().cloned() else {
                break;
            };
            if self.cpu.regs[15] != top.return_pc || self.cpu.thumb != top.return_thumb {
                break;
            }
            self.runtime.audio_trace.pending_selector_returns.pop();
            self.finish_real_audio_selector_return(top, origin);
            fired = fired.saturating_add(1);
        }
        fired
    }

    fn maybe_repair_real_audio_selector_return(
        &mut self,
        watch: &PendingAudioSelectorReturn,
        result: u32,
    ) -> Option<String> {
        if result == 0 {
            return None;
        }
        let expected_class = match watch.selector.as_str() {
            "sharedManager" if watch.receiver_class == "CDAudioManager" => Some("CDAudioManager"),
            "sharedEngine" if watch.receiver_class == "SimpleAudioEngine" => Some("SimpleAudioEngine"),
            "sharedEngine" if watch.receiver_class == "OALSimpleAudio" => Some("OALSimpleAudio"),
            "soundEngine" => Some("CDSoundEngine"),
            "initWithContentsOfURL:error:" | "initWithData:error:" if watch.receiver_class == "AVAudioPlayer" => Some("AVAudioPlayer"),
            _ => None,
        }?;
        let class_ptr = self.objc_lookup_class_by_name(expected_class)?;
        let mut repairs = Vec::new();
        if self.objc_class_name_for_receiver(result).is_none() {
            self.objc_attach_receiver_class(result, class_ptr, expected_class);
            repairs.push(format!("attach:{}", expected_class));
        }
        if matches!(watch.selector.as_str(), "sharedManager" | "sharedEngine" | "soundEngine") {
            self.objc_register_guest_singleton_object(class_ptr, expected_class, result, watch.selector.as_str());
            repairs.push(format!("singleton:{}", expected_class));
        }
        if repairs.is_empty() {
            None
        } else {
            Some(repairs.join("+"))
        }
    }

    fn finish_real_audio_selector_return(&mut self, watch: PendingAudioSelectorReturn, origin: &str) {
        let result = self.cpu.regs[0];
        let selector = watch.selector.as_str();
        let result_class = self.objc_receiver_class_name_hint(result).unwrap_or_else(|| "<unknown>".to_string());
        if selector == "soundEngine" && result == 0 {
            self.runtime.audio_trace.objc_audio_manager_soundengine_nil_results = self.runtime.audio_trace.objc_audio_manager_soundengine_nil_results.saturating_add(1);
        }
        self.runtime.audio_trace.objc_audio_last_class = Some(watch.receiver_class.clone());
        self.runtime.audio_trace.objc_audio_last_selector = Some(watch.selector.clone());
        self.runtime.audio_trace.objc_audio_last_resource = watch.resource.clone();
        self.runtime.audio_trace.objc_audio_last_result = Some(format!(
            "{} class={} imp=0x{:08x}",
            self.describe_ptr(result),
            result_class,
            watch.imp,
        ));
        let receiver_ivars_after = if !watch.receiver_ivars_before.is_empty()
            || Self::objc_should_trace_audio_receiver_ivars(selector, &watch.receiver_class)
        {
            self.objc_collect_audio_receiver_ivar_snapshot(watch.receiver)
        } else {
            Vec::new()
        };
        let mut repairs = Vec::new();
        if let Some(repair) = self.maybe_repair_real_audio_selector_return(&watch, result) {
            repairs.push(repair);
        }
        if watch.receiver_class == "CDAudioManager" {
            let canonical_reason = format!("receiver-{}", selector);
            if let Some(repair) = self.objc_maybe_register_live_cdaudio_manager_receiver(
                watch.receiver,
                &receiver_ivars_after,
                &canonical_reason,
            ) {
                repairs.push(repair);
            }
        }
        let repair = if repairs.is_empty() {
            None
        } else {
            Some(repairs.join("+"))
        };
        self.audio_trace_push_event(format!(
            "objc.audio.return class={} selector={} receiver={} imp=0x{:08x} result={} resultClass={} resource={} repair={} origin={}",
            if watch.receiver_class.is_empty() { "<unknown>" } else { &watch.receiver_class },
            selector,
            self.describe_ptr(watch.receiver),
            watch.imp,
            self.describe_ptr(result),
            result_class,
            watch.resource.clone().unwrap_or_else(|| "<none>".to_string()),
            repair.clone().unwrap_or_else(|| "none".to_string()),
            origin,
        ));
        if !watch.receiver_ivars_before.is_empty() || !receiver_ivars_after.is_empty() {
            let ivar_diff = Self::objc_format_audio_receiver_ivar_diff(
                &watch.receiver_ivars_before,
                &receiver_ivars_after,
            );
            self.audio_trace_push_event(format!(
                "objc.audio.ivars.after class={} selector={} receiver={} diff={} ivars={}",
                if watch.receiver_class.is_empty() { "<unknown>" } else { &watch.receiver_class },
                selector,
                self.describe_ptr(watch.receiver),
                ivar_diff,
                Self::objc_format_audio_receiver_ivar_snapshot(&receiver_ivars_after),
            ));
            if result == 0 {
                if let Some(inference) = self.objc_audio_nil_result_inference(selector, &receiver_ivars_after) {
                    self.audio_trace_push_event(format!(
                        "objc.audio.infer class={} selector={} reason=nil-result candidates={}",
                        if watch.receiver_class.is_empty() { "<unknown>" } else { &watch.receiver_class },
                        selector,
                        inference,
                    ));
                }
            }
            if matches!(selector, "configure:channelGroupDefinitions:channelGroupTotal:" | "preloadBackgroundMusic:")
                && ivar_diff == "<none>"
                && !receiver_ivars_after.is_empty()
            {
                self.audio_trace_push_event(format!(
                    "objc.audio.infer class={} selector={} reason=no-relevant-ivar-changes snapshot={}",
                    if watch.receiver_class.is_empty() { "<unknown>" } else { &watch.receiver_class },
                    selector,
                    Self::objc_format_audio_receiver_ivar_snapshot(&receiver_ivars_after),
                ));
            }
        }
        self.diag.trace.push(format!(
            "     ↳ audio.return-watch fire selector={} receiver={} receiverClass={} result={} resultClass={} resource={} imp=0x{:08x} origin={} dispatchPc=0x{:08x} return=0x{:08x}",
            selector,
            self.describe_ptr(watch.receiver),
            if watch.receiver_class.is_empty() { "<unknown>" } else { &watch.receiver_class },
            self.describe_ptr(result),
            result_class,
            watch.resource.clone().unwrap_or_else(|| "<none>".to_string()),
            watch.imp,
            origin,
            watch.dispatch_pc,
            watch.return_pc,
        ));
    }

    fn objc_should_watch_real_network_selector_return(
        &mut self,
        selector: &str,
        receiver: u32,
        arg2: u32,
        arg3: u32,
    ) -> bool {
        match selector {
            "URLWithString:" => self
                .objc_class_name_for_ptr(receiver)
                .map(|name| name.contains("NSURL"))
                .unwrap_or(false),
            "requestWithURL:" | "initWithURL:" => {
                self.objc_receiver_is_networkish(receiver)
                    || self
                        .objc_class_name_for_ptr(receiver)
                        .map(|name| name.contains("URLRequest"))
                        .unwrap_or(false)
            }
            "setDelegate:" | "scheduleInRunLoop:forMode:" | "start" | "cancel" => {
                self.objc_receiver_is_networkish(receiver)
            }
            "connectionWithRequest:delegate:" |
            "initWithRequest:delegate:" |
            "initWithRequest:delegate:startImmediately:" => {
                self.objc_receiver_is_networkish(receiver) || arg2 != 0 || arg3 != 0
            }
            "alloc" | "allocWithZone:" | "new" => {
                if receiver == 0 || !self.runtime.objc.objc_classes_by_ptr.contains_key(&receiver) {
                    return false;
                }
                let class_name = self
                    .objc_class_name_for_ptr(receiver)
                    .unwrap_or_else(|| format!("class@0x{receiver:08x}"));
                self.objc_class_is_network_owner_candidate(receiver, &class_name)
            }
            _ if selector.starts_with("init") => {
                let Some(class_ptr) = self.objc_class_ptr_for_receiver(receiver) else {
                    return false;
                };
                let class_name = self
                    .objc_class_name_for_ptr(class_ptr)
                    .unwrap_or_else(|| format!("class@0x{class_ptr:08x}"));
                self.objc_class_is_network_owner_candidate(class_ptr, &class_name)
            }
            _ => false,
        }
    }

    fn arm_real_network_selector_return_watch(
        &mut self,
        selector: &str,
        receiver: u32,
        arg2: u32,
        arg3: u32,
        aux: u32,
        current_pc: u32,
        return_lr: u32,
    ) {
        if !self.objc_should_watch_real_network_selector_return(selector, receiver, arg2, arg3) {
            return;
        }
        let return_pc = return_lr & !1;
        if return_pc == 0 {
            return;
        }
        let return_thumb = (return_lr & 1) != 0;
        let watch = PendingNetworkSelectorReturn {
            selector: selector.to_string(),
            receiver,
            arg2,
            arg3,
            aux,
            return_pc,
            return_thumb,
            dispatch_pc: current_pc,
        };
        self.runtime.ui_network.pending_selector_returns.push(watch);
        self.diag.trace.push(format!(
            "     ↳ network.return-watch arm selector={} receiver={} arg2={} arg3={} aux=0x{:08x} return=0x{:08x}({}) dispatchPc=0x{:08x} depth={}",
            selector,
            self.describe_ptr(receiver),
            self.describe_ptr(arg2),
            self.describe_ptr(arg3),
            aux,
            return_pc,
            if return_thumb { "thumb" } else { "arm" },
            current_pc,
            self.runtime.ui_network.pending_selector_returns.len(),
        ));
    }

    fn process_real_network_selector_return_watches(&mut self, origin: &str) -> usize {
        let mut fired = 0usize;
        loop {
            let Some(top) = self.runtime.ui_network.pending_selector_returns.last().cloned() else {
                break;
            };
            if self.cpu.regs[15] != top.return_pc || self.cpu.thumb != top.return_thumb {
                break;
            }
            self.runtime.ui_network.pending_selector_returns.pop();
            self.finish_real_network_selector_return(top, origin);
            fired = fired.saturating_add(1);
        }
        fired
    }

    fn finish_real_network_selector_return(&mut self, watch: PendingNetworkSelectorReturn, origin: &str) {
        let result = self.cpu.regs[0];
        let selector = watch.selector.as_str();
        let source = format!("real-return:{}", selector);
        self.note_network_seeded_slots(&source);
        let receiver_class = self.objc_receiver_class_name_hint(watch.receiver).unwrap_or_else(|| "<unknown>".to_string());
        let result_class = self.objc_receiver_class_name_hint(result).unwrap_or_else(|| "<unknown>".to_string());
        self.diag.trace.push(format!(
            "     ↳ network.return-watch fire selector={} receiver={} receiverClass={} arg2={} arg3={} aux=0x{:08x} result={} resultClass={} origin={} dispatchPc=0x{:08x} return=0x{:08x}",
            selector,
            self.describe_ptr(watch.receiver),
            receiver_class,
            self.describe_ptr(watch.arg2),
            self.describe_ptr(watch.arg3),
            watch.aux,
            self.describe_ptr(result),
            result_class,
            origin,
            watch.dispatch_pc,
            watch.return_pc,
        ));

        match selector {
            "URLWithString:" => {
                if result != 0 {
                    let old_url = self.runtime.ui_network.network_url;
                    self.note_network_slot_assignment("url", &source, old_url, result, watch.receiver, self.runtime.ui_network.network_request, "real selector return URL object");
                    self.runtime.ui_network.network_url = result;
                }
            }
            "requestWithURL:" | "initWithURL:" => {
                if watch.arg2 != 0 {
                    let old_url = self.runtime.ui_network.network_url;
                    self.note_network_slot_assignment("url", &source, old_url, watch.arg2, result, result, "real selector request argument URL");
                    self.runtime.ui_network.network_url = watch.arg2;
                }
                if result != 0 {
                    let old_request = self.runtime.ui_network.network_request;
                    self.note_network_slot_assignment("request", &source, old_request, result, result, result, "real selector request object");
                    self.runtime.ui_network.network_request = result;
                }
            }
            "connectionWithRequest:delegate:" | "initWithRequest:delegate:" | "initWithRequest:delegate:startImmediately:" => {
                if watch.arg2 != 0 {
                    let old_request = self.runtime.ui_network.network_request;
                    let owner = if result != 0 { result } else { watch.receiver };
                    self.note_network_slot_assignment("request", &source, old_request, watch.arg2, owner, watch.arg2, "real NSURLConnection constructor request");
                    self.runtime.ui_network.network_request = watch.arg2;
                }
                if result != 0 {
                    let old_connection = self.runtime.ui_network.network_connection;
                    self.note_network_slot_assignment("connection", &source, old_connection, result, result, watch.arg2, "real NSURLConnection constructor result");
                    self.runtime.ui_network.network_connection = result;
                    self.note_network_connection_birth(&source, watch.receiver, watch.arg2, watch.arg3, result);
                }
                if watch.arg3 != 0 {
                    let previous_delegate = self.runtime.ui_network.network_delegate;
                    self.note_objc_delegate_binding(&source, "NSURLConnection.delegate", if result != 0 { result } else { watch.receiver }, watch.arg3, watch.arg2, previous_delegate);
                    self.assign_network_delegate_with_provenance(
                        &source,
                        if result != 0 { result } else { watch.receiver },
                        watch.arg2,
                        watch.arg3,
                        "real NSURLConnection constructor delegate assignment",
                    );
                }
                self.runtime.ui_network.network_completed = false;
                self.runtime.ui_network.network_armed = if selector == "initWithRequest:delegate:startImmediately:" {
                    watch.aux != 0
                } else {
                    true
                };
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
                self.refresh_network_object_labels();
                self.recalc_runloop_sources();
            }
            "setDelegate:" => {
                if watch.arg2 != 0 && self.objc_receiver_is_networkish(watch.receiver) {
                    if watch.receiver != 0 {
                        let old_connection = self.runtime.ui_network.network_connection;
                        self.note_network_slot_assignment("connection", &source, old_connection, watch.receiver, watch.receiver, self.runtime.ui_network.network_request, "real selector delegate-setter receiver");
                        self.runtime.ui_network.network_connection = watch.receiver;
                    }
                    let previous_delegate = self.runtime.ui_network.network_delegate;
                    self.note_objc_delegate_binding(&source, "NSURLConnection.delegate", watch.receiver, watch.arg2, self.runtime.ui_network.network_request, previous_delegate);
                    self.assign_network_delegate_with_provenance(
                        &source,
                        watch.receiver,
                        self.runtime.ui_network.network_request,
                        watch.arg2,
                        "real selector delegate setter",
                    );
                }
            }
            "scheduleInRunLoop:forMode:" => {
                if watch.receiver != 0 && self.objc_receiver_is_networkish(watch.receiver) {
                    let old_connection = self.runtime.ui_network.network_connection;
                    self.note_network_slot_assignment("connection", &source, old_connection, watch.receiver, watch.receiver, self.runtime.ui_network.network_request, "real selector schedule receiver");
                    self.runtime.ui_network.network_connection = watch.receiver;
                    self.runtime.ui_network.network_armed = true;
                    self.runtime.ui_network.network_source_closed = false;
                    self.refresh_network_object_labels();
                    self.recalc_runloop_sources();
                }
            }
            "start" => {
                if watch.receiver != 0 && self.objc_receiver_is_networkish(watch.receiver) {
                    let old_connection = self.runtime.ui_network.network_connection;
                    self.note_network_slot_assignment("connection", &source, old_connection, watch.receiver, watch.receiver, self.runtime.ui_network.network_request, "real selector start receiver");
                    self.runtime.ui_network.network_connection = watch.receiver;
                    self.runtime.ui_network.network_armed = true;
                    self.runtime.ui_network.network_completed = false;
                    self.runtime.ui_network.network_source_closed = false;
                    self.runtime.ui_network.network_cancelled = false;
                    self.runtime.ui_network.network_faulted = false;
                    self.runtime.ui_network.network_fault_mode = 0;
                    self.runtime.ui_network.network_fault_history.clear();
                    self.refresh_network_object_labels();
                    self.recalc_runloop_sources();
                }
            }
            "cancel" => {
                if watch.receiver != 0 && self.objc_receiver_is_networkish(watch.receiver) {
                    let old_connection = self.runtime.ui_network.network_connection;
                    self.note_network_slot_assignment("connection", &source, old_connection, watch.receiver, watch.receiver, self.runtime.ui_network.network_request, "real selector cancel receiver");
                    self.runtime.ui_network.network_connection = watch.receiver;
                    self.runtime.ui_network.network_armed = false;
                    self.runtime.ui_network.network_completed = false;
                    self.runtime.ui_network.network_source_closed = true;
                    self.runtime.ui_network.network_cancelled = true;
                    self.runtime.ui_network.network_faulted = false;
                    self.refresh_network_object_labels();
                    self.recalc_runloop_sources();
                }
            }
            _ => {}
        }

        let candidate = match selector {
            "alloc" | "allocWithZone:" | "new" => result,
            _ if selector.starts_with("init") => {
                if result != 0 { result } else { watch.receiver }
            }
            _ => 0,
        };
        if candidate != 0 {
            if let Some(class_ptr) = self.objc_class_ptr_for_receiver(candidate) {
                let class_name = self
                    .objc_class_name_for_ptr(class_ptr)
                    .unwrap_or_else(|| format!("class@0x{class_ptr:08x}"));
                if self.objc_class_is_network_owner_candidate(class_ptr, &class_name) {
                    let detail = format!(
                        "selector={} origin={} receiver={} arg2={} arg3={} result={}",
                        selector,
                        origin,
                        self.describe_ptr(watch.receiver),
                        self.describe_ptr(watch.arg2),
                        self.describe_ptr(watch.arg3),
                        self.describe_ptr(result),
                    );
                    self.note_network_owner_candidate(&source, candidate, class_ptr, &class_name, Some(selector), &detail);
                }
            }
        }
    }

    fn objc_try_attach_receiver_class_for_selector(
        &mut self,
        receiver: u32,
        selector: &str,
        _origin: &str,
    ) -> Option<(String, String, u32)> {
        if receiver == 0 || self.objc_lookup_imp_for_receiver(receiver, selector).is_some() {
            return None;
        }

        if let Some(class_name) = self.runtime.objc.objc_bridge_delegate_class_name.clone() {
            if let Some(class_ptr) = self.objc_lookup_class_by_name(&class_name) {
                if self.objc_lookup_imp_in_class_chain(class_ptr, selector, false).is_some() {
                    self.objc_attach_receiver_class(receiver, class_ptr, &class_name);
                    return Some((class_name, "bridge-delegate".to_string(), class_ptr));
                }
            }
        }

        let unique_matches = self.objc_selector_unique_class_matches(selector);
        if unique_matches.len() == 1 {
            let (class_ptr, class_name) = unique_matches[0].clone();
            self.objc_attach_receiver_class(receiver, class_ptr, &class_name);
            return Some((class_name, "unique-selector-match".to_string(), class_ptr));
        }

        if Self::objc_selector_is_delegate_callback(selector) {
            if let Some((class_ptr, class_name, _hits)) = self.objc_infer_delegate_class_from_callbacks() {
                if self.objc_lookup_imp_in_class_chain(class_ptr, selector, false).is_some() {
                    self.objc_attach_receiver_class(receiver, class_ptr, &class_name);
                    return Some((class_name, "delegate-selector-coverage".to_string(), class_ptr));
                }
            }
        }

        None
    }

    fn trace_objc_selector_resolution(
        &mut self,
        receiver: u32,
        resolved_receiver: u32,
        selector: &str,
        origin: &str,
        repair: Option<(u32, String, String, u32)>,
    ) {
        let class_name = self
            .objc_class_name_for_receiver(receiver)
            .unwrap_or_else(|| "<unknown-class>".to_string());
        let resolved_class_name = self
            .objc_class_name_for_receiver(resolved_receiver)
            .unwrap_or_else(|| class_name.clone());
        let isa_mem = self.read_u32_le(receiver).unwrap_or(0);
        let isa_override = self.runtime.objc.objc_instance_isa_overrides.get(&receiver).copied().unwrap_or(0);
        let imp = self.objc_lookup_imp_for_receiver(resolved_receiver, selector);
        let imp_desc = imp
            .map(|value| format!("0x{value:08x}"))
            .unwrap_or_else(|| "<none>".to_string());
        let bridge_delegate = self
            .runtime
            .objc
            .objc_bridge_delegate_class_name
            .clone()
            .unwrap_or_else(|| "<none>".to_string());
        let unique_matches = self.objc_selector_unique_class_matches(selector);
        let unique_match_names = if unique_matches.is_empty() {
            "<none>".to_string()
        } else {
            unique_matches
                .iter()
                .take(4)
                .map(|(_, name)| name.as_str())
                .collect::<Vec<_>>()
                .join("|")
        };
        let observed_candidates = self.objc_observed_receiver_candidates_summary(selector);
        let created_candidates = self.objc_created_receiver_candidates_summary(selector);
        let repair_present = repair.is_some();
        let repair_desc = repair
            .as_ref()
            .map(|(repair_receiver, class_name, source, class_ptr)| {
                format!(
                    "receiver={} class={} via {} classPtr=0x{:08x}",
                    self.describe_ptr(*repair_receiver),
                    class_name,
                    source,
                    class_ptr,
                )
            })
            .unwrap_or_else(|| "<none>".to_string());
        self.diag.trace.push(format!(
            "     ↳ objc selector.resolve receiver={} resolvedReceiver={} selector={} origin={} class={} resolvedClass={} isaMem=0x{:08x} isaOverride={} bridgeDelegate={} imp={} uniqueMatches={} observedCandidates={} createdCandidates={} repair={}",
            self.describe_ptr(receiver),
            self.describe_ptr(resolved_receiver),
            selector,
            origin,
            class_name,
            resolved_class_name,
            isa_mem,
            if isa_override != 0 { format!("0x{isa_override:08x}") } else { "<none>".to_string() },
            bridge_delegate,
            imp_desc,
            unique_match_names,
            observed_candidates,
            created_candidates,
            repair_desc,
        ));
        if Self::objc_selector_is_delegate_callback(selector) {
            let should_trace = imp.is_none() || repair_present || selector == "connectionDidFinishLoading:";
            if should_trace {
                self.push_callback_trace(format!(
                    "objc.resolve tick={} sel={} recv={} resolved={} imp={} unique={} observed={} created={} repair={} origin={}",
                    self.runtime.ui_runtime.runloop_ticks,
                    selector,
                    self.describe_ptr(receiver),
                    self.describe_ptr(resolved_receiver),
                    imp_desc,
                    unique_match_names,
                    observed_candidates,
                    created_candidates,
                    repair_desc,
                    origin,
                ));
            }
        }
    }

    fn invoke_objc_selector_now_resolved(
        &mut self,
        receiver: u32,
        selector_name: &str,
        arg2: u32,
        arg3: u32,
        budget: u64,
        origin: &str,
    ) -> bool {
        let mut resolved_receiver = receiver;
        let mut repair: Option<(u32, String, String, u32)> = None;
        if self.objc_lookup_imp_for_receiver(receiver, selector_name).is_none() {
            if let Some((class_name, source, class_ptr)) =
                self.objc_try_attach_receiver_class_for_selector(receiver, selector_name, origin)
            {
                repair = Some((receiver, class_name, source, class_ptr));
            } else if let Some((redirect_receiver, class_name, source, class_ptr)) =
                self.objc_try_redirect_observed_receiver_for_selector(receiver, selector_name, origin)
            {
                resolved_receiver = redirect_receiver;
                repair = Some((redirect_receiver, class_name, source, class_ptr));
            }
        }
        self.trace_objc_selector_resolution(receiver, resolved_receiver, selector_name, origin, repair);
        self.invoke_objc_selector_now(resolved_receiver, selector_name, arg2, arg3, budget, origin)
    }

    fn objc_lookup_imp_for_super_call(&mut self, current_class: u32, selector: &str, skip_current_class: bool) -> Option<u32> {
        if current_class == 0 {
            return None;
        }
        self.ensure_objc_metadata_indexed();
        self.ensure_objc_class_hierarchy_indexed(current_class);
        let start_class = if skip_current_class {
            self.runtime.objc.objc_classes_by_ptr
                .get(&current_class)
                .map(|info| info.superclass)
                .filter(|superclass| *superclass != 0 && *superclass != current_class)?
        } else {
            current_class
        };
        self.ensure_objc_class_hierarchy_indexed(start_class);
        self.objc_lookup_imp_in_class_chain(start_class, selector, false)
    }

    fn objc_materialize_instance_with_extra(&mut self, class_ptr: u32, class_name: &str, extra_bytes: u32, tag: &str) -> Option<u32> {
        self.ensure_objc_metadata_indexed();
        let base_size = self
            .runtime.objc.objc_classes_by_ptr
            .get(&class_ptr)
            .map(|info| info.instance_size)
            .unwrap_or(0x40)
            .clamp(0x20, 0x4000);
        let size = base_size
            .checked_add(extra_bytes.min(0x10000))
            .unwrap_or(base_size)
            .clamp(0x20, 0x20000);
        let object = self
            .alloc_synthetic_heap_block(size, true, format!("objc.{tag}<{}>", class_name))
            .ok()?;
        self.write_u32_le(object, class_ptr).ok()?;
        self.runtime.objc.objc_instance_isa_overrides.insert(object, class_ptr);
        self.diag.object_labels
            .insert(object, format!("{}.instance(synth)", class_name));
        self.runtime.objc.objc_instances_materialized = self.runtime.objc.objc_instances_materialized.saturating_add(1);
        self.objc_note_created_instance(object, class_ptr, class_name, &format!("materialize:{}", tag));
        Some(object)
    }

    fn objc_materialize_instance(&mut self, class_ptr: u32, class_name: &str) -> Option<u32> {
        self.objc_materialize_instance_with_extra(class_ptr, class_name, 0, "alloc")
    }

    fn objc_class_ptr_for_receiver(&mut self, receiver: u32) -> Option<u32> {
        if receiver == 0 {
            return None;
        }
        self.ensure_objc_metadata_indexed();
        if self.runtime.objc.objc_classes_by_ptr.contains_key(&receiver) {
            self.ensure_objc_class_hierarchy_indexed(receiver);
            return Some(receiver);
        }
        if let Some(class_ptr) = self.runtime.objc.objc_instance_isa_overrides.get(&receiver).copied() {
            self.ensure_objc_class_hierarchy_indexed(class_ptr);
            return Some(class_ptr);
        }
        let isa = self.read_u32_le(receiver).ok()?;
        self.ensure_objc_class_hierarchy_indexed(isa);
        if self.runtime.objc.objc_classes_by_ptr.contains_key(&isa) {
            return Some(isa);
        }
        None
    }

    fn objc_hle_alloc_like(&mut self, receiver: u32, extra_bytes: u32, tag: &str) -> u32 {
        let class_ptr = match self.objc_class_ptr_for_receiver(receiver) {
            Some(ptr) => ptr,
            None => {
                self.runtime.objc.objc_last_alloc_class = None;
                self.runtime.objc.objc_last_alloc_receiver = Some(receiver);
                self.runtime.objc.objc_last_alloc_result = Some(receiver);
                return receiver;
            }
        };
        let class_name = self
            .objc_class_name_for_ptr(class_ptr)
            .unwrap_or_else(|| format!("class@0x{class_ptr:08x}"));
        let result = if self.runtime.objc.objc_classes_by_ptr.contains_key(&receiver) {
            self.objc_materialize_instance_with_extra(class_ptr, &class_name, extra_bytes, tag)
                .unwrap_or(0)
        } else if receiver != 0 {
            self.objc_attach_receiver_class(receiver, class_ptr, &class_name);
            receiver
        } else {
            0
        };
        self.runtime.objc.objc_last_alloc_class = Some(class_name);
        self.runtime.objc.objc_last_alloc_receiver = Some(receiver);
        self.runtime.objc.objc_last_alloc_result = Some(result);
        self.note_scene_instance_binding(receiver, result, "objc-alloc");
        result
    }

    fn objc_note_init_result(&mut self, receiver: u32, result: u32) {
        self.runtime.objc.objc_init_calls = self.runtime.objc.objc_init_calls.saturating_add(1);
        self.runtime.objc.objc_last_init_receiver = Some(receiver);
        self.runtime.objc.objc_last_init_result = Some(result);
        self.note_scene_instance_binding(receiver, result, "objc-init");
        if result != 0 {
            if let Some(class_ptr) = self.objc_class_ptr_for_receiver(result) {
                let class_name = self
                    .objc_class_name_for_ptr(class_ptr)
                    .unwrap_or_else(|| format!("class@0x{class_ptr:08x}"));
                if self.objc_is_bootstrap_consumer_class(class_ptr) {
                    let signals = self.objc_bootstrap_signals_for_class(class_ptr);
                    self.push_callback_trace(format!(
                        "objc.init tick={} recv={} result={} class={} signals={}",
                        self.runtime.ui_runtime.runloop_ticks,
                        self.describe_ptr(receiver),
                        self.describe_ptr(result),
                        class_name,
                        if signals.is_empty() { "<none>".to_string() } else { signals.join("|") },
                    ));
                }
                if self.objc_class_is_network_owner_candidate(class_ptr, &class_name) {
                    let detail = format!("initReceiver={} initResult={}", self.describe_ptr(receiver), self.describe_ptr(result));
                    self.note_network_owner_candidate("objc.init", result, class_ptr, &class_name, Some("init"), &detail);
                }
            }
        }
    }

    fn objc_note_observed_receiver(&mut self, receiver: u32, selector: &str) {
        if receiver == 0 || selector.trim().is_empty() {
            return;
        }
        let Some(class_ptr) = self.objc_class_ptr_for_receiver(receiver) else { return; };
        let class_name = self
            .objc_class_name_for_ptr(class_ptr)
            .unwrap_or_else(|| format!("class@0x{class_ptr:08x}"));
        let observed = self
            .runtime
            .objc
            .objc_observed_instances_by_class
            .entry(class_ptr)
            .or_default();
        if let Some(index) = observed.iter().position(|value| *value == receiver) {
            observed.remove(index);
        }
        observed.push(receiver);
        if observed.len() > 16 {
            let overflow = observed.len().saturating_sub(16);
            observed.drain(0..overflow);
        }
        self.runtime.objc.objc_recent_observed_receivers.push(ObjcObservedReceiver {
            receiver,
            class_ptr,
            class_name: class_name.clone(),
            selector: selector.to_string(),
            tick: self.runtime.ui_runtime.runloop_ticks,
        });
        if self.runtime.objc.objc_recent_observed_receivers.len() > 96 {
            let overflow = self
                .runtime
                .objc
                .objc_recent_observed_receivers
                .len()
                .saturating_sub(96);
            self.runtime.objc.objc_recent_observed_receivers.drain(0..overflow);
        }
        if self.objc_class_is_network_owner_candidate(class_ptr, &class_name) {
            let detail = format!("observedSelector={}", selector);
            self.note_network_owner_candidate("objc.observe", receiver, class_ptr, &class_name, Some(selector), &detail);
        }
    }

    fn objc_observed_receiver_candidates_for_selector(
        &mut self,
        selector: &str,
    ) -> Vec<(u32, u32, String, u32, String)> {
        let unique_matches = self.objc_selector_unique_class_matches(selector);
        if unique_matches.is_empty() {
            return Vec::new();
        }
        let allowed: HashMap<u32, String> = unique_matches.into_iter().collect();
        let recent = self.runtime.objc.objc_recent_observed_receivers.clone();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for entry in recent.iter().rev() {
            let Some(class_name) = allowed.get(&entry.class_ptr) else { continue; };
            if !seen.insert(entry.receiver) {
                continue;
            }
            let Some(current_class_ptr) = self.objc_class_ptr_for_receiver(entry.receiver) else {
                continue;
            };
            if current_class_ptr != entry.class_ptr {
                continue;
            }
            if self.objc_lookup_imp_for_receiver(entry.receiver, selector).is_none() {
                continue;
            }
            out.push((
                entry.receiver,
                entry.class_ptr,
                class_name.clone(),
                entry.tick,
                entry.selector.clone(),
            ));
            if out.len() >= 8 {
                break;
            }
        }
        out
    }

    fn objc_observed_receiver_candidates_summary(&mut self, selector: &str) -> String {
        let candidates = self.objc_observed_receiver_candidates_for_selector(selector);
        if candidates.is_empty() {
            return "<none>".to_string();
        }
        candidates
            .into_iter()
            .take(4)
            .map(|(receiver, _class_ptr, class_name, tick, last_selector)| {
                format!(
                    "{}<{}>@tick{} lastSel={}",
                    self.describe_ptr(receiver),
                    class_name,
                    tick,
                    last_selector,
                )
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn objc_try_redirect_observed_receiver_for_selector(
        &mut self,
        receiver: u32,
        selector: &str,
        _origin: &str,
    ) -> Option<(u32, String, String, u32)> {
        let mut candidates = self.objc_observed_receiver_candidates_for_selector(selector);
        candidates.retain(|(candidate_receiver, _, _, _, _)| *candidate_receiver != receiver);
        if candidates.is_empty() {
            return None;
        }
        if candidates.len() == 1 {
            let (candidate_receiver, class_ptr, class_name, _tick, _last_selector) =
                candidates.remove(0);
            return Some((
                candidate_receiver,
                class_name,
                "observed-live-instance".to_string(),
                class_ptr,
            ));
        }
        let distinct_classes: HashSet<u32> = candidates
            .iter()
            .map(|(_, class_ptr, _, _, _)| *class_ptr)
            .collect();
        if distinct_classes.len() == 1 {
            let (candidate_receiver, class_ptr, class_name, _tick, _last_selector) =
                candidates.remove(0);
            return Some((
                candidate_receiver,
                class_name,
                "observed-single-class".to_string(),
                class_ptr,
            ));
        }
        let newest_tick = candidates[0].3;
        let newest: Vec<_> = candidates
            .into_iter()
            .filter(|(_, _, _, tick, _)| *tick == newest_tick)
            .collect();
        if newest.len() == 1 {
            let (candidate_receiver, class_ptr, class_name, _tick, last_selector) =
                newest.into_iter().next()?;
            return Some((
                candidate_receiver,
                class_name,
                format!("observed-most-recent lastSel={}", last_selector),
                class_ptr,
            ));
        }
        None
    }

    fn objc_attach_receiver_class(&mut self, receiver: u32, class_ptr: u32, class_name: &str) {
        if receiver == 0 {
            return;
        }
        self.runtime.objc.objc_instance_isa_overrides.insert(receiver, class_ptr);
        let _ = self.write_u32_le(receiver, class_ptr);
        self.diag.object_labels
            .entry(receiver)
            .and_modify(|label| {
                if !label.contains('<') {
                    *label = format!("{}<{}>", label, class_name);
                }
            })
            .or_insert_with(|| format!("{}.instance(guest)", class_name));
    }

    fn objc_score_delegate_class_candidate(&self, class_ptr: u32, info: &ObjcClassInfo) -> (u32, u32) {
        let selectors = [
            ("application:didFinishLaunchingWithOptions:", 8u32),
            ("applicationDidFinishLaunching:", 7u32),
            ("applicationDidBecomeActive:", 4u32),
            ("connectionDidFinishLoading:", 3u32),
            ("connection:didReceiveData:", 3u32),
            ("connection:didReceiveResponse:", 3u32),
            ("reachabilityChanged:", 2u32),
        ];
        let mut score = 0u32;
        let mut hits = 0u32;
        for (selector, weight) in selectors {
            if self.objc_lookup_imp_in_class_chain(class_ptr, selector, false).is_some() {
                score = score.saturating_add(weight);
                hits = hits.saturating_add(1);
                if info.methods.contains_key(selector) {
                    score = score.saturating_add(1);
                }
            }
        }
        let lower_name = info.name.to_ascii_lowercase();
        if lower_name.contains("delegate") {
            score = score.saturating_add(2);
        }
        if lower_name.contains("app") {
            score = score.saturating_add(1);
        }
        (score, hits)
    }

    fn objc_infer_delegate_class_from_callbacks(&mut self) -> Option<(u32, String, u32)> {
        self.ensure_objc_metadata_indexed();
        let mut best: Option<(u32, String, u32, u32)> = None;
        for (&class_ptr, info) in &self.runtime.objc.objc_classes_by_ptr {
            let (score, hits) = self.objc_score_delegate_class_candidate(class_ptr, info);
            if hits < 2 || score < 8 {
                continue;
            }
            let replace = match &best {
                None => true,
                Some((_, best_name, best_score, best_hits)) => {
                    score > *best_score
                        || (score == *best_score && hits > *best_hits)
                        || (score == *best_score && hits == *best_hits && info.name.as_str() < best_name.as_str())
                }
            };
            if replace {
                best = Some((class_ptr, info.name.clone(), score, hits));
            }
        }
        if let Some((class_ptr, name, _score, hits)) = &best {
            self.runtime.objc.objc_bridge_inferred_class_name = Some(name.clone());
            self.runtime.objc.objc_bridge_inferred_selector_hits = *hits;
            return Some((*class_ptr, name.clone(), *hits));
        }
        self.runtime.objc.objc_bridge_inferred_class_name = None;
        self.runtime.objc.objc_bridge_inferred_selector_hits = 0;
        None
    }

    fn maybe_objc_string_name(&self, value: u32) -> Option<String> {
        let text = self.guest_string_value(value)?;
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.len() > 192 {
            return None;
        }
        let mut chars = trimmed.chars();
        let first = chars.next()?;
        if !(first.is_ascii_alphabetic() || first == '_') {
            return None;
        }
        if !trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return None;
        }
        Some(trimmed.to_string())
    }

    fn prepare_real_uimain_bridge(&mut self, principal_raw: u32, delegate_raw: u32) -> Option<UimainBridgePlan> {
        self.runtime.objc.objc_bridge_attempted = true;
        self.runtime.objc.objc_bridge_succeeded = false;
        self.runtime.objc.objc_bridge_failure_reason = None;
        self.runtime.objc.objc_bridge_inferred_class_name = None;
        self.runtime.objc.objc_bridge_inferred_selector_hits = 0;
        self.ensure_objc_metadata_indexed();
        let principal_name = self.maybe_objc_string_name(principal_raw);
        let delegate_name = self.maybe_objc_string_name(delegate_raw);
        self.runtime.objc.objc_bridge_delegate_name = delegate_name.clone().or_else(|| principal_name.clone());

        let mut receiver = 0u32;
        let mut delegate_class_name: Option<String> = None;

        if delegate_raw != 0 {
            if self.runtime.objc.objc_classes_by_ptr.contains_key(&delegate_raw) {
                delegate_class_name = self.objc_class_name_for_ptr(delegate_raw);
                if let Some(class_name) = delegate_class_name.clone() {
                    receiver = self.objc_materialize_instance(delegate_raw, &class_name)?;
                }
            } else if let Some(class_name) = self.objc_class_name_for_receiver(delegate_raw) {
                receiver = delegate_raw;
                delegate_class_name = Some(class_name);
            }
        }

        if receiver == 0 {
            let candidate_names = [delegate_name.clone(), principal_name.clone()];
            let mut saw_candidate_name = false;
            for maybe_name in candidate_names {
                let Some(name) = maybe_name else { continue; };
                saw_candidate_name = true;
                let Some(class_ptr) = self.objc_lookup_class_by_name(&name) else { continue; };
                if delegate_raw != 0 {
                    self.objc_attach_receiver_class(delegate_raw, class_ptr, &name);
                    receiver = delegate_raw;
                } else {
                    let Some(instance) = self.objc_materialize_instance(class_ptr, &name) else {
                        self.runtime.objc.objc_bridge_failure_reason = Some("delegate-instance-materialization-failed".to_string());
                        return None;
                    };
                    receiver = instance;
                }
                delegate_class_name = Some(name);
                break;
            }
            if receiver == 0 {
                if let Some((class_ptr, inferred_name, _hits)) = self.objc_infer_delegate_class_from_callbacks() {
                    if delegate_raw != 0 {
                        self.objc_attach_receiver_class(delegate_raw, class_ptr, &inferred_name);
                        receiver = delegate_raw;
                    } else {
                        let Some(instance) = self.objc_materialize_instance(class_ptr, &inferred_name) else {
                            self.runtime.objc.objc_bridge_failure_reason = Some("delegate-instance-materialization-failed".to_string());
                            return None;
                        };
                        receiver = instance;
                    }
                    delegate_class_name = Some(inferred_name.clone());
                    if self.runtime.objc.objc_bridge_delegate_name.is_none() {
                        self.runtime.objc.objc_bridge_delegate_name = Some(inferred_name);
                    }
                }
            }
            if receiver == 0 {
                self.runtime.objc.objc_bridge_failure_reason = Some(if saw_candidate_name {
                    "delegate-class-not-found".to_string()
                } else {
                    "delegate-name-unresolved".to_string()
                });
            }
        }

        let Some(delegate_class_name) = delegate_class_name else {
            if self.runtime.objc.objc_bridge_failure_reason.is_none() {
                self.runtime.objc.objc_bridge_failure_reason = Some("delegate-class-unresolved".to_string());
            }
            return None;
        };
        self.runtime.objc.objc_bridge_delegate_class_name = Some(delegate_class_name.clone());
        let Some(launch_selector) = [
            "application:didFinishLaunchingWithOptions:",
            "applicationDidFinishLaunching:",
        ]
        .into_iter()
        .find_map(|selector| {
            self.objc_lookup_imp_for_receiver(receiver, selector)
                .map(|imp| (selector.to_string(), imp))
        }) else {
            self.runtime.objc.objc_bridge_failure_reason = Some("launch-selector-not-found".to_string());
            return None;
        };
        let Some(selector_ptr) = self.alloc_selector_c_string(&launch_selector.0).ok() else {
            self.runtime.objc.objc_bridge_failure_reason = Some("selector-pool-exhausted".to_string());
            return None;
        };
        let return_stub = if (launch_selector.1 & 1) != 0 {
            HLE_STUB_UIAPPLICATION_POST_LAUNCH_THUMB | 1
        } else {
            HLE_STUB_UIAPPLICATION_POST_LAUNCH_ARM
        };
        self.runtime.objc.objc_bridge_launch_selector = Some(launch_selector.0.clone());
        self.runtime.objc.objc_bridge_launch_imp = Some(launch_selector.1);
        self.runtime.objc.objc_bridge_succeeded = true;
        self.runtime.objc.objc_bridge_failure_reason = None;
        Some(UimainBridgePlan {
            receiver,
            selector_name: launch_selector.0,
            selector_ptr,
            imp: launch_selector.1,
            return_stub,
            delegate_name,
            delegate_class_name,
        })
    }

    fn finish_real_uimain_bridge(&mut self) {
        self.bootstrap_synthetic_runloop();
        self.diag.trace.push(format!(
            "     ↳ ui lifecycle delegate bridge completed selector={} imp=0x{:08x} result=0x{:08x}",
            self.runtime.objc.objc_bridge_launch_selector.clone().unwrap_or_else(|| "<unknown>".to_string()),
            self.runtime.objc.objc_bridge_launch_imp.unwrap_or(0),
            self.cpu.regs[0],
        ));
        self.diag.trace.push("     ↳ ui lifecycle applicationDidBecomeActive: => synthetic".to_string());
        let resume_lr = self.runtime.objc.objc_bridge_resume_lr.unwrap_or(0);
        self.cpu.regs[0] = 0;
        self.cpu.regs[14] = resume_lr;
        self.cpu.regs[15] = resume_lr & !1;
        self.cpu.thumb = (resume_lr & 1) != 0;
    }

    fn objc_receiver_class_name_hint(&self, receiver: u32) -> Option<String> {
        self.objc_class_name_for_receiver(receiver)
            .or_else(|| self.objc_class_name_for_ptr(receiver))
    }

    fn objc_receiver_inherits_named(&self, receiver: u32, target: &str) -> bool {
        let receiver_u = receiver & 0xFFFF_FFFF;
        if receiver_u == 0 || target.is_empty() {
            return false;
        }
        let mut current = if self.runtime.objc.objc_classes_by_ptr.contains_key(&receiver_u) {
            receiver_u
        } else if let Some(class_ptr) = self.runtime.objc.objc_instance_isa_overrides.get(&receiver_u).copied() {
            class_ptr
        } else {
            match self.read_u32_le(receiver_u) {
                Ok(isa) if self.runtime.objc.objc_classes_by_ptr.contains_key(&isa) => isa,
                _ => 0,
            }
        };
        let mut hops = 0usize;
        while current != 0 && hops < 64 {
            let Some(info) = self.runtime.objc.objc_classes_by_ptr.get(&current) else {
                break;
            };
            if info.name == target {
                return true;
            }
            if info.superclass == 0 || info.superclass == current {
                break;
            }
            current = info.superclass;
            hops += 1;
        }
        false
    }
}
