impl MemoryArm32Backend {
// Cocos scene-tree helpers, synthetic textures, menu layout, and auto-scene plumbing.

    fn synthetic_dictionary_key(&self, value: u32) -> String {
        self.guest_string_value(value)
            .map(|text| text.to_ascii_lowercase())
            .unwrap_or_else(|| format!("@0x{value:08x}"))
    }


    fn alloc_synthetic_dictionary(&mut self, label: impl Into<String>) -> u32 {
        let obj = self.alloc_synthetic_ui_object(label);
        self.runtime.graphics.synthetic_dictionaries.entry(obj).or_default();
        obj
    }

    fn ensure_synthetic_dictionary(&mut self, dictionary: u32) -> &mut SyntheticDictionary {
        self.runtime.graphics.synthetic_dictionaries.entry(dictionary).or_default()
    }

    fn synthetic_dictionary_get(&self, dictionary: u32, key: &str) -> u32 {
        self.runtime.graphics.synthetic_dictionaries
            .get(&dictionary)
            .and_then(|dict| dict.entries.get(key).copied())
            .unwrap_or(0)
    }

    fn synthetic_texture_dimensions(&self, object: u32) -> Option<(u32, u32)> {
        self.runtime.graphics.synthetic_textures
            .get(&object)
            .map(|tex| (tex.width, tex.height))
            .or_else(|| self.runtime.graphics.synthetic_images.get(&object).map(|img| (img.width, img.height)))
    }

    fn synthetic_texture_gl_name(&self, object: u32) -> u32 {
        self.runtime.graphics.synthetic_textures
            .get(&object)
            .map(|tex| tex.gl_name)
            .unwrap_or(object)
    }

    fn synthetic_texture_has_pma(&self, object: u32) -> bool {
        self.runtime.graphics.synthetic_textures
            .get(&object)
            .map(|tex| tex.has_premultiplied_alpha)
            .unwrap_or(true)
    }

    fn synthetic_texture_debug_key(&self, object: u32) -> Option<String> {
        self.runtime.graphics.synthetic_textures
            .get(&object)
            .and_then(|texture| {
                let key = if !texture.cache_key.is_empty() {
                    texture.cache_key.clone()
                } else if !texture.source_key.is_empty() {
                    texture.source_key.clone()
                } else {
                    texture.source_path.clone()
                };
                if key.is_empty() { None } else { Some(key) }
            })
    }

    fn synthetic_resolve_texture_like_object(&self, object: u32) -> u32 {
        if object == 0 {
            return 0;
        }
        if self.runtime.graphics.synthetic_textures.contains_key(&object) {
            return object;
        }
        if let Some(atlas) = self.runtime.graphics.synthetic_texture_atlases.get(&object) {
            if atlas.texture != 0 {
                return atlas.texture;
            }
        }
        if let Some(state) = self.runtime.graphics.synthetic_sprites.get(&object) {
            if state.texture != 0 {
                return state.texture;
            }
        }
        0
    }

    fn synthetic_node_default_animation_texture(&self, node: u32) -> u32 {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return 0;
        };
        let dict = state.animation_dictionary;
        if dict == 0 {
            return 0;
        }
        let Some(dictionary) = self.runtime.graphics.synthetic_dictionaries.get(&dict) else {
            return 0;
        };

        let mut candidates: Vec<(String, usize)> = Vec::new();
        if state.last_display_frame_key != 0 {
            let key = self.synthetic_dictionary_key(state.last_display_frame_key);
            if !key.is_empty() {
                candidates.push((key, state.last_display_frame_index as usize));
            }
        }
        for key in ["frame", "normal", "default", "idle", "up"] {
            candidates.push((key.to_string(), 0));
        }
        if dictionary.entries.len() == 1 {
            if let Some((key, _)) = dictionary.entries.iter().next() {
                candidates.push((key.clone(), 0));
            }
        }

        for (key, index) in candidates {
            let frames = self.synthetic_dictionary_get(dict, &key);
            if frames == 0 {
                continue;
            }
            let frame_count = self.synthetic_array_len(frames);
            if frame_count == 0 {
                continue;
            }
            let frame_index = index.min(frame_count.saturating_sub(1));
            let frame = self.synthetic_array_get(frames, frame_index);
            let resolved = self.synthetic_resolve_texture_like_object(frame);
            if resolved != 0 {
                return resolved;
            }
        }

        0
    }

    fn synthetic_node_effective_texture(&self, node: u32) -> u32 {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return 0;
        };
        if state.texture != 0 {
            return state.texture;
        }
        let animation_texture = self.synthetic_node_default_animation_texture(node);
        if animation_texture != 0 {
            return animation_texture;
        }
        let mut parent = state.parent;
        let mut depth = 0usize;
        while parent != 0 && depth < 16 {
            let Some(parent_state) = self.runtime.graphics.synthetic_sprites.get(&parent) else {
                break;
            };
            let parent_label = self.diag.object_labels.get(&parent).map(String::as_str).unwrap_or("");
            if parent_state.texture != 0
                && (parent_label.contains("SpriteSheet")
                    || parent_label.contains("BatchNode")
                    || parent_label.contains("TextureAtlas"))
            {
                return parent_state.texture;
            }
            parent = parent_state.parent;
            depth = depth.saturating_add(1);
        }
        0
    }

    fn synthetic_node_prefers_bottom_left_texture_rect(&self, node: u32) -> bool {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return false;
        };
        let mut current = state.parent;
        let mut depth = 0usize;
        while current != 0 && depth < 16 {
            let Some(parent_state) = self.runtime.graphics.synthetic_sprites.get(&current) else {
                break;
            };
            let parent_label = self.diag.object_labels.get(&current).map(String::as_str).unwrap_or("");
            if parent_label.contains("SpriteSheet")
                || parent_label.contains("BatchNode")
                || parent_label.contains("TextureAtlas")
            {
                return true;
            }
            current = parent_state.parent;
            depth = depth.saturating_add(1);
        }

        let effective_texture = self.synthetic_node_effective_texture(node);
        if effective_texture != 0 {
            if let Some(texture) = self.runtime.graphics.synthetic_textures.get(&effective_texture) {
                let key = format!(
                    "{}|{}|{}",
                    texture.cache_key,
                    texture.source_key,
                    texture.source_path,
                )
                .to_ascii_lowercase();
                if let Some(prefer_bottom_left) = self.active_profile().texture_rect_origin_preference(&key) {
                    return prefer_bottom_left;
                }
                if key.contains("atlas") || (key.contains("sprite") && key.contains("sheet")) {
                    return true;
                }
            }
        }

        state.texture_rect_explicit
    }

    fn ensure_cocos_texture_cache_object(&mut self) -> u32 {
        if self.runtime.graphics.cocos_texture_cache_object != 0 {
            return self.runtime.graphics.cocos_texture_cache_object;
        }
        let obj = self.alloc_synthetic_ui_object("CCTextureCache.synthetic#0".to_string());
        self.runtime.graphics.cocos_texture_cache_object = obj;
        obj
    }

    fn ensure_cdaudio_manager_object(&mut self) -> u32 {
        if self.runtime.graphics.cocos_audio_manager_object != 0 {
            let obj = self.runtime.graphics.cocos_audio_manager_object;
            if let Some(class_ptr) = self.objc_lookup_class_by_name("CDAudioManager") {
                self.objc_attach_receiver_class(obj, class_ptr, "CDAudioManager");
            }
            return obj;
        }
        let obj = if let Some(class_ptr) = self.objc_lookup_class_by_name("CDAudioManager") {
            self.ensure_objc_singleton_object(class_ptr, "CDAudioManager", "cocos-audio-manager")
        } else {
            self.alloc_synthetic_ui_object("CDAudioManager.instance(synth)".to_string())
        };
        self.runtime.graphics.cocos_audio_manager_object = obj;
        obj
    }

    fn ensure_cdsound_engine_object(&mut self) -> u32 {
        if self.runtime.graphics.cocos_sound_engine_object != 0 {
            let obj = self.runtime.graphics.cocos_sound_engine_object;
            if let Some(class_ptr) = self.objc_lookup_class_by_name("CDSoundEngine") {
                self.objc_attach_receiver_class(obj, class_ptr, "CDSoundEngine");
            }
            return obj;
        }
        let obj = if let Some(class_ptr) = self.objc_lookup_class_by_name("CDSoundEngine") {
            self.ensure_objc_singleton_object(class_ptr, "CDSoundEngine", "cocos-sound-engine")
        } else {
            self.alloc_synthetic_ui_object("CDSoundEngine.instance(synth)".to_string())
        };
        self.runtime.graphics.cocos_sound_engine_object = obj;
        obj
    }

    fn ensure_synthetic_sprite_state(&mut self, sprite: u32) -> &mut SyntheticSpriteState {
        self.runtime.graphics.synthetic_sprites.entry(sprite).or_default()
    }

    fn ensure_synthetic_texture_atlas_state(&mut self, atlas: u32) -> &mut SyntheticTextureAtlasState {
        self.runtime.graphics.synthetic_texture_atlases.entry(atlas).or_default()
    }

    fn synthetic_texture_atlas_texture(&self, atlas: u32) -> u32 {
        self.runtime.graphics.synthetic_texture_atlases
            .get(&atlas)
            .map(|state| state.texture)
            .filter(|texture| *texture != 0)
            .unwrap_or_else(|| {
                self.runtime.graphics.synthetic_sprites
                    .get(&atlas)
                    .map(|state| state.texture)
                    .unwrap_or(0)
            })
    }

    fn synthetic_texture_atlas_capacity(&self, atlas: u32) -> u32 {
        self.runtime.graphics.synthetic_texture_atlases
            .get(&atlas)
            .map(|state| state.capacity)
            .unwrap_or(0)
    }

    fn synthetic_texture_atlas_total_quads(&self, atlas: u32) -> u32 {
        self.runtime.graphics.synthetic_texture_atlases
            .get(&atlas)
            .map(|state| state.total_quads)
            .unwrap_or(0)
    }

    fn ensure_synthetic_texture_atlas_storage(&mut self, atlas: u32, requested_capacity: u32) -> u32 {
        let requested_capacity = requested_capacity.max(1);
        let quad_stride = 96u32;
        let current = self
            .runtime.graphics.synthetic_texture_atlases
            .get(&atlas)
            .cloned()
            .unwrap_or_default();
        if current.quad_buffer_ptr != 0 && current.capacity >= requested_capacity {
            return current.quad_buffer_ptr;
        }
        let requested_bytes = requested_capacity.saturating_mul(quad_stride).max(quad_stride);
        let new_ptr = self
            .alloc_synthetic_heap_block(requested_bytes, true, format!("TextureAtlas.quads(0x{atlas:08x})"))
            .unwrap_or(0);
        if new_ptr == 0 {
            return current.quad_buffer_ptr;
        }
        if current.quad_buffer_ptr != 0 {
            let copy_bytes = current
                .capacity
                .saturating_mul(current.quad_stride.max(quad_stride))
                .min(requested_bytes);
            if copy_bytes != 0 {
                if let Ok(bytes) = self.read_guest_bytes(current.quad_buffer_ptr, copy_bytes) {
                    let _ = self.write_bytes(new_ptr, &bytes);
                }
            }
        }
        let state = self.ensure_synthetic_texture_atlas_state(atlas);
        state.capacity = state.capacity.max(requested_capacity);
        state.quad_stride = quad_stride;
        state.quad_buffer_ptr = new_ptr;
        new_ptr
    }

    fn configure_synthetic_texture_atlas(
        &mut self,
        atlas: u32,
        texture: u32,
        capacity_hint: Option<u32>,
        selector: &str,
    ) -> String {
        let dims = self
            .synthetic_texture_dimensions(texture)
            .unwrap_or((
                self.runtime.ui_graphics.graphics_surface_width.max(1),
                self.runtime.ui_graphics.graphics_surface_height.max(1),
            ));
        let requested_capacity = capacity_hint.unwrap_or(1).max(1);
        let buffer_ptr = self.ensure_synthetic_texture_atlas_storage(atlas, requested_capacity);
        {
            let state = self.ensure_synthetic_texture_atlas_state(atlas);
            state.texture = texture;
            state.capacity = state.capacity.max(requested_capacity);
            if state.quad_stride == 0 {
                state.quad_stride = 96;
            }
            if state.quad_buffer_ptr == 0 {
                state.quad_buffer_ptr = buffer_ptr;
            }
        }
        self.diag.object_labels
            .entry(atlas)
            .or_insert_with(|| "TextureAtlas.instance(synth)".to_string());
        let (sprite_w, sprite_h) = {
            let sprite = self.ensure_synthetic_sprite_state(atlas);
            sprite.visible = true;
            sprite.texture = texture;
            if sprite.width == 0 {
                sprite.width = dims.0;
            }
            if sprite.height == 0 {
                sprite.height = dims.1;
            }
            (sprite.width, sprite.height)
        };
        let capacity = self.synthetic_texture_atlas_capacity(atlas);
        let total_quads = self.synthetic_texture_atlas_total_quads(atlas);
        let texture_desc = self.describe_ptr(texture);
        let buffer_desc = self.describe_ptr(buffer_ptr);
        format!(
            "cocos {} textureAtlas texture={} capacity={} totalQuads={} buffer={} dims={}x{}",
            selector,
            texture_desc,
            capacity,
            total_quads,
            buffer_desc,
            sprite_w,
            sprite_h,
        )
    }

    fn resize_synthetic_texture_atlas(&mut self, atlas: u32, requested_capacity: u32, selector: &str) -> String {
        let requested_capacity = requested_capacity.max(1);
        let old_capacity = self.synthetic_texture_atlas_capacity(atlas);
        let old_buffer = self
            .runtime.graphics.synthetic_texture_atlases
            .get(&atlas)
            .map(|state| state.quad_buffer_ptr)
            .unwrap_or(0);
        let new_buffer = self.ensure_synthetic_texture_atlas_storage(atlas, requested_capacity);
        let new_capacity = {
            let state = self.ensure_synthetic_texture_atlas_state(atlas);
            state.capacity = state.capacity.max(requested_capacity);
            state.capacity
        };
        let old_buffer_desc = self.describe_ptr(old_buffer);
        let new_buffer_desc = self.describe_ptr(new_buffer);
        format!(
            "cocos {} textureAtlas oldCapacity={} -> {} oldBuffer={} newBuffer={}",
            selector,
            old_capacity,
            new_capacity,
            old_buffer_desc,
            new_buffer_desc,
        )
    }

    fn update_synthetic_texture_atlas_quad(
        &mut self,
        atlas: u32,
        texture_quad_ptr: u32,
        vertex_quad_ptr: u32,
        index: u32,
        selector: &str,
    ) -> String {
        let old_capacity = self.synthetic_texture_atlas_capacity(atlas);
        let current_capacity = old_capacity.max(1);
        let requested_capacity = if index < current_capacity {
            current_capacity
        } else {
            current_capacity.max(index.saturating_add(1))
        };
        let buffer_ptr = self.ensure_synthetic_texture_atlas_storage(atlas, requested_capacity);
        let stride = self
            .runtime.graphics.synthetic_texture_atlases
            .get(&atlas)
            .map(|state| state.quad_stride.max(1))
            .unwrap_or(96);
        let dst = buffer_ptr.saturating_add(index.saturating_mul(stride));
        let mut copied_texture_bytes = 0u32;
        let mut copied_vertex_bytes = 0u32;
        if dst != 0 {
            if texture_quad_ptr != 0 {
                let wanted = 32u32.min(stride);
                if let Ok(bytes) = self.read_guest_bytes(texture_quad_ptr, wanted) {
                    let texture_dst = dst.saturating_add(stride.saturating_sub(wanted));
                    let _ = self.write_bytes(texture_dst, &bytes);
                    copied_texture_bytes = wanted;
                }
            }
            if vertex_quad_ptr != 0 {
                let wanted = 64u32.min(stride);
                if let Ok(bytes) = self.read_guest_bytes(vertex_quad_ptr, wanted) {
                    let _ = self.write_bytes(dst, &bytes);
                    copied_vertex_bytes = wanted;
                }
            }
        }
        let (capacity, total_quads, invalid_count, texture) = {
            let state = self.ensure_synthetic_texture_atlas_state(atlas);
            if index >= old_capacity.max(1) {
                state.invalid_update_count = state.invalid_update_count.saturating_add(1);
            }
            state.capacity = state.capacity.max(requested_capacity);
            state.total_quads = state.total_quads.max(index.saturating_add(1));
            state.last_index = Some(index);
            state.max_index_seen = Some(state.max_index_seen.map(|value| value.max(index)).unwrap_or(index));
            state.update_count = state.update_count.saturating_add(1);
            (state.capacity, state.total_quads, state.invalid_update_count, state.texture)
        };
        format!(
            "cocos {} textureAtlas index={} capacity={} totalQuads={} texture={} buffer={} copiedTex={} copiedVertex={} invalidUpdates={}",
            selector,
            index,
            capacity,
            total_quads,
            self.describe_ptr(texture),
            self.describe_ptr(buffer_ptr),
            copied_texture_bytes,
            copied_vertex_bytes,
            invalid_count,
        )
    }

    fn reset_synthetic_texture_atlas_quads(&mut self, atlas: u32, selector: &str) -> String {
        let (buffer_ptr, capacity, stride) = self
            .runtime.graphics.synthetic_texture_atlases
            .get(&atlas)
            .map(|state| (state.quad_buffer_ptr, state.capacity, state.quad_stride.max(1)))
            .unwrap_or((0, 0, 96));
        if buffer_ptr != 0 && capacity != 0 {
            let zeroes = vec![0u8; capacity.saturating_mul(stride) as usize];
            let _ = self.write_bytes(buffer_ptr, &zeroes);
        }
        {
            let state = self.ensure_synthetic_texture_atlas_state(atlas);
            state.total_quads = 0;
            state.last_index = None;
        }
        let buffer_desc = self.describe_ptr(buffer_ptr);
        format!(
            "cocos {} textureAtlas reset buffer={} capacity={}",
            selector,
            buffer_desc,
            capacity,
        )
    }

    fn alloc_synthetic_array(&mut self, label: impl Into<String>) -> u32 {
        let obj = self.alloc_synthetic_ui_object(label);
        self.runtime.graphics.synthetic_arrays.entry(obj).or_default();
        obj
    }

    fn ensure_synthetic_array(&mut self, array: u32) -> &mut SyntheticArray {
        self.runtime.graphics.synthetic_arrays.entry(array).or_default()
    }

    fn synthetic_array_len(&self, array: u32) -> usize {
        self.runtime.graphics.synthetic_arrays.get(&array).map(|v| v.items.len()).unwrap_or(0)
    }

    fn synthetic_array_get(&self, array: u32, index: usize) -> u32 {
        self.runtime.graphics.synthetic_arrays
            .get(&array)
            .and_then(|v| v.items.get(index).copied())
            .unwrap_or(0)
    }

    fn synthetic_array_append_unique(&mut self, array: u32, value: u32) -> usize {
        let entry = self.ensure_synthetic_array(array);
        if let Some(idx) = entry.items.iter().position(|&item| item == value) {
            idx
        } else {
            entry.items.push(value);
            entry.mutation_count = entry.mutation_count.saturating_add(1);
            entry.items.len().saturating_sub(1)
        }
    }

    fn synthetic_array_push(&mut self, array: u32, value: u32) -> usize {
        let entry = self.ensure_synthetic_array(array);
        entry.items.push(value);
        entry.mutation_count = entry.mutation_count.saturating_add(1);
        entry.items.len().saturating_sub(1)
    }

    fn synthetic_array_insert_or_move(&mut self, array: u32, index: usize, value: u32) -> usize {
        let entry = self.ensure_synthetic_array(array);
        if let Some(existing) = entry.items.iter().position(|&item| item == value) {
            entry.items.remove(existing);
        }
        let insert_at = index.min(entry.items.len());
        entry.items.insert(insert_at, value);
        entry.mutation_count = entry.mutation_count.saturating_add(1);
        insert_at
    }

    fn synthetic_array_remove_value(&mut self, array: u32, value: u32) -> bool {
        let entry = self.ensure_synthetic_array(array);
        if let Some(idx) = entry.items.iter().position(|&item| item == value) {
            entry.items.remove(idx);
            entry.mutation_count = entry.mutation_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    fn synthetic_array_position(&self, array: u32, value: u32) -> Option<usize> {
        self.runtime.graphics.synthetic_arrays
            .get(&array)
            .and_then(|entry| entry.items.iter().position(|&item| item == value))
    }

    fn synthetic_parent_contains_child(&self, parent: u32, child: u32) -> bool {
        if parent == 0 || child == 0 {
            return false;
        }
        let children = self.runtime.graphics.synthetic_sprites.get(&parent).map(|state| state.children).unwrap_or(0);
        children != 0 && self.synthetic_array_position(children, child).is_some()
    }

    fn synthetic_child_insert_index_for_z(&self, parent: u32, child: u32, z_order: i32) -> usize {
        let children = self.runtime.graphics.synthetic_sprites.get(&parent).map(|state| state.children).unwrap_or(0);
        if children == 0 {
            return 0;
        }
        let items = self.runtime.graphics.synthetic_arrays.get(&children).map(|entry| entry.items.clone()).unwrap_or_default();
        if items.is_empty() {
            return 0;
        }
        let existing_index = items.iter().position(|&item| item == child);
        let mut insert_at = items.len();
        for (idx, sibling) in items.iter().copied().enumerate() {
            if sibling == 0 || sibling == child {
                continue;
            }
            let sibling_z = self.runtime.graphics.synthetic_sprites.get(&sibling).map(|state| state.z_order).unwrap_or(0);
            if z_order < sibling_z {
                insert_at = idx;
                break;
            }
        }
        if let Some(existing) = existing_index {
            if existing < insert_at {
                insert_at = insert_at.saturating_sub(1);
            }
        }
        insert_at
    }

    fn ensure_node_children_array(&mut self, node: u32) -> u32 {
        let existing = self.runtime.graphics.synthetic_sprites.get(&node).map(|state| state.children).unwrap_or(0);
        if existing != 0 {
            self.runtime.graphics.synthetic_arrays.entry(existing).or_default();
            return existing;
        }
        let label = self
            .diag.object_labels
            .get(&node)
            .cloned()
            .map(|name| format!("{}.children", name))
            .unwrap_or_else(|| format!("CCNode.children@0x{node:08x}"));
        let array = self.alloc_synthetic_array(label);
        self.ensure_synthetic_sprite_state(node).children = array;
        array
    }

    fn scene_graph_frame_token(&self) -> u64 {
        let scheduler_epoch = (self.runtime.ui_cocos.scheduler_mainloop_calls as u64)
            .wrapping_add((self.runtime.ui_cocos.scheduler_draw_scene_calls as u64) << 1)
            .wrapping_add((self.runtime.ui_cocos.scheduler_draw_frame_calls as u64) << 2)
            .wrapping_add((self.runtime.ui_cocos.scheduler_update_calls as u64) << 3)
            .wrapping_add((self.runtime.ui_cocos.scheduler_render_callback_calls as u64) << 4);
        (scheduler_epoch << 32)
            ^ ((self.runtime.ui_graphics.graphics_present_calls as u64) << 16)
            ^ (self.runtime.ui_graphics.graphics_frame_index as u64)
    }

    fn refresh_scene_graph_reentry_guards(&mut self) {
        let token = self.scene_graph_frame_token();
        if self.runtime.scene.traversal_guard_frame_token == token {
            return;
        }
        self.runtime.scene.traversal_guard_frame_token = token;
        self.runtime.scene.traversal_first_cycle_frame_token = 0;
        self.runtime.scene.traversal_first_cycle_message = None;
        self.runtime.scene.traversal_invalidate_nodes.clear();
        self.runtime.scene.traversal_adopt_nodes.clear();
    }

    fn refresh_host_dispatch_guards(&mut self) {
        let token = self.scene_graph_frame_token();
        if self.runtime.scene.selector_dispatch_guard_frame_token != token {
            self.runtime.scene.selector_dispatch_guard_frame_token = token;
            self.runtime.scene.selector_dispatch_depth = 0;
            self.runtime.scene.selector_dispatch_max_depth = 0;
            self.runtime.scene.selector_dispatch_stack.clear();
        }
        if self.runtime.scene.lifecycle_dispatch_guard_frame_token != token {
            self.runtime.scene.lifecycle_dispatch_guard_frame_token = token;
            self.runtime.scene.lifecycle_dispatch_depth = 0;
            self.runtime.scene.lifecycle_dispatch_max_depth = 0;
            self.runtime.scene.lifecycle_dispatch_stack.clear();
        }
    }

    fn push_crash_safe_console_line(&self, line: &str) {
        eprintln!("{}", line);
        let mut stderr = std::io::stderr();
        let _ = std::io::Write::flush(&mut stderr);
    }

    fn push_graph_trace_critical(&mut self, event: impl Into<String>) {
        let event = event.into();
        self.push_graph_trace(event.clone());
        self.diag.trace.push(format!("     ↳ {}", event));
        self.push_crash_safe_console_line(&format!("[mkea-critical] {}", event));
    }

    fn push_callback_trace_critical(&mut self, event: impl Into<String>) {
        let event = event.into();
        self.push_callback_trace(event.clone());
        self.diag.trace.push(format!("     ↳ {}", event));
        self.push_crash_safe_console_line(&format!("[mkea-critical] {}", event));
    }

    fn begin_selector_dispatch_guard(&mut self, receiver: u32, selector_name: &str, origin: &str) -> bool {
        const SELECTOR_DEPTH_LIMIT: u32 = 96;
        const SELECTOR_REENTRY_LIMIT: u32 = 8;
        self.refresh_host_dispatch_guards();
        let token = self.runtime.scene.selector_dispatch_guard_frame_token;
        let key = format!("{} {}", self.describe_ptr(receiver), selector_name);
        let repeats = self.runtime.scene.selector_dispatch_stack
            .iter()
            .filter(|entry| entry.as_str() == key.as_str())
            .count() as u32
            + 1;
        self.runtime.scene.selector_dispatch_depth = self.runtime.scene.selector_dispatch_depth.saturating_add(1);
        self.runtime.scene.selector_dispatch_max_depth = self.runtime.scene.selector_dispatch_max_depth.max(self.runtime.scene.selector_dispatch_depth);
        self.runtime.scene.selector_dispatch_stack.push(key.clone());
        let depth = self.runtime.scene.selector_dispatch_depth;
        let stack_snapshot = self.runtime.scene.selector_dispatch_stack.join(" => ");
        let mut blocked = false;
        if depth > SELECTOR_DEPTH_LIMIT {
            self.push_callback_trace_critical(format!(
                "dispatch.depth kind=selector frameToken={} depth={} limit={} receiver={} selector={} origin={} stack={}",
                token,
                depth,
                SELECTOR_DEPTH_LIMIT,
                self.describe_ptr(receiver),
                selector_name,
                origin,
                stack_snapshot,
            ));
            blocked = true;
        }
        if repeats > SELECTOR_REENTRY_LIMIT {
            self.push_callback_trace_critical(format!(
                "dispatch.reentry kind=selector frameToken={} repeats={} limit={} receiver={} selector={} origin={} stack={}",
                token,
                repeats,
                SELECTOR_REENTRY_LIMIT,
                self.describe_ptr(receiver),
                selector_name,
                origin,
                stack_snapshot,
            ));
            blocked = true;
        }
        blocked
    }

    fn end_selector_dispatch_guard(&mut self) {
        if self.runtime.scene.selector_dispatch_depth > 0 {
            self.runtime.scene.selector_dispatch_depth -= 1;
        }
        let _ = self.runtime.scene.selector_dispatch_stack.pop();
    }

    fn begin_lifecycle_dispatch_guard(&mut self, scene: u32, selector_name: &str, origin: &str) -> bool {
        const LIFECYCLE_DEPTH_LIMIT: u32 = 48;
        const LIFECYCLE_REENTRY_LIMIT: u32 = 6;
        self.refresh_host_dispatch_guards();
        let token = self.runtime.scene.lifecycle_dispatch_guard_frame_token;
        let key = format!("{} {}", self.describe_ptr(scene), selector_name);
        let repeats = self.runtime.scene.lifecycle_dispatch_stack
            .iter()
            .filter(|entry| entry.as_str() == key.as_str())
            .count() as u32
            + 1;
        self.runtime.scene.lifecycle_dispatch_depth = self.runtime.scene.lifecycle_dispatch_depth.saturating_add(1);
        self.runtime.scene.lifecycle_dispatch_max_depth = self.runtime.scene.lifecycle_dispatch_max_depth.max(self.runtime.scene.lifecycle_dispatch_depth);
        self.runtime.scene.lifecycle_dispatch_stack.push(key.clone());
        let depth = self.runtime.scene.lifecycle_dispatch_depth;
        let stack_snapshot = self.runtime.scene.lifecycle_dispatch_stack.join(" => ");
        let mut blocked = false;
        if depth > LIFECYCLE_DEPTH_LIMIT {
            self.push_callback_trace_critical(format!(
                "dispatch.depth kind=lifecycle frameToken={} depth={} limit={} scene={} selector={} origin={} stack={}",
                token,
                depth,
                LIFECYCLE_DEPTH_LIMIT,
                self.describe_ptr(scene),
                selector_name,
                origin,
                stack_snapshot,
            ));
            blocked = true;
        }
        if repeats > LIFECYCLE_REENTRY_LIMIT {
            self.push_callback_trace_critical(format!(
                "dispatch.reentry kind=lifecycle frameToken={} repeats={} limit={} scene={} selector={} origin={} stack={}",
                token,
                repeats,
                LIFECYCLE_REENTRY_LIMIT,
                self.describe_ptr(scene),
                selector_name,
                origin,
                stack_snapshot,
            ));
            blocked = true;
        }
        blocked
    }

    fn end_lifecycle_dispatch_guard(&mut self) {
        if self.runtime.scene.lifecycle_dispatch_depth > 0 {
            self.runtime.scene.lifecycle_dispatch_depth -= 1;
        }
        let _ = self.runtime.scene.lifecycle_dispatch_stack.pop();
    }

    fn format_scene_graph_path(&self, path: &[u32]) -> String {
        if path.is_empty() {
            return "[]".to_string();
        }
        path.iter()
            .map(|node| self.describe_ptr(*node))
            .collect::<Vec<_>>()
            .join(" -> ")
    }

    fn record_scene_graph_cycle(&mut self, walk: &str, path: &[u32], repeated: u32, depth: u32) {
        self.refresh_scene_graph_reentry_guards();
        let token = self.runtime.scene.traversal_guard_frame_token;
        let mut loop_nodes = path.to_vec();
        if loop_nodes.last().copied() != Some(repeated) {
            loop_nodes.push(repeated);
        }
        let message = format!(
            "traversal.cycle walk={} frameToken={} depth={} loop={}",
            walk,
            token,
            depth,
            self.format_scene_graph_path(&loop_nodes),
        );
        let should_emit = self.runtime.scene.traversal_first_cycle_frame_token != token
            || self.runtime.scene.traversal_first_cycle_message.is_none();
        self.runtime.scene.traversal_first_cycle_frame_token = token;
        self.runtime.scene.traversal_first_cycle_message = Some(message.clone());
        if should_emit {
            self.push_graph_trace_critical(message);
        }
    }

    fn should_skip_scene_graph_reentry(&mut self, guard: &str, node: u32) -> bool {
        if node == 0 {
            return false;
        }
        self.refresh_scene_graph_reentry_guards();
        let token = self.runtime.scene.traversal_guard_frame_token;
        let already_seen = match guard {
            "invalidate" => !self.runtime.scene.traversal_invalidate_nodes.insert(node),
            "adopt" => !self.runtime.scene.traversal_adopt_nodes.insert(node),
            _ => false,
        };
        if already_seen {
            self.push_graph_trace(format!(
                "traversal.reentry guard={} frameToken={} node={} action=skip",
                guard,
                token,
                self.describe_ptr(node),
            ));
        }
        already_seen
    }

    fn would_create_synthetic_cycle(&self, parent: u32, child: u32) -> Option<Vec<u32>> {
        if parent == 0 || child == 0 {
            return None;
        }
        if parent == child {
            return Some(vec![child, child]);
        }
        let mut chain = Vec::new();
        let mut visited = HashSet::new();
        let mut current = parent;
        for _ in 0..256 {
            if current == 0 {
                break;
            }
            if let Some(pos) = chain.iter().position(|value| *value == current) {
                let mut loop_nodes = chain[pos..].to_vec();
                loop_nodes.push(current);
                return Some(loop_nodes);
            }
            if !visited.insert(current) {
                break;
            }
            chain.push(current);
            if current == child {
                let mut loop_nodes = chain.clone();
                loop_nodes.reverse();
                loop_nodes.push(child);
                return Some(loop_nodes);
            }
            current = self.runtime.graphics.synthetic_sprites.get(&current).map(|state| state.parent).unwrap_or(0);
        }
        None
    }

    fn guest_array_count(&mut self, array: u32, origin: &str) -> Option<usize> {
        if array == 0 {
            return Some(0);
        }
        let result = self.invoke_objc_selector_now_capture_r0(array, "count", 0, 0, 60_000, origin)?;
        Some((result as usize).min(256))
    }

    fn guest_array_object_at_index(&mut self, array: u32, index: usize, origin: &str) -> Option<u32> {
        if array == 0 {
            return None;
        }
        let idx = u32::try_from(index).ok()?;
        self.invoke_objc_selector_now_capture_r0(array, "objectAtIndex:", idx, 0, 60_000, origin)
            .filter(|value| *value != 0)
    }

    fn guest_cocos_node_parent(&mut self, node: u32, origin: &str) -> Option<u32> {
        if node == 0 {
            return Some(0);
        }
        Some(self.invoke_objc_selector_now_capture_r0(node, "parent", 0, 0, 60_000, origin).unwrap_or(0))
    }

    fn guest_cocos_node_z_order(&mut self, node: u32, origin: &str) -> Option<i32> {
        if node == 0 {
            return Some(0);
        }
        let raw = self.invoke_objc_selector_now_capture_r0(node, "zOrder", 0, 0, 60_000, origin)?;
        Some(raw as i32)
    }

    fn guest_cocos_node_children(&mut self, node: u32, origin: &str) -> Option<Vec<u32>> {
        if node == 0 {
            return Some(Vec::new());
        }
        let array = self.invoke_objc_selector_now_capture_r0(node, "children", 0, 0, 80_000, origin).unwrap_or(0);
        if array == 0 {
            self.push_graph_trace(format!(
                "guest.graph.children-unavailable node={} origin={} synthChildren={}",
                self.describe_ptr(node),
                origin,
                self.runtime
                    .graphics
                    .synthetic_sprites
                    .get(&node)
                    .map(|state| if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 })
                    .unwrap_or(0),
            ));
            return None;
        }
        let count = self.guest_array_count(array, origin)?;
        let mut out = Vec::with_capacity(count.min(64));
        for index in 0..count.min(128) {
            if let Some(child) = self.guest_array_object_at_index(array, index, origin) {
                out.push(child);
            }
        }
        Some(out)
    }

    fn reconcile_synthetic_node_graph_from_guest(&mut self, root: u32, origin: &str) -> usize {
        if root == 0 {
            return 0;
        }
        let mut attached = 0usize;
        let mut seen = HashSet::new();
        let mut queue = std::collections::VecDeque::from([root]);
        let mut scanned = 0usize;
        while let Some(node) = queue.pop_front() {
            if node == 0 || !seen.insert(node) {
                continue;
            }
            scanned = scanned.saturating_add(1);
            if scanned > 128 {
                break;
            }
            self.ensure_synthetic_sprite_state(node).guest_graph_observed = true;
            let event_origin = format!("guest-graph:{}", origin);
            let children = match self.guest_cocos_node_children(node, &event_origin) {
                Some(children) => children,
                None => continue,
            };
            let guest_child_set: HashSet<u32> = children.iter().copied().collect();
            let synth_children = self.runtime.graphics.synthetic_sprites.get(&node).map(|state| state.children).unwrap_or(0);
            if synth_children != 0 {
                let existing = self.runtime.graphics.synthetic_arrays.get(&synth_children).map(|entry| entry.items.clone()).unwrap_or_default();
                for existing_child in existing {
                    if existing_child == 0 || guest_child_set.contains(&existing_child) {
                        continue;
                    }
                    let guest_observed = self.runtime.graphics.synthetic_sprites
                        .get(&existing_child)
                        .map(|state| state.guest_graph_observed)
                        .unwrap_or(false);
                    if !guest_observed {
                        continue;
                    }
                    let note = self.remove_child_from_node(node, existing_child, false);
                    attached = attached.saturating_add(1);
                    self.push_graph_trace(format!(
                        "guest.graph.prune parent={} child={} origin={} note={}",
                        self.describe_ptr(node),
                        self.describe_ptr(existing_child),
                        origin,
                        note,
                    ));
                }
            }
            let guest_parent = self.guest_cocos_node_parent(node, &event_origin).unwrap_or(0);
            self.push_graph_trace(format!(
                "guest.graph.scan node={} guestParent={} guestChildren={} synthParent={} synthChildren={} origin={}",
                self.describe_ptr(node),
                self.describe_ptr(guest_parent),
                children.len(),
                self.describe_ptr(self.runtime.graphics.synthetic_sprites.get(&node).map(|state| state.parent).unwrap_or(0)),
                self.runtime.graphics.synthetic_sprites.get(&node).map(|state| if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 }).unwrap_or(0),
                origin,
            ));
            for (index, child) in children.into_iter().enumerate() {
                if child == 0 {
                    continue;
                }
                let child_origin = format!("guest-graph-child:{}:{}", origin, index);
                self.ensure_synthetic_sprite_state(child).guest_graph_observed = true;
                let z_order = self.guest_cocos_node_z_order(child, &child_origin).unwrap_or(index as i32);
                let synth_parent = self.runtime.graphics.synthetic_sprites.get(&child).map(|state| state.parent).unwrap_or(0);
                let already_linked = synth_parent == node && self.synthetic_parent_contains_child(node, child);
                if !already_linked {
                    let note = self.attach_child_to_node(node, child, z_order, None, &child_origin);
                    attached = attached.saturating_add(1);
                    self.push_graph_trace(format!(
                        "guest.graph.attach parent={} child={} z={} origin={} note={}",
                        self.describe_ptr(node),
                        self.describe_ptr(child),
                        z_order,
                        origin,
                        note,
                    ));
                } else {
                    let children_array = self.ensure_node_children_array(node);
                    let insert_at = self.synthetic_child_insert_index_for_z(node, child, z_order);
                    let _ = self.synthetic_array_insert_or_move(children_array, insert_at, child);
                    let child_state = self.ensure_synthetic_sprite_state(child);
                    child_state.parent = node;
                    child_state.z_order = z_order;
                }
                if seen.len() < 256 {
                    queue.push_back(child);
                }
            }
        }
        if attached != 0 {
            self.push_graph_trace(format!(
                "guest.graph.reconciled root={} attached={} origin={} state=[{}]",
                self.describe_ptr(root),
                attached,
                origin,
                self.describe_node_graph_state(root),
            ));
        }
        attached
    }

    fn maybe_reconcile_guest_scene_graph(&mut self, scene: u32, origin: &str, age: u32) -> usize {
        if scene == 0 {
            return 0;
        }
        let child_count = self.runtime.graphics.synthetic_sprites
            .get(&scene)
            .map(|state| if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 })
            .unwrap_or(0);
        let total_nodes = self.runtime.graphics.synthetic_sprites.len();
        let should_probe = child_count == 0
            || (total_nodes > 1 && matches!(age, 1 | 2 | 4 | 8))
            || (child_count <= 1 && total_nodes > 4 && matches!(age, 3 | 6 | 12));
        if !should_probe {
            return 0;
        }
        self.reconcile_synthetic_node_graph_from_guest(scene, origin)
    }

    fn invalidate_synthetic_widget_content(&mut self, node: u32, reason: &str) -> u32 {
        if node != 0 && self.should_skip_scene_graph_reentry("invalidate", node) {
            let revision = self.runtime.graphics.synthetic_sprites.get(&node).map(|state| state.content_revision).unwrap_or(0);
            self.push_graph_trace(format!(
                "widget.invalidate node={} reason={} revision={} coalesced=YES",
                self.describe_ptr(node),
                reason,
                revision,
            ));
            return revision;
        }
        let revision = if node != 0 {
            let state = self.ensure_synthetic_sprite_state(node);
            state.content_revision = state.content_revision.saturating_add(1);
            state.content_revision
        } else {
            0
        };
        self.runtime.scene.auto_scene_last_present_signature = None;
        self.runtime.graphics.guest_framebuffer_dirty = true;
        self.push_graph_trace(format!(
            "widget.invalidate node={} reason={} revision={} coalesced=NO",
            self.describe_ptr(node),
            reason,
            revision,
        ));
        revision
    }

    fn is_host_synthetic_cocos_node(&self, node: u32) -> bool {
        if node == 0 || !self.runtime.graphics.synthetic_sprites.contains_key(&node) {
            return false;
        }
        // Synthetic cocos nodes are host-authored objects tracked in the synthetic sprite map.
        // Some of them still sit inside mapped guest pages (for example after synthetic adoption
        // or when a shim instance reuses a guest-looking address), so "find_region == none" is
        // not a reliable discriminator. Prefer the authoritative synthetic label/state first and
        // keep the no-region test only as a fallback.
        if let Some(label) = self.diag.object_labels.get(&node) {
            if label.contains("instance(synth)") || label.contains(".synthetic") || label.contains("(synth)") {
                return true;
            }
        }
        self.find_region(node, 4).is_none()
    }

    fn should_skip_real_imp_for_synthetic_cocos_selector(&self, receiver: u32, selector: &str) -> bool {
        self.is_host_synthetic_cocos_node(receiver)
            && matches!(
                selector,
                "addChild:"
                    | "addChild:z:"
                    | "insertChild:z:"
                    | "addChild:z:tag:"
                    | "setParent:"
                    | "removeChild:cleanup:"
            )
    }

    fn maybe_invoke_synthetic_cocos_lifecycle_selector(
        &mut self,
        receiver: u32,
        selector: &str,
        origin: &str,
    ) -> Option<u32> {
        if !self.is_host_synthetic_cocos_node(receiver) {
            return None;
        }
        let result = match selector {
            "onEnter" => {
                self.ensure_synthetic_sprite_state(receiver).entered = true;
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, format!(
                    "cocos lifecycle synthetic onEnter state=[{}] revision={} origin={}",
                    self.describe_node_graph_state(receiver),
                    revision,
                    origin,
                )))
            }
            "onExit" => {
                self.ensure_synthetic_sprite_state(receiver).entered = false;
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, format!(
                    "cocos lifecycle synthetic onExit state=[{}] revision={} origin={}",
                    self.describe_node_graph_state(receiver),
                    revision,
                    origin,
                )))
            }
            "onEnterTransitionDidFinish" => Some((receiver, format!(
                "cocos lifecycle synthetic onEnterTransitionDidFinish state=[{}] origin={}",
                self.describe_node_graph_state(receiver),
                origin,
            ))),
            _ => None,
        };
        if let Some((result, note)) = result {
            self.diag.trace.push(format!(
                "     ↳ synthetic selector invoke repaired receiver={} class={} selector={} arg2=nil arg3=nil origin={} strategy=cocos-lifecycle-synthetic result={} note={}",
                self.describe_ptr(receiver),
                self.objc_class_name_for_receiver(receiver).unwrap_or_else(|| "<unknown-class>".to_string()),
                selector,
                origin,
                self.describe_ptr(result),
                note,
            ));
            return Some(result);
        }
        None
    }

    fn maybe_adopt_guest_cocos_focus(&mut self, node: u32, origin: &str) -> usize {
        if node == 0 || !self.is_plausible_guest_cocos_object_ptr(node) {
            return 0;
        }
        if self.should_skip_scene_graph_reentry("adopt", node) {
            return 0;
        }
        let event_origin = format!("guest-focus:{}", origin);
        self.ensure_synthetic_sprite_state(node).guest_graph_observed = true;
        let guest_parent = match self.guest_cocos_node_parent(node, &event_origin) {
            Some(parent) => parent,
            None => return 0,
        };
        let guest_z = self.guest_cocos_node_z_order(node, &event_origin).unwrap_or_else(|| {
            self.runtime.graphics.synthetic_sprites.get(&node).map(|state| state.z_order).unwrap_or(0)
        });
        let current_parent = self.runtime.graphics.synthetic_sprites.get(&node).map(|state| state.parent).unwrap_or(0);
        let mut changed = 0usize;
        if guest_parent != 0 {
            self.ensure_synthetic_sprite_state(guest_parent).guest_graph_observed = true;
            let already_linked = current_parent == guest_parent && self.synthetic_parent_contains_child(guest_parent, node);
            if !already_linked {
                let note = self.attach_child_to_node(guest_parent, node, guest_z, None, &event_origin);
                changed = changed.saturating_add(1);
                self.push_graph_trace(format!(
                    "guest.focus.attach node={} parent={} z={} origin={} note={}",
                    self.describe_ptr(node),
                    self.describe_ptr(guest_parent),
                    guest_z,
                    origin,
                    note,
                ));
            } else {
                let children_array = self.ensure_node_children_array(guest_parent);
                let insert_at = self.synthetic_child_insert_index_for_z(guest_parent, node, guest_z);
                let _ = self.synthetic_array_insert_or_move(children_array, insert_at, node);
                let state = self.ensure_synthetic_sprite_state(node);
                state.parent = guest_parent;
                state.z_order = guest_z;
            }
        } else if current_parent != 0
            && self.runtime.graphics.synthetic_sprites.get(&current_parent).map(|state| state.guest_graph_observed).unwrap_or(false)
        {
            let note = self.remove_child_from_node(current_parent, node, false);
            changed = changed.saturating_add(1);
            self.push_graph_trace(format!(
                "guest.focus.detach node={} parent={} origin={} note={}",
                self.describe_ptr(node),
                self.describe_ptr(current_parent),
                origin,
                note,
            ));
        }

        let attached = self.reconcile_synthetic_node_graph_from_guest(node, &event_origin);
        let total = changed.saturating_add(attached);
        if total != 0 {
            self.invalidate_synthetic_widget_content(node, &event_origin);
        }
        total
    }

    fn detach_child_from_parent(&mut self, child: u32) {
        let old_parent = self.runtime.graphics.synthetic_sprites.get(&child).map(|state| state.parent).unwrap_or(0);
        if old_parent != 0 {
            let children = self.runtime.graphics.synthetic_sprites.get(&old_parent).map(|state| state.children).unwrap_or(0);
            if children != 0 {
                self.synthetic_array_remove_value(children, child);
            }
        }
        if child != 0 {
            let child_state = self.ensure_synthetic_sprite_state(child);
            child_state.parent = 0;
        }
    }

    fn maybe_detach_scene_from_transition_parent(&mut self, scene: u32, origin: &str) {
        if scene == 0 {
            return;
        }
        let old_parent = self.runtime.graphics.synthetic_sprites.get(&scene).map(|state| state.parent).unwrap_or(0);
        if old_parent == 0 {
            return;
        }
        let parent_label = self.diag.object_labels.get(&old_parent).cloned().unwrap_or_default();
        let should_detach = Self::is_transition_like_label(&parent_label)
            || parent_label.contains("SplashScreens")
            || self.runtime.graphics.synthetic_splash_destinations.get(&old_parent).copied().unwrap_or(0) == scene;
        if !should_detach {
            return;
        }
        self.detach_child_from_parent(scene);
        self.diag.trace.push(format!(
            "     ↳ hle scene.detach-from-wrapper scene={} oldParent={} origin={} parentLabel={} state=[{}]",
            self.describe_ptr(scene),
            self.describe_ptr(old_parent),
            origin,
            if parent_label.is_empty() { "<unknown>".to_string() } else { parent_label },
            self.describe_node_graph_state(scene),
        ));
    }

    fn remember_auto_scene_root(&mut self, node: u32, source: impl Into<String>) {
        if node == 0 {
            return;
        }
        let source = source.into();
        if source.contains("inferred") || source.contains("running_scene") {
            self.runtime.scene.auto_scene_inferred_root = node;
            self.runtime.scene.auto_scene_inferred_source = Some(source.clone());
        }
        self.runtime.scene.auto_scene_cached_root = node;
        self.runtime.scene.auto_scene_cached_source = Some(source);
    }

    fn sprite_watch_name_for_ptr(&self, ptr: u32) -> Option<String> {
        if ptr == 0 {
            return None;
        }
        if let Some(label) = self.diag.object_labels.get(&ptr) {
            if let Some(name) = self.active_profile().watched_sprite_match(label) {
                let matched: &'static str = name;
                return Some(String::from(matched));
            }
        }
        let texture = self.runtime.graphics.synthetic_sprites.get(&ptr).map(|state| state.texture).unwrap_or(0);
        if texture != 0 {
            if let Some(name) = self.runtime.graphics.synthetic_textures.get(&texture).map(|tex| tex.source_key.clone()) {
                if self.active_profile().watched_sprite_match(&name).is_some() {
                    return Some(name);
                }
            }
        }
        None
    }

    fn maybe_trace_sprite_watch_event(&mut self, ptr: u32, tag: &str, detail: impl Into<String>) {
        let Some(name) = self.sprite_watch_name_for_ptr(ptr) else {
            return;
        };
        let detail = detail.into();
        self.push_sprite_watch_trace(format!(
            "sprite={} ptr={} {} {}",
            name,
            self.describe_ptr(ptr),
            tag,
            detail,
        ));
        self.diag.trace.push(format!(
            "     ↳ sprite-watch '{}' ptr={} {} {}",
            name,
            self.describe_ptr(ptr),
            tag,
            detail,
        ));
    }

    fn active_effect_scene(&self) -> u32 {
        let effect = self.runtime.ui_cocos.effect_scene;
        if effect != 0 && self.runtime.graphics.synthetic_sprites.contains_key(&effect) {
            effect
        } else {
            0
        }
    }

    fn resolve_active_scene_root_for_input(&self) -> u32 {
        let effect = self.active_effect_scene();
        if effect != 0 {
            if let Some(destination) = self.runtime.graphics.synthetic_splash_destinations.get(&effect).copied() {
                if destination != 0 && self.runtime.graphics.synthetic_sprites.contains_key(&destination) {
                    return destination;
                }
            }
            return effect;
        }
        if self.runtime.ui_cocos.running_scene != 0 {
            let running = self.runtime.ui_cocos.running_scene;
            let running_label = self.diag.object_labels.get(&running).cloned().unwrap_or_default();
            if Self::is_transition_like_label(&running_label) {
                if let Some(destination) = self.runtime.graphics.synthetic_splash_destinations.get(&running).copied() {
                    if destination != 0 && self.runtime.graphics.synthetic_sprites.contains_key(&destination) {
                        return destination;
                    }
                }
            }
            return running;
        }
        if self.runtime.scene.auto_scene_cached_root != 0 {
            return self.runtime.scene.auto_scene_cached_root;
        }
        if self.runtime.scene.auto_scene_inferred_root != 0 {
            return self.runtime.scene.auto_scene_inferred_root;
        }
        self.runtime.ui_cocos.opengl_view
    }

    fn active_watched_sprite_context(&self) -> Option<(u32, String)> {
        for reg in 0..8usize {
            let value = self.cpu.regs[reg];
            if let Some(name) = self.sprite_watch_name_for_ptr(value) {
                return Some((value, format!("{} via r{}", name, reg)));
            }
        }
        None
    }

    fn cocos_director_ivar_layout_for(&mut self, director: u32) -> Option<profiles::DirectorIvarLayout> {
        if director == 0 {
            return None;
        }
        if let Some(layout) = self.active_profile().director_ivar_layout() {
            return Some(layout);
        }
        let open_gl_view_offset = self.objc_lookup_ivar_offset_in_class_chain(director, "openGLView_")?;
        let running_scene_offset = self.objc_lookup_ivar_offset_in_class_chain(director, "runningScene_")?;
        let next_scene_offset = self.objc_lookup_ivar_offset_in_class_chain(director, "nextScene_")?;
        let effect_scene_offset = self
            .objc_lookup_ivar_offset_in_class_chain(director, "effectScene_")
            .or_else(|| self.objc_lookup_ivar_offset_in_class_chain(director, "effectScene"));
        Some(profiles::DirectorIvarLayout {
            open_gl_view_offset,
            running_scene_offset,
            next_scene_offset,
            effect_scene_offset,
        })
    }

    fn maybe_trace_director_ivar_write(&mut self, addr: u32, value: u32, width: u32, kind: &str) {
        if width != 4 || self.exec.current_exec_pc == 0 {
            return;
        }
        let director = self.runtime.ui_cocos.cocos_director;
        if director == 0 {
            return;
        }
        let Some(layout) = self.cocos_director_ivar_layout_for(director) else {
            return;
        };
        let field = if addr == director.wrapping_add(layout.open_gl_view_offset) {
            "openGLView_"
        } else if addr == director.wrapping_add(layout.running_scene_offset) {
            "runningScene_"
        } else if addr == director.wrapping_add(layout.next_scene_offset) {
            "nextScene_"
        } else if layout.effect_scene_offset.map(|offset| addr == director.wrapping_add(offset)).unwrap_or(false) {
            "effectScene"
        } else {
            return;
        };
        let thumb = if self.exec.current_exec_thumb { "thumb" } else { "arm" };
        let class_hint = self.objc_receiver_class_name_hint(value).unwrap_or_default();
        let label_hint = self.diag.object_labels.get(&value).cloned().unwrap_or_default();
        let mut extras = Vec::new();
        if !class_hint.is_empty() {
            extras.push(format!("class={}", class_hint));
        }
        if !label_hint.is_empty() {
            extras.push(format!("label={}", label_hint));
        }
        self.diag.trace.push(format!(
            "     ↳ director-ivar-write kind={} director={} field={} pc=0x{:08x}({}) value={} {}",
            kind,
            self.describe_ptr(director),
            field,
            self.exec.current_exec_pc,
            thumb,
            self.describe_ptr(value),
            extras.join(" "),
        ));
    }

    fn maybe_trace_watched_sprite_write(&mut self, addr: u32, value: u32, width: u32, kind: &str) {
        self.maybe_trace_phase69_rect_builder_write(addr, value, width);
        self.maybe_trace_director_ivar_write(addr, value, width, kind);
        if self.exec.current_exec_pc == 0 {
            return;
        }
        let Some((watch_obj, context)) = self.active_watched_sprite_context() else {
            return;
        };
        let sp = self.cpu.regs[13];
        let obj_window = addr >= watch_obj && addr < watch_obj.wrapping_add(0x40);
        let stack_window = addr >= sp && addr < sp.wrapping_add(0x100);
        if !obj_window && !stack_window {
            return;
        }
        let as_float = Self::f32_from_bits(value);
        let plausible_float = Self::plausible_ui_scalar(as_float) || Self::plausible_ui_size(as_float);
        if !obj_window && value != watch_obj && !plausible_float {
            return;
        }
        let target = if obj_window {
            format!("obj+0x{:x}", addr.wrapping_sub(watch_obj))
        } else {
            format!("stack+0x{:x}", addr.wrapping_sub(sp))
        };
        let value_desc = if plausible_float {
            format!("0x{value:08x}/{:.3}", as_float)
        } else if value == watch_obj {
            format!("0x{value:08x}/self")
        } else if self.sprite_watch_name_for_ptr(value).is_some() {
            format!("0x{value:08x}/peer")
        } else {
            format!("0x{value:08x}")
        };
        let thumb = if self.exec.current_exec_thumb { "thumb" } else { "arm" };
        self.diag.trace.push(format!(
            "     ↳ sprite-watch write kind={} ctx={} pc=0x{:08x} {}=0x{:08x}({}) width={} value={}",
            kind,
            context,
            self.exec.current_exec_pc,
            target,
            addr,
            thumb,
            width,
            value_desc,
        ));
    }

    fn maybe_trace_phase69_rect_builder_write(&mut self, addr: u32, value: u32, width: u32) {
        let current_pc = self.exec.current_exec_pc;
        let sp = self.cpu.regs[13];
        let regs3 = self.cpu.regs[3];
        let trace_line = self.active_profile().phase69_rect_builder_trace(
            current_pc,
            sp,
            regs3,
            addr,
            value,
            width,
            &mut |read_addr| self.read_u32_le(read_addr).ok(),
        );
        if let Some(line) = trace_line {
            self.diag.trace.push(line);
        }
    }

    fn find_scene_ancestor(&mut self, node: u32) -> Option<(u32, &'static str)> {
        let mut cursor = node;
        let mut path = Vec::new();
        let mut visited = HashSet::new();
        for depth in 0..128u32 {
            if cursor == 0 {
                break;
            }
            if let Some(pos) = path.iter().position(|value| *value == cursor) {
                let loop_nodes = path[pos..].to_vec();
                self.record_scene_graph_cycle("find_scene_ancestor", &loop_nodes, cursor, depth);
                break;
            }
            if !visited.insert(cursor) {
                break;
            }
            path.push(cursor);
            if self.runtime.ui_cocos.running_scene != 0 && cursor == self.runtime.ui_cocos.running_scene {
                return Some((cursor, "running_scene"));
            }
            if self.runtime.scene.auto_scene_inferred_root != 0 && cursor == self.runtime.scene.auto_scene_inferred_root {
                return Some((cursor, "cached_inferred_scene"));
            }
            if self.runtime.scene.auto_scene_cached_root != 0 && cursor == self.runtime.scene.auto_scene_cached_root {
                return Some((cursor, "cached_scene"));
            }
            let state = match self.runtime.graphics.synthetic_sprites.get(&cursor) {
                Some(state) => state,
                None => break,
            };
            if state.entered {
                return Some((cursor, "entered_ancestor"));
            }
            cursor = state.parent;
        }
        None
    }

    fn propagate_entered_recursive(&mut self, node: u32, entered: bool) -> usize {
        let mut visited = HashSet::new();
        let mut path = Vec::new();
        self.propagate_entered_recursive_guarded(node, entered, 0, &mut visited, &mut path)
    }

    fn propagate_entered_recursive_guarded(
        &mut self,
        node: u32,
        entered: bool,
        depth: u32,
        visited: &mut HashSet<u32>,
        path: &mut Vec<u32>,
    ) -> usize {
        if node == 0 {
            return 0;
        }
        if depth > 256 {
            self.push_graph_trace_critical(format!(
                "traversal.depth-limit walk=propagate_entered depth={} node={}",
                depth,
                self.describe_ptr(node),
            ));
            return 0;
        }
        if let Some(pos) = path.iter().position(|value| *value == node) {
            let loop_nodes = path[pos..].to_vec();
            self.record_scene_graph_cycle("propagate_entered", &loop_nodes, node, depth);
            return 0;
        }
        if !visited.insert(node) {
            return 0;
        }
        path.push(node);
        let children = self.runtime.graphics
            .synthetic_sprites
            .get(&node)
            .and_then(|state| {
                if state.children != 0 {
                    self.runtime.graphics.synthetic_arrays.get(&state.children).map(|arr| arr.items.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        self.ensure_synthetic_sprite_state(node).entered = entered;
        let mut count = 1usize;
        for child in children {
            count = count.saturating_add(self.propagate_entered_recursive_guarded(
                child,
                entered,
                depth.saturating_add(1),
                visited,
                path,
            ));
        }
        path.pop();
        count
    }

    fn maybe_infer_running_scene(&mut self, node: u32, reason: &str) -> bool {
        if node == 0 {
            return false;
        }
        let state = match self.runtime.graphics.synthetic_sprites.get(&node) {
            Some(state) => state,
            None => return false,
        };
        if state.parent != 0 {
            return false;
        }
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let scene_like = label.contains("CCScene")
            || self.active_profile().is_first_scene_label(&label)
            || label.contains("Scene.instance")
            || label.contains("Scene.synthetic");
        if !scene_like {
            return false;
        }
        self.remember_auto_scene_root(node, format!("inferred:{}", reason));
        if self.runtime.ui_cocos.running_scene != 0 {
            return false;
        }
        self.runtime.ui_cocos.running_scene = node;
        self.diag.trace.push(format!(
            "     ↳ auto-scene inferred running_scene={} reason={} state=[{}]",
            self.describe_ptr(node),
            reason,
            self.describe_node_graph_state(node),
        ));
        true
    }

    fn active_loading_scene_node(&self) -> u32 {
        let scene = self.resolve_synthetic_progress_watch_scene(self.runtime.ui_cocos.running_scene);
        if scene == 0 {
            return 0;
        }
        let label = self.diag.object_labels.get(&scene).cloned().unwrap_or_default();
        if self.active_profile().is_loading_scene_label(&label) {
            scene
        } else {
            0
        }
    }

    fn loading_scene_has_continue_prompt(&self, scene: u32) -> bool {
        if scene == 0 {
            return false;
        }
        let mut stack = vec![scene];
        let mut seen = HashSet::new();
        while let Some(node) = stack.pop() {
            if node == 0 || !seen.insert(node) {
                continue;
            }
            if let Some(text) = self.string_backing(node) {
                if self.active_profile().loading_continue_prompt_matches(&text.text) {
                    return true;
                }
            }
            if let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) {
                if state.children != 0 {
                    if let Some(arr) = self.runtime.graphics.synthetic_arrays.get(&state.children) {
                        for child in arr.items.iter().rev() {
                            if *child != 0 {
                                stack.push(*child);
                            }
                        }
                    }
                }
            }
        }
        false
    }

    fn find_existing_loading_bmfont_node(&self, text: &str, fnt_file: &str) -> Option<u32> {
        let scene = self.active_loading_scene_node();
        if scene == 0 {
            return None;
        }
        let want_text = text.replace('\0', "");
        let want_fnt = fnt_file.replace('\0', "");
        let mut stack = vec![scene];
        let mut seen = HashSet::new();
        while let Some(node) = stack.pop() {
            if node == 0 || !seen.insert(node) {
                continue;
            }
            let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
            if label.contains("CCLabelBMFont") || label.contains("FontAtlas") {
                if let Some(backing) = self.string_backing(node) {
                    if backing.text.replace('\0', "") == want_text {
                        if want_fnt.is_empty() || label.contains(&want_fnt) {
                            return Some(node);
                        }
                    }
                }
            }
            if let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) {
                if state.children != 0 {
                    if let Some(arr) = self.runtime.graphics.synthetic_arrays.get(&state.children) {
                        for child in arr.items.iter().rev() {
                            if *child != 0 {
                                stack.push(*child);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    fn maybe_prepare_loading_scene_continue_path(&mut self, scene: u32, origin: &str, age: u32) -> Vec<String> {
        if scene == 0 || !self.loading_scene_has_continue_prompt(scene) {
            return Vec::new();
        }
        const BOOTSTRAP_CONTINUE_TOUCH_ENABLED: u32 = 1 << 8;
        const BOOTSTRAP_CONTINUE_PROBED: u32 = 1 << 9;

        let mut forced = Vec::new();
        let touch_enabled = self.runtime.graphics.synthetic_sprites.get(&scene).map(|state| state.touch_enabled).unwrap_or(false);
        if !touch_enabled {
            self.ensure_synthetic_sprite_state(scene).touch_enabled = true;
            self.runtime.ui_objects.first_responder = scene;
            if self.invoke_objc_selector_now(scene, "setIsTouchEnabled:", 1, 0, 120_000, "loading-scene-continue") {
                forced.push("LoadingMissionScene.setIsTouchEnabled:".to_string());
            }
            if self.invoke_objc_selector_now(scene, "registerWithTouchDispatcher", 0, 0, 120_000, "loading-scene-continue") {
                forced.push("LoadingMissionScene.registerWithTouchDispatcher".to_string());
            }
            self.loading_scene_bootstrap_mark_success(scene, BOOTSTRAP_CONTINUE_TOUCH_ENABLED);
        }

        let should_probe = !self.loading_scene_bootstrap_has(scene, BOOTSTRAP_CONTINUE_PROBED)
            && (age >= 6
                || self.runtime.host_input.last_dispatch.as_deref().map(|v| v.contains("without-active-touch") || v.contains("miss-down")).unwrap_or(false));
        if should_probe {
            self.runtime.scene.synthetic_touch_injections = self.runtime.scene.synthetic_touch_injections.saturating_add(1);
            self.runtime.ui_objects.first_responder = scene;
            let began = self.invoke_objc_selector_now(scene, "ccTouchBegan:withEvent:", 0, 0, 120_000, "loading-scene-continue");
            let ended = self.invoke_objc_selector_now(scene, "ccTouchEnded:withEvent:", 0, 0, 120_000, "loading-scene-continue");
            if began {
                forced.push("LoadingMissionScene.ccTouchBegan:withEvent:".to_string());
            }
            if ended {
                forced.push("LoadingMissionScene.ccTouchEnded:withEvent:".to_string());
            }
            if began || ended {
                self.loading_scene_bootstrap_mark_success(scene, BOOTSTRAP_CONTINUE_PROBED);
            }
        }

        if !forced.is_empty() {
            self.diag.trace.push(format!(
                "     ↳ hle loading-scene continue scene={} age={} origin={} forced=[{}] touchEnabled={} lastDispatch={}",
                self.describe_ptr(scene),
                age,
                origin,
                forced.join(","),
                if self.runtime.graphics.synthetic_sprites.get(&scene).map(|state| state.touch_enabled).unwrap_or(false) { "YES" } else { "NO" },
                self.runtime.host_input.last_dispatch.clone().unwrap_or_else(|| "<none>".to_string()),
            ));
        }
        forced
    }

    fn maybe_find_duplicate_loading_bmfont_child(&self, parent: u32, child: u32, z_order: i32) -> Option<u32> {
        if parent == 0 || child == 0 {
            return None;
        }
        let parent_label = self.diag.object_labels.get(&parent).cloned().unwrap_or_default();
        if !self.active_profile().should_dedupe_loading_bmfont_parent(&parent_label) {
            return None;
        }
        let child_label = self.diag.object_labels.get(&child).cloned().unwrap_or_default();
        if !(child_label.contains("CCLabelBMFont") || child_label.contains("FontAtlas")) {
            return None;
        }
        let child_text = self.string_backing(child)?.text.replace('\0', "");
        let child_state = self.runtime.graphics.synthetic_sprites.get(&child)?;
        let children = self.runtime.graphics.synthetic_sprites.get(&parent).map(|state| state.children).unwrap_or(0);
        if children == 0 {
            return None;
        }
        let count = self.synthetic_array_len(children);
        for index in 0..count {
            let sibling = self.synthetic_array_get(children, index);
            if sibling == 0 || sibling == child {
                continue;
            }
            let sibling_label = self.diag.object_labels.get(&sibling).cloned().unwrap_or_default();
            if sibling_label != child_label {
                continue;
            }
            let Some(sibling_text) = self.string_backing(sibling) else {
                continue;
            };
            if sibling_text.text.replace('\0', "") != child_text {
                continue;
            }
            let Some(sibling_state) = self.runtime.graphics.synthetic_sprites.get(&sibling) else {
                continue;
            };
            if sibling_state.z_order != z_order {
                continue;
            }
            if sibling_state.position_x_bits != child_state.position_x_bits
                || sibling_state.position_y_bits != child_state.position_y_bits
            {
                continue;
            }
            return Some(sibling);
        }
        None
    }

    fn attach_child_to_node(&mut self, parent: u32, child: u32, z_order: i32, tag: Option<u32>, selector: &str) -> String {
        if parent == 0 || child == 0 {
            return format!("cocos {} ignored parent={} child={}", selector, self.describe_ptr(parent), self.describe_ptr(child));
        }
        if let Some(existing) = self.maybe_find_duplicate_loading_bmfont_child(parent, child, z_order) {
            let child_state_snapshot = self.runtime.graphics.synthetic_sprites.get(&child).cloned();
            let child_text_snapshot = self.runtime.heap.synthetic_string_backing.get(&child).cloned();
            if let Some(snapshot) = child_state_snapshot {
                let existing_state = self.ensure_synthetic_sprite_state(existing);
                existing_state.visible = true;
                if snapshot.width != 0 {
                    existing_state.width = snapshot.width;
                }
                if snapshot.height != 0 {
                    existing_state.height = snapshot.height;
                }
                if snapshot.position_x_bits != 0 || snapshot.position_y_bits != 0 {
                    existing_state.position_x_bits = snapshot.position_x_bits;
                    existing_state.position_y_bits = snapshot.position_y_bits;
                }
                if snapshot.texture != 0 {
                    existing_state.texture = snapshot.texture;
                }
            }
            if let Some(text_backing) = child_text_snapshot {
                self.runtime.heap.synthetic_string_backing.insert(existing, text_backing);
            }
            let child_state = self.ensure_synthetic_sprite_state(child);
            child_state.visible = false;
            child_state.parent = 0;
            child_state.entered = false;
            return format!(
                "cocos {} deduped duplicate loading bmfont parent={} existing={} dropped={} z={} childState=[{}] existingState=[{}]",
                selector,
                self.describe_ptr(parent),
                self.describe_ptr(existing),
                self.describe_ptr(child),
                z_order,
                self.describe_node_graph_state(child),
                self.describe_node_graph_state(existing),
            );
        }

        if let Some(loop_nodes) = self.would_create_synthetic_cycle(parent, child) {
            self.record_scene_graph_cycle("attach_child", &loop_nodes, child, loop_nodes.len() as u32);
            return format!(
                "cocos {} cycle-blocked parent={} child={} loop={}",
                selector,
                self.describe_ptr(parent),
                self.describe_ptr(child),
                self.format_scene_graph_path(&loop_nodes),
            );
        }

        let old_parent = self.runtime.graphics.synthetic_sprites.get(&child).map(|state| state.parent).unwrap_or(0);
        let child_was_entered = self.runtime.graphics.synthetic_sprites.get(&child).map(|state| state.entered).unwrap_or(false);
        let old_parent_entered = old_parent != 0 && (
            old_parent == self.runtime.ui_cocos.running_scene
                || self.runtime.graphics.synthetic_sprites.get(&old_parent).map(|state| state.entered).unwrap_or(false)
                || self.find_scene_ancestor(old_parent).is_some()
        );
        let mut exited_propagated = 0usize;
        let mut exit_invoked = 0usize;
        if old_parent != 0 && old_parent != parent {
            self.detach_child_from_parent(child);
            if child_was_entered && old_parent_entered {
                exited_propagated = self.propagate_entered_recursive(child, false);
                exit_invoked = self.invoke_scene_lifecycle_selector_now(child, "onExit", selector);
            }
        }

        let children = self.ensure_node_children_array(parent);
        let insert_at = self.synthetic_child_insert_index_for_z(parent, child, z_order);
        let index = self.synthetic_array_insert_or_move(children, insert_at, child);
        {
            let child_state = self.ensure_synthetic_sprite_state(child);
            child_state.parent = parent;
            child_state.z_order = z_order;
            if let Some(value) = tag {
                child_state.tag = value;
            }
        }
        self.maybe_center_menu_child_in_parent(parent, child);
        self.maybe_align_menu_item_visual_child(parent, child);

        let inferred_scene = self.maybe_infer_running_scene(parent, selector);
        let parent_entered = self.runtime.graphics.synthetic_sprites.get(&parent).map(|state| state.entered).unwrap_or(false);
        let scene_ancestor = self.find_scene_ancestor(parent);
        let child_entered_before_attach = self.runtime.graphics.synthetic_sprites.get(&child).map(|state| state.entered).unwrap_or(false);
        let mut entered_propagated = 0usize;
        let mut enter_reason = "none".to_string();
        let mut on_enter_invoked = 0usize;
        let mut on_enter_finish_invoked = 0usize;
        if inferred_scene {
            entered_propagated = self.propagate_entered_recursive(parent, true);
            enter_reason = "inferred-scene".to_string();
        } else if parent == self.runtime.ui_cocos.running_scene || parent_entered {
            self.ensure_synthetic_sprite_state(parent).entered = true;
            entered_propagated = self.propagate_entered_recursive(child, true);
            enter_reason = if parent == self.runtime.ui_cocos.running_scene {
                "direct-running-scene".to_string()
            } else {
                "parent-entered".to_string()
            };
            if !child_entered_before_attach {
                on_enter_invoked = self.invoke_scene_lifecycle_selector_now(child, "onEnter", selector);
                if self.objc_lookup_imp_for_receiver(child, "onEnterTransitionDidFinish").is_some() {
                    on_enter_finish_invoked = self.invoke_scene_lifecycle_selector_now(child, "onEnterTransitionDidFinish", selector);
                }
            }
        } else if let Some((ancestor, why)) = scene_ancestor {
            entered_propagated = self.propagate_entered_recursive(parent, true);
            enter_reason = format!("ancestor:{}<-{}", why, self.describe_ptr(ancestor));
            if !child_entered_before_attach {
                on_enter_invoked = self.invoke_scene_lifecycle_selector_now(child, "onEnter", selector);
                if self.objc_lookup_imp_for_receiver(child, "onEnterTransitionDidFinish").is_some() {
                    on_enter_finish_invoked = self.invoke_scene_lifecycle_selector_now(child, "onEnterTransitionDidFinish", selector);
                }
            }
        }

        let child_tag = self.runtime.graphics.synthetic_sprites.get(&child).map(|state| state.tag).unwrap_or(0);
        format!(
            "cocos {} parent={} child={} z={} tag={} index={} children={} exitProp={} exitInv={} enterProp={} enterReason={} onEnterInv={} onEnterFinishInv={} sceneRoot={} childState=[{}]",
            selector,
            self.describe_ptr(parent),
            self.describe_ptr(child),
            z_order,
            child_tag,
            index,
            self.synthetic_array_len(children),
            exited_propagated,
            exit_invoked,
            entered_propagated,
            enter_reason,
            on_enter_invoked,
            on_enter_finish_invoked,
            if self.runtime.ui_cocos.running_scene != 0 {
                self.describe_ptr(self.runtime.ui_cocos.running_scene)
            } else if self.runtime.scene.auto_scene_inferred_root != 0 {
                self.describe_ptr(self.runtime.scene.auto_scene_inferred_root)
            } else if self.runtime.scene.auto_scene_cached_root != 0 {
                self.describe_ptr(self.runtime.scene.auto_scene_cached_root)
            } else {
                "nil".to_string()
            },
            self.describe_node_graph_state(child),
        )
    }

    fn maybe_center_menu_child_in_parent(&mut self, parent: u32, child: u32) {
        if parent == 0 || child == 0 {
            return;
        }
        let parent_label = self.diag.object_labels.get(&parent).cloned().unwrap_or_default();
        let child_label = self.diag.object_labels.get(&child).cloned().unwrap_or_default();
        let child_children = self.runtime.graphics.synthetic_sprites.get(&child).map(|state| state.children).unwrap_or(0);
        let is_menu_like = child_label.contains("CCMenu")
            || (child_children != 0 && self.synthetic_array_len(child_children) > 1 && child_label.contains("instance(synth)"));
        let parent_is_layer_like = self.active_profile().is_menu_layer_label(&parent_label)
            || parent_label.contains("CCLayer")
            || parent_label.contains("CCScene")
            || self.active_profile().is_first_scene_label(&parent_label);
        if !is_menu_like || !parent_is_layer_like {
            return;
        }
        if self.synthetic_node_has_fullscreenish_child(child) {
            return;
        }
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1) as f32;
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
        let state = self.ensure_synthetic_sprite_state(child);
        if state.position_x_bits == 0 && state.position_y_bits == 0 {
            state.position_x_bits = (surface_w * 0.5).to_bits();
            state.position_y_bits = (surface_h * 0.5).to_bits();
            if state.anchor_x_bits == 0 {
                state.anchor_x_bits = 0.5f32.to_bits();
            }
            if state.anchor_y_bits == 0 {
                state.anchor_y_bits = 0.5f32.to_bits();
            }
        }
    }
    fn maybe_align_menu_item_visual_child(&mut self, parent: u32, child: u32) {
        if parent == 0 || child == 0 {
            return;
        }
        let parent_label = self.diag.object_labels.get(&parent).cloned().unwrap_or_default();
        if !Self::is_menu_item_class_name(&parent_label) {
            return;
        }
        let child_label = self.diag.object_labels.get(&child).cloned().unwrap_or_default();
        if !(child_label.contains("CCSprite") || Self::is_label_class_name(&child_label)) {
            return;
        }
        let (parent_w, parent_h) = self.runtime.graphics.synthetic_sprites
            .get(&parent)
            .map(|state| (state.width.max(1), state.height.max(1)))
            .unwrap_or((0, 0));
        if parent_w == 0 || parent_h == 0 {
            return;
        }
        let child_state = self.ensure_synthetic_sprite_state(child);
        if child_state.position_x_bits == 0 && child_state.position_y_bits == 0 {
            child_state.position_x_bits = (parent_w as f32 * 0.5).to_bits();
            child_state.position_y_bits = (parent_h as f32 * 0.5).to_bits();
        }
        if !child_state.anchor_explicit {
            child_state.anchor_x_bits = 0.5f32.to_bits();
            child_state.anchor_y_bits = 0.5f32.to_bits();
        }
    }


    fn abovebelow_menu_stack_plan(&self, menu: u32, force: bool) -> Option<(Vec<u32>, f32, f32, f32)> {
        if menu == 0 {
            return None;
        }
        let menu_label = self.diag.object_labels.get(&menu).cloned().unwrap_or_default();
        if !menu_label.contains("CCMenu") {
            return None;
        }
        let children = self.runtime.graphics.synthetic_sprites.get(&menu).map(|state| state.children).unwrap_or(0);
        if children == 0 {
            return None;
        }
        let kids = self.runtime.graphics.synthetic_arrays.get(&children).map(|arr| arr.items.clone()).unwrap_or_default();
        if kids.is_empty() {
            return None;
        }
        let menu_parent = self.runtime.graphics.synthetic_sprites.get(&menu).map(|state| state.parent).unwrap_or(0);
        let menu_parent_label = self.diag.object_labels.get(&menu_parent).cloned().unwrap_or_default();
        let mut entries: Vec<profiles::MenuEntry> = Vec::new();
        for child in kids {
            if child == 0 {
                continue;
            }
            let child_label = self.diag.object_labels.get(&child).cloned().unwrap_or_default();
            if !child_label.contains("CCMenuItem") {
                continue;
            }
            let Some(state) = self.runtime.graphics.synthetic_sprites.get(&child) else {
                continue;
            };
            let mut cw = state.width as f32;
            let mut ch = state.height as f32;
            if (cw <= 0.0 || ch <= 0.0) && state.children != 0 {
                if let Some(arr) = self.runtime.graphics.synthetic_arrays.get(&state.children) {
                    for img in &arr.items {
                        if let Some(img_state) = self.runtime.graphics.synthetic_sprites.get(img) {
                            if img_state.width > 0 && img_state.height > 0 {
                                cw = img_state.width as f32;
                                ch = img_state.height as f32;
                                break;
                            }
                        }
                    }
                }
            }
            if !(cw > 0.0 && ch > 0.0) {
                continue;
            }
            if cw < 120.0 || ch < 40.0 {
                continue;
            }
            let px = Self::f32_from_bits(state.position_x_bits);
            let py = Self::f32_from_bits(state.position_y_bits);
            entries.push(profiles::MenuEntry {
                node: child,
                width: cw,
                height: ch,
                x: px,
                y: py,
            });
        }

        self.active_profile().menu_stack_plan(&menu_parent_label, &entries, force)
            .map(|plan| (plan.ordered, plan.pitch, plan.top_y, plan.anchor_x))
    }

    fn abovebelow_relayout_menu_buttons(&mut self, menu: u32, _reason: &str) -> usize {
        let Some((entries, pitch, top_y, anchor_x)) = self.abovebelow_menu_stack_plan(menu, true) else {
            return 0;
        };
        let mut changed = 0usize;
        for (idx, child) in entries.iter().enumerate() {
            let desired_x = anchor_x;
            let desired_y = top_y - pitch * (idx as f32);
            let state = self.ensure_synthetic_sprite_state(*child);
            let old_x = Self::f32_from_bits(state.position_x_bits);
            let old_y = Self::f32_from_bits(state.position_y_bits);
            if (old_x - desired_x).abs() <= 0.5 && (old_y - desired_y).abs() <= 0.5 {
                continue;
            }
            state.position_x_bits = desired_x.to_bits();
            state.position_y_bits = desired_y.to_bits();
            changed = changed.saturating_add(1);
        }
        changed
    }

    fn is_cocos_multiplex_layer(&self, receiver: u32) -> bool {
        if receiver == 0 {
            return false;
        }
        let label = self.diag.object_labels.get(&receiver).map(String::as_str).unwrap_or("");
        if label.contains("CCMultiplexLayer") {
            return true;
        }
        let class_name = self.objc_receiver_class_name_hint(receiver).unwrap_or_default();
        if class_name.contains("CCMultiplexLayer") {
            return true;
        }
        self.objc_receiver_inherits_named(receiver, "CCMultiplexLayer")
    }

    fn find_active_cocos_multiplex_layer(&self) -> Option<u32> {
        let roots = [self.runtime.ui_cocos.running_scene, self.runtime.scene.auto_scene_inferred_root, self.runtime.scene.auto_scene_cached_root];
        let mut best: Option<(u32, i32)> = None;
        for (&node, state) in &self.runtime.graphics.synthetic_sprites {
            if !self.is_cocos_multiplex_layer(node) {
                continue;
            }
            let mut score = 0i32;
            if state.entered {
                score += 50;
            }
            if state.visible {
                score += 20;
            }
            if state.parent != 0 {
                if roots.contains(&state.parent) {
                    score += 80;
                }
                let parent_label = self.diag.object_labels.get(&state.parent).map(String::as_str).unwrap_or("");
                if parent_label.contains("Scene") || self.active_profile().is_first_scene_label(&parent_label) {
                    score += 20;
                }
            }
            let layers = self.read_u32_le(node.wrapping_add(0x9c)).ok().unwrap_or(0);
            if layers != 0 && self.runtime.graphics.synthetic_arrays.contains_key(&layers) {
                score += 10 + self.synthetic_array_len(layers) as i32;
            }
            if state.children != 0 {
                score += self.synthetic_array_len(state.children) as i32;
            }
            match best {
                Some((_, best_score)) if best_score >= score => {}
                _ => best = Some((node, score)),
            }
        }
        best.map(|(node, _)| node)
    }

    fn try_handle_ccmultiplex_switch(&mut self, selector: &str, receiver: u32, target_index: u32) -> Option<(u32, String)> {
        if selector != "switchTo:" && selector != "switchToAndReleaseMe:" {
            return None;
        }
        let (multiplex, repair_note) = if self.is_cocos_multiplex_layer(receiver) {
            (receiver, String::new())
        } else if receiver == 0 {
            let repaired = self.find_active_cocos_multiplex_layer()?;
            (repaired, format!(" receiver-repair=nil->{}", self.describe_ptr(repaired)))
        } else {
            return None;
        };

        let layers = self.read_u32_le(multiplex.wrapping_add(0x9c)).ok().unwrap_or(0);
        if layers == 0 || !self.runtime.graphics.synthetic_arrays.contains_key(&layers) {
            return Some((
                multiplex,
                format!(
                    "cocos multiplex {} receiver={} active={}{} layers-missing",
                    selector,
                    self.describe_ptr(receiver),
                    self.describe_ptr(multiplex),
                    repair_note,
                ),
            ));
        }

        let layer_count = self.synthetic_array_len(layers);
        if layer_count == 0 {
            return Some((
                multiplex,
                format!(
                    "cocos multiplex {} receiver={} active={}{} layers-empty",
                    selector,
                    self.describe_ptr(receiver),
                    self.describe_ptr(multiplex),
                    repair_note,
                ),
            ));
        }
        if (target_index as usize) >= layer_count {
            return Some((
                multiplex,
                format!(
                    "cocos multiplex {} receiver={} active={}{} target={} out-of-range count={}",
                    selector,
                    self.describe_ptr(receiver),
                    self.describe_ptr(multiplex),
                    repair_note,
                    target_index,
                    layer_count,
                ),
            ));
        }

        let old_index = self.read_u32_le(multiplex.wrapping_add(0x98)).ok().unwrap_or(0);
        let old_layer = self.synthetic_array_get(layers, old_index as usize);
        let target_layer = self.synthetic_array_get(layers, target_index as usize);
        let remove_note = if old_layer != 0 && old_layer != target_layer {
            self.remove_child_from_node(multiplex, old_layer, false)
        } else {
            format!("cocos removeChild skipped current={}", self.describe_ptr(old_layer))
        };
        let attach_note = if target_layer != 0 {
            self.ensure_synthetic_sprite_state(target_layer).visible = true;
            self.attach_child_to_node(multiplex, target_layer, 0, None, selector)
        } else {
            "cocos addChild skipped target=nil".to_string()
        };
        for idx in 0..layer_count {
            let layer = self.synthetic_array_get(layers, idx);
            if layer == 0 {
                continue;
            }
            let state = self.ensure_synthetic_sprite_state(layer);
            state.visible = idx == target_index as usize;
            if idx != target_index as usize && layer == old_layer {
                state.entered = false;
            }
        }
        let _ = self.write_u32_le(multiplex.wrapping_add(0x98), target_index);
        let release_note = if selector == "switchToAndReleaseMe:" {
            " releaseCompat=deferred"
        } else {
            ""
        };
        let multiplex_label = self.diag.object_labels.get(&multiplex).cloned().unwrap_or_default();
        self.push_scene_progress_selector_event(
            selector,
            multiplex,
            &multiplex_label,
            selector,
            target_index,
            target_layer,
            Some(multiplex),
            false,
        );
        Some((
            multiplex,
            format!(
                "cocos multiplex {} receiver={} active={} oldIndex={} targetIndex={} oldLayer={} targetLayer={} layers={} count={}{}{} | {} | {}",
                selector,
                self.describe_ptr(receiver),
                self.describe_ptr(multiplex),
                old_index,
                target_index,
                self.describe_ptr(old_layer),
                self.describe_ptr(target_layer),
                self.describe_ptr(layers),
                layer_count,
                repair_note,
                release_note,
                remove_note,
                attach_note,
            ),
        ))
    }

    fn remove_child_from_node(&mut self, parent: u32, child: u32, cleanup: bool) -> String {
        if parent == 0 || child == 0 {
            return format!("cocos removeChild ignored parent={} child={}", self.describe_ptr(parent), self.describe_ptr(child));
        }
        let children = self.runtime.graphics.synthetic_sprites.get(&parent).map(|state| state.children).unwrap_or(0);
        let parent_entered = parent == self.runtime.ui_cocos.running_scene
            || self.runtime.graphics.synthetic_sprites.get(&parent).map(|state| state.entered).unwrap_or(false)
            || self.find_scene_ancestor(parent).is_some();
        let child_was_entered = self.runtime.graphics.synthetic_sprites.get(&child).map(|state| state.entered).unwrap_or(false);
        let removed = if children != 0 {
            self.synthetic_array_remove_value(children, child)
        } else {
            false
        };
        let mut exited_propagated = 0usize;
        let mut exit_invoked = 0usize;
        let mut cleanup_invoked = 0usize;
        if removed {
            if child_was_entered && parent_entered {
                exited_propagated = self.propagate_entered_recursive(child, false);
                exit_invoked = self.invoke_scene_lifecycle_selector_now(child, "onExit", "removeChild:cleanup:");
            }
            if cleanup {
                cleanup_invoked = self.invoke_scene_lifecycle_selector_now(child, "cleanup", "removeChild:cleanup:");
            }
            let child_state = self.ensure_synthetic_sprite_state(child);
            child_state.parent = 0;
            if !child_was_entered || !parent_entered {
                child_state.entered = false;
            }
            if cleanup {
                child_state.texture = 0;
            }
        }
        format!(
            "cocos removeChild parent={} child={} cleanup={} removed={} remaining={} exitProp={} exitInv={} cleanupInv={} childState=[{}]",
            self.describe_ptr(parent),
            self.describe_ptr(child),
            if cleanup { "YES" } else { "NO" },
            if removed { "YES" } else { "NO" },
            if children != 0 { self.synthetic_array_len(children) } else { 0 },
            exited_propagated,
            exit_invoked,
            cleanup_invoked,
            self.describe_node_graph_state(child),
        )
    }


    fn synthetic_menu_item_candidate(&self, ptr: u32) -> bool {
        if ptr == 0 {
            return false;
        }
        let label = self.diag.object_labels.get(&ptr).map(String::as_str).unwrap_or("");
        if label.contains("CCMenuItem") {
            return true;
        }
        let class_name = self.objc_class_name_for_receiver(ptr).unwrap_or_default();
        if class_name.contains("CCMenuItem") {
            return true;
        }
        self.runtime
            .graphics
            .synthetic_sprites
            .get(&ptr)
            .map(|state| state.callback_selector != 0)
            .unwrap_or(false)
    }

    fn collect_menu_items_from_message(&self, first_arg: u32, second_arg: u32, include_varargs: bool) -> Vec<u32> {
        let mut items = Vec::new();
        let mut push_candidate = |value: u32| {
            if value != 0 && self.synthetic_menu_item_candidate(value) && !items.contains(&value) {
                items.push(value);
            }
        };
        push_candidate(first_arg);
        push_candidate(second_arg);
        if include_varargs {
            for index in 0..8 {
                let Some(value) = self.peek_stack_u32(index) else { break; };
                if value == 0 {
                    break;
                }
                push_candidate(value);
            }
        }
        items
    }

    fn collect_menu_items_from_array_or_single(&self, arg: u32) -> Vec<u32> {
        if let Some(array) = self.runtime.graphics.synthetic_arrays.get(&arg) {
            return array
                .items
                .iter()
                .copied()
                .filter(|value| self.synthetic_menu_item_candidate(*value))
                .collect();
        }
        self.collect_menu_items_from_message(arg, 0, false)
    }

    fn describe_node_graph_state(&self, node: u32) -> String {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return "visible=YES children=0 parent=nil entered=NO touch=NO".to_string();
        };
        let child_count = if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 };
        let callback = if state.callback_selector != 0 {
            let selector = self.objc_read_selector_name(state.callback_selector).unwrap_or_else(|| format!("0x{:08x}", state.callback_selector));
            format!(" target={} selector={}", self.describe_ptr(state.callback_target), selector)
        } else {
            String::new()
        };
        format!(
            "visible={} children={} parent={} entered={} touch={} pos=({:.1},{:.1}) size={}x{}{}",
            if state.visible { "YES" } else { "NO" },
            child_count,
            self.describe_ptr(state.parent),
            if state.entered { "YES" } else { "NO" },
            if state.touch_enabled { "YES" } else { "NO" },
            Self::f32_from_bits(state.position_x_bits),
            Self::f32_from_bits(state.position_y_bits),
            state.width,
            state.height,
            callback,
        )
    }

    fn synthetic_node_looks_widget_like(&mut self, node: u32) -> bool {
        if node == 0 {
            return false;
        }
        let label = self.diag.object_labels.get(&node).map(String::as_str).unwrap_or("");
        let class_name = self.objc_class_name_for_receiver(node).unwrap_or_default();
        if label.contains("Button")
            || class_name.contains("Button")
            || label.contains("TextureNode")
            || class_name.contains("TextureNode")
        {
            return true;
        }
        [
            "texture",
            "setTexture:",
            "setDisplayFrame:index:",
            "setSize:",
            "size",
            "setTransformAnchor:",
            "setPositionBL:",
            "setStateNormal",
            "addAnimation:",
            "initAnimationDictionary",
        ]
        .iter()
        .any(|sel| self.objc_lookup_imp_for_receiver(node, sel).is_some())
    }

    fn should_trace_widget_selector(&mut self, node: u32, selector: &str) -> bool {
        if !self.synthetic_node_looks_widget_like(node) {
            return false;
        }
        matches!(
            selector,
            "setTexture:"
                | "texture"
                | "setContentSize:"
                | "contentSize"
                | "setSize:"
                | "size"
                | "setTextureRect:"
                | "setTextureRect:untrimmedSize:"
                | "initWithTexture:"
                | "initWithTexture:rect:"
                | "initWithTexture:rect:rotated:"
                | "setDisplayFrame:index:"
                | "draw"
                | "visit"
                | "setPositionBL:"
                | "setTransformAnchor:"
                | "setStateNormal"
                | "addAnimation:"
                | "initAnimationDictionary" 
        )
    }

    fn synthetic_node_draw_debug(&self, node: u32) -> (bool, String, u32, u32, u32) {
        let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
            return (false, "missing-state".to_string(), 0, 0, 0);
        };
        if !state.visible {
            return (false, "invisible".to_string(), state.texture, 0, 0);
        }
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let effective_texture = self.synthetic_node_effective_texture(node);
        let mut layout_w = state.width;
        let mut layout_h = state.height;
        let explicit_zero_rect = state.texture_rect_explicit && (state.width == 0 || state.height == 0);
        let mut has_image = false;
        if effective_texture != 0 {
            if let Some(texture) = self.runtime.graphics.synthetic_textures.get(&effective_texture) {
                if !explicit_zero_rect {
                    if layout_w == 0 {
                        layout_w = texture.width;
                    }
                    if layout_h == 0 {
                        layout_h = texture.height;
                    }
                }
                has_image = texture.image != 0;
            }
        }
        let has_text = self.string_backing(node).is_some();
        if (layout_w == 0 || layout_h == 0) && has_text {
            let text = self.string_backing(node).map(|backing| backing.text.replace('\0', "")).unwrap_or_default();
            let scale = Self::synthetic_text_scale_for_height(layout_h.max(14));
            let (text_w, text_h) = Self::synthetic_text_dimensions_5x7(&text, scale);
            if layout_w == 0 {
                layout_w = text_w.max(1);
            }
            if layout_h == 0 {
                layout_h = text_h.max(1);
            }
        }
        if layout_w == 0 || layout_h == 0 {
            if label.contains("CCColorLayer")
                || label.contains("CCScene")
                || label.contains("MenuLayer")
                || label.contains("FirstScene")
                || Self::is_transition_like_label(&label)
            {
                layout_w = layout_w.max(self.runtime.ui_graphics.graphics_surface_width.max(1));
                layout_h = layout_h.max(self.runtime.ui_graphics.graphics_surface_height.max(1));
            }
        }
        if layout_w == 0 || layout_h == 0 {
            let why = if effective_texture == 0 {
                "no-texture-zero-size"
            } else {
                "textured-zero-size"
            };
            return (false, why.to_string(), effective_texture, layout_w, layout_h);
        }
        let child_count = if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 };
        let is_sprite_sheet_like = label.contains("CCSpriteSheet")
            || label.contains("CCSpriteBatchNode")
            || label.contains("TextureAtlas");
        let container_only = child_count > 0
            && (((!has_image || is_sprite_sheet_like)
                && (label.contains("CCMenu")
                    || label.contains("MenuLayer")
                    || label.contains("CCScene")
                    || label.contains("CCLayer")
                    || label.contains("FirstScene")
                    || label.contains("CCNode")
                    || is_sprite_sheet_like))
                || Self::is_menu_item_class_name(&label));
        if container_only {
            return (false, "container-only".to_string(), effective_texture, layout_w, layout_h);
        }
        let draw_reason = if has_image {
            "image"
        } else if has_text {
            "text"
        } else if state.fill_rgba_explicit {
            "fill"
        } else {
            "debug-fill"
        };
        (true, draw_reason.to_string(), effective_texture, layout_w, layout_h)
    }

    fn maybe_trace_widget_selector_state(&mut self, node: u32, selector: &str, phase: &str) {
        if !self.should_trace_widget_selector(node, selector) {
            return;
        }
        let label = self.diag.object_labels.get(&node).cloned().unwrap_or_default();
        let class_name = self.objc_class_name_for_receiver(node).unwrap_or_default();
        let state = self.runtime.graphics.synthetic_sprites.get(&node).cloned().unwrap_or_default();
        let child_count = if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 };
        let texture_key = self.synthetic_texture_debug_key(self.synthetic_node_effective_texture(node)).unwrap_or_default();
        let (world_x, world_y) = self.compute_synthetic_node_world_position(node);
        let (draw_eligible, draw_reason, effective_texture, layout_w, layout_h) = self.synthetic_node_draw_debug(node);
        self.push_graph_trace(format!(
            "widget.selector phase={} node={} class={} label={} sel={} rawTex={} effTex={} key={} rawSize={}x{} layout={}x{} world=({:.1},{:.1}) visible={} entered={} touch={} children={} draw={} reason={}",
            phase,
            self.describe_ptr(node),
            if class_name.is_empty() { "<unknown>" } else { &class_name },
            if label.is_empty() { "<unknown>" } else { &label },
            selector,
            self.describe_ptr(state.texture),
            self.describe_ptr(effective_texture),
            if texture_key.is_empty() { "-" } else { &texture_key },
            state.width,
            state.height,
            layout_w,
            layout_h,
            world_x,
            world_y,
            if state.visible { "YES" } else { "NO" },
            if state.entered { "YES" } else { "NO" },
            if state.touch_enabled { "YES" } else { "NO" },
            child_count,
            if draw_eligible { "YES" } else { "NO" },
            draw_reason,
        ));
    }

    fn configure_menu_item_state(
        &mut self,
        item: u32,
        normal: u32,
        selected: u32,
        disabled: u32,
        target: u32,
        callback_sel: u32,
        selector: &str,
    ) -> String {
        let primary = self.runtime.graphics.synthetic_sprites.get(&normal).cloned();
        let selected_state = self.runtime.graphics.synthetic_sprites.get(&selected).cloned();
        let disabled_state = self.runtime.graphics.synthetic_sprites.get(&disabled).cloned();
        {
            let state = self.ensure_synthetic_sprite_state(item);
            state.visible = true;
            state.touch_enabled = true;
            state.callback_target = target;
            state.callback_selector = callback_sel;
            if let Some(src) = primary.as_ref() {
                if state.texture == 0 { state.texture = src.texture; }
                if state.width == 0 { state.width = src.width; }
                if state.height == 0 { state.height = src.height; }
            }
            if state.width == 0 {
                state.width = selected_state.as_ref().map(|v| v.width).unwrap_or_else(|| disabled_state.as_ref().map(|v| v.width).unwrap_or(0));
            }
            if state.height == 0 {
                state.height = selected_state.as_ref().map(|v| v.height).unwrap_or_else(|| disabled_state.as_ref().map(|v| v.height).unwrap_or(0));
            }
        }
        if normal != 0 {
            let _ = self.attach_child_to_node(item, normal, 0, Some(0), "menuItem.normalSprite");
            self.ensure_synthetic_sprite_state(normal).visible = true;
        }
        if selected != 0 {
            let _ = self.attach_child_to_node(item, selected, 1, Some(1), "menuItem.selectedSprite");
            self.ensure_synthetic_sprite_state(selected).visible = false;
        }
        if disabled != 0 {
            let _ = self.attach_child_to_node(item, disabled, 2, Some(2), "menuItem.disabledSprite");
            self.ensure_synthetic_sprite_state(disabled).visible = false;
        }
        let selector_name = if callback_sel != 0 {
            self.objc_read_selector_name(callback_sel).unwrap_or_else(|| format!("0x{callback_sel:08x}"))
        } else {
            "nil".to_string()
        };
        let mechanics_note = self.maybe_force_menu_item_default_variant(item, normal, selected, disabled, &selector_name);
        let variant_note = self.maybe_promote_menu_item_variant(item, normal, selected, disabled);
        let vis_note = if self.active_profile().is_achievements_selector(&selector_name) {
            Some(format!(
                "vis normal={} selected={} disabled={} scores n={} s={} d={}",
                if normal != 0 { if self.runtime.graphics.synthetic_sprites.get(&normal).map(|v| v.visible).unwrap_or(false) { "YES" } else { "NO" } } else { "nil" },
                if selected != 0 { if self.runtime.graphics.synthetic_sprites.get(&selected).map(|v| v.visible).unwrap_or(false) { "YES" } else { "NO" } } else { "nil" },
                if disabled != 0 { if self.runtime.graphics.synthetic_sprites.get(&disabled).map(|v| v.visible).unwrap_or(false) { "YES" } else { "NO" } } else { "nil" },
                self.synthetic_sprite_visual_score(normal),
                self.synthetic_sprite_visual_score(selected),
                self.synthetic_sprite_visual_score(disabled),
            ))
        } else {
            None
        };
        let mut notes = String::new();
        if let Some(note) = mechanics_note {
            notes.push_str(&format!(" mechanics=[{}]", note));
        }
        if let Some(note) = variant_note {
            notes.push_str(&format!(" variant=[{}]", note));
        }
        if let Some(note) = vis_note {
            notes.push_str(&format!(" achDebug=[{}]", note));
        }
        format!(
            "cocos menu-item {} target={} selector={} normal={} selected={} disabled={} state=[{}]{}",
            selector,
            self.describe_ptr(target),
            selector_name,
            self.describe_ptr(normal),
            self.describe_ptr(selected),
            self.describe_ptr(disabled),
            self.describe_node_graph_state(item),
            notes,
        )
    }

    fn configure_menu_from_items(&mut self, menu: u32, items: &[u32], selector: &str) -> String {
        let mut valid = Vec::new();
        for &item in items {
            if item == 0 { continue; }
            valid.push(item);
            let _ = self.attach_child_to_node(menu, item, 0, None, selector);
        }
        let mut width = 0u32;
        let mut height = 0u32;
        for item in &valid {
            if let Some(state) = self.runtime.graphics.synthetic_sprites.get(item) {
                width = width.max(state.width);
                height = height.saturating_add(state.height.max(1));
            }
        }
        {
            let state = self.ensure_synthetic_sprite_state(menu);
            state.visible = true;
            state.touch_enabled = true;
            if state.width == 0 { state.width = width.max(1); }
            if state.height == 0 { state.height = height.max(1); }
        }
        let layout_note = self.maybe_auto_layout_menu(menu);
        let ab_changed = self.abovebelow_relayout_menu_buttons(menu, selector);
        format!(
            "cocos {} items={} first={} state=[{}]{}{}",
            selector,
            valid.len(),
            valid.first().map(|ptr| self.describe_ptr(*ptr)).unwrap_or_else(|| "nil".to_string()),
            self.describe_node_graph_state(menu),
            layout_note.map(|note| format!(" layout=[{}]", note)).unwrap_or_default(),
            if ab_changed != 0 { format!(" abovebelowRelayout={}", ab_changed) } else { String::new() },
        )
    }

    fn layout_menu_children_vertically(&mut self, menu: u32, padding: f32) -> String {
        let children = self.ensure_node_children_array(menu);
        let items = self.runtime.graphics.synthetic_arrays.get(&children).map(|v| v.items.clone()).unwrap_or_default();
        let mut cursor_y = 0.0f32;
        let mut max_w = 0u32;
        for child in &items {
            let child_h = self.runtime.graphics.synthetic_sprites.get(child).map(|state| state.height.max(1)).unwrap_or(1) as f32;
            let child_w = self.runtime.graphics.synthetic_sprites.get(child).map(|state| state.width).unwrap_or(0);
            {
                let state = self.ensure_synthetic_sprite_state(*child);
                state.position_x_bits = 0.0f32.to_bits();
                state.position_y_bits = cursor_y.to_bits();
            }
            cursor_y -= child_h + padding;
            max_w = max_w.max(child_w);
        }
        let total_h = if items.is_empty() {
            0
        } else {
            let used = cursor_y.abs() - padding;
            used.max(0.0).round() as u32
        };
        {
            let state = self.ensure_synthetic_sprite_state(menu);
            if state.width == 0 { state.width = max_w.max(1); }
            if state.height == 0 { state.height = total_h.max(1); }
        }
        let ab_changed = self.abovebelow_relayout_menu_buttons(menu, "alignItemsVertically");
        format!(
            "cocos align menu vertically padding={:.1} items={} state=[{}]{}",
            padding,
            items.len(),
            self.describe_node_graph_state(menu),
            if ab_changed != 0 { format!(" abovebelowRelayout={}", ab_changed) } else { String::new() },
        )
    }

    fn configure_sprite_sheet_from_file(
        &mut self,
        sheet: u32,
        file_arg: u32,
        capacity_hint: Option<u32>,
        selector: &str,
    ) -> String {
        let name = self.guest_string_value(file_arg).unwrap_or_else(|| format!("0x{file_arg:08x}"));
        let texture = self.materialize_synthetic_texture_for_name(&name).unwrap_or(0);
        let dims = self
            .synthetic_texture_dimensions(texture)
            .unwrap_or((self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1)));
        self.diag.object_labels
            .entry(sheet)
            .or_insert_with(|| format!("CCSpriteSheet.instance(synth<'{}'>)", name));
        let atlas_note = self.configure_synthetic_texture_atlas(sheet, texture, capacity_hint, selector);
        let (out_w, out_h) = self
            .runtime.graphics.synthetic_sprites
            .get(&sheet)
            .map(|state| (state.width, state.height))
            .unwrap_or((dims.0, dims.1));
        let texture_desc = self.describe_ptr(texture);
        let path = self.runtime.fs.last_resource_path.clone().unwrap_or_default();
        let cache_key = self.synthetic_texture_debug_key(texture).unwrap_or_default();
        format!(
            "cocos {} '{}' sheetTexture={} -> {}x{} path={} cacheKey={} [{}]",
            selector,
            name,
            texture_desc,
            out_w,
            out_h,
            path,
            cache_key,
            atlas_note,
        )
    }

    fn configure_sprite_sheet_with_texture(
        &mut self,
        sheet: u32,
        texture: u32,
        capacity_hint: Option<u32>,
        selector: &str,
    ) -> String {
        let dims = self
            .synthetic_texture_dimensions(texture)
            .unwrap_or((self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1)));
        self.diag.object_labels
            .entry(sheet)
            .or_insert_with(|| "CCSpriteSheet.instance(synth)".to_string());
        let atlas_note = self.configure_synthetic_texture_atlas(sheet, texture, capacity_hint, selector);
        let (out_w, out_h) = self
            .runtime.graphics.synthetic_sprites
            .get(&sheet)
            .map(|state| (state.width, state.height))
            .unwrap_or((dims.0, dims.1));
        let texture_desc = self.describe_ptr(texture);
        let cache_key = self.synthetic_texture_debug_key(texture).unwrap_or_default();
        format!(
            "cocos {} texture={} sheet -> {}x{} cacheKey={} [{}]",
            selector,
            texture_desc,
            out_w,
            out_h,
            cache_key,
            atlas_note,
        )
    }

    fn configure_sprite_from_file(
        &mut self,
        sprite: u32,
        file_arg: u32,
        rect: Option<([u32; 4], String)>,
        selector: &str,
    ) -> String {
        let name = self.guest_string_value(file_arg).unwrap_or_else(|| format!("0x{file_arg:08x}"));
        let texture = self.materialize_synthetic_texture_for_name(&name).unwrap_or(0);
        let texture_desc = self.describe_ptr(texture);
        let fallback_dims = self.synthetic_texture_dimensions(texture)
            .unwrap_or((self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1)));
        let (path, cache_key) = self.runtime.graphics
            .synthetic_textures
            .get(&texture)
            .map(|tex| (tex.source_path.clone(), tex.cache_key.clone()))
            .unwrap_or_else(|| (self.runtime.fs.last_resource_path.clone().unwrap_or_default(), String::new()));
        match rect {
            Some((bits, source)) => {
                let w = Self::f32_from_bits(bits[2]).round().max(0.0) as u32;
                let h = Self::f32_from_bits(bits[3]).round().max(0.0) as u32;
                let (out_w, out_h) = {
                    let state = self.ensure_synthetic_sprite_state(sprite);
                    state.visible = true;
                    state.texture = texture;
                    state.texture_rect_x_bits = bits[0];
                    state.texture_rect_y_bits = bits[1];
                    state.texture_rect_w_bits = bits[2];
                    state.texture_rect_h_bits = bits[3];
                    state.texture_rect_explicit = true;
                    // Atlas/file rects must also win over stale inherited contentSize.
                    state.width = w;
                    state.height = h;
                    (state.width, state.height)
                };
                format!(
                    "cocos {} '{}' texture={} rect=({:.3},{:.3} {:.3}x{:.3}) src={} -> {}x{} path={} cacheKey={}",
                    selector,
                    name,
                    texture_desc,
                    Self::f32_from_bits(bits[0]),
                    Self::f32_from_bits(bits[1]),
                    Self::f32_from_bits(bits[2]),
                    Self::f32_from_bits(bits[3]),
                    source,
                    out_w,
                    out_h,
                    path,
                    cache_key,
                )
            }
            None => {
                let (out_w, out_h) = {
                    let state = self.ensure_synthetic_sprite_state(sprite);
                    state.visible = true;
                    state.texture = texture;
                    state.texture_rect_x_bits = 0;
                    state.texture_rect_y_bits = 0;
                    state.texture_rect_w_bits = 0;
                    state.texture_rect_h_bits = 0;
                    state.texture_rect_explicit = false;
                    state.untrimmed_w_bits = 0;
                    state.untrimmed_h_bits = 0;
                    state.untrimmed_explicit = false;
                    state.offset_x_bits = 0;
                    state.offset_y_bits = 0;
                    state.offset_explicit = false;
                    state.flip_x = false;
                    state.flip_y = false;
                    if state.width == 0 { state.width = fallback_dims.0; }
                    if state.height == 0 { state.height = fallback_dims.1; }
                    (state.width, state.height)
                };
                format!(
                    "cocos {} '{}' texture={} full-texture -> {}x{} path={} cacheKey={}",
                    selector,
                    name,
                    texture_desc,
                    out_w,
                    out_h,
                    path,
                    cache_key,
                )
            }
        }
    }

    fn lookup_cached_cocos_texture(&self, name: &str) -> Option<(String, u32)> {
        for key in Self::bundle_lookup_candidates(name) {
            if let Some(obj) = self.runtime.graphics.cocos_texture_cache_entries.get(&key).copied() {
                return Some((key, obj));
            }
        }
        None
    }

    fn maybe_trace_texture_cache_event(
        &mut self,
        request_name: &str,
        resolved_path: &str,
        cache_key: &str,
        texture_ptr: u32,
        cache_hit: bool,
    ) {
        if texture_ptr == 0 {
            return;
        }
        let previous_path = self.runtime.graphics
            .texture_ptr_last_request_path
            .get(&texture_ptr)
            .cloned()
            .unwrap_or_default();
        let previous_key = self.runtime.graphics
            .texture_ptr_last_request_key
            .get(&texture_ptr)
            .cloned()
            .unwrap_or_default();
        self.diag.trace.push(format!(
            "     ↳ texture-cache request='{}' resolvedPath={} cacheKey={} -> texture={} hit={} previousPathForPtr={} previousKeyForPtr={}",
            request_name,
            if resolved_path.is_empty() { "<none>" } else { resolved_path },
            if cache_key.is_empty() { "<none>" } else { cache_key },
            self.describe_ptr(texture_ptr),
            if cache_hit { "YES" } else { "NO" },
            if previous_path.is_empty() { "<none>" } else { &previous_path },
            if previous_key.is_empty() { "<none>" } else { &previous_key },
        ));
        self.runtime.graphics.texture_ptr_last_request_path.insert(texture_ptr, resolved_path.to_string());
        self.runtime.graphics.texture_ptr_last_request_key.insert(texture_ptr, cache_key.to_string());
    }

    fn materialize_synthetic_texture_for_name(&mut self, name: &str) -> Option<u32> {
        let resolved_hit = self.resolve_bundle_lookup_hit(name);
        let resolved_path = resolved_hit
            .as_ref()
            .map(|(_, path)| path.display().to_string())
            .unwrap_or_default();
        let resolved_key = resolved_hit
            .as_ref()
            .map(|(key, _)| key.clone())
            .unwrap_or_else(|| {
                Self::bundle_lookup_candidates(name)
                    .into_iter()
                    .next()
                    .unwrap_or_default()
            });
        if let Some((cache_key, obj)) = self.lookup_cached_cocos_texture(name) {
            if let Some(path) = resolved_hit.as_ref().map(|(_, path)| path.display().to_string()) {
                self.runtime.fs.last_resource_name = Some(name.to_string());
                self.runtime.fs.last_resource_path = Some(path);
            }
            self.maybe_trace_texture_cache_event(name, &resolved_path, &cache_key, obj, true);
            return Some(obj);
        }
        let image_obj = self.load_bundle_image_named(name)?;
        let image = self.runtime.graphics.synthetic_images.get(&image_obj)?.clone();
        let tex_obj = self.alloc_synthetic_ui_object(format!("CCTexture2D.synthetic<'{}'>", name));
        let gl_name = self.runtime.graphics.synthetic_gl_texture_name_cursor.max(1);
        self.runtime.graphics.synthetic_gl_texture_name_cursor = gl_name.saturating_add(1);
        self.runtime.graphics.synthetic_textures.insert(
            tex_obj,
            SyntheticTexture {
                width: image.width,
                height: image.height,
                gl_name,
                has_premultiplied_alpha: true,
                image: image_obj,
                source_key: name.to_string(),
                source_path: resolved_path.clone(),
                cache_key: resolved_key.clone(),
            },
        );
        for key in Self::bundle_lookup_candidates(name) {
            self.runtime.graphics.cocos_texture_cache_entries.insert(key, tex_obj);
        }
        self.maybe_trace_texture_cache_event(name, &resolved_path, &resolved_key, tex_obj, false);
        if name.eq_ignore_ascii_case("menu_background.png") {
            let image_fp = self.runtime.graphics
                .synthetic_images
                .get(&image_obj)
                .map(|img| sample_rgba_fingerprint(&img.rgba, img.width.max(1), img.height.max(1)))
                .unwrap_or_else(|| "missing-image".to_string());
            self.diag.trace.push(format!(
                "     ↳ ab-bgtex-create name={} tex={} image={} size={}x{} pma={} gl={} path={} cacheKey={} imgFp={}",
                name,
                self.describe_ptr(tex_obj),
                self.describe_ptr(image_obj),
                image.width,
                image.height,
                if true { "YES" } else { "NO" },
                gl_name,
                if resolved_path.is_empty() { "<none>" } else { &resolved_path },
                if resolved_key.is_empty() { "<none>" } else { &resolved_key },
                image_fp,
            ));
        }
        Some(tex_obj)
    }

}
