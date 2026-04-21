impl MemoryArm32Backend {
// cocos2d scene graph, scheduler, fastpaths, and synthetic action plumbing.

    fn is_cocos_director_class_name(class_name: &str) -> bool {
        let lower = class_name.to_ascii_lowercase();
        lower.contains("director")
    }

    fn is_gl_view_class_name(class_name: &str) -> bool {
        let lower = class_name.to_ascii_lowercase();
        lower.contains("eagl")
            || lower.contains("glview")
            || lower.contains("openglview")
            || lower.contains("ccgl")
            || (lower.contains("gl") && lower.contains("view"))
    }

    fn is_texture_cache_class_name(class_name: &str) -> bool {
        class_name.to_ascii_lowercase().contains("texturecache")
    }

    fn is_texture_class_name(class_name: &str) -> bool {
        let lower = class_name.to_ascii_lowercase();
        lower.contains("texture2d") || (lower.contains("texture") && lower.starts_with("cc"))
    }

    fn maybe_defer_real_cocos_audio_dispatch(
        &mut self,
        selector: &str,
        receiver: u32,
        class_name: &str,
        singleton_hint: u32,
    ) -> bool {
        let Some(class_ptr) = self.objc_lookup_class_by_name(class_name) else {
            return false;
        };
        let mut candidates = Vec::with_capacity(3);
        if receiver != 0 {
            if self.runtime.objc.objc_classes_by_ptr.contains_key(&receiver) {
                candidates.push(receiver);
            } else {
                self.objc_attach_receiver_class(receiver, class_ptr, class_name);
                candidates.push(receiver);
            }
        }
        if singleton_hint != 0 && !candidates.contains(&singleton_hint) {
            self.objc_attach_receiver_class(singleton_hint, class_ptr, class_name);
            candidates.push(singleton_hint);
        }
        if !candidates.contains(&class_ptr) {
            candidates.push(class_ptr);
        }
        for candidate in candidates {
            if let Some(imp) = self.objc_lookup_imp_for_receiver(candidate, selector) {
                self.audio_trace_push_event(format!(
                    "objc.audio.defer class={} selector={} receiver={} candidate={} imp=0x{:08x}",
                    class_name,
                    selector,
                    self.describe_ptr(receiver),
                    self.describe_ptr(candidate),
                    imp,
                ));
                return true;
            }
        }
        false
    }

    fn is_menu_item_class_name(class_name: &str) -> bool {
        class_name.contains("CCMenuItem")
    }

    fn is_menu_class_name(class_name: &str) -> bool {
        class_name.contains("CCMenu") && !class_name.contains("CCMenuItem")
    }

    fn is_label_class_name(class_name: &str) -> bool {
        let lower = class_name.to_ascii_lowercase();
        lower.contains("label")
            || lower.contains("bmfont")
            || lower.contains("bitmapfont")
            || lower.contains("fontatlas")
            || lower.contains("text")
    }

    fn synthetic_default_anchor(label: &str) -> f32 {
        if Self::is_label_class_name(label)
            || label.contains("CCSprite")
            || label.contains("CCMenuItem")
        {
            0.5
        } else {
            0.0
        }
    }

    fn synthetic_state_content_size_for_anchor(&self, receiver: u32, state: &SyntheticSpriteState) -> (f32, f32) {
        let label = self.diag.object_labels.get(&receiver).cloned().unwrap_or_default();
        let fallback_w = self.runtime.ui_graphics.graphics_surface_width.max(1) as f32;
        let fallback_h = self.runtime.ui_graphics.graphics_surface_height.max(1) as f32;
        let mut w = if state.untrimmed_explicit {
            Self::f32_from_bits(state.untrimmed_w_bits)
        } else {
            state.width as f32
        };
        let mut h = if state.untrimmed_explicit {
            Self::f32_from_bits(state.untrimmed_h_bits)
        } else {
            state.height as f32
        };
        if !w.is_finite() || w <= 0.0 {
            w = if state.width != 0 { state.width as f32 } else if state.texture_rect_explicit { Self::f32_from_bits(state.texture_rect_w_bits) } else { 0.0 };
        }
        if !h.is_finite() || h <= 0.0 {
            h = if state.height != 0 { state.height as f32 } else if state.texture_rect_explicit { Self::f32_from_bits(state.texture_rect_h_bits) } else { 0.0 };
        }
        if (!w.is_finite() || w <= 0.0) && (label.contains("CCLayer") || label.contains("CCScene") || label.contains("CCColorLayer")) {
            w = fallback_w;
        }
        if (!h.is_finite() || h <= 0.0) && (label.contains("CCLayer") || label.contains("CCScene") || label.contains("CCColorLayer")) {
            h = fallback_h;
        }
        if !w.is_finite() || w < 0.0 { w = 0.0; }
        if !h.is_finite() || h < 0.0 { h = 0.0; }
        (w, h)
    }

    fn synthetic_state_anchor_pixels(&self, receiver: u32, state: &SyntheticSpriteState) -> (f32, f32, &'static str) {
        if state.anchor_pixels_explicit {
            return (
                Self::f32_from_bits(state.anchor_pixels_x_bits),
                Self::f32_from_bits(state.anchor_pixels_y_bits),
                "anchorPointInPixels",
            );
        }
        let (content_w, content_h) = self.synthetic_state_content_size_for_anchor(receiver, state);
        let label = self.diag.object_labels.get(&receiver).cloned().unwrap_or_default();
        let default_anchor = Self::synthetic_default_anchor(&label);
        let anchor_x = if state.anchor_explicit { Self::f32_from_bits(state.anchor_x_bits) } else { default_anchor };
        let anchor_y = if state.anchor_explicit { Self::f32_from_bits(state.anchor_y_bits) } else { default_anchor };
        (anchor_x * content_w, anchor_y * content_h, "anchor*content")
    }

    fn synthetic_bmfont_parse_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
        let needle = format!("{}=", key);
        let start = line.find(&needle)? + needle.len();
        let rest = &line[start..];
        if let Some(rest) = rest.strip_prefix('"') {
            let end = rest.find('"')?;
            Some(&rest[..end])
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            Some(&rest[..end])
        }
    }

    fn synthetic_bmfont_metrics(&self, fnt_file: &str, text: &str) -> Option<(u32, u32, Option<String>)> {
        let path = self.resolve_bundle_resource_path(fnt_file, None)?;
        let body = fs::read_to_string(&path).ok()?;
        let mut line_height = 0u32;
        let mut page_file: Option<String> = None;
        let mut advances: HashMap<u32, u32> = HashMap::new();
        for raw in body.lines() {
            let line = raw.trim();
            if line.starts_with("common ") {
                if let Some(value) = Self::synthetic_bmfont_parse_value(line, "lineHeight") {
                    line_height = value.parse::<u32>().ok().unwrap_or(0);
                }
            } else if line.starts_with("page ") {
                if page_file.is_none() {
                    page_file = Self::synthetic_bmfont_parse_value(line, "file").map(|s| s.to_string());
                }
            } else if line.starts_with("char ") {
                let id = Self::synthetic_bmfont_parse_value(line, "id")
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(0);
                let advance = Self::synthetic_bmfont_parse_value(line, "xadvance")
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(0);
                if id != 0 {
                    advances.insert(id, advance);
                }
            }
        }
        let line_height = line_height.max(14);
        let default_advance = advances
            .get(&(b' ' as u32))
            .copied()
            .unwrap_or_else(|| (line_height / 2).max(6));
        let mut width = 0u32;
        let mut max_width = 0u32;
        let mut lines = 1u32;
        for ch in text.chars() {
            if ch == '\n' {
                max_width = max_width.max(width);
                width = 0;
                lines = lines.saturating_add(1);
                continue;
            }
            let advance = advances
                .get(&(ch as u32))
                .copied()
                .or_else(|| advances.get(&(ch.to_ascii_uppercase() as u32)).copied())
                .or_else(|| advances.get(&(ch.to_ascii_lowercase() as u32)).copied())
                .unwrap_or(default_advance);
            width = width.saturating_add(advance.max(1));
        }
        max_width = max_width.max(width);
        Some((max_width.max(1), line_height.saturating_mul(lines.max(1)), page_file))
    }

    fn install_synthetic_bmfont_node(&mut self, object: u32, text: &str, fnt_file: &str, preserve_existing_size: bool) -> String {
        let clean_text = text.replace('\0', "");
        let display_fnt = if fnt_file.is_empty() { "<unknown>" } else { fnt_file };
        let label = format!("CCLabelBMFont.instance(synth<'{}'>)", display_fnt);
        let _ = self.ensure_string_backing(object, label.clone(), &clean_text);
        let (text_w, text_h, page_file) = self
            .synthetic_bmfont_metrics(fnt_file, &clean_text)
            .unwrap_or_else(|| {
                let scale = 2u32;
                let (w, h) = Self::synthetic_text_dimensions_5x7(&clean_text, scale);
                (w.max(1), h.max(1), None)
            });
        let page_texture = page_file
            .as_ref()
            .and_then(|name| self.materialize_synthetic_texture_for_name(name));
        let texture_desc = page_texture
            .map(|ptr| self.describe_ptr(ptr))
            .unwrap_or_else(|| "nil".to_string());
        let page_desc = page_file.clone().unwrap_or_default().replace('\n', "\\n");
        let state = self.ensure_synthetic_sprite_state(object);
        state.visible = true;
        if !preserve_existing_size || state.width == 0 {
            state.width = text_w.max(state.width).max(1);
        }
        if !preserve_existing_size || state.height == 0 {
            state.height = text_h.max(state.height).max(1);
        }
        if !state.anchor_explicit {
            state.anchor_x_bits = 0.5f32.to_bits();
            state.anchor_y_bits = 0.5f32.to_bits();
        }
        if state.texture == 0 {
            if let Some(texture) = page_texture {
                state.texture = texture;
            }
        }
        format!(
            "bmfont node <- '{}' fntFile='{}' size={}x{} preserveExisting={} texture={} page={}",
            clean_text.replace('\n', "\\n"),
            display_fnt.replace('\n', "\\n"),
            state.width,
            state.height,
            if preserve_existing_size { "YES" } else { "NO" },
            texture_desc,
            page_desc,
        )
    }

    fn synthetic_text_scale_for_height(height: u32) -> u32 {
        match height {
            0..=10 => 1,
            11..=18 => 2,
            19..=28 => 3,
            _ => 4,
        }
    }

    fn synthetic_text_dimensions_5x7(text: &str, scale: u32) -> (u32, u32) {
        let scale = scale.max(1);
        let lines: Vec<&str> = text.split('\n').collect();
        let max_chars = lines.iter().map(|line| line.chars().count()).max().unwrap_or(0) as u32;
        let width = if max_chars == 0 { 0 } else { max_chars.saturating_mul(6).saturating_sub(1).saturating_mul(scale) };
        let line_count = lines.len().max(1) as u32;
        let height = line_count.saturating_mul(7 * scale).saturating_add(line_count.saturating_sub(1) * scale);
        (width, height)
    }

    fn decode_cccolor3b(arg: u32) -> [u8; 3] {
        [
            (arg & 0xff) as u8,
            ((arg >> 8) & 0xff) as u8,
            ((arg >> 16) & 0xff) as u8,
        ]
    }

    fn synthetic_font_name_key(name: &str) -> String {
        name.chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .flat_map(|ch| ch.to_lowercase())
            .collect()
    }

    fn synthetic_text_font_size_from_bits(bits: u32) -> Option<f32> {
        let value = Self::f32_from_bits(bits);
        if value.is_finite() && value > 0.0 && value <= 256.0 {
            Some(value)
        } else {
            None
        }
    }

    fn bundled_font_paths(&self) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for path in self.runtime.fs.bundle_resource_index.values() {
            let ext = path.extension().and_then(|v| v.to_str()).map(|v| v.to_ascii_lowercase()).unwrap_or_default();
            if matches!(ext.as_str(), "ttf" | "otf" | "ttc") {
                out.push(path.clone());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn host_font_search_roots() -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        if cfg!(target_os = "windows") {
            if let Some(windir) = std::env::var_os("WINDIR") {
                out.push(std::path::PathBuf::from(windir).join("Fonts"));
            }
            out.push(std::path::PathBuf::from(r"C:\Windows\Fonts"));
        } else if cfg!(target_os = "macos") {
            out.push(std::path::PathBuf::from("/System/Library/Fonts"));
            out.push(std::path::PathBuf::from("/Library/Fonts"));
            if let Some(home) = std::env::var_os("HOME") {
                out.push(std::path::PathBuf::from(home).join("Library").join("Fonts"));
            }
        } else {
            out.push(std::path::PathBuf::from("/usr/share/fonts"));
            out.push(std::path::PathBuf::from("/usr/local/share/fonts"));
            if let Some(home) = std::env::var_os("HOME") {
                out.push(std::path::PathBuf::from(&home).join(".fonts"));
                out.push(std::path::PathBuf::from(home).join(".local").join("share").join("fonts"));
            }
        }
        out.retain(|p| p.is_dir());
        out.sort();
        out.dedup();
        out
    }

    fn host_font_paths() -> Vec<std::path::PathBuf> {
        fn collect(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let mut stack = vec![root.to_path_buf()];
            let mut seen = std::collections::HashSet::new();
            while let Some(dir) = stack.pop() {
                if !seen.insert(dir.clone()) {
                    continue;
                }
                let entries = match fs::read_dir(&dir) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                        continue;
                    }
                    let ext = path
                        .extension()
                        .and_then(|v| v.to_str())
                        .map(|v| v.to_ascii_lowercase())
                        .unwrap_or_default();
                    if matches!(ext.as_str(), "ttf" | "otf" | "ttc") {
                        out.push(path);
                    }
                }
            }
        }

        let mut out = Vec::new();
        for root in Self::host_font_search_roots() {
            collect(&root, &mut out);
        }
        out.sort();
        out.dedup();
        out
    }

    fn font_request_style_flags(requested_name: &str) -> (bool, bool) {
        let lower = requested_name.to_ascii_lowercase();
        let wants_bold = ["bold", "black", "heavy", "semibold", "semi bold", "demibold", "demi bold"]
            .iter()
            .any(|token| lower.contains(token));
        let wants_italic = ["italic", "oblique"].iter().any(|token| lower.contains(token));
        (wants_bold, wants_italic)
    }

    fn font_name_keys_for_path(path: &std::path::Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut push_key = |value: &str| {
            let key = Self::synthetic_font_name_key(value);
            if !key.is_empty() && seen.insert(key.clone()) {
                out.push(key);
            }
        };

        let stem = path.file_stem().and_then(|v| v.to_str()).unwrap_or_default();
        push_key(stem);

        let bytes = match fs::read(path) {
            Ok(v) => v,
            Err(_) => return out,
        };
        let face_count = ttf_parser::fonts_in_collection(&bytes).unwrap_or(1);
        for face_index in 0..face_count {
            let face = match ttf_parser::Face::parse(&bytes, face_index) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let mut family_names: Vec<String> = Vec::new();
            let mut subfamily_names: Vec<String> = Vec::new();
            let mut typo_family_names: Vec<String> = Vec::new();
            let mut typo_subfamily_names: Vec<String> = Vec::new();
            for record in face.names() {
                let name_id = record.name_id;
                let Some(candidate) = record.to_string() else { continue; };
                match name_id {
                    ttf_parser::name_id::FULL_NAME | ttf_parser::name_id::POST_SCRIPT_NAME => {
                        push_key(&candidate);
                    }
                    ttf_parser::name_id::FAMILY => {
                        push_key(&candidate);
                        family_names.push(candidate);
                    }
                    ttf_parser::name_id::TYPOGRAPHIC_FAMILY => {
                        push_key(&candidate);
                        typo_family_names.push(candidate);
                    }
                    ttf_parser::name_id::SUBFAMILY => {
                        push_key(&candidate);
                        subfamily_names.push(candidate);
                    }
                    ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY => {
                        push_key(&candidate);
                        typo_subfamily_names.push(candidate);
                    }
                    _ => {}
                }
            }
            for family in family_names.iter().chain(typo_family_names.iter()) {
                for style in subfamily_names.iter().chain(typo_subfamily_names.iter()) {
                    let combined = format!("{} {}", family, style).trim().to_string();
                    push_key(&combined);
                }
            }
        }
        out
    }

    fn font_style_flags_for_path(path: &std::path::Path) -> (bool, bool) {
        let mut is_bold = false;
        let mut is_italic = false;
        let bytes = match fs::read(path) {
            Ok(v) => v,
            Err(_) => return (false, false),
        };
        let face_count = ttf_parser::fonts_in_collection(&bytes).unwrap_or(1);
        for face_index in 0..face_count {
            let face = match ttf_parser::Face::parse(&bytes, face_index) {
                Ok(v) => v,
                Err(_) => continue,
            };
            for record in face.names() {
                let name_id = record.name_id;
                if !(name_id == ttf_parser::name_id::FULL_NAME
                    || name_id == ttf_parser::name_id::POST_SCRIPT_NAME
                    || name_id == ttf_parser::name_id::SUBFAMILY
                    || name_id == ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY)
                {
                    continue;
                }
                let Some(candidate) = record.to_string() else { continue; };
                let lower = candidate.to_ascii_lowercase();
                if ["bold", "black", "heavy", "semibold", "semi bold", "demibold", "demi bold"]
                    .iter()
                    .any(|token| lower.contains(token))
                {
                    is_bold = true;
                }
                if ["italic", "oblique"].iter().any(|token| lower.contains(token)) {
                    is_italic = true;
                }
            }
        }
        (is_bold, is_italic)
    }

    fn font_name_matches_path(path: &std::path::Path, want: &str, allow_partial: bool) -> bool {
        Self::font_name_keys_for_path(path)
            .into_iter()
            .any(|key| key == want || (allow_partial && (key.contains(want) || want.contains(&key))))
    }

    fn font_match_score(path: &std::path::Path, requested_name: &str, allow_partial: bool) -> Option<i64> {
        let want = Self::synthetic_font_name_key(requested_name);
        if want.is_empty() {
            return None;
        }
        let (wants_bold, wants_italic) = Self::font_request_style_flags(requested_name);
        let (is_bold, is_italic) = Self::font_style_flags_for_path(path);
        let mut best = None;
        for key in Self::font_name_keys_for_path(path) {
            let mut score = if key == want {
                1_000_000
            } else {
                if !allow_partial || !(key.contains(&want) || want.contains(&key)) {
                    continue;
                }
                let overlap = key.len().min(want.len()) as i64;
                let penalty = (key.len() as i64 - want.len() as i64).abs();
                100_000 + overlap * 100 - penalty
            };
            if wants_bold != is_bold {
                score -= 50_000;
            }
            if wants_italic != is_italic {
                score -= 25_000;
            }
            best = Some(best.unwrap_or(i64::MIN).max(score));
        }
        best
    }

    fn resolve_font_from_candidates(
        fonts: &[std::path::PathBuf],
        requested_name: &str,
        allow_partial: bool,
    ) -> Option<std::path::PathBuf> {
        fonts
            .iter()
            .filter_map(|path| {
                let score = Self::font_match_score(path.as_path(), requested_name, allow_partial)?;
                let path_len = path.to_string_lossy().len() as i64;
                Some((score, -path_len, path.clone()))
            })
            .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
            .map(|(_, _, path)| path)
    }

    fn font_alias_candidates(requested_name: &str) -> Vec<String> {
        let lower = requested_name.trim().to_ascii_lowercase();
        let mut out: Vec<String> = Vec::new();
        if lower.is_empty() {
            return out;
        }
        out.push(requested_name.trim().to_string());

        if lower.contains("american typewriter") {
            out.extend([
                "Bookman Old Style",
                "Rockwell",
                "Baskerville",
                "Cambria",
                "Constantia",
                "Georgia",
                "Palatino Linotype",
                "Times New Roman",
                "Liberation Serif",
                "Nimbus Roman",
                "DejaVu Serif",
            ].into_iter().map(str::to_string));
        } else if lower.contains("typewriter") {
            out.extend([
                "American Typewriter",
                "Bookman Old Style",
                "Rockwell",
                "Georgia",
                "Palatino Linotype",
                "Cambria",
                "Constantia",
                "Baskerville",
                "Times New Roman",
                "Liberation Serif",
                "Nimbus Roman",
                "DejaVu Serif",
                "Courier New",
                "Courier",
            ].into_iter().map(str::to_string));
        } else if lower.contains("courier") || lower.contains("mono") {
            out.extend([
                "Courier New",
                "Courier",
                "Consolas",
                "Liberation Mono",
                "Nimbus Mono PS",
                "DejaVu Sans Mono",
            ].into_iter().map(str::to_string));
        } else if lower.contains("times")
            || lower.contains("georgia")
            || lower.contains("garamond")
            || lower.contains("palatino")
            || lower.contains("baskerville")
            || lower.contains("serif")
        {
            out.extend([
                "Times New Roman",
                "Georgia",
                "Palatino Linotype",
                "Cambria",
                "Constantia",
                "Liberation Serif",
                "Nimbus Roman",
                "DejaVu Serif",
            ].into_iter().map(str::to_string));
        } else {
            out.extend([
                "Helvetica",
                "Arial",
                "Segoe UI",
                "Liberation Sans",
                "Nimbus Sans",
                "DejaVu Sans",
            ].into_iter().map(str::to_string));
        }

        let mut dedup = std::collections::HashSet::new();
        out.into_iter()
            .filter(|name| dedup.insert(Self::synthetic_font_name_key(name)))
            .collect()
    }

    fn resolve_bundled_font_path(&self, requested_name: Option<&str>) -> Option<std::path::PathBuf> {
        let bundled = self.bundled_font_paths();
        let host = Self::host_font_paths();

        if let Some(name) = requested_name.map(str::trim).filter(|name| !name.is_empty()) {
            // Exact name resolution order matters:
            // 1) host fonts (system font request),
            // 2) bundled fonts with the *same* explicit name (custom in-app font),
            // 3) host-side family/style fallbacks.
            //
            // Do not resolve aliased system requests against unrelated bundled fonts.
            // Example: asking for "American Typewriter" must not silently pick a bundled
            // "cour.ttf" or "faktos.ttf" just because they happen to be the only TTFs in the app.
            if let Some(path) = Self::resolve_font_from_candidates(&host, name, false) {
                return Some(path);
            }
            if let Some(path) = Self::resolve_font_from_candidates(&bundled, name, false) {
                return Some(path);
            }
            if let Some(path) = Self::resolve_font_from_candidates(&host, name, true) {
                return Some(path);
            }
            if let Some(path) = Self::resolve_font_from_candidates(&bundled, name, true) {
                return Some(path);
            }

            let aliases = Self::font_alias_candidates(name);
            for candidate in aliases.iter().skip(1) {
                if let Some(path) = Self::resolve_font_from_candidates(&host, candidate, false) {
                    return Some(path);
                }
            }
            for candidate in aliases.iter().skip(1) {
                if let Some(path) = Self::resolve_font_from_candidates(&host, candidate, true) {
                    return Some(path);
                }
            }
            return None;
        }

        for candidate in ["Helvetica", "Arial", "Segoe UI", "Liberation Sans", "DejaVu Sans"] {
            if let Some(path) = Self::resolve_font_from_candidates(&host, candidate, false) {
                return Some(path);
            }
        }
        for candidate in ["Helvetica", "Arial", "Segoe UI", "Liberation Sans", "DejaVu Sans"] {
            if let Some(path) = Self::resolve_font_from_candidates(&host, candidate, true) {
                return Some(path);
            }
        }

        if bundled.len() == 1 {
            bundled.into_iter().next()
        } else {
            None
        }
    }

    fn rasterize_text_rgba_bundled_font(
        &self,
        text: &str,
        requested_font_name: Option<&str>,
        font_size: f32,
    ) -> Option<(Vec<u8>, u32, u32, String)> {
        let font_path = self.resolve_bundled_font_path(requested_font_name)?;
        let font_bytes = fs::read(&font_path).ok()?;
        let font = ab_glyph::FontRef::try_from_slice(&font_bytes).ok()?;
        let px = font_size.clamp(6.0, 96.0);
        let scale = ab_glyph::PxScale::from(px);
        let scaled = ab_glyph::Font::as_scaled(&font, scale);
        let ascent = ab_glyph::ScaleFont::ascent(&scaled);
        let descent = ab_glyph::ScaleFont::descent(&scaled);
        let line_gap = ab_glyph::ScaleFont::line_gap(&scaled);
        let baseline_advance = (ascent - descent + line_gap).ceil().max(1.0);
        let lines: Vec<&str> = text.split('\n').collect();
        let mut max_width = 0.0f32;
        for line in &lines {
            let mut width = 0.0f32;
            let mut prev = None;
            for ch in line.chars() {
                let id = ab_glyph::ScaleFont::glyph_id(&scaled, ch);
                if let Some(prev_id) = prev {
                    width += ab_glyph::ScaleFont::kern(&scaled, prev_id, id);
                }
                width += ab_glyph::ScaleFont::h_advance(&scaled, id);
                prev = Some(id);
            }
            max_width = max_width.max(width);
        }
        let padding = (px * 0.3).ceil().max(2.0) as u32;
        let width = max_width.ceil().max(1.0) as u32 + padding.saturating_mul(2);
        let height = (baseline_advance * lines.len().max(1) as f32).ceil().max(1.0) as u32 + padding.saturating_mul(2);
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        for (line_idx, line) in lines.iter().enumerate() {
            let mut pen_x = padding as f32;
            let baseline_y = padding as f32 + ascent + baseline_advance * line_idx as f32;
            let mut prev = None;
            for ch in line.chars() {
                let id = ab_glyph::ScaleFont::glyph_id(&scaled, ch);
                if let Some(prev_id) = prev {
                    pen_x += ab_glyph::ScaleFont::kern(&scaled, prev_id, id);
                }
                let glyph = id.with_scale_and_position(scale, ab_glyph::point(pen_x, baseline_y));
                if let Some(outlined) = ab_glyph::Font::outline_glyph(&font, glyph) {
                    let bounds = outlined.px_bounds();
                    outlined.draw(|gx, gy, coverage| {
                        let x = gx as i32 + bounds.min.x.floor() as i32;
                        let y = gy as i32 + bounds.min.y.floor() as i32;
                        if x < 0 || y < 0 {
                            return;
                        }
                        let xu = x as u32;
                        let yu = y as u32;
                        if xu >= width || yu >= height {
                            return;
                        }
                        let alpha = (255.0 * coverage).round().clamp(0.0, 255.0) as u8;
                        Self::ui_blend_pixel(&mut rgba, width, height, x, y, [255, 255, 255, alpha]);
                    });
                }
                pen_x += ab_glyph::ScaleFont::h_advance(&scaled, id);
                prev = Some(id);
            }
        }
        Some((
            rgba,
            width.max(1),
            height.max(1),
            font_path.file_name().and_then(|v| v.to_str()).unwrap_or_default().to_string(),
        ))
    }

    fn install_synthetic_text_texture(&mut self, rgba: Vec<u8>, width: u32, height: u32, source_desc: &str) -> u32 {
        let image_obj = self.alloc_synthetic_ui_object(format!("UIImage.synthetic.text<'{}'>", source_desc));
        self.runtime.graphics.synthetic_images.insert(image_obj, SyntheticImage {
            width: width.max(1),
            height: height.max(1),
            rgba,
        });
        let tex_obj = self.alloc_synthetic_ui_object(format!("CCTexture2D.synthetic.text<'{}'>", source_desc));
        let gl_name = self.runtime.graphics.synthetic_gl_texture_name_cursor.max(1);
        self.runtime.graphics.synthetic_gl_texture_name_cursor = gl_name.saturating_add(1);
        self.runtime.graphics.synthetic_textures.insert(
            tex_obj,
            SyntheticTexture {
                width: width.max(1),
                height: height.max(1),
                gl_name,
                has_premultiplied_alpha: true,
                image: image_obj,
                source_key: format!("__synthetic_text__:{}", source_desc),
                source_path: String::new(),
                cache_key: format!("__synthetic_text__:{}:{}x{}", source_desc, width.max(1), height.max(1)),
            },
        );
        tex_obj
    }

    fn install_synthetic_text_node(
        &mut self,
        object: u32,
        class_hint: &str,
        text: &str,
        preserve_existing_size: bool,
        font_name: Option<String>,
        font_size_bits: Option<u32>,
    ) -> String {
        let clean_text = text.replace('\0', "");
        let label = if class_hint.is_empty() {
            "CCLabel.instance(synth)".to_string()
        } else {
            format!("{}.instance(synth)", class_hint)
        };
        let previous_backing = self.runtime.heap.synthetic_string_backing.get(&object).cloned();
        let _ = self.ensure_string_backing(object, label.clone(), &clean_text);
        if let Some(backing) = self.runtime.heap.synthetic_string_backing.get_mut(&object) {
            if let Some(name) = font_name.clone().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
                backing.font_name = Some(name);
            } else if let Some(prev_name) = previous_backing.as_ref().and_then(|v| v.font_name.clone()) {
                backing.font_name = Some(prev_name);
            }
            if let Some(bits) = font_size_bits {
                backing.font_size_bits = bits;
                backing.font_size_explicit = true;
            } else if let Some(prev) = previous_backing.as_ref().filter(|v| v.font_size_explicit) {
                backing.font_size_bits = prev.font_size_bits;
                backing.font_size_explicit = true;
            }
        }
        let backing = self.runtime.heap.synthetic_string_backing.get(&object).cloned();
        let effective_font_name = backing.as_ref().and_then(|v| v.font_name.clone());
        let effective_font_size = backing
            .as_ref()
            .and_then(|v| if v.font_size_explicit { Self::synthetic_text_font_size_from_bits(v.font_size_bits) } else { None })
            .unwrap_or(14.0);
        let text_texture = self.rasterize_text_rgba_bundled_font(&clean_text, effective_font_name.as_deref(), effective_font_size);
        let fallback_scale = Self::synthetic_text_scale_for_height(effective_font_size.round().max(1.0) as u32);
        let (fallback_w, fallback_h) = Self::synthetic_text_dimensions_5x7(&clean_text, fallback_scale);
        let (text_w, text_h, texture_note, texture_obj) = if let Some((rgba, w, h, source_desc)) = text_texture {
            let tex_obj = self.install_synthetic_text_texture(rgba, w, h, &source_desc);
            (
                w.max(1),
                h.max(1),
                format!(" texture={} font='{}'", self.describe_ptr(tex_obj), source_desc.replace('\n', "\\n")),
                Some(tex_obj),
            )
        } else {
            (fallback_w.max(1), fallback_h.max(1), String::new(), None)
        };
        let state = self.ensure_synthetic_sprite_state(object);
        state.visible = true;
        if !preserve_existing_size || state.width == 0 {
            state.width = text_w.max(state.width).max(1);
        }
        if !preserve_existing_size || state.height == 0 {
            state.height = text_h.max(state.height).max(1);
        }
        if !state.anchor_explicit {
            state.anchor_x_bits = 0;
            state.anchor_y_bits = 0;
        }
        if let Some(tex_obj) = texture_obj {
            state.texture = tex_obj;
        }
        format!(
            "text node <- '{}' class={} size={}x{} preserveExisting={} fontName={} fontSize={:.2}{}",
            clean_text.replace('\n', "\\n"),
            if class_hint.is_empty() { "<unknown>" } else { class_hint },
            state.width,
            state.height,
            if preserve_existing_size { "YES" } else { "NO" },
            effective_font_name.unwrap_or_default().replace('\n', "\\n"),
            effective_font_size,
            texture_note,
        )
    }

    fn is_plausible_guest_cocos_object_ptr(&self, value: u32) -> bool {
        if value == 0 {
            return true;
        }
        self.find_region(value, 4).is_some()
            || self.runtime.graphics.synthetic_sprites.contains_key(&value)
            || self.runtime.graphics.synthetic_textures.contains_key(&value)
            || self.diag.object_labels.contains_key(&value)
    }

    // Some older cocos2d titles ship with a concrete CCDirector guest ivar layout that the
    // HLE runtime needs to mirror into. Keep the layout definition in the profile layer so generic cocos
    // plumbing does not hardcode one game's offsets forever.
    fn sync_cocos_director_guest_ivars(&mut self, reason: &str) {
        if self.runtime.ui_cocos.director_ivar_sync_inflight {
            return;
        }

        let director = self.runtime.ui_cocos.cocos_director;
        if director == 0 {
            return;
        }

        let Some(layout) = self.cocos_director_ivar_layout_for(director) else {
            return;
        };

        self.runtime.ui_cocos.director_ivar_sync_inflight = true;

        let mut mirrored = Vec::new();
        let mut adopted = Vec::new();

        let open_gl_view_addr = director.wrapping_add(layout.open_gl_view_offset);
        let running_scene_addr = director.wrapping_add(layout.running_scene_offset);
        let next_scene_addr = director.wrapping_add(layout.next_scene_offset);
        let effect_scene_addr = layout.effect_scene_offset.map(|offset| director.wrapping_add(offset));

        let guest_open_gl_view = self.read_u32_le(open_gl_view_addr).unwrap_or(0);
        let guest_running_scene = self.read_u32_le(running_scene_addr).unwrap_or(0);
        let guest_next_scene = self.read_u32_le(next_scene_addr).unwrap_or(0);
        let guest_effect_scene = effect_scene_addr.and_then(|addr| self.read_u32_le(addr).ok()).unwrap_or(0);

        if guest_open_gl_view != 0
            && guest_open_gl_view != self.runtime.ui_cocos.opengl_view
            && self.is_plausible_guest_cocos_object_ptr(guest_open_gl_view)
        {
            self.runtime.ui_cocos.opengl_view = guest_open_gl_view;
            self.diag.object_labels
                .entry(guest_open_gl_view)
                .or_insert_with(|| "EAGLView.instance(guest)".to_string());
            adopted.push(format!("openGLView->{}", self.describe_ptr(guest_open_gl_view)));
        }

        if guest_next_scene != self.runtime.ui_cocos.next_scene
            && self.is_plausible_guest_cocos_object_ptr(guest_next_scene)
        {
            self.runtime.ui_cocos.next_scene = guest_next_scene;
            if guest_next_scene != 0 {
                self.diag.object_labels
                    .entry(guest_next_scene)
                    .or_insert_with(|| "CCScene.pending(guest)".to_string());
            }
            adopted.push(format!("nextScene->{}", self.describe_ptr(guest_next_scene)));
        }

        if guest_effect_scene != self.runtime.ui_cocos.effect_scene
            && self.is_plausible_guest_cocos_object_ptr(guest_effect_scene)
        {
            if guest_effect_scene != 0 {
                self.diag.object_labels
                    .entry(guest_effect_scene)
                    .or_insert_with(|| "CCScene.effect(guest)".to_string());
            }
            self.set_effect_scene(guest_effect_scene, reason);
            adopted.push(format!("effectScene->{}", self.describe_ptr(guest_effect_scene)));
        }

        let guest_running_scene_is_stale_transition = self.guest_transition_scene_is_stale(guest_running_scene);
        if guest_running_scene != 0
            && guest_running_scene != self.runtime.ui_cocos.running_scene
            && self.is_plausible_guest_cocos_object_ptr(guest_running_scene)
        {
            self.diag.object_labels
                .entry(guest_running_scene)
                .or_insert_with(|| "CCScene.running(guest)".to_string());
            if guest_running_scene_is_stale_transition {
                let destination = self.runtime.graphics.synthetic_splash_destinations.get(&guest_running_scene).copied().unwrap_or(0);
                self.diag.trace.push(format!(
                    "     ↳ hle director.ivars.sync skip stale guest-runningScene={} destination={} authoritative={} reason={}",
                    self.describe_ptr(guest_running_scene),
                    self.describe_ptr(destination),
                    self.describe_ptr(self.runtime.ui_cocos.running_scene),
                    reason,
                ));
            } else {
                let entered = self.adopt_guest_running_scene_without_lifecycle(guest_running_scene, reason);
                if self.runtime.ui_cocos.next_scene == guest_running_scene {
                    self.runtime.ui_cocos.next_scene = 0;
                }
                adopted.push(format!(
                    "runningScene->{} enterProp={}",
                    self.describe_ptr(guest_running_scene),
                    entered,
                ));
                self.clear_effect_scene_if_redundant(guest_running_scene, reason);
            }
        }

        let desired_next_scene = if self.runtime.ui_cocos.next_scene != 0 {
            self.runtime.ui_cocos.next_scene
        } else if guest_next_scene != 0 {
            guest_next_scene
        } else {
            self.runtime.graphics.synthetic_splash_destinations
                .get(&self.runtime.ui_cocos.running_scene)
                .copied()
                .unwrap_or(0)
        };

        let bindings = [
            ("openGLView_", open_gl_view_addr, self.runtime.ui_cocos.opengl_view, guest_open_gl_view),
            ("runningScene_", running_scene_addr, self.runtime.ui_cocos.running_scene, guest_running_scene),
        ];

        for (name, addr, value, guest_value) in bindings {
            if value == 0 || guest_value == value {
                continue;
            }
            let stale_transition_binding = name == "runningScene_" && self.guest_transition_scene_is_stale(guest_value);
            if guest_value != 0 && self.is_plausible_guest_cocos_object_ptr(guest_value) && !stale_transition_binding {
                continue;
            }
            if self.write_u32_le(addr, value).is_ok() {
                mirrored.push(format!("{}<-{}", name, self.describe_ptr(value)));
            }
        }

        if guest_next_scene != desired_next_scene {
            let should_mirror_next_scene = desired_next_scene != 0
                || guest_next_scene == 0
                || !self.is_plausible_guest_cocos_object_ptr(guest_next_scene);
            if should_mirror_next_scene && self.write_u32_le(next_scene_addr, desired_next_scene).is_ok() {
                mirrored.push(format!("nextScene<-{}", self.describe_ptr(desired_next_scene)));
            }
        }

        if let Some(effect_scene_addr) = effect_scene_addr {
            let desired_effect_scene = self.runtime.ui_cocos.effect_scene;
            if guest_effect_scene != desired_effect_scene {
                let should_mirror_effect_scene = desired_effect_scene != 0
                    || guest_effect_scene == 0
                    || !self.is_plausible_guest_cocos_object_ptr(guest_effect_scene);
                if should_mirror_effect_scene && self.write_u32_le(effect_scene_addr, desired_effect_scene).is_ok() {
                    mirrored.push(format!("effectScene<-{}", self.describe_ptr(desired_effect_scene)));
                }
            }
        }

        if !adopted.is_empty() || !mirrored.is_empty() {
            let mut details = Vec::new();
            if !adopted.is_empty() {
                details.push(format!("guest:{}", adopted.join(" ")));
            }
            if !mirrored.is_empty() {
                details.push(format!("mirror:{}", mirrored.join(" ")));
            }
            self.diag.trace.push(format!(
                "     ↳ hle director.ivars.sync director={} reason={} {}",
                self.describe_ptr(director),
                reason,
                details.join(" "),
            ));
        }

        self.runtime.ui_cocos.director_ivar_sync_inflight = false;
    }

    fn ensure_cocos_director_object(&mut self, receiver: u32, class_name: Option<&str>) -> u32 {
        if receiver != 0 && !self.runtime.objc.objc_classes_by_ptr.contains_key(&receiver) {
            self.runtime.ui_cocos.cocos_director = receiver;
            let label = class_name
                .map(|name| format!("{}.instance(guest)", name))
                .unwrap_or_else(|| "CCDirector.instance(guest)".to_string());
            self.diag.object_labels.entry(receiver).or_insert(label);
            self.sync_cocos_director_guest_ivars("ensure-director:guest");
            return receiver;
        }
        if self.runtime.ui_cocos.cocos_director != 0 {
            self.sync_cocos_director_guest_ivars("ensure-director:cached");
            return self.runtime.ui_cocos.cocos_director;
        }
        let label = class_name
            .map(|name| format!("{}.synthetic#0", name))
            .unwrap_or_else(|| "CCDirector.synthetic#0".to_string());
        let obj = if receiver != 0 && self.runtime.objc.objc_classes_by_ptr.contains_key(&receiver) {
            class_name
                .and_then(|name| self.objc_materialize_instance(receiver, name))
                .unwrap_or_else(|| self.alloc_synthetic_ui_object(label.clone()))
        } else {
            self.alloc_synthetic_ui_object(label.clone())
        };
        self.diag.object_labels.entry(obj).or_insert(label);
        self.runtime.ui_cocos.cocos_director = obj;
        self.sync_cocos_director_guest_ivars("ensure-director:new");
        obj
    }

    fn adopt_real_cocos_director(&mut self, director: u32, origin: &str) {
        if director == 0 {
            return;
        }
        let previous = self.runtime.ui_cocos.cocos_director;
        self.runtime.ui_cocos.cocos_director = director;
        let class_name = self.objc_receiver_class_name_hint(director).unwrap_or_default();
        let label = if class_name.is_empty() {
            "CCDirector.instance(guest)".to_string()
        } else {
            format!("{}.instance(guest)", class_name)
        };
        self.diag.object_labels.entry(director).or_insert(label);
        self.diag.trace.push(format!(
            "     ↳ hle director.adopt real-return director={} previous={} origin={}",
            self.describe_ptr(director),
            self.describe_ptr(previous),
            origin,
        ));
    }

    fn adopt_real_cocos_view(&mut self, view: u32, origin: &str) {
        if view == 0 {
            return;
        }
        let previous = self.runtime.ui_cocos.opengl_view;
        self.runtime.ui_cocos.opengl_view = view;
        let class_name = self.objc_receiver_class_name_hint(view).unwrap_or_default();
        let label = if class_name.is_empty() {
            "EAGLView.instance(guest)".to_string()
        } else {
            format!("{}.instance(guest)", class_name)
        };
        self.diag.object_labels.entry(view).or_insert(label);
        if self.runtime.ui_objects.first_responder == 0 || self.runtime.ui_objects.first_responder == self.runtime.ui_objects.root_controller {
            self.runtime.ui_objects.first_responder = view;
        }
        self.diag.trace.push(format!(
            "     ↳ hle director.view adopt view={} previous={} origin={}",
            self.describe_ptr(view),
            self.describe_ptr(previous),
            origin,
        ));
    }

    fn ensure_cocos_opengl_view(&mut self) -> u32 {
        if self.runtime.ui_cocos.opengl_view == 0 {
            let view = self.alloc_synthetic_ui_object("EAGLView.synthetic#0");
            self.diag.object_labels.entry(view).or_insert_with(|| "EAGLView.synthetic#0".to_string());
            self.runtime.ui_cocos.opengl_view = view;
        }
        let view = self.runtime.ui_cocos.opengl_view;
        self.ui_set_frame_bits(view, self.ui_surface_rect_bits());
        self.ui_set_bounds_bits(view, Self::ui_rect_size_bits(self.ui_surface_rect_bits()));
        self.ui_set_content_scale_bits(view, 1.0f32.to_bits());
        self.ui_attach_layer_to_view(view, self.runtime.ui_graphics.eagl_layer);
        view
    }

    fn bootstrap_cocos_window_path(&mut self, reason: &str) {
        self.bootstrap_synthetic_runloop();
        self.bootstrap_synthetic_graphics();
        self.runtime.ui_runtime.app_active = true;
        self.runtime.ui_runtime.window_visible = true;
        self.runtime.ui_graphics.graphics_context_current = true;
        self.runtime.ui_graphics.graphics_layer_attached = true;
        self.runtime.ui_graphics.graphics_surface_ready = true;
        self.runtime.ui_graphics.graphics_framebuffer_complete = true;
        self.runtime.ui_graphics.graphics_viewport_ready = true;
        if self.runtime.ui_graphics.graphics_viewport_width == 0 {
            self.runtime.ui_graphics.graphics_viewport_width = self.runtime.ui_graphics.graphics_surface_width.max(1);
        }
        if self.runtime.ui_graphics.graphics_viewport_height == 0 {
            self.runtime.ui_graphics.graphics_viewport_height = self.runtime.ui_graphics.graphics_surface_height.max(1);
        }
        self.runtime.ui_cocos.display_link_armed = true;
        self.runtime.ui_runtime.runloop_live = true;
        self.recalc_runloop_sources();
        if self.runtime.ui_cocos.opengl_view != 0 && self.runtime.ui_objects.first_responder == self.runtime.ui_objects.root_controller {
            self.runtime.ui_objects.first_responder = self.runtime.ui_cocos.opengl_view;
        }
        self.diag.object_labels
            .entry(self.runtime.ui_objects.window)
            .or_insert_with(|| "UIWindow.main".to_string());
        self.ui_set_frame_bits(self.runtime.ui_objects.window, self.ui_surface_rect_bits());
        self.ui_set_bounds_bits(self.runtime.ui_objects.window, Self::ui_rect_size_bits(self.ui_surface_rect_bits()));
        self.ui_set_frame_bits(self.runtime.ui_objects.screen, self.ui_surface_rect_bits());
        self.ui_set_bounds_bits(self.runtime.ui_objects.screen, Self::ui_rect_size_bits(self.ui_surface_rect_bits()));
        if self.runtime.ui_cocos.opengl_view != 0 {
            self.ui_set_frame_bits(self.runtime.ui_cocos.opengl_view, self.ui_surface_rect_bits());
            self.ui_set_bounds_bits(self.runtime.ui_cocos.opengl_view, Self::ui_rect_size_bits(self.ui_surface_rect_bits()));
            self.ui_set_content_scale_bits(self.runtime.ui_cocos.opengl_view, 1.0f32.to_bits());
            self.ui_attach_layer_to_view(self.runtime.ui_cocos.opengl_view, self.runtime.ui_graphics.eagl_layer);
            self.diag.object_labels
                .entry(self.runtime.ui_cocos.opengl_view)
                .or_insert_with(|| "EAGLView.synthetic#0".to_string());
        }
        if self.runtime.ui_cocos.cocos_director != 0 {
            self.diag.object_labels
                .entry(self.runtime.ui_cocos.cocos_director)
                .or_insert_with(|| "CCDirector.synthetic#0".to_string());
        }
        self.refresh_graphics_object_labels();
        self.diag.trace.push(format!(
            "     ↳ hle cocos bootstrap reason={} director={} view={} window={} size={}x{}",
            reason,
            self.describe_ptr(self.runtime.ui_cocos.cocos_director),
            self.describe_ptr(self.runtime.ui_cocos.opengl_view),
            self.describe_ptr(self.runtime.ui_objects.window),
            self.runtime.ui_graphics.graphics_surface_width,
            self.runtime.ui_graphics.graphics_surface_height,
        ));
    }

    fn drive_cocos_frame_pipeline(&mut self, reason: &str, ticks: u32) {
        self.bootstrap_cocos_window_path(reason);
        let burst = ticks.max(1).min(6);
        for _ in 0..burst {
            self.push_synthetic_runloop_tick(reason, true);
            self.simulate_graphics_tick();
        }
    }

    fn note_synthetic_splash_destination(&mut self, splash: u32, destination: u32, origin: &str) {
        if splash == 0 || destination == 0 {
            return;
        }
        let previous = self.runtime.graphics.synthetic_splash_destinations.insert(splash, destination);
        let updated = previous != Some(destination);
        self.push_scene_progress_trace(format!(
            "scene.destination scene={} destination={} origin={} updated={} previous={} destinationState=[{}]",
            self.describe_ptr(splash),
            self.describe_ptr(destination),
            origin,
            if updated { "YES" } else { "NO" },
            previous.map(|value| self.describe_ptr(value)).unwrap_or_else(|| "nil".to_string()),
            self.describe_node_graph_state(destination),
        ));
        self.diag.trace.push(format!(
            "     ↳ hle scene.destination scene={} destination={} origin={} destinationState=[{}]",
            self.describe_ptr(splash),
            self.describe_ptr(destination),
            origin,
            self.describe_node_graph_state(destination),
        ));
    }

    fn scene_request_arg_to_destination(&mut self, scene_arg: u32) -> Option<u32> {
        if scene_arg == 0 {
            return None;
        }
        if self.runtime.graphics.synthetic_sprites.contains_key(&scene_arg) {
            return Some(scene_arg);
        }
        if let Some(bound) = self.runtime.ui_cocos.scene_instances_by_class.get(&scene_arg).copied() {
            if bound != 0 {
                return Some(bound);
            }
        }
        if let Some(class_name) = self.objc_class_name_for_ptr(scene_arg) {
            let scene_like = class_name.contains("Scene")
                || class_name.contains("Layer")
                || Self::is_transition_like_label(&class_name)
                || self.active_profile().is_first_scene_label(&class_name)
                || self.active_profile().is_menu_layer_label(&class_name);
            if scene_like {
                return None;
            }
        }
        None
    }

    fn try_materialize_scene_request_destination(&mut self, scene_arg: u32, origin: &str) -> Option<u32> {
        let scene_class = if self.runtime.objc.objc_classes_by_ptr.contains_key(&scene_arg) {
            scene_arg
        } else {
            0
        };
        if scene_class == 0 {
            return None;
        }
        if let Some(bound) = self.runtime.ui_cocos.scene_instances_by_class.get(&scene_class).copied() {
            if bound != 0 {
                return Some(bound);
            }
        }
        let class_name = self.objc_class_name_for_ptr(scene_class).unwrap_or_default();
        let scene_like = class_name.contains("Scene")
            || class_name.contains("Layer")
            || Self::is_transition_like_label(&class_name)
            || self.active_profile().is_first_scene_label(&class_name)
            || self.active_profile().is_menu_layer_label(&class_name);
        if !scene_like {
            return None;
        }
        for selector in ["scene", "node", "new"] {
            if self.objc_lookup_imp_for_receiver(scene_class, selector).is_none() {
                continue;
            }
            if let Some(result) = self.invoke_objc_selector_now_capture_r0(
                scene_class,
                selector,
                0,
                0,
                120_000,
                &format!("scene-route-factory:{}:{}", selector, origin),
            ) {
                if result != 0 {
                    self.note_scene_instance_binding(scene_class, result, &format!("scene-route-factory:{}", selector));
                    self.diag.trace.push(format!(
                        "     ↳ hle scene.route factory selector={} class={} result={} origin={}",
                        selector,
                        if class_name.is_empty() { format!("0x{scene_class:08x}") } else { class_name.clone() },
                        self.describe_ptr(result),
                        origin,
                    ));
                    return Some(result);
                }
            }
        }
        None
    }

    fn set_effect_scene(&mut self, scene: u32, origin: &str) -> bool {
        if self.runtime.ui_cocos.effect_scene == scene {
            return false;
        }
        let previous = self.runtime.ui_cocos.effect_scene;
        self.runtime.ui_cocos.effect_scene = scene;
        if scene != 0 {
            self.diag.object_labels
                .entry(scene)
                .or_insert_with(|| "CCScene.effect".to_string());
            self.remember_auto_scene_root(scene, format!("effect_scene:{}", origin));
            let route_destination = self.runtime.ui_cocos.pending_scene_route_destination;
            if route_destination != 0 {
                self.note_synthetic_splash_destination(
                    scene,
                    route_destination,
                    &format!("effect-scene-route:{}", origin),
                );
            }
        }
        self.push_scene_event(format!(
            "effectScene previous={} current={} origin={} destination={}",
            self.describe_ptr(previous),
            self.describe_ptr(scene),
            origin,
            self.runtime.graphics.synthetic_splash_destinations
                .get(&scene)
                .copied()
                .map(|value| self.describe_ptr(value))
                .unwrap_or_else(|| "nil".to_string()),
        ));
        self.diag.trace.push(format!(
            "     ↳ hle director.effectScene previous={} current={} origin={} state=[{}]",
            self.describe_ptr(previous),
            self.describe_ptr(scene),
            origin,
            self.describe_node_graph_state(scene),
        ));
        true
    }

    fn clear_effect_scene_if_redundant(&mut self, running_scene: u32, origin: &str) {
        let effect = self.runtime.ui_cocos.effect_scene;
        if effect == 0 {
            return;
        }
        let destination = self.runtime.graphics.synthetic_splash_destinations.get(&effect).copied().unwrap_or(0);
        if effect == running_scene || (destination != 0 && destination == running_scene) {
            let previous = self.runtime.ui_cocos.effect_scene;
            self.runtime.ui_cocos.effect_scene = 0;
            self.diag.trace.push(format!(
                "     ↳ hle director.effectScene clear previous={} running={} origin={} destination={}",
                self.describe_ptr(previous),
                self.describe_ptr(running_scene),
                origin,
                self.describe_ptr(destination),
            ));
        }
    }

    fn guest_transition_scene_is_stale(&self, guest_scene: u32) -> bool {
        if guest_scene == 0 {
            return false;
        }
        let label = self.diag.object_labels.get(&guest_scene).cloned().unwrap_or_default();
        if !Self::is_transition_like_label(&label) {
            return false;
        }
        let destination = self.runtime.graphics.synthetic_splash_destinations.get(&guest_scene).copied().unwrap_or(0);
        if destination == 0 || !self.runtime.graphics.synthetic_sprites.contains_key(&destination) {
            return false;
        }
        let running_scene = self.runtime.ui_cocos.running_scene;
        running_scene != 0 && running_scene != guest_scene && running_scene == destination
    }

    fn is_scene_route_selector(selector: &str) -> bool {
        selector.starts_with("replaceScene")
            || matches!(selector, "runWithScene:" | "pushScene:" | "popScene")
    }

    fn maybe_commit_pending_scene_route(&mut self, origin: &str) -> bool {
        let Some(selector) = self.runtime.ui_cocos.pending_scene_route_selector.clone() else {
            return false;
        };
        let destination = self.runtime.ui_cocos.pending_scene_route_destination;
        if destination == 0 {
            return false;
        }
        if self.runtime.ui_cocos.running_scene == destination {
            self.runtime.ui_cocos.pending_scene_route_class = 0;
            self.runtime.ui_cocos.pending_scene_route_owner = 0;
            self.runtime.ui_cocos.pending_scene_route_selector = None;
            self.runtime.ui_cocos.pending_scene_route_destination = 0;
            return false;
        }
        if self.runtime.ui_cocos.effect_scene != 0 {
            return false;
        }
        let need_request = !self.runtime.ui_cocos.scene_handoff_pending
            || self.runtime.ui_cocos.next_scene != destination;
        if need_request {
            self.runtime.ui_cocos.scene_transition_calls = self.runtime.ui_cocos.scene_transition_calls.saturating_add(1);
            if selector == "runWithScene:" {
                self.runtime.ui_cocos.scene_run_with_scene_calls = self.runtime.ui_cocos.scene_run_with_scene_calls.saturating_add(1);
            } else if selector == "pushScene:" {
                self.runtime.ui_cocos.scene_push_scene_calls = self.runtime.ui_cocos.scene_push_scene_calls.saturating_add(1);
            } else if selector.starts_with("replaceScene") {
                self.runtime.ui_cocos.scene_replace_scene_calls = self.runtime.ui_cocos.scene_replace_scene_calls.saturating_add(1);
            }
            self.record_director_scene_handoff_request(&selector, destination, origin);
        }
        self.maybe_commit_director_scene_handoff(&format!("scene-route:{origin}"))
    }

    fn record_scene_route_request(&mut self, selector: &str, receiver: u32, scene_arg: u32, callback_arg: u32, origin: &str) {
        if !Self::is_scene_route_selector(selector) {
            return;
        }
        let mut resolved = self.scene_request_arg_to_destination(scene_arg);
        let scene_class = if self.runtime.objc.objc_classes_by_ptr.contains_key(&scene_arg) {
            scene_arg
        } else {
            0
        };
        if scene_class != 0 {
            self.runtime.ui_cocos.pending_scene_route_class = scene_class;
            self.runtime.ui_cocos.pending_scene_route_owner = receiver;
            self.runtime.ui_cocos.pending_scene_route_selector = Some(selector.to_string());
            self.runtime.ui_cocos.pending_scene_route_destination = resolved.unwrap_or(0);
            if self.runtime.ui_cocos.pending_scene_route_destination == 0 {
                if let Some(destination) = self.try_materialize_scene_request_destination(
                    scene_class,
                    &format!("{}:{}", selector, origin),
                ) {
                    self.runtime.ui_cocos.pending_scene_route_destination = destination;
                    resolved = Some(destination);
                }
            }
        } else if let Some(destination) = resolved {
            self.runtime.ui_cocos.pending_scene_route_class = 0;
            self.runtime.ui_cocos.pending_scene_route_owner = receiver;
            self.runtime.ui_cocos.pending_scene_route_selector = Some(selector.to_string());
            self.runtime.ui_cocos.pending_scene_route_destination = destination;
            if self.runtime.ui_cocos.effect_scene != 0 {
                self.note_synthetic_splash_destination(
                    self.runtime.ui_cocos.effect_scene,
                    destination,
                    &format!("scene-route:{}:{}", selector, origin),
                );
            }
        } else {
            self.runtime.ui_cocos.pending_scene_route_destination = 0;
        }
        let receiver_class = self.objc_receiver_class_name_hint(receiver).unwrap_or_default();
        self.push_scene_progress_selector_event(
            selector,
            receiver,
            &receiver_class,
            selector,
            scene_arg,
            callback_arg,
            resolved,
            resolved.is_some(),
        );
        self.diag.trace.push(format!(
            "     ↳ hle scene.route request selector={} owner={} arg={} resolved={} origin={}",
            selector,
            self.describe_ptr(receiver),
            self.describe_ptr(scene_arg),
            resolved.map(|value| self.describe_ptr(value)).unwrap_or_else(|| "nil".to_string()),
            origin,
        ));
    }

    fn note_scene_instance_binding(&mut self, receiver: u32, result: u32, origin: &str) {
        if receiver == 0 || result == 0 {
            return;
        }
        let Some(class_ptr) = self.objc_class_ptr_for_receiver(receiver) else {
            return;
        };
        let class_name = self.objc_class_name_for_ptr(class_ptr).unwrap_or_default();
        let scene_like = class_name.contains("Scene")
            || class_name.contains("Layer")
            || Self::is_transition_like_label(&class_name)
            || self.active_profile().is_first_scene_label(&class_name)
            || self.active_profile().is_menu_layer_label(&class_name)
            || self.objc_receiver_inherits_named(result, "CCScene")
            || self.objc_receiver_inherits_named(result, "CCLayer")
            || self.objc_receiver_inherits_named(result, "CCTransitionScene");
        if !scene_like {
            return;
        }
        let previous = self.runtime.ui_cocos.scene_instances_by_class.insert(class_ptr, result);
        self.diag.trace.push(format!(
            "     ↳ hle scene.factory bind class={} instance={} origin={} previous={}",
            if class_name.is_empty() { format!("0x{class_ptr:08x}") } else { class_name.clone() },
            self.describe_ptr(result),
            origin,
            previous.map(|value| self.describe_ptr(value)).unwrap_or_else(|| "nil".to_string()),
        ));
        if self.runtime.ui_cocos.pending_scene_route_class == class_ptr {
            let selector = self.runtime.ui_cocos.pending_scene_route_selector.clone().unwrap_or_else(|| "replaceScene:".to_string());
            let owner = self.runtime.ui_cocos.pending_scene_route_owner;
            self.runtime.ui_cocos.pending_scene_route_destination = result;
            if self.runtime.ui_cocos.effect_scene != 0 {
                self.note_synthetic_splash_destination(
                    self.runtime.ui_cocos.effect_scene,
                    result,
                    &format!("scene-route-resolved:{}", origin),
                );
            }
            self.push_scene_progress_selector_event(
                &selector,
                owner,
                &self.objc_receiver_class_name_hint(owner).unwrap_or_default(),
                &selector,
                class_ptr,
                0,
                Some(result),
                true,
            );
            self.diag.trace.push(format!(
                "     ↳ hle scene.route resolved selector={} owner={} class={} instance={} origin={} effectScene={} destinationStored={}",
                selector,
                self.describe_ptr(owner),
                if class_name.is_empty() { format!("0x{class_ptr:08x}") } else { class_name },
                self.describe_ptr(result),
                origin,
                self.describe_ptr(self.runtime.ui_cocos.effect_scene),
                self.describe_ptr(self.runtime.ui_cocos.pending_scene_route_destination),
            ));
            self.runtime.ui_cocos.pending_scene_route_class = 0;
            let _ = self.maybe_commit_pending_scene_route(&format!("scene-route-resolved:{origin}"));
        }
    }

    fn handle_real_scene_selector_return(&mut self, watch: &PendingSchedulerReturn, origin: &str) {
        let selector = watch.selector.as_str();
        let result = self.cpu.regs[0];
        match selector {
            "sharedDirector" => {
                if result != 0 {
                    self.adopt_real_cocos_director(result, &format!("real-return:{}:{}", selector, origin));
                    self.bootstrap_synthetic_runloop();
                    self.recalc_runloop_sources();
                }
            }
            "openGLView" | "view" => {
                if result != 0 {
                    self.adopt_real_cocos_view(result, &format!("real-return:{}:{}", selector, origin));
                }
            }
            "transitionWithDuration:scene:" | "initWithDuration:scene:" => {
                if watch.arg3 != 0 && result != 0 {
                    self.note_synthetic_splash_destination(result, watch.arg3, &format!("real-return:{}", selector));
                    self.diag.object_labels
                        .entry(result)
                        .and_modify(|label| {
                            if !Self::is_transition_like_label(label) {
                                *label = format!("CCTransitionScene.instance(guest)<{}>", label);
                            }
                        })
                        .or_insert_with(|| "CCTransitionScene.instance(guest)".to_string());
                    let class_desc = self.objc_receiver_class_name_hint(watch.receiver).unwrap_or_default();
                    self.push_scene_progress_selector_event(
                        selector,
                        watch.receiver,
                        &class_desc,
                        selector,
                        watch.arg2,
                        watch.arg3,
                        Some(result),
                        true,
                    );
                }
            }
            "effectScene" => {
                let class_desc = self.objc_receiver_class_name_hint(watch.receiver).unwrap_or_default();
                self.set_effect_scene(result, &format!("real-return:{}:{}", selector, origin));
                self.push_scene_progress_selector_event(
                    selector,
                    watch.receiver,
                    &class_desc,
                    selector,
                    result,
                    0,
                    Some(result),
                    self.runtime.graphics.synthetic_splash_destinations.contains_key(&result),
                );
            }
            "setDirectorType:" => {
                self.runtime.ui_cocos.director_type = watch.arg2;
            }
            "attachInWindow:" | "attachInWindow" => {
                if watch.arg2 != 0 {
                    self.runtime.ui_objects.window = watch.arg2;
                    self.diag.object_labels
                        .entry(watch.arg2)
                        .or_insert_with(|| "UIWindow.main".to_string());
                }
            }
            "setEffectScene:" => {
                self.set_effect_scene(watch.arg2, &format!("real-return:{}:{}", selector, origin));
            }
            "setOpenGLView:" | "setView:" | "setGLView:" | "setEAGLView:" | "setMainView:" => {
                if watch.arg2 != 0 {
                    self.adopt_real_cocos_view(watch.arg2, &format!("real-return:{}:{}", selector, origin));
                }
            }
            "setAnimationInterval:" => {
                let interval_bits = self.nstimeinterval_f32_bits_from_regs(watch.arg2, watch.arg3);
                self.runtime.ui_cocos.animation_interval_bits = interval_bits;
                self.runtime.ui_cocos.animation_running = true;
                self.runtime.ui_cocos.display_link_armed = true;
                self.bootstrap_synthetic_runloop();
                self.recalc_runloop_sources();
                self.diag.trace.push(format!(
                    "     ↳ hle director.interval real-return selector={} bits=0x{:08x} secs={:.6} origin={}",
                    selector,
                    interval_bits,
                    Self::f32_from_bits(interval_bits),
                    origin,
                ));
            }
            "startAnimation" => {
                self.runtime.ui_cocos.animation_running = true;
                self.runtime.ui_cocos.display_link_armed = true;
                self.bootstrap_synthetic_runloop();
                self.recalc_runloop_sources();
            }
            "stopAnimation" => {
                self.runtime.ui_cocos.animation_running = false;
                self.runtime.ui_cocos.display_link_armed = false;
                self.recalc_runloop_sources();
            }
            "setDisplayFPS:" => {
                self.runtime.ui_cocos.display_fps_enabled = watch.arg2 != 0;
            }
            "node" | "scene" | "new" => {
                if result != 0 {
                    self.note_scene_instance_binding(watch.receiver, result, &format!("real-return:{}", selector));
                }
            }
            _ => {}
        }
    }

    fn is_transition_like_label(label: &str) -> bool {
        label.contains("Transition") || label.contains("transition")
    }

    fn is_loading_like_scene_label(&self, label: &str) -> bool {
        label.contains("SplashScreens")
            || self.active_profile().is_loading_scene_label(label)
            || label.contains("LoadingScene")
            || label.contains("MissionLoading")
            || ((label.contains("Loading") || label.contains("loading"))
                && (label.contains("Scene")
                    || label.contains("scene")
                    || label.contains("Layer")
                    || label.contains("layer")))
    }

    fn resolve_synthetic_progress_watch_scene(&self, root_scene: u32) -> u32 {
        if root_scene == 0 {
            return 0;
        }
        let Some(root_state) = self.runtime.graphics.synthetic_sprites.get(&root_scene) else {
            return root_scene;
        };
        let root_label = self.diag.object_labels.get(&root_scene).cloned().unwrap_or_default();
        let root_destination = self.runtime.graphics.synthetic_splash_destinations.get(&root_scene).copied().unwrap_or(0);
        let mut best = root_scene;
        let mut best_score = if Self::is_transition_like_label(&root_label) && root_destination != 0 {
            400
        } else if root_destination != 0 {
            320
        } else if self.is_loading_like_scene_label(&root_label) {
            240
        } else {
            0
        };

        if root_state.children == 0 {
            return best;
        }
        let child_count = self.synthetic_array_len(root_state.children);
        for index in 0..child_count {
            let child = self.synthetic_array_get(root_state.children, index);
            if child == 0 {
                continue;
            }
            let Some(child_state) = self.runtime.graphics.synthetic_sprites.get(&child) else {
                continue;
            };
            let child_label = self.diag.object_labels.get(&child).cloned().unwrap_or_default();
            let child_destination = self.runtime.graphics.synthetic_splash_destinations.get(&child).copied().unwrap_or(0);
            let scene_like = child_label.contains("Scene")
                || child_label.contains("scene")
                || child_label.contains("Layer")
                || child_label.contains("layer")
                || child_destination != 0;
            if !scene_like {
                continue;
            }
            let mut score = 0i32;
            if Self::is_transition_like_label(&child_label) && child_destination != 0 {
                score += 420;
            } else if child_destination != 0 {
                score += 340;
            }
            if self.is_loading_like_scene_label(&child_label) {
                score += 260;
            }
            if child_state.entered {
                score += 24;
            }
            if child_state.visible {
                score += 12;
            }
            if child_state.children != 0 {
                score += 8;
            }
            if score > best_score {
                best = child;
                best_score = score;
            }
        }
        best
    }

    fn synthetic_splash_auto_advance_age_threshold(&self) -> u32 {
        self.active_profile()
            .synthetic_splash_auto_advance_age_threshold(self.tuning.live_host_mode)
            .unwrap_or(10)
    }

    fn synthetic_splash_auto_advance_idle_threshold(&self) -> u32 {
        self.active_profile()
            .synthetic_splash_auto_advance_idle_threshold(self.tuning.live_host_mode)
            .unwrap_or(2)
    }

    fn preferred_first_responder_for_scene(&self, scene: u32) -> u32 {
        if scene == 0 {
            return 0;
        }
        let mut stack = vec![scene];
        let mut seen = HashSet::new();
        let mut touch_fallback = 0u32;
        while let Some(node) = stack.pop() {
            if node == 0 || !seen.insert(node) {
                continue;
            }
            let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
                continue;
            };
            if state.children != 0 {
                if let Some(arr) = self.runtime.graphics.synthetic_arrays.get(&state.children) {
                    for child in arr.items.iter().rev() {
                        if *child != 0 {
                            stack.push(*child);
                        }
                    }
                }
            }
            if !state.visible {
                continue;
            }
            if state.touch_enabled && state.callback_selector != 0 {
                return node;
            }
            if touch_fallback == 0 && state.touch_enabled {
                touch_fallback = node;
            }
        }
        if touch_fallback != 0 {
            touch_fallback
        } else {
            scene
        }
    }

    fn adopt_guest_running_scene_without_lifecycle(&mut self, scene: u32, origin: &str) -> usize {
        if scene == 0 {
            return 0;
        }
        let previous = self.runtime.ui_cocos.running_scene;
        let previous_responder = self.runtime.ui_objects.first_responder;
        self.maybe_detach_scene_from_transition_parent(scene, origin);
        if previous != 0 && previous != scene && self.runtime.graphics.synthetic_sprites.contains_key(&previous) {
            self.propagate_entered_recursive(previous, false);
        }
        self.runtime.ui_cocos.running_scene = scene;
        if self.runtime.ui_cocos.next_scene == scene {
            self.runtime.ui_cocos.next_scene = 0;
        }
        if self.runtime.ui_cocos.pending_scene_route_destination == scene {
            self.runtime.ui_cocos.pending_scene_route_class = 0;
            self.runtime.ui_cocos.pending_scene_route_owner = 0;
            self.runtime.ui_cocos.pending_scene_route_selector = None;
            self.runtime.ui_cocos.pending_scene_route_destination = 0;
        }
        self.runtime.ui_cocos.scene_handoff_pending = false;
        self.runtime.ui_cocos.scene_handoff_selector = None;
        self.runtime.ui_cocos.scene_handoff_wait_ticks = 0;
        self.runtime.scheduler.loading.scene_startup_attempts.remove(&scene);
        self.runtime.scheduler.loading.scene_bootstrap_state.remove(&scene);
        self.remember_auto_scene_root(scene, format!("guest-running_scene:{}", origin));
        self.runtime.scene.auto_scene_last_present_signature = None;
        self.runtime.graphics.guest_framebuffer_dirty = false;
        self.runtime.graphics.guest_draws_since_present = 0;
        self.runtime.graphics.uikit_framebuffer_dirty = false;
        self.runtime.host_input.active_touch = None;
        self.runtime.host_input.last_target = None;
        self.runtime.scene.synthetic_menu_probe_inflight = false;
        let entered = self.propagate_entered_recursive(scene, true);
        let preferred_responder = self.preferred_first_responder_for_scene(scene);
        if previous_responder == 0 || previous_responder == previous || self.tuning.live_host_mode {
            self.runtime.ui_objects.first_responder = preferred_responder;
        }
        self.runtime.scene.synthetic_last_running_scene = scene;
        self.runtime.scene.synthetic_running_scene_ticks = 0;
        self.clear_effect_scene_if_redundant(scene, origin);
        self.push_scene_event(format!(
            "guest-runningScene {} via={} propagated={}",
            self.describe_ptr(scene),
            origin,
            entered,
        ));
        self.push_scene_progress_trace(format!(
            "runningScene.activate scene={} via={} propagated={} state=[{}] responder={}",
            self.describe_ptr(scene),
            origin,
            entered,
            self.describe_node_graph_state(scene),
            self.describe_ptr(self.runtime.ui_objects.first_responder),
        ));
        self.diag.trace.push(format!(
            "     ↳ hle guest-runningScene scene={} via={} propagated={} state=[{}]",
            self.describe_ptr(scene),
            origin,
            entered,
            self.describe_node_graph_state(scene),
        ));
        entered
    }

    fn record_director_scene_handoff_request(&mut self, selector: &str, scene: u32, origin: &str) {
        if scene == 0 {
            return;
        }
        if selector == "pushScene:" {
            let running = self.runtime.ui_cocos.running_scene;
            if running != 0 && self.runtime.ui_cocos.scene_stack.last().copied() != Some(running) {
                self.runtime.ui_cocos.scene_stack.push(running);
            }
        }
        self.runtime.ui_cocos.next_scene = scene;
        self.runtime.ui_cocos.scene_handoff_pending = true;
        self.runtime.ui_cocos.scene_handoff_selector = Some(selector.to_string());
        self.runtime.ui_cocos.scene_handoff_wait_ticks = 0;
        self.diag.object_labels
            .entry(scene)
            .or_insert_with(|| "CCScene.pending".to_string());
        self.push_scene_event(format!(
            "handoff.request selector={} running={} next={} origin={}",
            selector,
            self.describe_ptr(self.runtime.ui_cocos.running_scene),
            self.describe_ptr(scene),
            origin,
        ));
        self.push_scene_progress_trace(format!(
            "handoff.request selector={} running={} next={} origin={} state=[{}]",
            selector,
            self.describe_ptr(self.runtime.ui_cocos.running_scene),
            self.describe_ptr(scene),
            origin,
            self.describe_node_graph_state(scene),
        ));
        self.arm_scheduler_trace_window(scene, origin, selector);
        self.diag.trace.push(format!(
            "     ↳ hle director.handoff request selector={} running={} next={} origin={} state=[{}]",
            selector,
            self.describe_ptr(self.runtime.ui_cocos.running_scene),
            self.describe_ptr(scene),
            origin,
            self.describe_node_graph_state(scene),
        ));
    }

    fn maybe_commit_director_scene_handoff(&mut self, origin: &str) -> bool {
        if !self.runtime.ui_cocos.scene_handoff_pending {
            return false;
        }
        self.runtime.ui_cocos.scene_handoff_wait_ticks = self.runtime.ui_cocos.scene_handoff_wait_ticks.saturating_add(1);
        let next_scene = self.runtime.ui_cocos.next_scene;
        if next_scene == 0 {
            self.runtime.ui_cocos.scene_handoff_pending = false;
            self.runtime.ui_cocos.scene_handoff_selector = None;
            self.runtime.ui_cocos.scene_handoff_wait_ticks = 0;
            self.push_scene_progress_trace(format!(
                "handoff.cleared origin={} running={} reason=nextScene=nil",
                origin,
                self.describe_ptr(self.runtime.ui_cocos.running_scene),
            ));
            self.diag.trace.push(format!(
                "     ↳ hle director.handoff cleared origin={} reason=nextScene=nil running={}",
                origin,
                self.describe_ptr(self.runtime.ui_cocos.running_scene),
            ));
            return false;
        }
        if self.runtime.ui_cocos.running_scene == next_scene {
            let wait_ticks = self.runtime.ui_cocos.scene_handoff_wait_ticks;
            self.runtime.ui_cocos.next_scene = 0;
            self.runtime.ui_cocos.scene_handoff_pending = false;
            self.runtime.ui_cocos.scene_handoff_selector = None;
            self.runtime.ui_cocos.scene_handoff_wait_ticks = 0;
            self.push_scene_progress_trace(format!(
                "handoff.settled origin={} scene={} waitTicks={}",
                origin,
                self.describe_ptr(next_scene),
                wait_ticks,
            ));
            self.diag.trace.push(format!(
                "     ↳ hle director.handoff settled origin={} scene={} waitTicks={}",
                origin,
                self.describe_ptr(next_scene),
                wait_ticks,
            ));
            return false;
        }
        let selector = self.runtime.ui_cocos.scene_handoff_selector.clone().unwrap_or_else(|| "<unknown>".to_string());
        let previous = self.runtime.ui_cocos.running_scene;
        let wait_ticks = self.runtime.ui_cocos.scene_handoff_wait_ticks;
        let entered = self.activate_running_scene(next_scene, &format!("director-handoff:{}", selector));
        self.sync_cocos_director_guest_ivars(origin);
        self.push_scene_progress_trace(format!(
            "handoff.commit selector={} origin={} previous={} running={} enterProp={} waitTicks={} state=[{}]",
            selector,
            origin,
            self.describe_ptr(previous),
            self.describe_ptr(self.runtime.ui_cocos.running_scene),
            entered,
            wait_ticks,
            self.describe_node_graph_state(self.runtime.ui_cocos.running_scene),
        ));
        self.diag.trace.push(format!(
            "     ↳ hle director.handoff commit selector={} origin={} previous={} running={} enterProp={} waitTicks={} state=[{}]",
            selector,
            origin,
            self.describe_ptr(previous),
            self.describe_ptr(self.runtime.ui_cocos.running_scene),
            entered,
            wait_ticks,
            self.describe_node_graph_state(self.runtime.ui_cocos.running_scene),
        ));
        self.push_scene_event(format!(
            "handoff.commit selector={} previous={} running={} origin={} enterProp={} waitTicks={}",
            selector,
            self.describe_ptr(previous),
            self.describe_ptr(self.runtime.ui_cocos.running_scene),
            origin,
            entered,
            wait_ticks,
        ));
        true
    }

    fn activate_running_scene(&mut self, scene: u32, origin: &str) -> usize {
        if scene == 0 {
            return 0;
        }
        let previous = self.runtime.ui_cocos.running_scene;
        let previous_responder = self.runtime.ui_objects.first_responder;
        self.maybe_detach_scene_from_transition_parent(scene, origin);
        let previous_label = if previous != 0 {
            self.diag.object_labels.get(&previous).cloned().unwrap_or_default()
        } else {
            String::new()
        };
        let old_responder_label = if previous_responder != 0 {
            self.diag.object_labels.get(&previous_responder).cloned().unwrap_or_default()
        } else {
            String::new()
        };
        let was_entered = self.runtime.graphics.synthetic_sprites.get(&scene).map(|state| state.entered).unwrap_or(false);
        if previous != 0 && previous != scene && self.runtime.graphics.synthetic_sprites.contains_key(&previous) {
            let exited = self.propagate_entered_recursive(previous, false);
            self.runtime.ui_cocos.scene_on_exit_events = self.runtime.ui_cocos.scene_on_exit_events.saturating_add(1);
            self.push_scene_event(format!("onExit {} via={} propagated={}", self.describe_ptr(previous), origin, exited));
            self.push_scene_progress_trace(format!(
                "scene.lifecycle onExit scene={} via={} propagated={} state=[{}]",
                self.describe_ptr(previous),
                origin,
                exited,
                self.describe_node_graph_state(previous),
            ));
            self.diag.trace.push(format!(
                "     ↳ hle scene.lifecycle onExit scene={} via={} propagated={} state=[{}]",
                self.describe_ptr(previous),
                origin,
                exited,
                self.describe_node_graph_state(previous),
            ));
            let exit_invoked = self.invoke_scene_lifecycle_selector_now(previous, "onExit", origin);
            self.diag.trace.push(format!(
                "     ↳ hle scene.lifecycle onExit-dispatch scene={} via={} invokedTargets={}",
                self.describe_ptr(previous),
                origin,
                exit_invoked,
            ));
        }
        self.runtime.ui_cocos.running_scene = scene;
        if self.runtime.ui_cocos.next_scene == scene {
            self.runtime.ui_cocos.next_scene = 0;
        }
        if self.runtime.ui_cocos.pending_scene_route_destination == scene {
            self.runtime.ui_cocos.pending_scene_route_class = 0;
            self.runtime.ui_cocos.pending_scene_route_owner = 0;
            self.runtime.ui_cocos.pending_scene_route_selector = None;
            self.runtime.ui_cocos.pending_scene_route_destination = 0;
        }
        self.runtime.ui_cocos.scene_handoff_pending = false;
        self.runtime.ui_cocos.scene_handoff_selector = None;
        self.runtime.ui_cocos.scene_handoff_wait_ticks = 0;
        self.runtime.scheduler.loading.scene_startup_attempts.remove(&scene);
        self.runtime.scheduler.loading.scene_bootstrap_state.remove(&scene);
        self.sync_cocos_director_guest_ivars(origin);
        self.remember_auto_scene_root(scene, format!("running_scene:{}", origin));
        self.runtime.scene.auto_scene_last_present_signature = None;
        self.runtime.graphics.guest_framebuffer_dirty = false;
        self.runtime.graphics.guest_draws_since_present = 0;
        self.runtime.graphics.uikit_framebuffer_dirty = false;
        self.runtime.host_input.active_touch = None;
        self.runtime.host_input.last_target = None;
        self.runtime.scene.synthetic_menu_probe_inflight = false;
        let entered = self.propagate_entered_recursive(scene, true);
        let preferred_responder = self.preferred_first_responder_for_scene(scene);
        let should_retarget_responder = previous_responder == 0
            || previous_responder == previous
            || old_responder_label.contains("SplashScreens")
            || previous_label.contains("SplashScreens")
            || self.tuning.live_host_mode;
        if should_retarget_responder {
            self.runtime.ui_objects.first_responder = preferred_responder;
        }
        self.runtime.scene.synthetic_last_running_scene = scene;
        self.runtime.scene.synthetic_running_scene_ticks = 0;
        self.clear_effect_scene_if_redundant(scene, origin);
        if previous != scene || !was_entered {
            self.runtime.ui_cocos.scene_on_enter_events = self.runtime.ui_cocos.scene_on_enter_events.saturating_add(1);
            self.push_scene_event(format!("onEnter {} via={} propagated={}", self.describe_ptr(scene), origin, entered));
            self.push_scene_progress_trace(format!(
                "scene.lifecycle onEnter scene={} via={} propagated={} state=[{}]",
                self.describe_ptr(scene),
                origin,
                entered,
                self.describe_node_graph_state(scene),
            ));
            self.diag.trace.push(format!(
                "     ↳ hle scene.lifecycle onEnter scene={} via={} propagated={} state=[{}]",
                self.describe_ptr(scene),
                origin,
                entered,
                self.describe_node_graph_state(scene),
            ));
            let enter_invoked = self.invoke_scene_lifecycle_selector_now(scene, "onEnter", origin);
            self.diag.trace.push(format!(
                "     ↳ hle scene.lifecycle onEnter-dispatch scene={} via={} invokedTargets={}",
                self.describe_ptr(scene),
                origin,
                enter_invoked,
            ));
            let finish_available = self.scene_lifecycle_selector_available_targets(scene, "onEnterTransitionDidFinish");
            if finish_available != 0 {
                self.runtime.ui_cocos.scene_on_enter_transition_finish_events = self.runtime.ui_cocos.scene_on_enter_transition_finish_events.saturating_add(1);
                self.push_scene_event(format!("onEnterTransitionDidFinish {} via={}", self.describe_ptr(scene), origin));
                self.push_scene_progress_trace(format!(
                    "scene.lifecycle onEnterTransitionDidFinish scene={} via={} responders={}",
                    self.describe_ptr(scene),
                    origin,
                    finish_available,
                ));
                self.diag.trace.push(format!(
                    "     ↳ hle scene.lifecycle onEnterTransitionDidFinish scene={} via={} responders={}",
                    self.describe_ptr(scene),
                    origin,
                    finish_available,
                ));
                let finish_invoked = self.invoke_scene_lifecycle_selector_now(scene, "onEnterTransitionDidFinish", origin);
                self.diag.trace.push(format!(
                    "     ↳ hle scene.lifecycle onEnterTransitionDidFinish-dispatch scene={} via={} invokedTargets={}",
                    self.describe_ptr(scene),
                    origin,
                    finish_invoked,
                ));
            } else {
                self.push_scene_progress_trace(format!(
                    "scene.lifecycle onEnterTransitionDidFinish skipped scene={} via={} responders=0",
                    self.describe_ptr(scene),
                    origin,
                ));
                self.diag.trace.push(format!(
                    "     ↳ hle scene.lifecycle onEnterTransitionDidFinish skipped scene={} via={} responders=0",
                    self.describe_ptr(scene),
                    origin,
                ));
                self.push_callback_trace(format!(
                    "scene.lifecycle.skip selector=onEnterTransitionDidFinish scene={} origin={} responders=0 state=[{}]",
                    self.describe_ptr(scene),
                    origin,
                    self.describe_node_graph_state(scene),
                ));
            }
            self.diag.trace.push(format!(
                "     ↳ hle scene.focus scene={} via={} firstResponder={} previousResponder={}",
                self.describe_ptr(scene),
                origin,
                self.describe_ptr(self.runtime.ui_objects.first_responder),
                self.describe_ptr(previous_responder),
            ));
        }
        entered
    }

    fn collect_scene_lifecycle_targets(&self, scene: u32) -> Vec<u32> {
        if scene == 0 {
            return Vec::new();
        }
        let watched = self.resolve_synthetic_progress_watch_scene(scene);
        let mut roots = Vec::new();
        if watched != 0 && watched != scene {
            roots.push(watched);
        }
        roots.push(scene);
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = roots;
        while let Some(node) = stack.pop() {
            if node == 0 || !seen.insert(node) {
                continue;
            }
            out.push(node);
            let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) else {
                continue;
            };
            if state.children == 0 {
                continue;
            }
            let child_count = self.synthetic_array_len(state.children);
            for idx in (0..child_count).rev() {
                let child = self.synthetic_array_get(state.children, idx);
                if child != 0 {
                    stack.push(child);
                }
            }
        }
        out
    }

    fn scene_lifecycle_selector_available_targets(&mut self, scene: u32, selector: &str) -> usize {
        if scene == 0 || selector.is_empty() {
            return 0;
        }
        self.collect_scene_lifecycle_targets(scene)
            .into_iter()
            .filter(|target| self.objc_lookup_imp_for_receiver(*target, selector).is_some())
            .count()
    }

    fn invoke_scene_lifecycle_selector_now(&mut self, scene: u32, selector: &str, origin: &str) -> usize {
        if scene == 0 || selector.is_empty() {
            return 0;
        }
        if self.begin_lifecycle_dispatch_guard(scene, selector, origin) {
            self.end_lifecycle_dispatch_guard();
            return 0;
        }
        let targets = self.collect_scene_lifecycle_targets(scene);
        self.push_scene_progress_trace(format!(
            "scene.lifecycle.begin selector={} scene={} origin={} targets={} state=[{}]",
            selector,
            self.describe_ptr(scene),
            origin,
            targets.len(),
            self.describe_node_graph_state(scene),
        ));
        self.push_callback_trace(format!(
            "scene.lifecycle.begin selector={} scene={} origin={} targets={} state=[{}]",
            selector,
            self.describe_ptr(scene),
            origin,
            targets.len(),
            self.describe_node_graph_state(scene),
        ));
        let mut invoked = 0usize;
        for target in targets {
            let relation = self.lifecycle_relation_to_scene(target, scene);
            let label = self.diag.object_labels.get(&target).cloned().unwrap_or_default();
            let responds = self.objc_lookup_imp_for_receiver(target, selector).is_some();
            self.push_callback_trace(format!(
                "scene.lifecycle.child-dispatch selector={} scene={} target={} relation={} label={} origin={} responds={}",
                selector,
                self.describe_ptr(scene),
                self.describe_ptr(target),
                relation,
                if label.is_empty() { "<none>" } else { &label },
                origin,
                if responds { "YES" } else { "NO" },
            ));
            let did_invoke = if responds {
                self.invoke_objc_selector_now(target, selector, 0, 0, 180_000, origin)
            } else {
                false
            };
            self.diag.trace.push(format!(
                "     ↳ hle scene.lifecycle.dispatch selector={} target={} relation={} origin={} responds={} invoked={}",
                selector,
                self.describe_ptr(target),
                relation,
                origin,
                if responds { "YES" } else { "NO" },
                if did_invoke { "YES" } else { "NO" },
            ));
            self.push_scene_progress_trace(format!(
                "scene.lifecycle.dispatch selector={} target={} relation={} origin={} responds={} invoked={} parentChain={}",
                selector,
                self.describe_ptr(target),
                relation,
                origin,
                if responds { "YES" } else { "NO" },
                if did_invoke { "YES" } else { "NO" },
                self.describe_parent_chain(target),
            ));
            self.push_callback_trace(format!(
                "scene.lifecycle.finish selector={} target={} relation={} origin={} responds={} invoked={} parentChain={}",
                selector,
                self.describe_ptr(target),
                relation,
                origin,
                if responds { "YES" } else { "NO" },
                if did_invoke { "YES" } else { "NO" },
                self.describe_parent_chain(target),
            ));
            if did_invoke {
                invoked = invoked.saturating_add(1);
            }
        }
        self.end_lifecycle_dispatch_guard();
        invoked
    }

    fn is_real_cocos_director_bootstrap_selector(selector: &str) -> bool {
        matches!(
            selector,
            "sharedDirector"
                | "setDirectorType:"
                | "setAnimationInterval:"
                | "setDisplayFPS:"
                | "setDeviceOrientation:"
                | "setOpenGLView:"
                | "setView:"
                | "setGLView:"
                | "setEAGLView:"
                | "setMainView:"
                | "openGLView"
                | "view"
                | "attachInWindow:"
                | "attachInWindow"
                | "initOpenGLViewWithView:withFrame:"
                | "attachInView:"
                | "effectScene"
                | "setEffectScene:"
                | "startAnimation"
                | "stopAnimation"
        )
    }

    fn should_defer_to_real_cocos_director_bootstrap_imp(&mut self, selector: &str, receiver: u32) -> bool {
        receiver != 0
            && Self::is_real_cocos_director_bootstrap_selector(selector)
            && self.objc_lookup_imp_for_receiver(receiver, selector).is_some()
    }

    fn is_real_cocos_window_bootstrap_selector(selector: &str) -> bool {
        matches!(
            selector,
            "setOpenGLView:"
                | "setView:"
                | "setGLView:"
                | "setEAGLView:"
                | "setMainView:"
                | "openGLView"
                | "view"
                | "attachInWindow:"
                | "attachInWindow"
                | "initOpenGLViewWithView:withFrame:"
                | "attachInView:"
                | "effectScene"
                | "setEffectScene:"
        )
    }

    fn should_watch_real_scheduler_selector_return(&self, selector: &str, receiver: u32) -> bool {
        match selector {
            "mainLoop" | "drawScene" => {
                receiver != 0 && (receiver == self.runtime.ui_cocos.cocos_director || self.objc_receiver_class_name_hint(receiver).unwrap_or_default().contains("Director"))
            }
            "sharedDirector"
            | "setDirectorType:"
            | "setAnimationInterval:"
            | "setDisplayFPS:"
            | "setDeviceOrientation:"
            | "setOpenGLView:"
            | "setView:"
            | "setGLView:"
            | "setEAGLView:"
            | "setMainView:"
            | "openGLView"
            | "view"
            | "attachInWindow:"
            | "attachInWindow"
            | "initOpenGLViewWithView:withFrame:"
            | "attachInView:"
            | "effectScene"
            | "setEffectScene:"
            | "startAnimation"
            | "stopAnimation"
            | "drawFrame:"
            | "swapBuffers" => receiver != 0,
            "presentRenderbuffer:" | "setNeedsDisplay" | "layoutIfNeeded" | "layoutSubviews" | "display" | "displayIfNeeded" => receiver != 0,
            "transitionWithDuration:scene:" | "initWithDuration:scene:" => receiver != 0,
            "node" | "scene" | "new" => {
                receiver != 0
                    && self.runtime.ui_cocos.pending_scene_route_class != 0
                    && receiver == self.runtime.ui_cocos.pending_scene_route_class
            }
            _ => false,
        }
    }

    fn arm_real_scheduler_selector_return_watch(
        &mut self,
        selector: &str,
        receiver: u32,
        arg2: u32,
        arg3: u32,
        current_pc: u32,
        return_lr: u32,
    ) {
        if !self.should_watch_real_scheduler_selector_return(selector, receiver) {
            return;
        }
        let return_pc = return_lr & !1;
        if return_pc == 0 {
            return;
        }
        let return_thumb = (return_lr & 1) != 0;
        let watch = PendingSchedulerReturn {
            selector: selector.to_string(),
            receiver,
            arg2,
            arg3,
            return_pc,
            return_thumb,
            presents_before: self.runtime.ui_graphics.graphics_present_calls,
            frame_before: self.runtime.ui_graphics.graphics_frame_index,
            dispatch_pc: current_pc,
        };
        self.runtime.ui_cocos.pending_scheduler_returns.push(watch);
        self.diag.trace.push(format!(
            "     ↳ scheduler.return-watch arm selector={} receiver={} return=0x{:08x}({}) dispatchPc=0x{:08x} depth={}",
            selector,
            self.describe_ptr(receiver),
            return_pc,
            if return_thumb { "thumb" } else { "arm" },
            current_pc,
            self.runtime.ui_cocos.pending_scheduler_returns.len(),
        ));
    }

    fn process_real_scheduler_selector_return_watches(&mut self, origin: &str) -> usize {
        let mut fired = 0usize;
        loop {
            let Some(top) = self.runtime.ui_cocos.pending_scheduler_returns.last().cloned() else {
                break;
            };
            if self.cpu.regs[15] != top.return_pc || self.cpu.thumb != top.return_thumb {
                break;
            }
            self.runtime.ui_cocos.pending_scheduler_returns.pop();
            self.finish_real_scheduler_selector_return(top, origin);
            fired = fired.saturating_add(1);
        }
        fired
    }

    pub(crate) fn process_runtime_post_step_hooks(&mut self, origin: &str) -> usize {
        let scheduler = self.process_real_scheduler_selector_return_watches(origin);
        let audio = self.process_real_audio_selector_return_watches(origin);
        let network = self.process_real_network_selector_return_watches(origin);
        scheduler.saturating_add(audio).saturating_add(network)
    }

    fn finish_real_scheduler_selector_return(&mut self, watch: PendingSchedulerReturn, origin: &str) {
        let selector = watch.selector.as_str();
        let sync_origin = format!("real-return:{}:{}", selector, origin);
        self.handle_real_scene_selector_return(&watch, origin);
        self.sync_cocos_director_guest_ivars(&sync_origin);
        if Self::is_real_cocos_window_bootstrap_selector(selector) {
            self.bootstrap_cocos_window_path(&sync_origin);
        }
        let handoff_committed = matches!(selector, "mainLoop" | "drawScene" | "drawFrame:")
            && self.maybe_commit_director_scene_handoff(&sync_origin);
        let forced_present = self.maybe_force_display_link_present(
            &sync_origin,
            selector,
            watch.receiver,
            watch.presents_before,
            handoff_committed,
        );
        self.diag.trace.push(format!(
            "     ↳ scheduler.return-watch fire selector={} receiver={} origin={} dispatchPc=0x{:08x} return=0x{:08x} handoffCommitted={} forcedPresent={} presents={}=>{} frames={}=>{} source={} reason={}",
            selector,
            self.describe_ptr(watch.receiver),
            origin,
            watch.dispatch_pc,
            watch.return_pc,
            if handoff_committed { "YES" } else { "NO" },
            if forced_present { "YES" } else { "NO" },
            watch.presents_before,
            self.runtime.ui_graphics.graphics_present_calls,
            watch.frame_before,
            self.runtime.ui_graphics.graphics_frame_index,
            self.runtime.ui_graphics.graphics_last_present_source.clone().unwrap_or_else(|| "unknown".to_string()),
            self.runtime.ui_graphics.graphics_last_present_decision.clone().unwrap_or_else(|| "<none>".to_string()),
        ));
    }

    fn maybe_force_display_link_present(
        &mut self,
        origin: &str,
        selector: &str,
        target: u32,
        presents_before: u32,
        handoff_committed: bool,
    ) -> bool {
        let presents_after = self.runtime.ui_graphics.graphics_present_calls;
        if presents_after != presents_before {
            return false;
        }
        let should_force = handoff_committed
            || self.runtime.ui_cocos.animation_running
            || self.runtime.graphics.guest_framebuffer_dirty
            || self.runtime.graphics.uikit_framebuffer_dirty
            || self.runtime.ui_graphics.graphics_present_calls == 0
            || self.runtime.ui_graphics.graphics_frame_index == 0;
        if !should_force {
            return false;
        }
        let frame_before = self.runtime.ui_graphics.graphics_frame_index;
        self.runtime.ui_cocos.scheduler_render_callback_calls = self
            .runtime
            .ui_cocos
            .scheduler_render_callback_calls
            .saturating_add(1);
        self.push_scheduler_event(format!(
            "render-cb fallback {} target={} handoff={} origin={}",
            selector,
            self.describe_ptr(target),
            if handoff_committed { "YES" } else { "NO" },
            origin,
        ));
        self.simulate_graphics_tick();
        let forced = self.runtime.ui_graphics.graphics_present_calls != presents_after
            || self.runtime.ui_graphics.graphics_frame_index != frame_before;
        self.diag.trace.push(format!(
            "     ↳ hle displaylink fallback-present selector={} target={} origin={} handoffCommitted={} forced={} presents={}=>{} frame={}=>{} source={} reason={}",
            selector,
            self.describe_ptr(target),
            origin,
            if handoff_committed { "YES" } else { "NO" },
            if forced { "YES" } else { "NO" },
            presents_before,
            self.runtime.ui_graphics.graphics_present_calls,
            frame_before,
            self.runtime.ui_graphics.graphics_frame_index,
            self.runtime.ui_graphics.graphics_last_present_source.clone().unwrap_or_else(|| "unknown".to_string()),
            self.runtime.ui_graphics.graphics_last_present_decision.clone().unwrap_or_else(|| "<none>".to_string()),
        ));
        forced
    }

    fn dispatch_synthetic_display_link_tick(&mut self, origin: &str) -> (u32, &'static str, bool) {
        self.sync_cocos_director_guest_ivars(origin);
        if self.runtime.ui_cocos.cocos_director != 0 {
            let director = self.runtime.ui_cocos.cocos_director;
            let presents_before = self.runtime.ui_graphics.graphics_present_calls;
            if self.invoke_objc_selector_now(director, "mainLoop", 0, 0, 180_000, origin) {
                self.sync_cocos_director_guest_ivars("display-link:return:mainLoop");
                let handoff_committed = self.maybe_commit_director_scene_handoff("display-link:return:mainLoop");
                self.maybe_force_display_link_present("display-link:return:mainLoop", "mainLoop", director, presents_before, handoff_committed);
                self.runtime.ui_cocos.scheduler_mainloop_calls = self.runtime.ui_cocos.scheduler_mainloop_calls.saturating_add(1);
                self.push_scheduler_event(format!("mainLoop {}", self.describe_ptr(director)));
                return (director, "mainLoop", true);
            }
            let presents_before = self.runtime.ui_graphics.graphics_present_calls;
            if self.invoke_objc_selector_now(director, "drawScene", 0, 0, 180_000, origin) {
                self.sync_cocos_director_guest_ivars("display-link:return:drawScene");
                let handoff_committed = self.maybe_commit_director_scene_handoff("display-link:return:drawScene");
                self.maybe_force_display_link_present("display-link:return:drawScene", "drawScene", director, presents_before, handoff_committed);
                self.runtime.ui_cocos.scheduler_draw_scene_calls = self.runtime.ui_cocos.scheduler_draw_scene_calls.saturating_add(1);
                self.push_scheduler_event(format!("drawScene {}", self.describe_ptr(director)));
                return (director, "drawScene", true);
            }
        }
        let draw_target = if self.runtime.ui_cocos.opengl_view != 0 {
            self.runtime.ui_cocos.opengl_view
        } else {
            self.runtime.ui_objects.root_controller
        };
        let presents_before = self.runtime.ui_graphics.graphics_present_calls;
        if draw_target != 0
            && self.invoke_objc_selector_now(draw_target, "drawFrame:", self.runtime.ui_cocos.synthetic_display_link, 0, 180_000, origin)
        {
            self.sync_cocos_director_guest_ivars("display-link:return:drawFrame");
            let handoff_committed = self.maybe_commit_director_scene_handoff("display-link:return:drawFrame");
            self.maybe_force_display_link_present("display-link:return:drawFrame", "drawFrame:", draw_target, presents_before, handoff_committed);
            self.runtime.ui_cocos.scheduler_draw_frame_calls = self.runtime.ui_cocos.scheduler_draw_frame_calls.saturating_add(1);
            self.push_scheduler_event(format!("drawFrame {}", self.describe_ptr(draw_target)));
            return (draw_target, "drawFrame:", true);
        }
        (draw_target, "drawFrame:", false)
    }

    fn decode_cocos_schedule_selector_name(&self, selector_ptr: u32) -> Option<String> {
        self.objc_read_selector_name(selector_ptr)
            .or_else(|| self.guest_string_value(selector_ptr))
            .map(|value| value.trim_matches('\0').to_string())
            .filter(|value| !value.is_empty())
    }

    fn cocos_schedule_interval_ticks(&self, interval_bits: u32) -> u32 {
        if interval_bits == 0 {
            return 1;
        }
        let secs = Self::f32_from_bits(interval_bits);
        if !secs.is_finite() || secs <= 0.0 {
            return 1;
        }
        let ticks = (secs * 60.0).round() as i32;
        ticks.max(1) as u32
    }

    fn loading_scene_bootstrap_mark_success(&mut self, scene: u32, bit: u32) {
        let entry = self.runtime.scheduler.loading.scene_bootstrap_state.entry(scene).or_insert(0);
        *entry |= bit;
    }

    fn loading_scene_bootstrap_has(&self, scene: u32, bit: u32) -> bool {
        self.runtime.scheduler.loading.scene_bootstrap_state
            .get(&scene)
            .map(|state| (*state & bit) != 0)
            .unwrap_or(false)
    }

    fn maybe_force_loading_mission_scene_bootstrap(&mut self, scene: u32, label: &str, origin: &str, age: u32) -> Vec<String> {
        if scene == 0 || !self.active_profile().is_loading_mission_scene_label(label) {
            return Vec::new();
        }
        const BOOTSTRAP_SET_SCENE: u32 = 1 << 0;
        const BOOTSTRAP_LOAD_MISSION: u32 = 1 << 1;

        let mut forced = Vec::new();
        let manager = self
            .objc_lookup_class_by_name("MissionManager")
            .map(|class_ptr| self.ensure_objc_singleton_object(class_ptr, "MissionManager", "loading-scene-bootstrap"))
            .unwrap_or(0);
        if manager != 0 {
            self.push_callback_trace(format!(
                "loading-scene.bootstrap kind=mission scene={} manager={} age={} origin={} networkCompleted={} scheduled={} idleAfterCompletion={} bootstrapState=0x{:x}",
                self.describe_ptr(scene),
                self.describe_ptr(manager),
                age,
                origin,
                if self.runtime.ui_network.network_completed { "YES" } else { "NO" },
                if self.cocos_has_scheduled_selector_for_target(scene) { "YES" } else { "NO" },
                self.runtime.ui_runtime.idle_ticks_after_completion,
                self.runtime.scheduler.loading.scene_bootstrap_state.get(&scene).copied().unwrap_or(0),
            ));

            if !self.loading_scene_bootstrap_has(scene, BOOTSTRAP_SET_SCENE)
                && self.invoke_objc_selector_now(manager, "setScene:", scene, 0, 180_000, "loading-scene-bootstrap")
            {
                self.loading_scene_bootstrap_mark_success(scene, BOOTSTRAP_SET_SCENE);
                forced.push("MissionManager.setScene:".to_string());
            }

            if age <= 2
                && !self.loading_scene_bootstrap_has(scene, BOOTSTRAP_LOAD_MISSION)
                && self.invoke_objc_selector_now(manager, "loadMission", 0, 0, 180_000, "loading-scene-bootstrap")
            {
                self.loading_scene_bootstrap_mark_success(scene, BOOTSTRAP_LOAD_MISSION);
                forced.push("MissionManager.loadMission".to_string());
            }

            if self.invoke_objc_selector_now(manager, "setTransitionEnded:", 1, 0, 180_000, "loading-scene-bootstrap") {
                forced.push("MissionManager.setTransitionEnded:".to_string());
            }

            if age >= 2 || (self.runtime.ui_network.network_completed && self.runtime.ui_runtime.idle_ticks_after_completion >= 1) {
                if self.invoke_objc_selector_now(manager, "setLoadingEnded:", 1, 0, 180_000, "loading-scene-bootstrap") {
                    forced.push("MissionManager.setLoadingEnded:".to_string());
                }
            }
        } else {
            self.push_callback_trace(format!(
                "loading-scene.bootstrap kind=mission scene={} manager=<missing> age={} origin={} networkCompleted={} scheduled={} idleAfterCompletion={} bootstrapState=0x{:x}",
                self.describe_ptr(scene),
                age,
                origin,
                if self.runtime.ui_network.network_completed { "YES" } else { "NO" },
                if self.cocos_has_scheduled_selector_for_target(scene) { "YES" } else { "NO" },
                self.runtime.ui_runtime.idle_ticks_after_completion,
                self.runtime.scheduler.loading.scene_bootstrap_state.get(&scene).copied().unwrap_or(0),
            ));
        }

        if self.invoke_objc_selector_now(scene, "foo", 0, 0, 180_000, "loading-scene-bootstrap") {
            forced.push("LoadingMissionScene.foo".to_string());
        }

        if !self.cocos_has_scheduled_selector_for_target(scene) {
            self.register_cocos_scheduled_selector(scene, "foo", 0, None, "loading-scene-bootstrap");
            self.runtime.ui_cocos.scheduler_schedule_calls = self.runtime.ui_cocos.scheduler_schedule_calls.saturating_add(1);
            self.push_scheduler_event(format!("schedule forced-loading foo {}", self.describe_ptr(scene)));
            forced.push("schedule foo".to_string());
        }
        forced
    }

    fn maybe_prime_loading_scene_startup(&mut self, scene: u32, origin: &str, age: u32) -> bool {
        let label = self.diag.object_labels.get(&scene).cloned().unwrap_or_default();
        let is_loading_scene = self.active_profile().is_loading_scene_label(&label);
        if !is_loading_scene {
            return false;
        }
        let attempts_before = self.runtime.scheduler.loading.scene_startup_attempts.get(&scene).copied().unwrap_or(0);
        let scheduled = self.cocos_has_scheduled_selector_for_target(scene);
        let continue_prompt_visible = self.loading_scene_has_continue_prompt(scene);
        let mut invoked = Vec::new();
        if continue_prompt_visible {
            invoked.extend(self.maybe_prepare_loading_scene_continue_path(scene, origin, age));
        }
        if age == 1 && attempts_before == 0 {
            if self.invoke_objc_selector_now(scene, "onEnter", 0, 0, 180_000, "loading-scene-prime") {
                invoked.push("onEnter".to_string());
            }
            if self.objc_lookup_imp_for_receiver(scene, "onEnterTransitionDidFinish").is_some()
                && self.invoke_objc_selector_now(scene, "onEnterTransitionDidFinish", 0, 0, 180_000, "loading-scene-prime")
            {
                invoked.push("onEnterTransitionDidFinish".to_string());
            }
        }
        if !continue_prompt_visible && ((!scheduled && matches!(age, 2 | 4 | 8 | 16)) || (attempts_before == 0 && age == 1)) {
            if self.invoke_objc_selector_now(scene, "foo", 0, 0, 180_000, "loading-scene-prime") {
                invoked.push("foo".to_string());
            }
            if self.invoke_objc_selector_now(scene, "update:", if self.runtime.ui_cocos.animation_interval_bits != 0 { self.runtime.ui_cocos.animation_interval_bits } else { (1.0f32 / 60.0f32).to_bits() }, 0, 180_000, "loading-scene-prime") {
                invoked.push("update:".to_string());
            }
        }
        if matches!(age, 1 | 2 | 4 | 8 | 16) {
            if !continue_prompt_visible {
                invoked.extend(self.maybe_force_loading_mission_scene_bootstrap(scene, &label, origin, age));
            } else {
                self.unschedule_cocos_selector(scene, "foo", "loading-scene-continue");
            }
        }
        if !invoked.is_empty() {
            let entry = self.runtime.scheduler.loading.scene_startup_attempts.entry(scene).or_insert(0);
            *entry = entry.saturating_add(invoked.len() as u32);
            self.diag.trace.push(format!(
                "     ↳ hle loading-scene prime scene={} label={} age={} scheduled={} continuePrompt={} invoked=[{}] origin={}",
                self.describe_ptr(scene),
                if label.is_empty() { "<unknown>" } else { &label },
                age,
                if scheduled { "YES" } else { "NO" },
                if continue_prompt_visible { "YES" } else { "NO" },
                invoked.join(","),
                origin,
            ));
        } else if matches!(age, 1 | 2 | 4 | 8 | 16) {
            self.diag.trace.push(format!(
                "     ↳ hle loading-scene watch scene={} label={} age={} scheduled={} continuePrompt={} invoked=[none] origin={}",
                self.describe_ptr(scene),
                if label.is_empty() { "<unknown>" } else { &label },
                age,
                if scheduled { "YES" } else { "NO" },
                if continue_prompt_visible { "YES" } else { "NO" },
                origin,
            ));
        }
        true
    }

    fn synthetic_menu_item_variants(&self, item: u32) -> [u32; 3] {
        let children = self.runtime.graphics.synthetic_sprites.get(&item).map(|state| state.children).unwrap_or(0);
        if children == 0 {
            return [0, 0, 0];
        }
        let items = self.runtime.graphics.synthetic_arrays.get(&children).map(|arr| arr.items.clone()).unwrap_or_default();
        [
            items.get(0).copied().unwrap_or(0),
            items.get(1).copied().unwrap_or(0),
            items.get(2).copied().unwrap_or(0),
        ]
    }

    fn set_synthetic_menu_item_pressed(&mut self, item: u32, pressed: bool) {
        let [normal, selected, disabled] = self.synthetic_menu_item_variants(item);
        if selected == 0 && normal == 0 && disabled == 0 {
            return;
        }
        if normal != 0 {
            self.ensure_synthetic_sprite_state(normal).visible = !pressed;
        }
        if selected != 0 {
            self.ensure_synthetic_sprite_state(selected).visible = pressed;
        }
        if disabled != 0 {
            self.ensure_synthetic_sprite_state(disabled).visible = false;
        }
    }

    fn find_synthetic_menu_item_by_selector(&self, root: u32, selector_name: &str) -> Option<u32> {
        if root == 0 || selector_name.trim().is_empty() {
            return None;
        }
        let target = selector_name.trim();
        let mut stack = vec![root];
        let mut seen = HashSet::new();
        while let Some(node) = stack.pop() {
            if node == 0 || !seen.insert(node) {
                continue;
            }
            if let Some(state) = self.runtime.graphics.synthetic_sprites.get(&node) {
                if state.callback_selector != 0 {
                    if let Some(found) = self.objc_read_selector_name(state.callback_selector) {
                        if found == target && state.visible {
                            return Some(node);
                        }
                    }
                }
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

    fn synthetic_selector_fastpath_result(
        &mut self,
        receiver: u32,
        selector_name: &str,
        arg2: u32,
        _arg3: u32,
    ) -> Option<(u32, String)> {
        let selector = selector_name.trim_matches('\0').trim();
        if selector.is_empty() || receiver == 0 {
            return None;
        }

        if self.runtime.graphics.synthetic_arrays.contains_key(&receiver) {
            return match selector {
                "count" => Some((
                    self.synthetic_array_len(receiver) as u32,
                    format!("synthetic-array count -> {}", self.synthetic_array_len(receiver)),
                )),
                "objectAtIndex:" => {
                    let index = arg2 as usize;
                    let result = self.synthetic_array_get(receiver, index);
                    Some((
                        result,
                        format!("synthetic-array objectAtIndex {} -> {}", index, self.describe_ptr(result)),
                    ))
                }
                "lastObject" => {
                    let len = self.synthetic_array_len(receiver);
                    let result = if len == 0 { 0 } else { self.synthetic_array_get(receiver, len.saturating_sub(1)) };
                    Some((result, format!("synthetic-array lastObject -> {}", self.describe_ptr(result))))
                }
                _ => None,
            };
        }

        if self.runtime.graphics.synthetic_sprites.contains_key(&receiver) {
            return match selector {
                "children" => {
                    let children = self.ensure_node_children_array(receiver);
                    Some((
                        children,
                        format!(
                            "synthetic-node children -> {} count={}",
                            self.describe_ptr(children),
                            self.synthetic_array_len(children)
                        ),
                    ))
                }
                "parent" => {
                    let parent = self
                        .runtime
                        .graphics
                        .synthetic_sprites
                        .get(&receiver)
                        .map(|state| state.parent)
                        .unwrap_or(0);
                    Some((parent, format!("synthetic-node parent -> {}", self.describe_ptr(parent))))
                }
                "zOrder" => {
                    let z = self
                        .runtime
                        .graphics
                        .synthetic_sprites
                        .get(&receiver)
                        .map(|state| state.z_order)
                        .unwrap_or(0) as u32;
                    Some((z, format!("synthetic-node zOrder -> {}", z as i32)))
                }
                "tag" => {
                    let tag = self
                        .runtime
                        .graphics
                        .synthetic_sprites
                        .get(&receiver)
                        .map(|state| state.tag)
                        .unwrap_or(0);
                    Some((tag, format!("synthetic-node tag -> {}", tag)))
                }
                "isVisible" => {
                    let visible = self
                        .runtime
                        .graphics
                        .synthetic_sprites
                        .get(&receiver)
                        .map(|state| state.visible)
                        .unwrap_or(false);
                    Some(((if visible { 1 } else { 0 }), format!("synthetic-node isVisible -> {}", if visible { "YES" } else { "NO" })))
                }
                _ => None,
            };
        }

        None
    }

    fn maybe_invoke_objc_selector_missing_fastpath(
        &mut self,
        receiver: u32,
        class_hint: &str,
        selector_name: &str,
        arg2: u32,
        arg3: u32,
        origin: &str,
    ) -> Option<u32> {
        let selector = selector_name.trim_matches('\0').trim();
        if selector.is_empty() {
            return None;
        }

        if let Some((result, note)) = self.synthetic_selector_fastpath_result(receiver, selector, arg2, arg3) {
            self.diag.trace.push(format!(
                "     ↳ synthetic selector invoke repaired receiver={} class={} selector={} arg2={} arg3={} origin={} strategy=synthetic-state result={} note={}",
                self.describe_ptr(receiver),
                class_hint,
                selector,
                self.describe_ptr(arg2),
                self.describe_ptr(arg3),
                origin,
                self.describe_ptr(result),
                note,
            ));
            return Some(result);
        }

        if matches!(selector, "mainLoop" | "mainLoop:")
            && matches!(class_hint, "GameControl" | "GUIController")
        {
            self.bootstrap_cocos_window_path("objc-dispatch-mainloop-repair");
            let (display_target, display_selector, invoked) =
                self.dispatch_synthetic_display_link_tick(origin);
            self.diag.trace.push(format!(
                "     ↳ synthetic selector invoke repaired receiver={} class={} selector={} arg2={} arg3={} origin={} strategy=proxy-display-link target={} used={} invoked={}",
                self.describe_ptr(receiver),
                class_hint,
                selector,
                self.describe_ptr(arg2),
                self.describe_ptr(arg3),
                origin,
                self.describe_ptr(display_target),
                display_selector,
                if invoked { "YES" } else { "NO" },
            ));
            return Some(receiver);
        }

        if selector == "reachabilityChanged:" {
            let current_delegate = self.current_network_delegate();
            let likely_delegate = class_hint.contains("Delegate")
                || receiver == self.runtime.ui_objects.delegate
                || (current_delegate != 0 && receiver == current_delegate);
            if likely_delegate {
                self.diag.trace.push(format!(
                    "     ↳ synthetic selector invoke repaired receiver={} class={} selector={} arg2={} arg3={} origin={} strategy=delegate-noop",
                    self.describe_ptr(receiver),
                    class_hint,
                    selector,
                    self.describe_ptr(arg2),
                    self.describe_ptr(arg3),
                    origin,
                ));
                return Some(receiver);
            }
        }

        let uikitish_receiver = receiver == self.runtime.ui_objects.app
            || receiver == self.runtime.ui_objects.window
            || receiver == self.runtime.ui_objects.root_controller
            || self.ui_object_is_view_like(receiver);
        if uikitish_receiver {
            match selector {
                "nextResponder" => {
                    let next = self.ui_next_responder(receiver);
                    self.diag.trace.push(format!(
                        "     ↳ synthetic selector invoke repaired receiver={} class={} selector={} arg2={} arg3={} origin={} strategy=uikit-next-responder result={}",
                        self.describe_ptr(receiver),
                        class_hint,
                        selector,
                        self.describe_ptr(arg2),
                        self.describe_ptr(arg3),
                        origin,
                        self.describe_ptr(next),
                    ));
                    return Some(next);
                }
                "pointInside:withEvent:" => {
                    if let Some((bits, source)) = self.read_msgsend_point_arg(arg2, arg3) {
                        let inside = self.ui_view_contains_local_point(
                            receiver,
                            Self::f32_from_bits(bits[0]),
                            Self::f32_from_bits(bits[1]),
                        );
                        self.diag.trace.push(format!(
                            "     ↳ synthetic selector invoke repaired receiver={} class={} selector={} arg2={} arg3={} origin={} strategy=uikit-point-inside point=({:.3},{:.3}) via={} result={}",
                            self.describe_ptr(receiver),
                            class_hint,
                            selector,
                            self.describe_ptr(arg2),
                            self.describe_ptr(arg3),
                            origin,
                            Self::f32_from_bits(bits[0]),
                            Self::f32_from_bits(bits[1]),
                            source,
                            if inside { "YES" } else { "NO" },
                        ));
                        return Some(if inside { 1 } else { 0 });
                    }
                }
                "hitTest:withEvent:" => {
                    if let Some((bits, source)) = self.read_msgsend_point_arg(arg2, arg3) {
                        let hit = self.ui_hit_test_view_subtree(
                            receiver,
                            Self::f32_from_bits(bits[0]),
                            Self::f32_from_bits(bits[1]),
                        ).unwrap_or(0);
                        self.diag.trace.push(format!(
                            "     ↳ synthetic selector invoke repaired receiver={} class={} selector={} arg2={} arg3={} origin={} strategy=uikit-hit-test point=({:.3},{:.3}) via={} result={}",
                            self.describe_ptr(receiver),
                            class_hint,
                            selector,
                            self.describe_ptr(arg2),
                            self.describe_ptr(arg3),
                            origin,
                            Self::f32_from_bits(bits[0]),
                            Self::f32_from_bits(bits[1]),
                            source,
                            self.describe_ptr(hit),
                        ));
                        return Some(hit);
                    }
                }
                "sendEvent:" => {
                    if let Some(phase) = self.synthetic_phase_name_for_event(arg2) {
                        let routed = self.dispatch_uikit_event_object_via_window_chain(phase, arg2, origin);
                        self.diag.trace.push(format!(
                            "     ↳ synthetic selector invoke repaired receiver={} class={} selector={} arg2={} arg3={} origin={} strategy=uikit-send-event phase={} routed={}",
                            self.describe_ptr(receiver),
                            class_hint,
                            selector,
                            self.describe_ptr(arg2),
                            self.describe_ptr(arg3),
                            origin,
                            phase,
                            routed
                                .as_ref()
                                .map(|(dispatch_target, hit_view, dispatched)| format!(
                                    "YES(dispatchTarget={}, hitView={}, selector={})",
                                    self.describe_ptr(*dispatch_target),
                                    self.describe_ptr(*hit_view),
                                    dispatched,
                                ))
                                .unwrap_or_else(|| "NO".to_string()),
                        ));
                        return Some(receiver);
                    }
                }
                _ => {}
            }
        }

        None
    }

    fn synthetic_selector_dt_trace(&self, selector_name: &str, arg2: u32, arg3: u32) -> Option<String> {
        let clean = selector_name.trim_matches('\0').trim();
        match clean {
            "update:" | "step:" | "tick:" => Some(format!(
                "decodedDt[arg2-f32]=0x{:08x}/{:.6}",
                arg2,
                Self::f32_from_bits(arg2)
            )),
            "setAnimationInterval:" => {
                let secs = self.nstimeinterval_secs_from_regs(arg2, arg3)?;
                Some(format!(
                    "decodedInterval[arg2:arg3-f64]=0x{:08x}:0x{:08x}/{:.6}",
                    arg3,
                    arg2,
                    secs,
                ))
            }
            _ => None,
        }
    }

    fn invoke_objc_selector_now_capture_r0(
        &mut self,
        receiver: u32,
        selector_name: &str,
        arg2: u32,
        arg3: u32,
        budget: u64,
        origin: &str,
    ) -> Option<u32> {
        let receiver = receiver & 0xFFFF_FFFF;
        if receiver == 0 || selector_name.trim().is_empty() {
            return None;
        }
        if self.begin_selector_dispatch_guard(receiver, selector_name, origin) {
            self.end_selector_dispatch_guard();
            return None;
        }
        let class_hint = self
            .objc_class_name_for_receiver(receiver)
            .unwrap_or_else(|| "<unknown-class>".to_string());
        if let Some(result) = self.maybe_invoke_synthetic_cocos_lifecycle_selector(receiver, selector_name, origin) {
            self.end_selector_dispatch_guard();
            return Some(result);
        }
        let Some(imp) = self.objc_lookup_imp_for_receiver(receiver, selector_name) else {
            if let Some(result) = self.maybe_invoke_objc_selector_missing_fastpath(
                receiver,
                &class_hint,
                selector_name,
                arg2,
                arg3,
                origin,
            ) {
                self.end_selector_dispatch_guard();
                return Some(result);
            }
            self.diag.trace.push(format!(
                "     ↳ synthetic selector invoke miss receiver={} class={} selector={} origin={}",
                self.describe_ptr(receiver),
                class_hint,
                selector_name,
                origin,
            ));
            self.end_selector_dispatch_guard();
            return None;
        };
        let Ok(selector_ptr) = self.alloc_selector_c_string(selector_name) else {
            self.diag.trace.push(format!(
                "     ↳ synthetic selector invoke selector-pool miss receiver={} selector={} origin={}",
                self.describe_ptr(receiver),
                selector_name,
                origin,
            ));
            self.end_selector_dispatch_guard();
            return None;
        };
        let return_pc = if (imp & 1) != 0 {
            HLE_STUB_UIAPPLICATION_POST_LAUNCH_THUMB
        } else {
            HLE_STUB_UIAPPLICATION_POST_LAUNCH_ARM
        };
        let return_thumb = (imp & 1) != 0;
        let return_lr = if return_thumb { return_pc | 1 } else { return_pc };
        let saved_regs = self.cpu.regs;
        let saved_thumb = self.cpu.thumb;
        let saved_stop_reason = self.diag.stop_reason.clone();
        let saved_status = self.diag.status.clone();
        let saved_exec_pc = self.exec.current_exec_pc;
        let saved_exec_word = self.exec.current_exec_word;
        let saved_exec_thumb = self.exec.current_exec_thumb;
        self.diag.stop_reason = "nested-selector-dispatch".to_string();
        self.cpu.regs[0] = receiver;
        self.cpu.regs[1] = selector_ptr;
        self.cpu.regs[2] = arg2;
        self.cpu.regs[3] = arg3;
        self.cpu.regs[14] = return_lr;
        self.cpu.regs[15] = imp & !1;
        self.cpu.thumb = (imp & 1) != 0;
        let mut ok = false;
        let step_budget = budget.max(1).min(250_000);
        for idx in 0..step_budget {
            if self.cpu.regs[15] == return_pc && self.cpu.thumb == return_thumb {
                ok = true;
                break;
            }
            let current_pc = self.cpu.regs[15];
            self.exec.current_exec_pc = current_pc;
            self.exec.current_exec_word = 0;
            self.exec.current_exec_thumb = self.cpu.thumb;
            let trace_index = self.diag.executed_instructions.saturating_add(idx);
            match self.handle_hle_stub(trace_index, current_pc) {
                Ok(Some(StepControl::Continue)) => {
                    self.process_runtime_post_step_hooks("nested-selector:hle-continue");
                    continue;
                }
                Ok(Some(StepControl::Stop(reason))) => {
                    self.diag.trace.push(format!(
                        "     ↳ synthetic selector invoke stopped receiver={} selector={} origin={} reason={}",
                        self.describe_ptr(receiver),
                        selector_name,
                        origin,
                        reason,
                    ));
                    self.diag.stop_reason = reason;
                    break;
                }
                Ok(None) => {}
                Err(err) => {
                    self.diag.trace.push(format!(
                        "     ↳ synthetic selector invoke stub error receiver={} selector={} origin={} error={}",
                        self.describe_ptr(receiver),
                        selector_name,
                        origin,
                        err,
                    ));
                    self.diag.stop_reason = format!("nested selector stub error: {err}");
                    break;
                }
            }
            self.diag.executed_instructions = self.diag.executed_instructions.saturating_add(1);
            let step_result = if self.cpu.thumb {
                match self.read_u16_le(current_pc) {
                    Ok(halfword) => {
                        self.exec.current_exec_word = halfword as u32;
                        self.step_thumb(halfword, current_pc)
                    }
                    Err(err) => Err(err),
                }
            } else {
                match self.read_u32_le(current_pc) {
                    Ok(word) => {
                        self.exec.current_exec_word = word;
                        self.step_arm(word, current_pc)
                    }
                    Err(err) => Err(err),
                }
            };
            match step_result {
                Ok(StepControl::Continue) => {
                    self.process_runtime_post_step_hooks("nested-selector:step");
                }
                Ok(StepControl::Stop(reason)) => {
                    self.diag.trace.push(format!(
                        "     ↳ synthetic selector invoke step-stop receiver={} selector={} origin={} reason={}",
                        self.describe_ptr(receiver),
                        selector_name,
                        origin,
                        reason,
                    ));
                    self.diag.stop_reason = reason;
                    break;
                }
                Err(err) => {
                    self.diag.trace.push(format!(
                        "     ↳ synthetic selector invoke step-error receiver={} selector={} origin={} error={}",
                        self.describe_ptr(receiver),
                        selector_name,
                        origin,
                        err,
                    ));
                    self.diag.stop_reason = format!("nested selector step error: {err}");
                    break;
                }
            }
        }
        if self.cpu.regs[15] == return_pc && self.cpu.thumb == return_thumb {
            ok = true;
        }
        let result_r0 = self.cpu.regs[0];
        self.cpu.regs = saved_regs;
        self.cpu.thumb = saved_thumb;
        self.diag.stop_reason = saved_stop_reason;
        self.diag.status = saved_status;
        self.exec.current_exec_pc = saved_exec_pc;
        self.exec.current_exec_word = saved_exec_word;
        self.exec.current_exec_thumb = saved_exec_thumb;
        let dt_trace = self.synthetic_selector_dt_trace(selector_name, arg2, arg3);
        self.diag.trace.push(format!(
            "     ↳ synthetic selector invoke receiver={} selector={} arg2={} arg3={} origin={} imp=0x{:08x} result={} ok={}{}",
            self.describe_ptr(receiver),
            selector_name,
            self.describe_ptr(arg2),
            self.describe_ptr(arg3),
            origin,
            imp,
            self.describe_ptr(result_r0),
            if ok { "YES" } else { "NO" },
            dt_trace
                .as_deref()
                .map(|value| format!(" {}", value))
                .unwrap_or_default(),
        ));
        self.end_selector_dispatch_guard();
        if ok { Some(result_r0) } else { None }
    }

    fn invoke_objc_selector_now(
        &mut self,
        receiver: u32,
        selector_name: &str,
        arg2: u32,
        arg3: u32,
        budget: u64,
        origin: &str,
    ) -> bool {
        self.invoke_objc_selector_now_capture_r0(receiver, selector_name, arg2, arg3, budget, origin)
            .is_some()
    }

    fn maybe_fire_synthetic_menu_probe(&mut self, scene: u32, origin: &str, age: u32) {
        let Some(selector_name) = self.tuning.synthetic_menu_probe_selector.clone() else {
            return;
        };
        if self.runtime.scene.synthetic_menu_probe_fired || self.runtime.scene.synthetic_menu_probe_inflight {
            return;
        }
        if self.runtime.scene.synthetic_menu_probe_attempts >= 3 {
            return;
        }
        let fire_after = self.tuning.synthetic_menu_probe_after_ticks.max(1);
        if age < fire_after {
            return;
        }
        let Some(item) = self.find_synthetic_menu_item_by_selector(scene, &selector_name) else {
            if age == fire_after || ((age - fire_after) % 4 == 0) {
                self.diag.trace.push(format!(
                    "     ↳ synthetic menu probe pending scene={} selector={} age={} origin={} attempts={}",
                    self.describe_ptr(scene),
                    selector_name,
                    age,
                    origin,
                    self.runtime.scene.synthetic_menu_probe_attempts,
                ));
            }
            return;
        };
        let state = self.runtime.graphics.synthetic_sprites.get(&item).cloned().unwrap_or_default();
        let callback_target = if state.callback_target != 0 { state.callback_target } else { scene };
        let callback_selector = if state.callback_selector != 0 {
            self.objc_read_selector_name(state.callback_selector).unwrap_or_else(|| selector_name.clone())
        } else {
            selector_name.clone()
        };
        self.runtime.scene.synthetic_menu_probe_attempts = self.runtime.scene.synthetic_menu_probe_attempts.saturating_add(1);
        self.runtime.scene.synthetic_menu_probe_inflight = true;
        self.runtime.scene.synthetic_touch_injections = self.runtime.scene.synthetic_touch_injections.saturating_add(1);
        self.runtime.ui_objects.first_responder = item;
        self.set_synthetic_menu_item_pressed(item, true);
        self.diag.trace.push(format!(
            "     ↳ hle synthetic menu touch.begin scene={} item={} selector={} target={} origin={} age={} injections={} attempt={}",
            self.describe_ptr(scene),
            self.describe_ptr(item),
            callback_selector,
            self.describe_ptr(callback_target),
            origin,
            age,
            self.runtime.scene.synthetic_touch_injections,
            self.runtime.scene.synthetic_menu_probe_attempts,
        ));
        let invoked = self.invoke_objc_selector_now(callback_target, &callback_selector, item, 0, 120_000, "synthetic-menu-probe");
        self.runtime.scene.synthetic_touch_injections = self.runtime.scene.synthetic_touch_injections.saturating_add(1);
        self.set_synthetic_menu_item_pressed(item, false);
        self.runtime.scene.synthetic_menu_probe_inflight = false;
        self.diag.trace.push(format!(
            "     ↳ hle synthetic menu touch.end scene={} item={} selector={} target={} origin={} age={} injections={} invoked={} runningScene={}",
            self.describe_ptr(scene),
            self.describe_ptr(item),
            callback_selector,
            self.describe_ptr(callback_target),
            origin,
            age,
            self.runtime.scene.synthetic_touch_injections,
            if invoked { "YES" } else { "NO" },
            self.describe_ptr(self.runtime.ui_cocos.running_scene),
        ));
        if invoked {
            self.runtime.scene.synthetic_menu_probe_fired = true;
        }
    }

    fn maybe_auto_advance_howto_scene(&mut self, scene: u32, label: &str, origin: &str, age: u32) -> Vec<String> {
        if scene == 0 || !label.contains("HowToScene") {
            return Vec::new();
        }
        let mut forced = Vec::new();
        let should_probe = matches!(age, 6 | 12 | 24)
            || self
                .runtime.host_input.last_dispatch
                .as_deref()
                .map(|value| value.contains("without-active-touch") || value.contains("miss-down"))
                .unwrap_or(false);
        if !should_probe {
            return forced;
        }
        if let Some(item) = self
            .find_synthetic_menu_item_by_selector(scene, "nextCallback")
            .or_else(|| self.find_synthetic_menu_item_by_selector(scene, "playCallback"))
        {
            if let Some((callback_target, _, selector_name)) = self.synthetic_callback_for_node(item) {
                self.runtime.scene.synthetic_touch_injections = self.runtime.scene.synthetic_touch_injections.saturating_add(1);
                self.runtime.ui_objects.first_responder = item;
                self.set_synthetic_menu_item_pressed(item, false);
                if self.invoke_objc_selector_now(callback_target, &selector_name, item, 0, 120_000, "howto-auto-advance") {
                    forced.push(selector_name);
                }
            }
        }
        if !forced.is_empty() {
            self.diag.trace.push(format!(
                "     ↳ hle howto auto-advance scene={} age={} origin={} forced=[{}] lastDispatch={}",
                self.describe_ptr(scene),
                age,
                origin,
                forced.join(","),
                self.runtime.host_input.last_dispatch.clone().unwrap_or_else(|| "<none>".to_string()),
            ));
        }
        forced
    }

    fn maybe_trace_missing_running_scene(&mut self, origin: &str, root_scene: u32, watch_scene: u32) {
        let tick = self.runtime.ui_runtime.runloop_ticks;
        let should_trace = matches!(tick, 1 | 2 | 4 | 8 | 16 | 32 | 64) || (tick != 0 && tick % 32 == 0);
        if !should_trace {
            return;
        }
        let recent_scene = if self.runtime.ui_cocos.scene_recent_events.is_empty() {
            "<none>".to_string()
        } else {
            self.runtime
                .ui_cocos
                .scene_recent_events
                .iter()
                .rev()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let recent_scheduler = if self.runtime.ui_cocos.scheduler_recent_events.is_empty() {
            "<none>".to_string()
        } else {
            self.runtime
                .ui_cocos
                .scheduler_recent_events
                .iter()
                .rev()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let pending_route_selector = self
            .runtime
            .ui_cocos
            .pending_scene_route_selector
            .clone()
            .unwrap_or_else(|| "<none>".to_string());
        self.push_scene_progress_trace(format!(
            "scene.nil-watch tick={} origin={} root={} watch={} running={} next={} effect={} director={} view={} transitionPending={} pendingSelector={} pendingDest={} networkCompleted={} recentScene={} recentScheduler={}",
            tick,
            origin,
            if root_scene != 0 { self.describe_ptr(root_scene) } else { "nil".to_string() },
            if watch_scene != 0 { self.describe_ptr(watch_scene) } else { "nil".to_string() },
            if self.runtime.ui_cocos.running_scene != 0 { self.describe_ptr(self.runtime.ui_cocos.running_scene) } else { "nil".to_string() },
            if self.runtime.ui_cocos.next_scene != 0 { self.describe_ptr(self.runtime.ui_cocos.next_scene) } else { "nil".to_string() },
            if self.runtime.ui_cocos.effect_scene != 0 { self.describe_ptr(self.runtime.ui_cocos.effect_scene) } else { "nil".to_string() },
            if self.runtime.ui_cocos.cocos_director != 0 { self.describe_ptr(self.runtime.ui_cocos.cocos_director) } else { "nil".to_string() },
            if self.runtime.ui_cocos.opengl_view != 0 { self.describe_ptr(self.runtime.ui_cocos.opengl_view) } else { "nil".to_string() },
            if self.runtime.ui_cocos.scene_handoff_pending { "YES" } else { "NO" },
            pending_route_selector,
            if self.runtime.ui_cocos.pending_scene_route_destination != 0 { self.describe_ptr(self.runtime.ui_cocos.pending_scene_route_destination) } else { "nil".to_string() },
            if self.runtime.ui_network.network_completed { "YES" } else { "NO" },
            recent_scene,
            recent_scheduler,
        ));
        self.diag.trace.push(format!(
            "     ↳ hle scene.nil-watch tick={} origin={} running=nil next={} effect={} director={} view={} transitionPending={} pendingSelector={} pendingDest={} networkCompleted={} recentScene={} recentScheduler={}",
            tick,
            origin,
            if self.runtime.ui_cocos.next_scene != 0 { self.describe_ptr(self.runtime.ui_cocos.next_scene) } else { "nil".to_string() },
            if self.runtime.ui_cocos.effect_scene != 0 { self.describe_ptr(self.runtime.ui_cocos.effect_scene) } else { "nil".to_string() },
            if self.runtime.ui_cocos.cocos_director != 0 { self.describe_ptr(self.runtime.ui_cocos.cocos_director) } else { "nil".to_string() },
            if self.runtime.ui_cocos.opengl_view != 0 { self.describe_ptr(self.runtime.ui_cocos.opengl_view) } else { "nil".to_string() },
            if self.runtime.ui_cocos.scene_handoff_pending { "YES" } else { "NO" },
            self.runtime
                .ui_cocos
                .pending_scene_route_selector
                .clone()
                .unwrap_or_else(|| "<none>".to_string()),
            if self.runtime.ui_cocos.pending_scene_route_destination != 0 { self.describe_ptr(self.runtime.ui_cocos.pending_scene_route_destination) } else { "nil".to_string() },
            if self.runtime.ui_network.network_completed { "YES" } else { "NO" },
            recent_scene,
            recent_scheduler,
        ));
    }

    fn maybe_drive_synthetic_scene_progression(&mut self, origin: &str) {
        let _ = self.maybe_commit_pending_scene_route(&format!("scene-progress:{origin}"));
        let root_scene = if self.active_effect_scene() != 0 {
            self.active_effect_scene()
        } else {
            self.runtime.ui_cocos.running_scene
        };
        let scene = self.resolve_synthetic_progress_watch_scene(root_scene);
        if scene == 0 || !self.runtime.graphics.synthetic_sprites.contains_key(&scene) {
            self.maybe_trace_missing_running_scene(origin, root_scene, scene);
            self.runtime.scene.synthetic_last_running_scene = scene;
            self.runtime.scene.synthetic_running_scene_ticks = 0;
            return;
        }

        if self.runtime.scene.synthetic_last_running_scene != scene {
            self.runtime.scene.synthetic_last_running_scene = scene;
            self.runtime.scene.synthetic_running_scene_ticks = 0;
            let label = self.diag.object_labels.get(&scene).cloned().unwrap_or_default();
            let destination = self.runtime.graphics.synthetic_splash_destinations.get(&scene).copied().unwrap_or(0);
            if self.is_loading_like_scene_label(&label) {
                self.arm_scheduler_trace_window(scene, origin, &label);
            }
            self.push_scene_event(format!("watch {} origin={} label={} destination={}", self.describe_ptr(scene), origin, if label.is_empty() { "<unknown>".to_string() } else { label.clone() }, if destination != 0 { self.describe_ptr(destination) } else { "nil".to_string() }));
            self.push_scene_progress_trace(format!(
                "scene.watch scene={} origin={} label={} touch={} destination={}",
                self.describe_ptr(scene),
                origin,
                if label.is_empty() { "<unknown>".to_string() } else { label.clone() },
                if self.runtime.graphics.synthetic_sprites.get(&scene).map(|state| state.touch_enabled).unwrap_or(false) { "YES" } else { "NO" },
                if destination != 0 { self.describe_ptr(destination) } else { "nil".to_string() },
            ));
            self.diag.trace.push(format!(
                "     ↳ hle scene.watch active scene={} origin={} label={} touch={} destination={}",
                self.describe_ptr(scene),
                origin,
                if label.is_empty() { "<unknown>".to_string() } else { label },
                if self.runtime.graphics.synthetic_sprites.get(&scene).map(|state| state.touch_enabled).unwrap_or(false) { "YES" } else { "NO" },
                if destination != 0 { self.describe_ptr(destination) } else { "nil".to_string() },
            ));
        }

        self.runtime.scene.synthetic_running_scene_ticks = self.runtime.scene.synthetic_running_scene_ticks.saturating_add(1);
        let age = self.runtime.scene.synthetic_running_scene_ticks;
        let label = self.diag.object_labels.get(&scene).cloned().unwrap_or_default();
        let touch_enabled = self.runtime.graphics.synthetic_sprites.get(&scene).map(|state| state.touch_enabled).unwrap_or(false);
        let destination = self.runtime.graphics.synthetic_splash_destinations.get(&scene).copied().unwrap_or(0);
        let is_splash = self.is_loading_like_scene_label(&label);
        let is_transition = Self::is_transition_like_label(&label) && destination != 0;

        if is_transition {
            if matches!(age, 1 | 2 | 4 | 6) {
                self.diag.trace.push(format!(
                    "     ↳ hle transition.watch scene={} age={} origin={} destination={} entered={} childCount={} state=[{}]",
                    self.describe_ptr(scene),
                    age,
                    origin,
                    self.describe_ptr(destination),
                    if self.runtime.graphics.synthetic_sprites.get(&destination).map(|state| state.entered).unwrap_or(false) { "YES" } else { "NO" },
                    self.runtime.graphics.synthetic_sprites
                        .get(&destination)
                        .map(|state| if state.children != 0 { self.synthetic_array_len(state.children) } else { 0 })
                        .unwrap_or(0),
                    self.describe_node_graph_state(destination),
                ));
            }
            if age >= 6 {
                self.runtime.scene.synthetic_scene_transitions = self.runtime.scene.synthetic_scene_transitions.saturating_add(1);
                self.runtime.ui_cocos.scene_transition_calls = self.runtime.ui_cocos.scene_transition_calls.saturating_add(1);
                self.diag.trace.push(format!(
                    "     ↳ hle synthetic transition finish scene={} -> {} origin={} age={} transitions={}",
                    self.describe_ptr(scene),
                    self.describe_ptr(destination),
                    origin,
                    age,
                    self.runtime.scene.synthetic_scene_transitions,
                ));
                self.activate_running_scene(destination, "synthetic-transition-finish");
            }
            return;
        }

        if !is_splash {
            let reconciled = self.maybe_reconcile_guest_scene_graph(scene, origin, age);
            if reconciled != 0 {
                self.diag.trace.push(format!(
                    "     ↳ hle guest scene-graph reconcile scene={} origin={} age={} attached={} state=[{}]",
                    self.describe_ptr(scene),
                    origin,
                    age,
                    reconciled,
                    self.describe_node_graph_state(scene),
                ));
                self.push_scene_progress_trace(format!(
                    "scene.graph.reconcile scene={} origin={} age={} attached={}",
                    self.describe_ptr(scene),
                    origin,
                    age,
                    reconciled,
                ));
            }
            let forced = self.maybe_auto_advance_howto_scene(scene, &label, origin, age);
            if !forced.is_empty() {
                self.push_scene_progress_trace(format!(
                    "howto.auto-advance scene={} origin={} age={} forced=[{}]",
                    self.describe_ptr(scene),
                    origin,
                    age,
                    forced.join(","),
                ));
            }
            self.maybe_fire_synthetic_menu_probe(scene, origin, age);
            return;
        }

        if destination == 0 && self.maybe_prime_loading_scene_startup(scene, origin, age) {
            let cocos_count = self.runtime.scheduler.timers.cocos_selectors.values().filter(|entry| entry.target == scene).count();
            let foundation_count = self.runtime.scheduler.timers.foundation_timers.values().filter(|entry| entry.target == scene).count();
            let delayed_count = self.runtime.scheduler.timers.delayed_selectors.iter().filter(|entry| entry.target == scene).count();
            if cocos_count > 0 || foundation_count > 0 || delayed_count > 0 {
                self.diag.trace.push(format!(
                    "     ↳ hle loading-scene scheduled scene={} cocos={} foundation={} delayed={} origin={} age={}",
                    self.describe_ptr(scene),
                    cocos_count,
                    foundation_count,
                    delayed_count,
                    origin,
                    age,
                ));
            }
            return;
        }

        if matches!(age, 1 | 2 | 4 | 6) {
            self.diag.trace.push(format!(
                "     ↳ hle splash.watch scene={} age={} origin={} touch={} networkCompleted={} idleAfterCompletion={} destination={}",
                self.describe_ptr(scene),
                age,
                origin,
                if touch_enabled { "YES" } else { "NO" },
                if self.runtime.ui_network.network_completed { "YES" } else { "NO" },
                self.runtime.ui_runtime.idle_ticks_after_completion,
                if destination != 0 { self.describe_ptr(destination) } else { "nil".to_string() },
            ));
        }

        if touch_enabled && age == 3 {
            self.runtime.scene.synthetic_touch_injections = self.runtime.scene.synthetic_touch_injections.saturating_add(1);
            self.runtime.ui_objects.first_responder = scene;
            self.diag.trace.push(format!(
                "     ↳ hle synthetic touch.begin target={} frame#{} age={} injections={} reason=splash-progress",
                self.describe_ptr(scene),
                self.runtime.ui_graphics.graphics_frame_index,
                age,
                self.runtime.scene.synthetic_touch_injections,
            ));
        }

        if touch_enabled && age == 4 {
            self.runtime.scene.synthetic_touch_injections = self.runtime.scene.synthetic_touch_injections.saturating_add(1);
            self.diag.trace.push(format!(
                "     ↳ hle synthetic touch.end target={} frame#{} age={} injections={} reason=splash-progress",
                self.describe_ptr(scene),
                self.runtime.ui_graphics.graphics_frame_index,
                age,
                self.runtime.scene.synthetic_touch_injections,
            ));
        }

        let auto_advance_idle_threshold = self.synthetic_splash_auto_advance_idle_threshold();
        let auto_advance_age_threshold = self.synthetic_splash_auto_advance_age_threshold();
        let network_ready = self.runtime.ui_network.network_completed
            && self.runtime.ui_runtime.idle_ticks_after_completion >= auto_advance_idle_threshold;
        let should_advance = destination != 0 && (network_ready || age >= auto_advance_age_threshold);
        if destination != 0 && !should_advance {
            let watch_age = auto_advance_age_threshold.saturating_div(2).max(1);
            if age == 4 || age == watch_age || age == auto_advance_age_threshold {
                self.push_scene_progress_trace(format!(
                    "splash.defer scene={} destination={} origin={} age={} idleAfterCompletion={} idleThreshold={} ageThreshold={} titleAboveBelow={}",
                    self.describe_ptr(scene),
                    self.describe_ptr(destination),
                    origin,
                    age,
                    self.runtime.ui_runtime.idle_ticks_after_completion,
                    auto_advance_idle_threshold,
                    auto_advance_age_threshold,
                    if self.has_specific_profile() { "YES" } else { "NO" },
                ));
                self.diag.trace.push(format!(
                    "     ↳ hle synthetic splash defer scene={} destination={} origin={} age={} idleAfterCompletion={} idleThreshold={} ageThreshold={} titleAboveBelow={}",
                    self.describe_ptr(scene),
                    self.describe_ptr(destination),
                    origin,
                    age,
                    self.runtime.ui_runtime.idle_ticks_after_completion,
                    auto_advance_idle_threshold,
                    auto_advance_age_threshold,
                    if self.has_specific_profile() { "YES" } else { "NO" },
                ));
            }
        }
        if should_advance {
            self.runtime.scene.synthetic_scene_transitions = self.runtime.scene.synthetic_scene_transitions.saturating_add(1);
            self.runtime.ui_cocos.scene_transition_calls = self.runtime.ui_cocos.scene_transition_calls.saturating_add(1);
            self.push_scene_progress_trace(format!(
                "splash.auto-advance scene={} destination={} origin={} age={} idleAfterCompletion={} idleThreshold={} ageThreshold={} transitions={} touches={}",
                self.describe_ptr(scene),
                self.describe_ptr(destination),
                origin,
                age,
                self.runtime.ui_runtime.idle_ticks_after_completion,
                auto_advance_idle_threshold,
                auto_advance_age_threshold,
                self.runtime.scene.synthetic_scene_transitions,
                self.runtime.scene.synthetic_touch_injections,
            ));
            self.diag.trace.push(format!(
                "     ↳ hle synthetic splash auto-advance scene={} -> {} origin={} age={} idleAfterCompletion={} idleThreshold={} ageThreshold={} transitions={} touches={}",
                self.describe_ptr(scene),
                self.describe_ptr(destination),
                origin,
                age,
                self.runtime.ui_runtime.idle_ticks_after_completion,
                auto_advance_idle_threshold,
                auto_advance_age_threshold,
                self.runtime.scene.synthetic_scene_transitions,
                self.runtime.scene.synthetic_touch_injections,
            ));
            self.activate_running_scene(destination, "synthetic-splash-autoadvance");
        }
    }


    fn collect_cocos_action_varargs(&self, arg2: u32, arg3: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for value in [arg2, arg3] {
            if value != 0 && !out.contains(&value) {
                out.push(value);
            }
        }
        for idx in 0..8usize {
            let value = self.peek_stack_u32(idx as u32).unwrap_or(0);
            if value == 0 {
                break;
            }
            if !out.contains(&value) {
                out.push(value);
            }
        }
        out
    }

    fn should_fastpath_cocos_super_init(&self, class_desc: &str, receiver: u32) -> bool {
        let receiver_label = self.diag.object_labels.get(&receiver).map(|v| v.as_str()).unwrap_or("");
        if class_desc.contains("CCNode")
            || class_desc.contains("CCLayer")
            || class_desc.contains("CCScene")
            || class_desc.contains("CCColorLayer")
            || self.active_profile().is_menu_layer_label(class_desc)
            || self.active_profile().is_first_scene_label(class_desc)
            || Self::is_transition_like_label(class_desc)
        {
            return true;
        }
        receiver_label.contains("CCNode")
            || receiver_label.contains("CCLayer")
            || receiver_label.contains("CCScene")
            || receiver_label.contains("CCColorLayer")
            || self.active_profile().is_menu_layer_label(&receiver_label)
            || self.active_profile().is_first_scene_label(&receiver_label)
            || Self::is_transition_like_label(receiver_label)
    }

    fn apply_cocos_init_defaults(&mut self, receiver: u32, class_hint: &str, reason: &str) -> String {
        let label = self.diag.object_labels.get(&receiver).cloned().unwrap_or_default();
        let size_like = class_hint.contains("CCLayer")
            || class_hint.contains("CCScene")
            || class_hint.contains("CCColorLayer")
            || self.active_profile().is_menu_layer_label(class_hint)
            || self.active_profile().is_first_scene_label(class_hint)
            || Self::is_transition_like_label(class_hint)
            || label.contains("CCLayer")
            || label.contains("CCScene")
            || label.contains("CCColorLayer")
            || self.active_profile().is_menu_layer_label(&label)
            || self.active_profile().is_first_scene_label(&label)
            || Self::is_transition_like_label(&label);
        let touch_like = class_hint.contains("CCMenu") || label.contains("CCMenu");
        let surface_w = self.runtime.ui_graphics.graphics_surface_width.max(1);
        let surface_h = self.runtime.ui_graphics.graphics_surface_height.max(1);
        {
            let state = self.ensure_synthetic_sprite_state(receiver);
            state.visible = true;
            let anchor_relative_like = Self::is_label_class_name(class_hint)
                || class_hint.contains("CCSprite")
                || Self::is_menu_item_class_name(class_hint)
                || label.contains("CCSprite")
                || Self::is_label_class_name(&label)
                || Self::is_menu_item_class_name(&label);
            if !state.relative_anchor_point_explicit {
                state.relative_anchor_point = anchor_relative_like;
            }
            if size_like {
                if state.width == 0 {
                    state.width = surface_w;
                }
                if state.height == 0 {
                    state.height = surface_h;
                }
            }
            if touch_like {
                state.touch_enabled = true;
            }
        }
        format!(
            "cocos {} class={} state=[{}]",
            reason,
            if class_hint.is_empty() { "<unknown>" } else { class_hint },
            self.describe_node_graph_state(receiver),
        )
    }

    fn maybe_auto_layout_menu(&mut self, menu: u32) -> Option<String> {
        let children = self.runtime.graphics.synthetic_sprites.get(&menu).map(|state| state.children).unwrap_or(0);
        if children == 0 || self.synthetic_array_len(children) <= 1 {
            return None;
        }
        let items = self.runtime.graphics.synthetic_arrays.get(&children).map(|v| v.items.clone()).unwrap_or_default();
        if items.is_empty() {
            return None;
        }
        let mut all_at_origin = true;
        for item in &items {
            if let Some(state) = self.runtime.graphics.synthetic_sprites.get(item) {
                if state.position_x_bits != 0 || state.position_y_bits != 0 {
                    all_at_origin = false;
                    break;
                }
            }
        }
        if !all_at_origin {
            return None;
        }
        Some(self.layout_menu_children_vertically(menu, 12.0))
    }

    fn maybe_handle_cocos_fastpath(&mut self, selector: &str, receiver: u32, arg2: u32, arg3: u32) -> Option<(u32, String)> {
        self.ensure_objc_metadata_indexed();
        let class_name = self.objc_receiver_class_name_hint(receiver);
        let class_str = class_name.as_deref().unwrap_or("");
        let receiver_inherits_director = self.objc_receiver_inherits_named(receiver, "CCDirector");
        let on_director = selector == "sharedDirector"
            || receiver == self.runtime.ui_cocos.cocos_director
            || Self::is_cocos_director_class_name(class_str)
            || receiver_inherits_director;
        let on_gl_view = receiver == self.runtime.ui_cocos.opengl_view || Self::is_gl_view_class_name(class_str);
        if on_gl_view && receiver != 0 {
            self.runtime.ui_cocos.opengl_view = receiver;
            if self.runtime.ui_objects.first_responder == 0 || self.runtime.ui_objects.first_responder == self.runtime.ui_objects.root_controller {
                self.runtime.ui_objects.first_responder = receiver;
            }
            self.diag.object_labels
                .entry(receiver)
                .or_insert_with(|| "EAGLView.synthetic#0".to_string());
        }
        if on_director && self.should_defer_to_real_cocos_director_bootstrap_imp(selector, receiver) {
            self.diag.trace.push(format!(
                "     ↳ hle cocos.fastpath defer selector={} receiver={} class={} reason=real-director-bootstrap-imp",
                selector,
                self.describe_ptr(receiver),
                if class_str.is_empty() { "<unknown>" } else { class_str },
            ));
            return None;
        }
        let mut receiver_label = self.diag.object_labels.get(&receiver).cloned().unwrap_or_default();
        let sprite_factory_selector = matches!(selector, "spriteWithFile:" | "initWithFile:" | "spriteWithFile:rect:" | "initWithFile:rect:");
        let sprite_factory_receiver = sprite_factory_selector
            && (
                class_str.contains("Sprite")
                    || receiver_label.contains("Sprite")
                    || self.objc_class_name_for_ptr(receiver).map(|name| name.contains("Sprite")).unwrap_or(false)
                    || (
                        self.objc_lookup_imp_for_receiver(receiver, "alloc").is_some()
                            && (
                                self.objc_lookup_imp_for_receiver(receiver, "initWithTexture:rect:").is_some()
                                    || self.objc_lookup_imp_for_receiver(receiver, "initWithTexture:").is_some()
                                    || self.objc_lookup_imp_for_receiver(receiver, "initWithFile:").is_some()
                            )
                    )
            );
        if sprite_factory_receiver && receiver != 0 && receiver_label.is_empty() {
            receiver_label = "CCSprite.class(guest)".to_string();
            self.diag.object_labels.entry(receiver).or_insert_with(|| receiver_label.clone());
        }
        let on_texture_cache = receiver == self.runtime.graphics.cocos_texture_cache_object || Self::is_texture_cache_class_name(class_str);
        let on_texture = self.runtime.graphics.synthetic_textures.contains_key(&receiver)
            || self.runtime.graphics.synthetic_images.contains_key(&receiver)
            || Self::is_texture_class_name(class_str);
        let receiver_inherits_sprite = self.objc_receiver_inherits_named(receiver, "CCSprite");
        let receiver_inherits_menu_item = self.objc_receiver_inherits_named(receiver, "CCMenuItem");
        let receiver_inherits_menu = self.objc_receiver_inherits_named(receiver, "CCMenu");
        let receiver_inherits_node = self.objc_receiver_inherits_named(receiver, "CCNode");
        let receiver_inherits_texture_node = self.objc_receiver_inherits_named(receiver, "TextureNode");
        let receiver_inherits_layer = self.objc_receiver_inherits_named(receiver, "CCLayer");
        let receiver_inherits_color_layer = self.objc_receiver_inherits_named(receiver, "CCColorLayer");
        let receiver_inherits_scene = self.objc_receiver_inherits_named(receiver, "CCScene");
        let receiver_inherits_transition_scene = self.objc_receiver_inherits_named(receiver, "CCTransitionScene");
        let receiver_looks_node_like_by_selectors = receiver != 0 && [
            "setPositionBL:",
            "setTransformAnchor:",
            "setSize:",
            "size",
            "visit",
            "draw",
            "addChild:",
            "children",
        ]
        .iter()
        .any(|sel| self.objc_lookup_imp_for_receiver(receiver, sel).is_some());
        let receiver_looks_texture_node_by_selectors = receiver != 0 && [
            "setTexture:",
            "texture",
            "setTextureRect:",
            "setDisplayFrame:index:",
            "initWithTexture:",
            "initWithTexture:rect:",
        ]
        .iter()
        .any(|sel| self.objc_lookup_imp_for_receiver(receiver, sel).is_some());
        let on_transition = Self::is_transition_like_label(class_str)
            || Self::is_transition_like_label(&receiver_label)
            || receiver_inherits_transition_scene;
        let on_sprite = class_str.contains("CCSprite")
            || receiver_label.contains("CCSprite")
            || class_str.contains("Sprite")
            || sprite_factory_receiver
            || receiver_inherits_sprite;
        let on_sprite_sheet = class_str.contains("SpriteSheet")
            || class_str.contains("BatchNode")
            || class_str.contains("TextureAtlas")
            || receiver_label.contains("SpriteSheet")
            || receiver_label.contains("BatchNode")
            || receiver_label.contains("TextureAtlas")
            || self.objc_receiver_inherits_named(receiver, "CCSpriteSheet")
            || self.objc_receiver_inherits_named(receiver, "CCSpriteBatchNode");
        let on_menu_item = Self::is_menu_item_class_name(class_str)
            || receiver_label.contains("CCMenuItem")
            || receiver_inherits_menu_item;
        let on_menu = Self::is_menu_class_name(class_str)
            || receiver_label.contains("CCMenu<")
            || receiver_label.starts_with("CCMenu.")
            || receiver_label.contains("CCMenu.instance")
            || receiver_inherits_menu;
        let on_label = Self::is_label_class_name(class_str)
            || Self::is_label_class_name(&receiver_label)
            || self.objc_receiver_inherits_named(receiver, "CCLabel")
            || self.objc_receiver_inherits_named(receiver, "CCLabelTTF")
            || self.objc_receiver_inherits_named(receiver, "CCLabelBMFont");
        let on_texture_node = class_str.contains("TextureNode")
            || receiver_label.contains("TextureNode")
            || receiver_inherits_texture_node
            || receiver_looks_texture_node_by_selectors;
        let has_synthetic_node_state = self.runtime.graphics.synthetic_sprites.contains_key(&receiver);
        let on_cocos_node = on_sprite
            || on_menu_item
            || on_menu
            || on_label
            || on_transition
            || class_str.contains("CCNode")
            || class_str.contains("CCLayer")
            || class_str.contains("CCColorLayer")
            || class_str.contains("CCScene")
            || self.active_profile().is_menu_layer_label(class_str)
            || receiver_label.contains("CCNode")
            || receiver_label.contains("CCLayer")
            || receiver_label.contains("CCColorLayer")
            || receiver_label.contains("CCScene")
            || receiver_inherits_node
            || receiver_inherits_layer
            || receiver_inherits_color_layer
            || receiver_inherits_scene
            || receiver_inherits_texture_node
            || receiver_looks_node_like_by_selectors
            || receiver_looks_texture_node_by_selectors
            || self.active_profile().is_first_scene_label(&receiver_label)
            || self.active_profile().is_menu_layer_label(&receiver_label)
            || has_synthetic_node_state;
        if receiver != 0 {
            self.maybe_trace_widget_selector_state(receiver, selector, "pre");
        }
        let on_color_layer = class_str.contains("CCColorLayer") || receiver_label.contains("CCColorLayer");
        let on_audio_manager = class_str.contains("CDAudioManager") || receiver_label.contains("CDAudioManager");
        let on_sound_engine = class_str.contains("CDSoundEngine") || receiver_label.contains("CDSoundEngine");
        match selector {
            "actionWithTarget:selector:" if class_str.contains("CCCallFunc") => {
                let selector_name = self.decode_cocos_schedule_selector_name(arg3).unwrap_or_else(|| format!("0x{arg3:08x}"));
                self.note_passive_loading_callfunc(arg2, &selector_name, 0, selector);
                None
            }
            "actionWithTarget:selector:data:" if class_str.contains("CCCallFunc") => {
                let data_arg = self.peek_stack_u32(0).unwrap_or(0);
                let selector_name = self.decode_cocos_schedule_selector_name(arg3).unwrap_or_else(|| format!("0x{arg3:08x}"));
                self.note_passive_loading_callfunc(arg2, &selector_name, data_arg, selector);
                None
            }
            "initWithTarget:selector:" if class_str.contains("CCCallFunc") || receiver_label.contains("CCCallFunc") => {
                let selector_name = self.decode_cocos_schedule_selector_name(arg3).unwrap_or_else(|| format!("0x{arg3:08x}"));
                self.note_passive_loading_callfunc(arg2, &selector_name, 0, selector);
                None
            }
            "initWithTarget:selector:data:" if class_str.contains("CCCallFunc") || receiver_label.contains("CCCallFunc") => {
                let data_arg = self.peek_stack_u32(0).unwrap_or(0);
                let selector_name = self.decode_cocos_schedule_selector_name(arg3).unwrap_or_else(|| format!("0x{arg3:08x}"));
                self.note_passive_loading_callfunc(arg2, &selector_name, data_arg, selector);
                None
            }
            "actionWithDuration:" if class_str.contains("CCDelayTime") => {
                self.note_passive_loading_delay(arg2, selector);
                None
            }
            "initWithDuration:" if class_str.contains("CCDelayTime") || receiver_label.contains("CCDelayTime") => {
                self.note_passive_loading_delay(arg2, selector);
                None
            }
            "actionWithDuration:scaleX:scaleY:" if class_str.contains("CCScaleTo") || class_str.contains("CCScaleBy") => {
                None
            }
            "initWithDuration:scaleX:scaleY:" if receiver_label.contains("CCScaleTo") || receiver_label.contains("CCScaleBy") || class_str.contains("CCScaleTo") || class_str.contains("CCScaleBy") => {
                let class_hint = if receiver_label.contains("CCScaleBy") || class_str.contains("CCScaleBy") { "CCScaleBy" } else { "CCScaleTo" };
                let kind = if class_hint.contains("ScaleBy") { "interval-scale-by" } else { "interval-scale-to" };
                let decoded = self.read_msgsend_float_triplet_arg();
                let (duration_bits, scale_x_bits, scale_y_bits, source) = if let Some((bits, source)) = decoded {
                    (bits[0], Some(bits[1]), Some(bits[2]), source)
                } else {
                    (arg2, None, None, "fallback".to_string())
                };
                self.note_synthetic_cocos_interval_action(receiver, class_hint, kind, duration_bits, scale_x_bits, scale_y_bits, selector);
                if let (Some(scale_x_bits), Some(scale_y_bits)) = (scale_x_bits, scale_y_bits) {
                    self.push_callback_trace(format!(
                        "action.interval.decode action={} class={} source={} duration={:.3} scale=({:.3},{:.3}) origin={}",
                        self.describe_ptr(receiver),
                        class_hint,
                        source,
                        Self::f32_from_bits(duration_bits),
                        Self::f32_from_bits(scale_x_bits),
                        Self::f32_from_bits(scale_y_bits),
                        selector,
                    ));
                }
                None
            }
            "actionOne:two:" if class_str.contains("CCSequence") || class_str.contains("CCSpawn") => {
                let _ = self.note_passive_loading_sequence(0, selector);
                None
            }
            "initOne:two:" if class_str.contains("CCSequence") || class_str.contains("CCSpawn") || receiver_label.contains("CCSequence") || receiver_label.contains("CCSpawn") => {
                let _ = self.note_passive_loading_sequence(0, selector);
                None
            }
            "actions:" if class_str.contains("CCSequence") || class_str.contains("CCSpawn") => {
                let _ = self.note_passive_loading_sequence(0, selector);
                None
            }
            "copyWithZone:" if self.runtime.scheduler.actions.cocos_actions.contains_key(&receiver) => {
                let obj = self.objc_hle_alloc_like(receiver, 0, selector);
                if let Some(state) = self.runtime.scheduler.actions.cocos_actions.get(&receiver).cloned() {
                    self.runtime.scheduler.actions.cocos_actions.insert(obj, state.clone());
                    if let Some(label) = self.diag.object_labels.get(&receiver).cloned() {
                        self.diag.object_labels.insert(obj, format!("{}<copy>", label));
                    }
                    self.push_callback_trace(format!("action.copy source={} result={} origin={}", self.describe_ptr(receiver), self.describe_ptr(obj), selector));
                }
                Some((obj, format!("cocos action copy {} -> {}", self.describe_ptr(receiver), self.describe_ptr(obj))))
            }
            "setTarget:" if self.runtime.scheduler.actions.cocos_actions.contains_key(&receiver) => {
                if let Some(state) = self.runtime.scheduler.actions.cocos_actions.get_mut(&receiver) {
                    state.target = arg2;
                }
                self.push_callback_trace(format!("action.setTarget action={} target={} origin={}", self.describe_ptr(receiver), self.describe_ptr(arg2), selector));
                Some((receiver, format!("cocos action target <- {}", self.describe_ptr(arg2))))
            }
            "setSelector:" if self.runtime.scheduler.actions.cocos_actions.contains_key(&receiver) => {
                let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{arg2:08x}"));
                if let Some(state) = self.runtime.scheduler.actions.cocos_actions.get_mut(&receiver) {
                    state.selector_name = Some(selector_name.clone());
                }
                self.push_callback_trace(format!("action.setSelector action={} selector={} origin={}", self.describe_ptr(receiver), selector_name, selector));
                Some((receiver, format!("cocos action selector <- {}", selector_name)))
            }
            "target" if self.runtime.scheduler.actions.cocos_actions.contains_key(&receiver) => {
                let target = self.runtime.scheduler.actions.cocos_actions.get(&receiver).map(|state| state.target).unwrap_or(0);
                Some((target, format!("cocos action target -> {}", self.describe_ptr(target))))
            }
            "selector" if self.runtime.scheduler.actions.cocos_actions.contains_key(&receiver) => {
                let selector_name = self.runtime.scheduler.actions.cocos_actions.get(&receiver).and_then(|state| state.selector_name.clone()).unwrap_or_default();
                let selector_ptr = if selector_name.is_empty() { 0 } else { self.alloc_selector_c_string(&selector_name).unwrap_or(0) };
                Some((selector_ptr, format!("cocos action selector -> {} ptr={}", if selector_name.is_empty() { "<none>" } else { &selector_name }, self.describe_ptr(selector_ptr))))
            }
            "startWithTarget:" if self.runtime.scheduler.actions.cocos_actions.contains_key(&receiver) => {
                let kind = self.runtime.scheduler.actions.cocos_actions.get(&receiver).map(|state| state.kind.clone()).unwrap_or_default();
                if let Some(state) = self.runtime.scheduler.actions.cocos_actions.get_mut(&receiver) {
                    state.last_owner = arg2;
                }
                self.push_callback_trace(format!("action.start action={} owner={} kind={} origin={}", self.describe_ptr(receiver), self.describe_ptr(arg2), kind, selector));
                if kind.starts_with("interval") {
                    None
                } else {
                    Some((receiver, format!("cocos action start owner={}", self.describe_ptr(arg2))))
                }
            }
            "execute" if self.runtime.scheduler.actions.cocos_actions.contains_key(&receiver) => {
                let (target, selector_name, object_arg) = self.runtime.scheduler.actions.cocos_actions.get(&receiver)
                    .map(|state| (state.target, state.selector_name.clone().unwrap_or_default(), state.object_arg))
                    .unwrap_or((0, String::new(), 0));
                let invoked = if target != 0 && !selector_name.is_empty() {
                    self.invoke_objc_selector_now(target, &selector_name, object_arg, 0, 180_000, selector)
                } else {
                    false
                };
                if let Some(state) = self.runtime.scheduler.actions.cocos_actions.get_mut(&receiver) {
                    state.execute_count = state.execute_count.saturating_add(1);
                }
                self.push_callback_trace(format!(
                    "action.execute action={} target={} selector={} object={} invoked={} origin={}",
                    self.describe_ptr(receiver),
                    self.describe_ptr(target),
                    if selector_name.is_empty() { "<none>" } else { &selector_name },
                    self.describe_ptr(object_arg),
                    if invoked { "YES" } else { "NO" },
                    selector,
                ));
                Some((receiver, format!("cocos action execute target={} selector={} invoked={}", self.describe_ptr(target), if selector_name.is_empty() { "<none>" } else { &selector_name }, if invoked { "YES" } else { "NO" })))
            }
            "runAction:" if has_synthetic_node_state => {
                let mut notes = Vec::new();
                if let Some(note) = self.queue_synthetic_cocos_interval_action(receiver, arg2, selector) {
                    notes.push(note);
                }
                if self.active_profile().is_loading_scene_label(&receiver_label) {
                    if let Some(note) = self.bind_passive_loading_plan_owner(receiver, selector) {
                        notes.push(note);
                    }
                }
                if notes.is_empty() {
                    Some((receiver, format!("cocos node runAction ignored action={}", self.describe_ptr(arg2))))
                } else {
                    Some((receiver, notes.join(", ")))
                }
            }
            "addAction:target:paused:" if class_str.contains("CCActionManager") || receiver_label.contains("CCActionManager") => {
                let _ = self.peek_stack_u32(0).unwrap_or(0) != 0;
                let _ = self.bind_passive_loading_plan_owner(arg3, selector);
                None
            }
            "bitmapFontAtlasWithString:fntFile:" | "labelWithString:fntFile:" => {
                let text = self.guest_string_value(arg2).unwrap_or_default();
                let fnt_file = self.guest_string_value(arg3).unwrap_or_default();
                if let Some(existing) = self.find_existing_loading_bmfont_node(&text, &fnt_file) {
                    let mut note = self.install_synthetic_bmfont_node(existing, &text, &fnt_file, true);
                    note.push_str(&format!(", reuseExisting={}", self.describe_ptr(existing)));
                    Some((existing, note))
                } else {
                    let obj = self.alloc_synthetic_ui_object("CCLabelBMFont.instance(synth)".to_string());
                    let mut note = self.install_synthetic_bmfont_node(obj, &text, &fnt_file, false);
                    if !fnt_file.is_empty() {
                        self.diag.object_labels.insert(obj, format!("CCLabelBMFont.instance(synth<'{}'>)", fnt_file));
                        note.push_str(&format!(", fntFile='{}'", fnt_file.replace('\n', "\\n")));
                    }
                    Some((obj, note))
                }
            }
            "initWithString:fntFile:" | "setString:fntFile:" if on_label || receiver_label.contains("FontAtlas") || receiver_label.contains("BMFont") => {
                let text = self.guest_string_value(arg2).unwrap_or_default();
                let fnt_file = self.guest_string_value(arg3).unwrap_or_default();
                let mut note = self.install_synthetic_bmfont_node(receiver, &text, &fnt_file, selector == "setString:fntFile:");
                if !fnt_file.is_empty() {
                    self.diag.object_labels.insert(receiver, format!("CCLabelBMFont.instance(synth<'{}'>)", fnt_file));
                    note.push_str(&format!(", fntFile='{}'", fnt_file.replace('\n', "\\n")));
                }
                Some((receiver, note))
            }
            "configurationWithFNTFile:" => {
                let obj = self.alloc_synthetic_ui_object("CCBitmapFontConfiguration.instance(synth)".to_string());
                let fnt_file = self.guest_string_value(arg2).unwrap_or_default();
                let _ = self.ensure_string_backing(obj, "CCBitmapFontConfiguration.instance(synth)".to_string(), &fnt_file);
                let note = if fnt_file.is_empty() {
                    "bitmap font configuration synthetic".to_string()
                } else {
                    self.diag.object_labels.insert(obj, format!("CCBitmapFontConfiguration.instance(synth<'{}'>)", fnt_file));
                    format!("bitmap font configuration synthetic fntFile='{}'", fnt_file.replace('\n', "\\n"))
                };
                Some((obj, note))
            }
            "initWithFNTfile:" if receiver_label.contains("BitmapFontConfiguration") || class_str.contains("BitmapFontConfiguration") => {
                let fnt_file = self.guest_string_value(arg2).unwrap_or_default();
                let _ = self.ensure_string_backing(receiver, "CCBitmapFontConfiguration.instance(synth)".to_string(), &fnt_file);
                let note = if fnt_file.is_empty() {
                    "bitmap font configuration init synthetic".to_string()
                } else {
                    self.diag.object_labels.insert(receiver, format!("CCBitmapFontConfiguration.instance(synth<'{}'>)", fnt_file));
                    format!("bitmap font configuration init synthetic fntFile='{}'", fnt_file.replace('\n', "\\n"))
                };
                Some((receiver, note))
            }
            "labelWithString:"
            | "labelWithString:fontName:fontSize:"
            | "labelWithString:fontSize:"
                if on_label || Self::is_label_class_name(class_str) => {
                let obj = self.objc_hle_alloc_like(receiver, 0, selector);
                let text = self.guest_string_value(arg2).unwrap_or_default();
                let font_name = if selector == "labelWithString:fontName:fontSize:" {
                    self.guest_string_value(arg3).filter(|v| !v.is_empty())
                } else {
                    None
                };
                let font_size_bits = match selector {
                    "labelWithString:fontName:fontSize:" => self.peek_stack_u32(0),
                    "labelWithString:fontSize:" => Some(arg3),
                    _ => None,
                };
                let note = self.install_synthetic_text_node(
                    obj,
                    if class_str.is_empty() { "CCLabel" } else { class_str },
                    &text,
                    false,
                    font_name,
                    font_size_bits,
                );
                Some((obj, note))
            }
            "initWithString:"
            | "initWithString:fontName:fontSize:"
            | "initWithString:fontSize:"
                if on_label => {
                let text = self.guest_string_value(arg2).unwrap_or_default();
                let font_name = if selector == "initWithString:fontName:fontSize:" {
                    self.guest_string_value(arg3).filter(|v| !v.is_empty())
                } else {
                    None
                };
                let font_size_bits = match selector {
                    "initWithString:fontName:fontSize:" => self.peek_stack_u32(0),
                    "initWithString:fontSize:" => Some(arg3),
                    _ => None,
                };
                let note = self.install_synthetic_text_node(
                    receiver,
                    if class_str.is_empty() { "CCLabel" } else { class_str },
                    &text,
                    false,
                    font_name,
                    font_size_bits,
                );
                Some((receiver, note))
            }
            "setString:" if on_label || self.string_backing(receiver).is_some() => {
                let text = self.guest_string_value(arg2).unwrap_or_default();
                let note = self.install_synthetic_text_node(
                    receiver,
                    if class_str.is_empty() { "CCLabel" } else { class_str },
                    &text,
                    true,
                    None,
                    None,
                );
                Some((receiver, note))
            }
            "string" if on_label || self.string_backing(receiver).is_some() => {
                let text = self.string_backing(receiver).map(|v| v.text.clone()).unwrap_or_default();
                let obj = self.materialize_host_string_object("NSString.synthetic.label", &text);
                Some((obj, format!("label string -> {} '{}'", self.describe_ptr(obj), text.replace('\n', "\\n"))))
            }
            "scheduleUpdate" if on_cocos_node => {
                self.register_cocos_scheduled_selector(receiver, "update:", 0, None, selector);
                Some((receiver, format!("cocos scheduleUpdate target={}", self.describe_ptr(receiver))))
            }
            "schedule:" if on_cocos_node => {
                let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{arg2:08x}"));
                self.register_cocos_scheduled_selector(receiver, &selector_name, 0, None, selector);
                Some((receiver, format!("cocos schedule target={} selector={}", self.describe_ptr(receiver), selector_name)))
            }
            "schedule:interval:" if on_cocos_node => {
                let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{arg2:08x}"));
                self.register_cocos_scheduled_selector(receiver, &selector_name, arg3, None, selector);
                Some((receiver, format!("cocos schedule target={} selector={} intervalBits=0x{:08x}", self.describe_ptr(receiver), selector_name, arg3)))
            }
            "schedule:repeat:" if on_cocos_node => {
                let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{arg2:08x}"));
                self.register_cocos_scheduled_selector(receiver, &selector_name, 0, Some(arg3), selector);
                Some((receiver, format!("cocos schedule target={} selector={} repeat={}", self.describe_ptr(receiver), selector_name, arg3)))
            }
            "schedule:interval:repeat:" if on_cocos_node => {
                let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{arg2:08x}"));
                let repeats = self.read_u32_le(self.cpu.regs[13]).ok();
                self.register_cocos_scheduled_selector(receiver, &selector_name, arg3, repeats, selector);
                Some((receiver, format!("cocos schedule target={} selector={} intervalBits=0x{:08x} repeat={:?}", self.describe_ptr(receiver), selector_name, arg3, repeats)))
            }
            "unschedule:" if on_cocos_node => {
                let selector_name = self.decode_cocos_schedule_selector_name(arg2).unwrap_or_else(|| format!("0x{arg2:08x}"));
                self.unschedule_cocos_selector(receiver, &selector_name, selector);
                Some((receiver, format!("cocos unschedule target={} selector={}", self.describe_ptr(receiver), selector_name)))
            }
            "switchTo:" | "switchToAndReleaseMe:" => {
                self.try_handle_ccmultiplex_switch(selector, receiver, arg2)
            }
            "transitionWithDuration:scene:" if on_transition || Self::is_transition_like_label(class_str) => {
                let obj = self.objc_hle_alloc_like(receiver, 0, selector);
                if obj != 0 && arg3 != 0 {
                    self.note_synthetic_splash_destination(obj, arg3, selector);
                    self.diag.object_labels
                        .entry(obj)
                        .and_modify(|label| {
                            if !Self::is_transition_like_label(label) {
                                *label = format!("CCTransitionScene.instance(synth)<{}>", label);
                            }
                        })
                        .or_insert_with(|| "CCTransitionScene.instance(synth)".to_string());
                }
                self.push_scene_progress_selector_event(selector, receiver, class_str, selector, arg2, arg3, Some(obj), arg3 != 0);
                Some((obj, format!(
                    "cocos transition factory duration={:.3} scene={} result={}",
                    Self::f32_from_bits(arg2),
                    self.describe_ptr(arg3),
                    self.describe_ptr(obj),
                )))
            }
            "initWithDuration:scene:" if on_transition => {
                if arg3 != 0 {
                    self.note_synthetic_splash_destination(receiver, arg3, selector);
                }
                let note = self.apply_cocos_init_defaults(receiver, class_str, selector);
                self.push_scene_progress_selector_event(selector, receiver, class_str, selector, arg2, arg3, Some(receiver), arg3 != 0);
                Some((receiver, format!(
                    "{} duration={:.3} scene={}",
                    note,
                    Self::f32_from_bits(arg2),
                    self.describe_ptr(arg3),
                )))
            }
            "sharedDirector" if on_director || Self::is_cocos_director_class_name(class_str) || self.runtime.ui_cocos.cocos_director != 0 => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                self.bootstrap_synthetic_runloop();
                self.recalc_runloop_sources();
                Some((director, format!("cocos sharedDirector -> {} class={}", self.describe_ptr(director), if class_str.is_empty() { "<unknown>" } else { class_str })))
            }
            "setDirectorType:" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                self.runtime.ui_cocos.director_type = arg2;
                self.bootstrap_cocos_window_path("setDirectorType:");
                Some((director, format!("cocos directorType <- {} on {}", arg2, self.describe_ptr(director))))
            }
            "setAnimationInterval:" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                let interval_bits = self.nstimeinterval_f32_bits_from_regs(arg2, arg3);
                let interval_secs = Self::f32_from_bits(interval_bits);
                self.runtime.ui_cocos.animation_interval_bits = interval_bits;
                self.runtime.ui_cocos.animation_running = true;
                self.runtime.ui_cocos.display_link_armed = true;
                self.bootstrap_synthetic_runloop();
                self.recalc_runloop_sources();
                Some((director, format!(
                    "cocos animationInterval(raw=0x{:08x}:0x{:08x}, bits=0x{:08x}, secs={:.6}) running=YES",
                    arg3,
                    arg2,
                    interval_bits,
                    interval_secs,
                )))
            }
            "setDisplayFPS:" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                self.runtime.ui_cocos.display_fps_enabled = arg2 != 0;
                Some((director, format!("cocos displayFPS <- {}", if arg2 != 0 { "YES" } else { "NO" })))
            }
            "setDeviceOrientation:" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                Some((director, format!("cocos deviceOrientation <- {}", arg2)))
            }
            "setOpenGLView:" | "setView:" | "setGLView:" | "setEAGLView:" | "setMainView:" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                let view = if arg2 != 0 { arg2 } else { self.ensure_cocos_opengl_view() };
                self.runtime.ui_cocos.opengl_view = view;
                self.diag.object_labels.entry(view).or_insert_with(|| "EAGLView.synthetic#0".to_string());
                self.sync_cocos_director_guest_ivars(selector);
                self.bootstrap_cocos_window_path(selector);
                Some((director, format!("cocos openGLView <- {}", self.describe_ptr(view))))
            }
            "openGLView" | "view" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                let view = self.ensure_cocos_opengl_view();
                self.bootstrap_cocos_window_path(selector);
                Some((view, format!("cocos {} on {} -> {}", selector, self.describe_ptr(director), self.describe_ptr(view))))
            }
            "attachInWindow:" | "attachInWindow" if on_director => {
                let _director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                let host_window = if arg2 != 0 { arg2 } else { self.runtime.ui_objects.window };
                if host_window != 0 {
                    self.runtime.ui_objects.window = host_window;
                    self.diag.object_labels
                        .entry(host_window)
                        .or_insert_with(|| "UIWindow.main".to_string());
                }
                let eagl_view = self.ensure_cocos_opengl_view();
                self.runtime.ui_cocos.opengl_view = eagl_view;
                self.runtime.ui_objects.view_superviews.insert(eagl_view, host_window);
                let entry = self.runtime.ui_objects.view_subviews.entry(host_window).or_default();
                if !entry.contains(&eagl_view) {
                    entry.push(eagl_view);
                }
                self.runtime.ui_objects.first_responder = eagl_view;
                self.sync_cocos_director_guest_ivars("attachInWindow:");
                self.bootstrap_cocos_window_path("attachInWindow:");
                None
            }
            "initOpenGLViewWithView:withFrame:" if on_director => {
                let _director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                let host_view = if arg2 != 0 { arg2 } else { self.runtime.ui_objects.window };
                let eagl_view = self.ensure_cocos_opengl_view();
                self.runtime.ui_cocos.opengl_view = eagl_view;
                self.runtime.ui_objects.view_superviews.insert(eagl_view, host_view);
                let entry = self.runtime.ui_objects.view_subviews.entry(host_view).or_default();
                if !entry.contains(&eagl_view) {
                    entry.push(eagl_view);
                }
                self.runtime.ui_objects.first_responder = eagl_view;
                self.sync_cocos_director_guest_ivars("initOpenGLViewWithView:withFrame:");
                self.bootstrap_cocos_window_path("initOpenGLViewWithView:withFrame:");
                None
            }
            "attachInView:" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                let host_view = if arg2 != 0 { arg2 } else { self.runtime.ui_objects.window };
                let eagl_view = self.ensure_cocos_opengl_view();
                self.runtime.ui_cocos.opengl_view = eagl_view;
                self.runtime.ui_objects.first_responder = eagl_view;
                self.sync_cocos_director_guest_ivars("attachInView:");
                self.bootstrap_cocos_window_path("attachInView:");
                Some((director, format!("cocos attachInView host={} glView={}", self.describe_ptr(host_view), self.describe_ptr(eagl_view))))
            }
            "effectScene" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                self.sync_cocos_director_guest_ivars(selector);
                let effect = self.runtime.ui_cocos.effect_scene;
                self.push_scene_progress_selector_event(
                    selector,
                    receiver,
                    class_str,
                    selector,
                    effect,
                    0,
                    Some(effect),
                    self.runtime.graphics.synthetic_splash_destinations.contains_key(&effect),
                );
                Some((effect, format!("cocos effectScene -> {}", self.describe_ptr(effect))))
            }
            "setEffectScene:" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                if arg2 != 0 {
                    self.diag.object_labels.entry(arg2).or_insert_with(|| "CCScene.effect".to_string());
                }
                self.set_effect_scene(arg2, selector);
                self.sync_cocos_director_guest_ivars(selector);
                self.bootstrap_cocos_window_path(selector);
                self.push_scene_progress_selector_event(
                    selector,
                    receiver,
                    class_str,
                    selector,
                    arg2,
                    0,
                    Some(director),
                    self.runtime.graphics.synthetic_splash_destinations.contains_key(&arg2),
                );
                Some((director, format!("cocos effectScene <- {}", self.describe_ptr(arg2))))
            }
            "runWithScene:" | "replaceScene:" | "pushScene:" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                if self.objc_lookup_imp_for_receiver(director, selector).is_some() {
                    self.runtime.ui_cocos.scene_transition_calls = self.runtime.ui_cocos.scene_transition_calls.saturating_add(1);
                    match selector {
                        "runWithScene:" => self.runtime.ui_cocos.scene_run_with_scene_calls = self.runtime.ui_cocos.scene_run_with_scene_calls.saturating_add(1),
                        "replaceScene:" => self.runtime.ui_cocos.scene_replace_scene_calls = self.runtime.ui_cocos.scene_replace_scene_calls.saturating_add(1),
                        "pushScene:" => self.runtime.ui_cocos.scene_push_scene_calls = self.runtime.ui_cocos.scene_push_scene_calls.saturating_add(1),
                        _ => {}
                    }
                    if arg2 != 0 {
                        self.diag.object_labels.entry(arg2).or_insert_with(|| "CCScene.running".to_string());
                        self.record_director_scene_handoff_request(selector, arg2, "scene-selector-real-imp");
                    }
                    self.runtime.ui_cocos.animation_running = true;
                    self.sync_cocos_director_guest_ivars(selector);
                    self.bootstrap_cocos_window_path(selector);
                    self.push_scene_progress_selector_event(selector, receiver, class_str, selector, arg2, 0, Some(director), false);
                    return None;
                }
                self.runtime.ui_cocos.scene_transition_calls = self.runtime.ui_cocos.scene_transition_calls.saturating_add(1);
                match selector {
                    "runWithScene:" => self.runtime.ui_cocos.scene_run_with_scene_calls = self.runtime.ui_cocos.scene_run_with_scene_calls.saturating_add(1),
                    "replaceScene:" => self.runtime.ui_cocos.scene_replace_scene_calls = self.runtime.ui_cocos.scene_replace_scene_calls.saturating_add(1),
                    "pushScene:" => self.runtime.ui_cocos.scene_push_scene_calls = self.runtime.ui_cocos.scene_push_scene_calls.saturating_add(1),
                    _ => {}
                }
                self.push_scene_event(format!("transition {} -> {}", selector, self.describe_ptr(arg2)));
                let mut entered = 0usize;
                if arg2 != 0 {
                    self.diag.object_labels.entry(arg2).or_insert_with(|| "CCScene.running".to_string());
                    self.record_director_scene_handoff_request(selector, arg2, "scene-selector-synthetic");
                    entered = self.activate_running_scene(arg2, selector);
                }
                self.runtime.ui_cocos.animation_running = true;
                self.drive_cocos_frame_pipeline(selector, 4);
                self.push_scene_progress_selector_event(selector, receiver, class_str, selector, arg2, 0, Some(director), false);
                Some((director, format!(
                    "cocos scene <- {} via {} enterProp={} state=[{}]",
                    self.describe_ptr(self.runtime.ui_cocos.running_scene),
                    selector,
                    entered,
                    self.describe_node_graph_state(self.runtime.ui_cocos.running_scene),
                )))
            }
            "popScene" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                self.runtime.ui_cocos.scene_transition_calls = self.runtime.ui_cocos.scene_transition_calls.saturating_add(1);
                let running = self.runtime.ui_cocos.running_scene;
                let mut popped = 0u32;
                while let Some(candidate) = self.runtime.ui_cocos.scene_stack.pop() {
                    if candidate != 0 && candidate != running {
                        popped = candidate;
                        break;
                    }
                }
                if popped != 0 {
                    self.diag.object_labels.entry(popped).or_insert_with(|| "CCScene.running".to_string());
                    self.record_director_scene_handoff_request(selector, popped, "scene-selector-pop");
                }
                self.runtime.ui_cocos.animation_running = true;
                self.sync_cocos_director_guest_ivars(selector);
                self.bootstrap_cocos_window_path(selector);
                self.push_scene_progress_selector_event(selector, receiver, class_str, selector, popped, 0, Some(director), false);
                if self.objc_lookup_imp_for_receiver(director, selector).is_some() {
                    return None;
                }
                let mut entered = 0usize;
                if popped != 0 {
                    entered = self.activate_running_scene(popped, selector);
                }
                self.drive_cocos_frame_pipeline(selector, 4);
                Some((director, format!(
                    "cocos popScene -> {} enterProp={} state=[{}]",
                    self.describe_ptr(self.runtime.ui_cocos.running_scene),
                    entered,
                    self.describe_node_graph_state(self.runtime.ui_cocos.running_scene),
                )))
            }
            "startAnimation" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                self.runtime.ui_cocos.animation_running = true;
                self.drive_cocos_frame_pipeline("startAnimation", 4);
                Some((director, "cocos animation started; displayLink armed=YES".to_string()))
            }
            "stopAnimation" if on_director => {
                let director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                self.runtime.ui_cocos.animation_running = false;
                self.runtime.ui_cocos.display_link_armed = false;
                self.recalc_runloop_sources();
                Some((director, "cocos animation stopped; displayLink armed=NO".to_string()))
            }
            // Let guest CCDirector::mainLoop/drawScene execute for real.
            // Fastpathing these here causes recursive synthetic runloop re-entry:
            // CADisplayLink -> invoke mainLoop -> guest mainLoop sends drawScene ->
            // fastpath drawScene -> drive_cocos_frame_pipeline -> synthetic runloop ->
            // CADisplayLink -> invoke mainLoop ... until host stack overflow.
            "mainLoop" | "drawScene" if on_director => {
                let _director = self.ensure_cocos_director_object(receiver, class_name.as_deref());
                self.runtime.ui_cocos.animation_running = true;
                self.sync_cocos_director_guest_ivars(selector);
                self.bootstrap_cocos_window_path(selector);
                None
            }
            "layer" if on_gl_view => {
                self.bootstrap_cocos_window_path("layer");
                Some((self.runtime.ui_graphics.eagl_layer, format!("cocos view layer -> {}", self.describe_ptr(self.runtime.ui_graphics.eagl_layer))))
            }
            // Synthetic EAGLView has no real UIKit-backed present path of its own.
            // Once guest CCDirector::mainLoop reaches swapBuffers on our synthetic view,
            // we need to publish the current scene framebuffer right here without kicking
            // the synthetic runloop again (which would recurse back into mainLoop).
            // Keep guest swapBuffers for non-synthetic/guest-backed GL views.
            "swapBuffers" if on_gl_view => {
                self.sync_cocos_director_guest_ivars("swapBuffers");
                self.bootstrap_cocos_window_path("swapBuffers");
                let synthetic_view = receiver != 0 && (
                    receiver == self.runtime.ui_cocos.opengl_view
                        || receiver_label.contains("EAGLView.synthetic")
                        || receiver_label.contains("CCEAGLView.synthetic")
                );
                if synthetic_view {
                    self.simulate_graphics_tick();
                    Some((
                        receiver,
                        format!(
                            "swapBuffers synthetic-view present frame#{} source={}",
                            self.runtime.ui_graphics.graphics_frame_index,
                            self.runtime.ui_graphics.graphics_last_present_source.clone().unwrap_or_else(|| "unknown".to_string()),
                        ),
                    ))
                } else {
                    None
                }
            }
            "spriteSheetWithFile:" | "spriteSheetWithFile:capacity:" if on_sprite_sheet => {
                let name = self.guest_string_value(arg2).unwrap_or_else(|| format!("0x{arg2:08x}"));
                let obj = self.objc_hle_alloc_like(receiver, 0, selector);
                if obj != 0 {
                    self.diag.object_labels
                        .entry(obj)
                        .or_insert_with(|| format!("CCSpriteSheet.instance(synth<'{}'>)", name));
                }
                let out = if obj != 0 { obj } else { receiver };
                let note = self.configure_sprite_sheet_from_file(
                    out,
                    arg2,
                    if selector.ends_with(":capacity:") { Some(arg3.max(1)) } else { None },
                    selector,
                );
                Some((out, note))
            }
            "initWithFile:" | "initWithFile:capacity:" if on_sprite_sheet => {
                let note = self.configure_sprite_sheet_from_file(
                    receiver,
                    arg2,
                    if selector.ends_with(":capacity:") { Some(arg3.max(1)) } else { None },
                    selector,
                );
                Some((receiver, note))
            }
            "spriteSheetWithTexture:" | "spriteSheetWithTexture:capacity:" if on_sprite_sheet => {
                let obj = self.objc_hle_alloc_like(receiver, 0, selector);
                if obj != 0 {
                    self.diag.object_labels
                        .entry(obj)
                        .or_insert_with(|| "CCSpriteSheet.instance(synth)".to_string());
                }
                let out = if obj != 0 { obj } else { receiver };
                let note = self.configure_sprite_sheet_with_texture(
                    out,
                    arg2,
                    if selector.ends_with(":capacity:") { Some(arg3.max(1)) } else { None },
                    selector,
                );
                Some((out, note))
            }
            "initWithTexture:" | "initWithTexture:capacity:" if on_sprite_sheet => {
                let note = self.configure_sprite_sheet_with_texture(
                    receiver,
                    arg2,
                    if selector.ends_with(":capacity:") { Some(arg3.max(1)) } else { None },
                    selector,
                );
                Some((receiver, note))
            }
            "sharedTextureCache" if on_texture_cache || Self::is_texture_cache_class_name(class_str) || self.runtime.graphics.cocos_texture_cache_object != 0 => {
                let cache = self.ensure_cocos_texture_cache_object();
                Some((cache, format!("cocos sharedTextureCache -> {} entries={}", self.describe_ptr(cache), self.runtime.graphics.cocos_texture_cache_entries.len())))
            }
            "addImage:" | "textureForKey:" if on_texture_cache => {
                let name = self.guest_string_value(arg2).unwrap_or_else(|| format!("0x{arg2:08x}"));
                let result = self.materialize_synthetic_texture_for_name(&name).unwrap_or(0);
                let note = if result != 0 {
                    let dims = self.synthetic_texture_dimensions(result).unwrap_or((0, 0));
                    format!("cocos {} '{}' -> {} {}x{} path={}", selector, name, self.describe_ptr(result), dims.0, dims.1, self.runtime.fs.last_resource_path.clone().unwrap_or_default())
                } else {
                    format!("cocos {} '{}' -> miss", selector, name)
                };
                Some((result, note))
            }
            "spriteWithFile:" if on_sprite => {
                let name = self.guest_string_value(arg2).unwrap_or_else(|| format!("0x{arg2:08x}"));
                let obj = self.alloc_synthetic_ui_object(format!("CCSprite.instance(synth<'{}'>)", name));
                self.diag.object_labels.entry(obj).or_insert_with(|| format!("CCSprite.instance(synth<'{}'>)", name));
                let note = self.configure_sprite_from_file(obj, arg2, None, selector);
                self.maybe_trace_sprite_watch_event(obj, "selector", note.clone());
                Some((obj, note))
            }
            "initWithFile:" if on_sprite => {
                let note = self.configure_sprite_from_file(receiver, arg2, None, selector);
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "spriteWithFile:rect:" if on_sprite => {
                let name = self.guest_string_value(arg2).unwrap_or_else(|| format!("0x{arg2:08x}"));
                let obj = self.alloc_synthetic_ui_object(format!("CCSprite.instance(synth<'{}'>)", name));
                self.diag.object_labels.entry(obj).or_insert_with(|| format!("CCSprite.instance(synth<'{}'>)", name));
                let note = self.configure_sprite_from_file(obj, arg2, self.read_msgsend_rect_after_object_arg(), selector);
                self.maybe_trace_sprite_watch_event(obj, "selector", note.clone());
                Some((obj, note))
            }
            "initWithFile:rect:" if on_sprite => {
                let note = self.configure_sprite_from_file(receiver, arg2, self.read_msgsend_rect_after_object_arg(), selector);
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "hasPremultipliedAlpha" if on_texture => {
                let premultiplied = self.synthetic_texture_has_pma(receiver);
                Some(((if premultiplied { 1 } else { 0 }), format!("texture hasPremultipliedAlpha {}", if premultiplied { "YES" } else { "NO" })))
            }
            "pixelsWide" if on_texture => {
                let width = self.synthetic_texture_dimensions(receiver).map(|dims| dims.0).unwrap_or(0);
                Some((width, format!("texture pixelsWide -> {}", width)))
            }
            "pixelsHigh" if on_texture => {
                let height = self.synthetic_texture_dimensions(receiver).map(|dims| dims.1).unwrap_or(0);
                Some((height, format!("texture pixelsHigh -> {}", height)))
            }
            "name" if on_texture => {
                let gl_name = self.synthetic_texture_gl_name(receiver);
                Some((gl_name, format!("texture name -> {}", gl_name)))
            }
            "setTexture:" if on_texture_node && !on_sprite_sheet && !on_sprite => {
                let note = {
                    let dims = self.synthetic_texture_dimensions(arg2);
                    let texture_desc = self.describe_ptr(arg2);
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    state.texture = arg2;
                    if let Some((w, h)) = dims {
                        if state.width == 0 { state.width = w; }
                        if state.height == 0 { state.height = h; }
                    }
                    format!(
                        "cocos texture-node texture <- {} size={}x{}",
                        texture_desc,
                        state.width,
                        state.height,
                    )
                };
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, format!("{} revision={}", note, revision)))
            }
            "texture" if on_texture_node && !on_sprite_sheet && !on_sprite => {
                let texture = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.texture).unwrap_or(0);
                if texture != 0 {
                    Some((texture, format!("texture-node texture -> {}", self.describe_ptr(texture))))
                } else {
                    None
                }
            }
            "setTexture:" if on_sprite_sheet => {
                let note = self.configure_synthetic_texture_atlas(
                    receiver,
                    arg2,
                    Some(self.synthetic_texture_atlas_capacity(receiver).max(1)),
                    selector,
                );
                Some((receiver, note))
            }
            "texture" if on_sprite_sheet => {
                let texture = self.synthetic_texture_atlas_texture(receiver);
                Some((texture, format!("sprite-sheet texture -> {}", self.describe_ptr(texture))))
            }
            "capacity" if on_sprite_sheet => {
                let capacity = self.synthetic_texture_atlas_capacity(receiver);
                Some((capacity, format!("texture-atlas capacity -> {}", capacity)))
            }
            "totalQuads" if on_sprite_sheet => {
                let total = self.synthetic_texture_atlas_total_quads(receiver);
                Some((total, format!("texture-atlas totalQuads -> {}", total)))
            }
            "quads" if on_sprite_sheet => {
                let ptr = self
                    .runtime.graphics.synthetic_texture_atlases
                    .get(&receiver)
                    .map(|state| state.quad_buffer_ptr)
                    .unwrap_or(0);
                Some((ptr, format!("texture-atlas quads -> {}", self.describe_ptr(ptr))))
            }
            "resizeCapacity:" if on_sprite_sheet => {
                let note = self.resize_synthetic_texture_atlas(receiver, arg2.max(1), selector);
                Some((1, note))
            }
            "removeAllQuads" if on_sprite_sheet => {
                let note = self.reset_synthetic_texture_atlas_quads(receiver, selector);
                Some((receiver, note))
            }
            "updateQuadWithTexture:vertexQuad:atIndex:" if on_sprite_sheet => {
                let index = self.peek_stack_u32(0).unwrap_or(0);
                let note = self.update_synthetic_texture_atlas_quad(receiver, arg2, arg3, index, selector);
                Some((receiver, note))
            }
            "setTexture:" if on_sprite => {
                let dims = self.synthetic_texture_dimensions(arg2);
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    state.texture = arg2;
                    if let Some((w, h)) = dims {
                        if state.width == 0 { state.width = w; }
                        if state.height == 0 { state.height = h; }
                    }
                    format!("sprite texture <- {}", self.describe_ptr(arg2))
                };
                self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, note))
            }
            "texture" if on_sprite => {
                let texture = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|v| v.texture).unwrap_or(0);
                Some((texture, format!("sprite texture -> {}", self.describe_ptr(texture))))
            }
            "initAnimationDictionary" if on_cocos_node => {
                let note = {
                    let mut dict = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.animation_dictionary).unwrap_or(0);
                    if dict == 0 {
                        dict = self.alloc_synthetic_dictionary(format!("AnimationDictionary.instance(synth)<0x{receiver:08x}>"));
                        self.ensure_synthetic_sprite_state(receiver).animation_dictionary = dict;
                    }
                    format!(
                        "cocos node initAnimationDictionary -> {} entries={}",
                        self.describe_ptr(dict),
                        self.runtime.graphics.synthetic_dictionaries.get(&dict).map(|entry| entry.entries.len()).unwrap_or(0),
                    )
                };
                Some((receiver, note))
            }
            "addAnimation:" if on_cocos_node => {
                let note = {
                    let mut dict = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.animation_dictionary).unwrap_or(0);
                    if dict == 0 {
                        dict = self.alloc_synthetic_dictionary(format!("AnimationDictionary.instance(synth)<0x{receiver:08x}>"));
                        self.ensure_synthetic_sprite_state(receiver).animation_dictionary = dict;
                    }
                    let name_ptr = self
                        .objc_lookup_ivar_offset_in_class_chain(arg2, "name")
                        .and_then(|offset| self.read_u32_le(arg2.wrapping_add(offset)).ok())
                        .unwrap_or(0);
                    let frames_ptr = self
                        .objc_lookup_ivar_offset_in_class_chain(arg2, "frames")
                        .and_then(|offset| self.read_u32_le(arg2.wrapping_add(offset)).ok())
                        .unwrap_or(0);
                    let key = self.synthetic_dictionary_key(name_ptr);
                    if !key.is_empty() {
                        self.ensure_synthetic_dictionary(dict).entries.insert(key.clone(), frames_ptr);
                    }
                    format!(
                        "cocos node addAnimation name={} frames={} count={} dict={} entries={}",
                        if name_ptr != 0 { key } else { format!("<unnamed:{}>", self.describe_ptr(arg2)) },
                        self.describe_ptr(frames_ptr),
                        self.synthetic_array_len(frames_ptr),
                        self.describe_ptr(dict),
                        self.runtime.graphics.synthetic_dictionaries.get(&dict).map(|entry| entry.entries.len()).unwrap_or(0),
                    )
                };
                Some((receiver, note))
            }
            "setDisplayFrame:index:" if on_cocos_node => {
                let index = self.peek_stack_u32(0).unwrap_or(arg3) as usize;
                let key = self.synthetic_dictionary_key(arg2);
                let note = {
                    let dict = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.animation_dictionary).unwrap_or(0);
                    let frames = if dict != 0 { self.synthetic_dictionary_get(dict, &key) } else { 0 };
                    let frame = if frames != 0 { self.synthetic_array_get(frames, index) } else { 0 };
                    let frame_state = if frame != 0 {
                        self.runtime.graphics.synthetic_sprites.get(&frame).cloned()
                    } else {
                        None
                    };
                    let resolved_texture = if frame != 0 {
                        let direct_dims = self.synthetic_texture_dimensions(frame);
                        if direct_dims.is_some() {
                            frame
                        } else {
                            frame_state.as_ref().map(|state| state.texture).unwrap_or(0)
                        }
                    } else {
                        0
                    };
                    let resolved_dims = self.synthetic_texture_dimensions(resolved_texture).or_else(|| self.synthetic_texture_dimensions(frame));
                    let (state_texture, state_width, state_height, frame_desc) = {
                        let state = self.ensure_synthetic_sprite_state(receiver);
                        state.last_display_frame_key = arg2;
                        state.last_display_frame_index = index as u32;
                        if let Some(frame_state) = frame_state.as_ref() {
                            if frame_state.texture != 0 {
                                state.texture = frame_state.texture;
                            } else if resolved_texture != 0 {
                                state.texture = resolved_texture;
                            }
                            state.texture_rect_x_bits = frame_state.texture_rect_x_bits;
                            state.texture_rect_y_bits = frame_state.texture_rect_y_bits;
                            state.texture_rect_w_bits = frame_state.texture_rect_w_bits;
                            state.texture_rect_h_bits = frame_state.texture_rect_h_bits;
                            state.texture_rect_explicit = frame_state.texture_rect_explicit;
                            state.untrimmed_w_bits = frame_state.untrimmed_w_bits;
                            state.untrimmed_h_bits = frame_state.untrimmed_h_bits;
                            state.untrimmed_explicit = frame_state.untrimmed_explicit;
                            state.offset_x_bits = frame_state.offset_x_bits;
                            state.offset_y_bits = frame_state.offset_y_bits;
                            state.offset_explicit = frame_state.offset_explicit;
                            state.flip_x = frame_state.flip_x;
                            state.flip_y = frame_state.flip_y;
                            if frame_state.width != 0 || frame_state.height != 0 {
                                state.width = frame_state.width;
                                state.height = frame_state.height;
                            }
                        } else {
                            if resolved_texture != 0 {
                                state.texture = resolved_texture;
                            }
                            if state.width == 0 || state.height == 0 {
                                if let Some((w, h)) = resolved_dims {
                                    if state.width == 0 { state.width = w; }
                                    if state.height == 0 { state.height = h; }
                                }
                            }
                        }
                        let frame_desc = frame_state
                            .as_ref()
                            .map(|frame_state| format!(
                                " rect=({},{} {}x{}) untrimmed=({},{} explicit={}) offset=({},{} explicit={}) flip=({}, {})",
                                Self::f32_from_bits(frame_state.texture_rect_x_bits),
                                Self::f32_from_bits(frame_state.texture_rect_y_bits),
                                Self::f32_from_bits(frame_state.texture_rect_w_bits),
                                Self::f32_from_bits(frame_state.texture_rect_h_bits),
                                Self::f32_from_bits(frame_state.untrimmed_w_bits),
                                Self::f32_from_bits(frame_state.untrimmed_h_bits),
                                if frame_state.untrimmed_explicit { "YES" } else { "NO" },
                                Self::f32_from_bits(frame_state.offset_x_bits),
                                Self::f32_from_bits(frame_state.offset_y_bits),
                                if frame_state.offset_explicit { "YES" } else { "NO" },
                                if frame_state.flip_x { "YES" } else { "NO" },
                                if frame_state.flip_y { "YES" } else { "NO" },
                            ))
                            .unwrap_or_default();
                        (state.texture, state.width, state.height, frame_desc)
                    };
                    format!(
                        "cocos node setDisplayFrame key={} index={} frames={} frame={} texture={} size={}x{}{}",
                        key,
                        index,
                        self.describe_ptr(frames),
                        self.describe_ptr(frame),
                        self.describe_ptr(state_texture),
                        state_width,
                        state_height,
                        frame_desc,
                    )
                };
                let adopted = self.maybe_adopt_guest_cocos_focus(receiver, selector);
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                let note = format!("{} adopted={} revision={}", note, adopted, revision);
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "childrenAlloc" if on_cocos_node => {
                let children = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.children).unwrap_or(0);
                let alloc = if children != 0 {
                    self.synthetic_array_len(children).max(4) as u32
                } else {
                    4
                };
                Some((alloc, format!("cocos node childrenAlloc -> {}", alloc)))
            }
            "children" if on_cocos_node => {
                let children = self.ensure_node_children_array(receiver);
                Some((children, format!("cocos node children -> {} count={}", self.describe_ptr(children), self.synthetic_array_len(children))))
            }
            "parent" if on_cocos_node => {
                let parent = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.parent).unwrap_or(0);
                Some((parent, format!("cocos node parent -> {}", self.describe_ptr(parent))))
            }
            "tag" if on_cocos_node => {
                let tag = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.tag).unwrap_or(0);
                Some((tag, format!("cocos node tag -> {}", tag)))
            }
            "setTag:" if on_cocos_node => {
                self.ensure_synthetic_sprite_state(receiver).tag = arg2;
                Some((receiver, format!("cocos node tag <- {}", arg2)))
            }
            "setContentSize:" if on_cocos_node => {
                let decoded = self.read_msgsend_pair_arg(true, true, false);
                let mut w = decoded.as_ref().map(|(bits, _)| Self::f32_from_bits(bits[0]).round().max(1.0) as u32).unwrap_or(0);
                let mut h = decoded.as_ref().map(|(bits, _)| Self::f32_from_bits(bits[1]).round().max(1.0) as u32).unwrap_or(0);
                if w == 0 && h == 0 {
                    let fallback_w = self.runtime.ui_graphics.graphics_surface_width.max(1);
                    let fallback_h = self.runtime.ui_graphics.graphics_surface_height.max(1);
                    if on_color_layer || receiver_label.contains("Scene") || receiver_label.contains("Layer") || self.active_profile().is_first_scene_label(&receiver_label) {
                        w = fallback_w;
                        h = fallback_h;
                    }
                }
                let source = decoded.map(|(_, src)| src).unwrap_or_else(|| "fallback".to_string());
                let (out_w, out_h) = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if w != 0 { state.width = w; }
                    if h != 0 { state.height = h; }
                    (state.width, state.height)
                };
                let note = format!("cocos node contentSize <- {}x{} src={}", out_w, out_h, source);
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                let note = format!("{} revision={}", note, revision);
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "setSize:" if on_cocos_node => {
                let decoded = self.read_msgsend_pair_arg(true, true, false);
                let (width, height, source) = if let Some((bits, source)) = decoded {
                    (
                        Self::f32_from_bits(bits[0]).round().max(0.0) as u32,
                        Self::f32_from_bits(bits[1]).round().max(0.0) as u32,
                        source,
                    )
                } else {
                    (
                        Self::f32_from_bits(arg2).round().max(0.0) as u32,
                        Self::f32_from_bits(arg3).round().max(0.0) as u32,
                        "fallback".to_string(),
                    )
                };
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if width != 0 { state.width = width; }
                    if height != 0 { state.height = height; }
                    format!("cocos node setSize <- {}x{} src={}", state.width, state.height, source)
                };
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                let note = format!("{} revision={}", note, revision);
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "setTransformAnchor:" if on_cocos_node => {
                let decoded = self.read_msgsend_pair_arg(false, false, true);
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if let Some((bits, source)) = decoded {
                        let ax_px = Self::f32_from_bits(bits[0]);
                        let ay_px = Self::f32_from_bits(bits[1]);
                        state.anchor_pixels_x_bits = bits[0];
                        state.anchor_pixels_y_bits = bits[1];
                        state.anchor_pixels_explicit = true;
                        if state.width != 0 {
                            state.anchor_x_bits = (ax_px / state.width as f32).to_bits();
                            state.anchor_explicit = true;
                        }
                        if state.height != 0 {
                            state.anchor_y_bits = (ay_px / state.height as f32).to_bits();
                            state.anchor_explicit = true;
                        }
                        format!(
                            "cocos node setTransformAnchor <- ({:.3},{:.3})px src={}",
                            ax_px,
                            ay_px,
                            source,
                        )
                    } else {
                        state.anchor_pixels_x_bits = arg2;
                        state.anchor_pixels_y_bits = arg3;
                        state.anchor_pixels_explicit = true;
                        format!("cocos node setTransformAnchor(bits)=0x{arg2:08x},0x{arg3:08x}")
                    }
                };
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "setPositionBL:" if on_cocos_node => {
                let decoded = self.read_msgsend_pair_arg(false, false, true);
                let note = {
                    let anchor_preview = self
                        .runtime
                        .graphics
                        .synthetic_sprites
                        .get(&receiver)
                        .cloned()
                        .unwrap_or_default();
                    if let Some((bits, source)) = decoded {
                        let bl_x = Self::f32_from_bits(bits[0]);
                        let bl_y = Self::f32_from_bits(bits[1]);
                        let (anchor_x_px, anchor_y_px, anchor_src) =
                            self.synthetic_state_anchor_pixels(receiver, &anchor_preview);
                        let state = self.ensure_synthetic_sprite_state(receiver);
                        state.position_bl_x_bits = bits[0];
                        state.position_bl_y_bits = bits[1];
                        state.position_bl_explicit = true;
                        let pos_x = bl_x + anchor_x_px;
                        let pos_y = bl_y + anchor_y_px;
                        state.position_x_bits = pos_x.to_bits();
                        state.position_y_bits = pos_y.to_bits();
                        // setPositionBL: this is a bottom-left placement helper. Preserve the
                        // authored BL coordinates and lift them through the *effective* current
                        // anchor (explicit anchor pixels, explicit normalized anchor, or the
                        // class default such as CCSprite=0.5). Keeping the BL authoring around
                        // lets rendering recompute the anchor-relative position later if the
                        // texture rect / untrimmed size / anchor changes after placement.
                        let lifted_by_anchor = anchor_x_px.abs() > 0.001 || anchor_y_px.abs() > 0.001;
                        let use_anchor_relative_bl = lifted_by_anchor && !on_sprite;
                        if use_anchor_relative_bl {
                            state.relative_anchor_point = true;
                        }
                        format!(
                            "cocos node setPositionBL <- ({:.3},{:.3}) -> position=({:.3},{:.3}) src={} anchorPx=({:.3},{:.3}) anchorSrc={} relAnchor={}",
                            bl_x,
                            bl_y,
                            pos_x,
                            pos_y,
                            source,
                            anchor_x_px,
                            anchor_y_px,
                            anchor_src,
                            if use_anchor_relative_bl { "YES" } else { "NO" },
                        )
                    } else {
                        let state = self.ensure_synthetic_sprite_state(receiver);
                        state.position_bl_x_bits = arg2;
                        state.position_bl_y_bits = arg3;
                        state.position_bl_explicit = true;
                        state.position_x_bits = arg2;
                        state.position_y_bits = arg3;
                        format!("cocos node setPositionBL(bits)=0x{arg2:08x},0x{arg3:08x}")
                    }
                };
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "setAnchorPoint:" if on_cocos_node => {
                let decoded = self.read_msgsend_pair_arg(false, false, true);
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if let Some((bits, source)) = decoded {
                        state.anchor_x_bits = bits[0];
                        state.anchor_y_bits = bits[1];
                        state.anchor_explicit = true;
                        let ax = Self::f32_from_bits(bits[0]);
                        let ay = Self::f32_from_bits(bits[1]);
                        format!("cocos node setAnchorPoint <- ({:.3},{:.3}) src={}", ax, ay, source)
                    } else {
                        state.anchor_x_bits = arg2;
                        state.anchor_y_bits = arg3;
                        state.anchor_explicit = true;
                        format!("cocos node setAnchorPoint(bits)=0x{arg2:08x},0x{arg3:08x}")
                    }
                };
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "setAnchorPointInPixels:" if on_cocos_node => {
                let decoded = self.read_msgsend_pair_arg(false, false, true);
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if let Some((bits, source)) = decoded {
                        state.anchor_pixels_x_bits = bits[0];
                        state.anchor_pixels_y_bits = bits[1];
                        state.anchor_pixels_explicit = true;
                        let ax = Self::f32_from_bits(bits[0]);
                        let ay = Self::f32_from_bits(bits[1]);
                        format!("cocos node setAnchorPointInPixels <- ({:.3},{:.3}) src={}", ax, ay, source)
                    } else {
                        state.anchor_pixels_x_bits = arg2;
                        state.anchor_pixels_y_bits = arg3;
                        state.anchor_pixels_explicit = true;
                        format!("cocos node setAnchorPointInPixels(bits)=0x{arg2:08x},0x{arg3:08x}")
                    }
                };
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "setRelativeAnchorPoint:" if on_cocos_node => {
                let decoded = self.read_msgsend_bool_arg();
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if let Some((enabled, source)) = decoded {
                        state.relative_anchor_point = enabled;
                        state.relative_anchor_point_explicit = true;
                        format!(
                            "cocos node setRelativeAnchorPoint <- {} src={}",
                            if enabled { "YES" } else { "NO" },
                            source,
                        )
                    } else {
                        state.relative_anchor_point = (arg2 & 1) != 0;
                        state.relative_anchor_point_explicit = true;
                        format!(
                            "cocos node setRelativeAnchorPoint(raw)=0x{arg2:08x}, tail=0x{arg3:08x} -> {}",
                            if state.relative_anchor_point { "YES" } else { "NO" },
                        )
                    }
                };
                Some((receiver, note))
            }
            "isRelativeAnchorPoint" if on_cocos_node => {
                let enabled = self
                    .runtime.graphics.synthetic_sprites
                    .get(&receiver)
                    .map(|state| state.relative_anchor_point)
                    .unwrap_or(false);
                Some((if enabled { 1 } else { 0 }, format!(
                    "cocos node isRelativeAnchorPoint -> {}",
                    if enabled { "YES" } else { "NO" },
                )))
            }
            "setScale:" if on_cocos_node => {
                let decoded = self.read_msgsend_float_arg();
                let runloop_tick = self.runtime.ui_runtime.runloop_ticks;
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if let Some((bits, source)) = decoded {
                        let scale = Self::f32_from_bits(bits);
                        state.scale_x_bits = bits;
                        state.scale_y_bits = bits;
                        state.scale_explicit = true;
                        state.last_guest_scale_tick = runloop_tick;
                        format!("cocos node setScale <- {:.3} src={}", scale, source)
                    } else {
                        let scale = Self::f32_from_bits(arg2);
                        state.scale_x_bits = arg2;
                        state.scale_y_bits = arg2;
                        state.scale_explicit = true;
                        state.last_guest_scale_tick = runloop_tick;
                        format!("cocos node setScale(bits)=0x{arg2:08x} -> {:.3}", scale)
                    }
                };
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                let note = format!("{} revision={}", note, revision);
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "setScaleX:" if on_cocos_node => {
                let decoded = self.read_msgsend_float_arg();
                let runloop_tick = self.runtime.ui_runtime.runloop_ticks;
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if let Some((bits, source)) = decoded {
                        let scale = Self::f32_from_bits(bits);
                        state.scale_x_bits = bits;
                        state.scale_explicit = true;
                        state.last_guest_scale_tick = runloop_tick;
                        format!("cocos node setScaleX <- {:.3} src={}", scale, source)
                    } else {
                        let scale = Self::f32_from_bits(arg2);
                        state.scale_x_bits = arg2;
                        state.scale_explicit = true;
                        state.last_guest_scale_tick = runloop_tick;
                        format!("cocos node setScaleX(bits)=0x{arg2:08x} -> {:.3}", scale)
                    }
                };
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                let note = format!("{} revision={}", note, revision);
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "setScaleY:" if on_cocos_node => {
                let decoded = self.read_msgsend_float_arg();
                let runloop_tick = self.runtime.ui_runtime.runloop_ticks;
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if let Some((bits, source)) = decoded {
                        let scale = Self::f32_from_bits(bits);
                        state.scale_y_bits = bits;
                        state.scale_explicit = true;
                        state.last_guest_scale_tick = runloop_tick;
                        format!("cocos node setScaleY <- {:.3} src={}", scale, source)
                    } else {
                        let scale = Self::f32_from_bits(arg2);
                        state.scale_y_bits = arg2;
                        state.scale_explicit = true;
                        state.last_guest_scale_tick = runloop_tick;
                        format!("cocos node setScaleY(bits)=0x{arg2:08x} -> {:.3}", scale)
                    }
                };
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                let note = format!("{} revision={}", note, revision);
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "scale" if on_cocos_node => {
                let bits = self
                    .runtime.graphics.synthetic_sprites
                    .get(&receiver)
                    .map(|state| if state.scale_explicit && state.scale_x_bits != 0 { state.scale_x_bits } else { 1.0f32.to_bits() })
                    .unwrap_or_else(|| 1.0f32.to_bits());
                let scale = Self::f32_from_bits(bits);
                Some((bits, format!("cocos node scale -> {:.3}", scale)))
            }
            "scaleX" if on_cocos_node => {
                let bits = self
                    .runtime.graphics.synthetic_sprites
                    .get(&receiver)
                    .map(|state| if state.scale_explicit && state.scale_x_bits != 0 { state.scale_x_bits } else { 1.0f32.to_bits() })
                    .unwrap_or_else(|| 1.0f32.to_bits());
                let scale = Self::f32_from_bits(bits);
                Some((bits, format!("cocos node scaleX -> {:.3}", scale)))
            }
            "scaleY" if on_cocos_node => {
                let bits = self
                    .runtime.graphics.synthetic_sprites
                    .get(&receiver)
                    .map(|state| if state.scale_explicit && state.scale_y_bits != 0 { state.scale_y_bits } else { 1.0f32.to_bits() })
                    .unwrap_or_else(|| 1.0f32.to_bits());
                let scale = Self::f32_from_bits(bits);
                Some((bits, format!("cocos node scaleY -> {:.3}", scale)))
            }
            "setPosition:" if on_cocos_node => {
                let decoded = self.read_msgsend_pair_arg(false, false, true);
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    state.position_bl_explicit = false;
                    if let Some((bits, source)) = decoded {
                        state.position_x_bits = bits[0];
                        state.position_y_bits = bits[1];
                        let px = Self::f32_from_bits(bits[0]);
                        let py = Self::f32_from_bits(bits[1]);
                        let suspicious_tail = bits[0] == 0 && bits[1] != 0 && Self::looks_like_single_scalar_tail(bits[1]);
                        if suspicious_tail {
                            format!(
                                "cocos node setPosition <- ({:.3},{:.3}) src={} [single-scalar-tail?]",
                                px, py, source,
                            )
                        } else {
                            format!("cocos node setPosition <- ({:.3},{:.3}) src={}", px, py, source)
                        }
                    } else {
                        state.position_x_bits = arg2;
                        state.position_y_bits = arg3;
                        format!("cocos node setPosition(bits)=0x{arg2:08x},0x{arg3:08x}")
                    }
                };
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "setOffsetPosition:" | "setOffsetPositionInPixels:" | "setUnflippedOffsetPositionFromCenter:" if on_sprite => {
                let decoded = self.read_msgsend_pair_arg(false, true, true);
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if let Some((bits, source)) = decoded {
                        state.offset_x_bits = bits[0];
                        state.offset_y_bits = bits[1];
                        state.offset_explicit = true;
                        format!(
                            "sprite {} <- ({:.3},{:.3}) src={}",
                            selector,
                            Self::f32_from_bits(bits[0]),
                            Self::f32_from_bits(bits[1]),
                            source,
                        )
                    } else {
                        state.offset_x_bits = arg2;
                        state.offset_y_bits = arg3;
                        state.offset_explicit = true;
                        format!("sprite {}(bits)=0x{arg2:08x},0x{arg3:08x}", selector)
                    }
                };
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "setFlipX:" | "setFlipY:" if on_sprite => {
                let decoded = self.read_msgsend_bool_arg();
                let (enabled, source) = decoded.unwrap_or(((arg2 & 1) != 0, "raw".to_string()));
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if selector == "setFlipX:" {
                        state.flip_x = enabled;
                    } else {
                        state.flip_y = enabled;
                    }
                    format!(
                        "sprite {} <- {} src={}",
                        selector,
                        if enabled { "YES" } else { "NO" },
                        source,
                    )
                };
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                let note = format!("{} revision={}", note, revision);
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "initWithTexture:rect:" | "initWithTexture:rect:rotated:" if on_sprite => {
                let fallback_dims = self.synthetic_texture_dimensions(arg2)
                    .unwrap_or((self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1)));
                let decoded = self.read_msgsend_rect_after_object_arg();
                let texture_desc = self.describe_ptr(arg2);
                let rotated_raw = if selector == "initWithTexture:rect:rotated:" {
                    self.peek_stack_u32(3)
                } else {
                    None
                };
                let prev_dims = self
                    .runtime.graphics.synthetic_sprites
                    .get(&receiver)
                    .map(|state| (state.width, state.height))
                    .unwrap_or((0, 0));
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    state.texture = arg2;
                    if let Some((bits, source)) = decoded {
                        let w = Self::f32_from_bits(bits[2]).round().max(0.0) as u32;
                        let h = Self::f32_from_bits(bits[3]).round().max(0.0) as u32;
                        state.texture_rect_x_bits = bits[0];
                        state.texture_rect_y_bits = bits[1];
                        state.texture_rect_w_bits = bits[2];
                        state.texture_rect_h_bits = bits[3];
                        state.texture_rect_explicit = true;
                        state.untrimmed_w_bits = bits[2];
                        state.untrimmed_h_bits = bits[3];
                        state.untrimmed_explicit = true;
                        // Keep explicit rect dimensions authoritative, even when one axis is zero.
                        // The loading screen has atlas children that briefly publish rects like 33x0;
                        // preserving an earlier fullscreen-ish contentSize here creates the long
                        // stretched column artifact instead of suppressing the sprite draw.
                        state.width = w;
                        state.height = h;
                        format!(
                            "sprite {} texture={} rect=({:.3},{:.3} {:.3}x{:.3}) src={} prev={}x{} -> {}x{}{}",
                            selector,
                            texture_desc,
                            Self::f32_from_bits(bits[0]),
                            Self::f32_from_bits(bits[1]),
                            Self::f32_from_bits(bits[2]),
                            Self::f32_from_bits(bits[3]),
                            source,
                            prev_dims.0,
                            prev_dims.1,
                            state.width,
                            state.height,
                            rotated_raw
                                .map(|raw| format!(" rotatedRaw=0x{raw:08x}"))
                                .unwrap_or_default(),
                        )
                    } else {
                        if state.width == 0 { state.width = fallback_dims.0; }
                        if state.height == 0 { state.height = fallback_dims.1; }
                        format!(
                            "sprite {} texture={} fallback prev={}x{} -> {}x{}{}",
                            selector,
                            texture_desc,
                            prev_dims.0,
                            prev_dims.1,
                            state.width,
                            state.height,
                            rotated_raw
                                .map(|raw| format!(" rotatedRaw=0x{raw:08x}"))
                                .unwrap_or_default(),
                        )
                    }
                };
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                let note = format!("{} revision={}", note, revision);
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "setTextureRect:" | "setTextureRect:untrimmedSize:" | "updateTextureCoords:" if on_sprite => {
                let fallback_dims = {
                    let tex = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|v| v.texture).unwrap_or(0);
                    self.synthetic_texture_dimensions(tex)
                        .unwrap_or((self.runtime.ui_graphics.graphics_surface_width.max(1), self.runtime.ui_graphics.graphics_surface_height.max(1)))
                };
                let decoded = self.read_msgsend_rect_arg();
                let prev_dims = self
                    .runtime.graphics.synthetic_sprites
                    .get(&receiver)
                    .map(|state| (state.width, state.height))
                    .unwrap_or((0, 0));
                let (untrimmed_bits, untrimmed_tail) = if selector == "setTextureRect:untrimmedSize:" {
                    match (self.peek_stack_u32(2), self.peek_stack_u32(3)) {
                        (Some(a), Some(b)) => (
                            Some([a, b]),
                            format!(
                                " untrimmed=({:.3},{:.3}) raw=STACK[2..3]",
                                Self::f32_from_bits(a),
                                Self::f32_from_bits(b),
                            ),
                        ),
                        _ => (None, " untrimmed=<missing>".to_string()),
                    }
                } else {
                    (None, String::new())
                };
                let note = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if let Some((bits, source)) = decoded {
                        let rect_w = Self::f32_from_bits(bits[2]).round().max(0.0) as u32;
                        let rect_h = Self::f32_from_bits(bits[3]).round().max(0.0) as u32;
                        let draw_w = untrimmed_bits
                            .map(|bits| Self::f32_from_bits(bits[0]).round().max(0.0) as u32)
                            .filter(|value| *value != 0)
                            .unwrap_or(rect_w);
                        let draw_h = untrimmed_bits
                            .map(|bits| Self::f32_from_bits(bits[1]).round().max(0.0) as u32)
                            .filter(|value| *value != 0)
                            .unwrap_or(rect_h);
                        state.texture_rect_x_bits = bits[0];
                        state.texture_rect_y_bits = bits[1];
                        state.texture_rect_w_bits = bits[2];
                        state.texture_rect_h_bits = bits[3];
                        state.texture_rect_explicit = true;
                        if let Some(untrimmed_bits) = untrimmed_bits {
                            state.untrimmed_w_bits = untrimmed_bits[0];
                            state.untrimmed_h_bits = untrimmed_bits[1];
                            state.untrimmed_explicit = true;
                        } else if selector != "setTextureRect:untrimmedSize:" {
                            state.untrimmed_w_bits = 0;
                            state.untrimmed_h_bits = 0;
                            state.untrimmed_explicit = false;
                        }
                        // setTextureRect is more reliable than any previously inherited contentSize.
                        // If the guest says one axis is zero, keep it zero so rendering skips instead of
                        // stretching stale 320x480 / 33x480 boxes across the screen.
                        state.width = draw_w;
                        state.height = draw_h;
                        format!(
                            "sprite {} rect=({:.3},{:.3} {:.3}x{:.3}) src={} prev={}x{} -> {}x{}{}",
                            selector,
                            Self::f32_from_bits(bits[0]),
                            Self::f32_from_bits(bits[1]),
                            Self::f32_from_bits(bits[2]),
                            Self::f32_from_bits(bits[3]),
                            source,
                            prev_dims.0,
                            prev_dims.1,
                            state.width,
                            state.height,
                            untrimmed_tail,
                        )
                    } else {
                        if state.width == 0 { state.width = fallback_dims.0; }
                        if state.height == 0 { state.height = fallback_dims.1; }
                        format!(
                            "sprite {} normalized-from-texture prev={}x{} -> {}x{}{}",
                            selector,
                            prev_dims.0,
                            prev_dims.1,
                            state.width,
                            state.height,
                            untrimmed_tail,
                        )
                    }
                };
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                let note = format!("{} revision={}", note, revision);
                self.maybe_trace_sprite_watch_event(receiver, "selector", note.clone());
                Some((receiver, note))
            }
            "updateBlendFunc" if on_sprite => {
                Some((receiver, "sprite updateBlendFunc synthetic-ok".to_string()))
            }
            "layerWithColor:width:height:" if on_color_layer || class_str.contains("CCColorLayer") => {
                let w = f32::from_bits(arg3).max(0.0) as u32;
                let h_bits = self.peek_stack_u32(0).unwrap_or(0);
                let h = f32::from_bits(h_bits).max(0.0) as u32;
                let obj = self.alloc_synthetic_ui_object("CCColorLayer.instance(synth)".to_string());
                let fallback_w = self.runtime.ui_graphics.graphics_surface_width.max(1);
                let fallback_h = self.runtime.ui_graphics.graphics_surface_height.max(1);
                let out_desc = self.describe_ptr(obj);
                let (out_w, out_h) = {
                    let state = self.ensure_synthetic_sprite_state(obj);
                    state.width = if w != 0 { w } else { fallback_w };
                    state.height = if h != 0 { h } else { fallback_h };
                    state.fill_rgba = Self::decode_cccolor4b(arg2);
                    state.fill_rgba_explicit = true;
                    (state.width, state.height)
                };
                Some((obj, format!("cocos layerWithColor:width:height: color=0x{arg2:08x} -> {} {}x{}", out_desc, out_w, out_h)))
            }
            "initWithColor:width:height:" if on_color_layer => {
                let w = f32::from_bits(arg3).max(0.0) as u32;
                let h_bits = self.peek_stack_u32(0).unwrap_or(0);
                let h = f32::from_bits(h_bits).max(0.0) as u32;
                let fallback_w = self.runtime.ui_graphics.graphics_surface_width.max(1);
                let fallback_h = self.runtime.ui_graphics.graphics_surface_height.max(1);
                let (out_w, out_h) = {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if w != 0 { state.width = w; }
                    if h != 0 { state.height = h; }
                    if state.width == 0 { state.width = fallback_w; }
                    if state.height == 0 { state.height = fallback_h; }
                    state.fill_rgba = Self::decode_cccolor4b(arg2);
                    state.fill_rgba_explicit = true;
                    (state.width, state.height)
                };
                Some((receiver, format!("cocos color-layer init color=0x{arg2:08x} size={}x{}", out_w, out_h)))
            }
            "itemFromNormalSprite:selectedSprite:disabledSprite:target:selector:" if Self::is_menu_item_class_name(class_str) || receiver_label.contains("CCMenuItemSprite") => {
                let disabled = self.peek_stack_u32(0).unwrap_or(0);
                let target = self.peek_stack_u32(1).unwrap_or(0);
                let callback_sel = self.peek_stack_u32(2).unwrap_or(0);
                let item = self.alloc_synthetic_ui_object("CCMenuItemSprite.instance(synth)".to_string());
                self.diag.object_labels.entry(item).or_insert_with(|| "CCMenuItemSprite.instance(synth)".to_string());
                let note = self.configure_menu_item_state(item, arg2, arg3, disabled, target, callback_sel, selector);
                Some((item, note))
            }
            "initFromNormalSprite:selectedSprite:disabledSprite:target:selector:" if on_menu_item => {
                let disabled = self.peek_stack_u32(0).unwrap_or(0);
                let target = self.peek_stack_u32(1).unwrap_or(0);
                let callback_sel = self.peek_stack_u32(2).unwrap_or(0);
                let note = self.configure_menu_item_state(receiver, arg2, arg3, disabled, target, callback_sel, selector);
                Some((receiver, note))
            }
            "initWithTarget:selector:" if on_menu_item => {
                let selector_name = self.objc_read_selector_name(arg3).unwrap_or_else(|| format!("0x{arg3:08x}"));
                {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    state.callback_target = arg2;
                    state.callback_selector = arg3;
                    state.touch_enabled = true;
                }
                Some((receiver, format!(
                    "cocos menu-item initWithTarget:selector: target={} selector={} state=[{}]",
                    self.describe_ptr(arg2),
                    selector_name,
                    self.describe_node_graph_state(receiver),
                )))
            }
            "menuWithItems:" if Self::is_menu_class_name(class_str) || receiver_label.contains("CCMenu") => {
                let items = self.collect_menu_items_from_message(arg2, arg3, true);
                let menu = self.alloc_synthetic_ui_object("CCMenu.instance(synth)".to_string());
                self.diag.object_labels.entry(menu).or_insert_with(|| "CCMenu.instance(synth)".to_string());
                let note = self.configure_menu_from_items(menu, &items, selector);
                Some((menu, note))
            }
            "initWithItems:" if on_menu => {
                let items = self.collect_menu_items_from_message(arg2, arg3, true);
                let note = self.configure_menu_from_items(receiver, &items, selector);
                Some((receiver, note))
            }
            "initWithArray:" | "menuWithArray:" if on_menu || Self::is_menu_class_name(class_str) => {
                let items = self.collect_menu_items_from_array_or_single(arg2);
                let menu = if selector == "menuWithArray:" {
                    let obj = self.alloc_synthetic_ui_object("CCMenu.instance(synth)".to_string());
                    self.diag.object_labels.entry(obj).or_insert_with(|| "CCMenu.instance(synth)".to_string());
                    obj
                } else {
                    receiver
                };
                let note = self.configure_menu_from_items(menu, &items, selector);
                Some((menu, note))
            }
            "alignItemsVertically" | "alignItemsVerticallyWithPadding:" if on_menu => {
                let padding = if selector == "alignItemsVerticallyWithPadding:" {
                    Self::f32_from_bits(arg2)
                } else {
                    5.0
                };
                let note = self.layout_menu_children_vertically(receiver, padding.max(0.0));
                Some((receiver, note))
            }
            "setColor:" if on_cocos_node => {
                let rgb = Self::decode_cccolor3b(arg2);
                let alpha_before = self.runtime.graphics.synthetic_sprites.get(&receiver)
                    .filter(|state| state.fill_rgba_explicit)
                    .map(|state| state.fill_rgba[3])
                    .unwrap_or(255);
                {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    state.fill_rgba = [rgb[0], rgb[1], rgb[2], alpha_before];
                    state.fill_rgba_explicit = true;
                }
                let rgba = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.fill_rgba).unwrap_or([rgb[0], rgb[1], rgb[2], alpha_before]);
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, format!(
                    "cocos node color <- rgba({},{},{},{}) state=[{}] revision={}",
                    rgba[0],
                    rgba[1],
                    rgba[2],
                    rgba[3],
                    self.describe_node_graph_state(receiver),
                    revision,
                )))
            }
            "setOpacity:" if on_cocos_node => {
                let alpha = (arg2 & 0xff) as u8;
                {
                    let state = self.ensure_synthetic_sprite_state(receiver);
                    if !state.fill_rgba_explicit {
                        state.fill_rgba = [255, 255, 255, alpha];
                    } else {
                        state.fill_rgba[3] = alpha;
                    }
                    state.fill_rgba_explicit = true;
                }
                let rgba = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.fill_rgba).unwrap_or([255, 255, 255, alpha]);
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, format!(
                    "cocos node opacity <- {} rgba({},{},{},{}) state=[{}] revision={}",
                    alpha,
                    rgba[0],
                    rgba[1],
                    rgba[2],
                    rgba[3],
                    self.describe_node_graph_state(receiver),
                    revision,
                )))
            }
            "setVisible:" if on_cocos_node => {
                self.ensure_synthetic_sprite_state(receiver).visible = arg2 != 0;
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, format!(
                    "cocos node visible <- {} state=[{}] revision={}",
                    if arg2 != 0 { "YES" } else { "NO" },
                    self.describe_node_graph_state(receiver),
                    revision,
                )))
            }
            "isVisible" if on_cocos_node => {
                let visible = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.visible).unwrap_or(true);
                Some(((if visible { 1 } else { 0 }), format!(
                    "cocos node isVisible -> {} state=[{}]",
                    if visible { "YES" } else { "NO" },
                    self.describe_node_graph_state(receiver),
                )))
            }
            "setIsTouchEnabled:" if on_cocos_node => {
                self.ensure_synthetic_sprite_state(receiver).touch_enabled = arg2 != 0;
                Some((receiver, format!(
                    "cocos node touchEnabled <- {} state=[{}]",
                    if arg2 != 0 { "YES" } else { "NO" },
                    self.describe_node_graph_state(receiver),
                )))
            }
            "setTouchMode:" if on_cocos_node => {
                self.ensure_synthetic_sprite_state(receiver).touch_enabled = arg2 != 0;
                Some((receiver, format!(
                    "cocos node touchMode <- {} touchEnabled={} state=[{}]",
                    arg2,
                    if arg2 != 0 { "YES" } else { "NO" },
                    self.describe_node_graph_state(receiver),
                )))
            }
            "setStateNormal" if on_cocos_node => {
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, format!(
                    "cocos node setStateNormal state=[{}] revision={}",
                    self.describe_node_graph_state(receiver),
                    revision,
                )))
            }
            "setSoundEffect:" if on_cocos_node => {
                Some((receiver, format!(
                    "cocos node soundEffect <- {} state=[{}]",
                    self.describe_ptr(arg2),
                    self.describe_node_graph_state(receiver),
                )))
            }
            "addChild:" if on_cocos_node => {
                let real_invoked = if self.should_skip_real_imp_for_synthetic_cocos_selector(receiver, selector) {
                    false
                } else {
                    self.invoke_objc_selector_now(receiver, selector, arg2, 0, 180_000, "guest-graph-adopt")
                };
                let adopted = self.maybe_adopt_guest_cocos_focus(receiver, selector)
                    .saturating_add(self.maybe_adopt_guest_cocos_focus(arg2, selector));
                let linked = self.synthetic_parent_contains_child(receiver, arg2);
                let note = if linked {
                    format!(
                        "cocos {} parent={} child={} realImp={} adopted={} state=[{}]",
                        selector,
                        self.describe_ptr(receiver),
                        self.describe_ptr(arg2),
                        if real_invoked { "YES" } else { "NO" },
                        adopted,
                        self.describe_node_graph_state(arg2),
                    )
                } else {
                    let fallback = self.attach_child_to_node(receiver, arg2, 0, None, selector);
                    format!("{} realImp={} adopted={} fallback=YES", fallback, if real_invoked { "YES" } else { "NO" }, adopted)
                };
                let _ = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, note))
            }
            "addChild:z:" | "insertChild:z:" if on_cocos_node => {
                let real_invoked = if self.should_skip_real_imp_for_synthetic_cocos_selector(receiver, selector) {
                    false
                } else {
                    self.invoke_objc_selector_now(receiver, selector, arg2, arg3, 180_000, "guest-graph-adopt")
                };
                let adopted = self.maybe_adopt_guest_cocos_focus(receiver, selector)
                    .saturating_add(self.maybe_adopt_guest_cocos_focus(arg2, selector));
                let linked = self.synthetic_parent_contains_child(receiver, arg2);
                let note = if linked {
                    if let Some(state) = self.runtime.graphics.synthetic_sprites.get_mut(&arg2) {
                        state.z_order = arg3 as i32;
                    }
                    format!(
                        "cocos {} parent={} child={} z={} realImp={} adopted={} state=[{}]",
                        selector,
                        self.describe_ptr(receiver),
                        self.describe_ptr(arg2),
                        arg3 as i32,
                        if real_invoked { "YES" } else { "NO" },
                        adopted,
                        self.describe_node_graph_state(arg2),
                    )
                } else {
                    let fallback = self.attach_child_to_node(receiver, arg2, arg3 as i32, None, selector);
                    format!("{} realImp={} adopted={} fallback=YES", fallback, if real_invoked { "YES" } else { "NO" }, adopted)
                };
                let _ = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, note))
            }
            "addChild:z:tag:" if on_cocos_node => {
                let tag = self.peek_stack_u32(0).unwrap_or(0);
                let real_invoked = if self.should_skip_real_imp_for_synthetic_cocos_selector(receiver, selector) {
                    false
                } else {
                    self.invoke_objc_selector_now(receiver, selector, arg2, arg3, 180_000, "guest-graph-adopt")
                };
                let adopted = self.maybe_adopt_guest_cocos_focus(receiver, selector)
                    .saturating_add(self.maybe_adopt_guest_cocos_focus(arg2, selector));
                let linked = self.synthetic_parent_contains_child(receiver, arg2);
                let note = if linked {
                    let state = self.ensure_synthetic_sprite_state(arg2);
                    state.z_order = arg3 as i32;
                    state.tag = tag;
                    format!(
                        "cocos {} parent={} child={} z={} tag={} realImp={} adopted={} state=[{}]",
                        selector,
                        self.describe_ptr(receiver),
                        self.describe_ptr(arg2),
                        arg3 as i32,
                        tag,
                        if real_invoked { "YES" } else { "NO" },
                        adopted,
                        self.describe_node_graph_state(arg2),
                    )
                } else {
                    let fallback = self.attach_child_to_node(receiver, arg2, arg3 as i32, Some(tag), selector);
                    format!("{} realImp={} adopted={} fallback=YES", fallback, if real_invoked { "YES" } else { "NO" }, adopted)
                };
                let _ = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, note))
            }
            "setParent:" if on_cocos_node => {
                let current_parent = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.parent).unwrap_or(0);
                let current_z = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.z_order).unwrap_or(0);
                let current_tag = self.runtime.graphics.synthetic_sprites.get(&receiver).map(|state| state.tag).unwrap_or(0);
                let real_invoked = if self.should_skip_real_imp_for_synthetic_cocos_selector(receiver, selector) {
                    false
                } else {
                    self.invoke_objc_selector_now(receiver, selector, arg2, 0, 180_000, "guest-graph-adopt")
                };
                let adopted = self.maybe_adopt_guest_cocos_focus(receiver, selector)
                    .saturating_add(self.maybe_adopt_guest_cocos_focus(arg2, selector));
                let note = if arg2 == 0 {
                    if current_parent != 0 && !real_invoked {
                        let note = self.remove_child_from_node(current_parent, receiver, false);
                        format!("cocos setParent child={} parent=nil oldParent={} realImp={} adopted={} note={}", self.describe_ptr(receiver), self.describe_ptr(current_parent), if real_invoked { "YES" } else { "NO" }, adopted, note)
                    } else {
                        self.ensure_synthetic_sprite_state(receiver).parent = 0;
                        format!("cocos setParent child={} parent=nil realImp={} adopted={} state=[{}]", self.describe_ptr(receiver), if real_invoked { "YES" } else { "NO" }, adopted, self.describe_node_graph_state(receiver))
                    }
                } else if self.synthetic_parent_contains_child(arg2, receiver) {
                    self.ensure_synthetic_sprite_state(receiver).parent = arg2;
                    format!("cocos setParent child={} parent={} realImp={} adopted={} state=[{}]", self.describe_ptr(receiver), self.describe_ptr(arg2), if real_invoked { "YES" } else { "NO" }, adopted, self.describe_node_graph_state(receiver))
                } else {
                    let fallback = self.attach_child_to_node(arg2, receiver, current_z, Some(current_tag), selector);
                    format!("{} realImp={} adopted={} fallback=YES", fallback, if real_invoked { "YES" } else { "NO" }, adopted)
                };
                let _ = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, note))
            }
            "removeChild:cleanup:" if on_cocos_node => {
                let real_invoked = if self.should_skip_real_imp_for_synthetic_cocos_selector(receiver, selector) {
                    false
                } else {
                    self.invoke_objc_selector_now(receiver, selector, arg2, arg3, 180_000, "guest-graph-adopt")
                };
                let adopted = self.maybe_adopt_guest_cocos_focus(receiver, selector)
                    .saturating_add(self.maybe_adopt_guest_cocos_focus(arg2, selector));
                let still_linked = self.synthetic_parent_contains_child(receiver, arg2);
                let note = if still_linked && !real_invoked {
                    let fallback = self.remove_child_from_node(receiver, arg2, arg3 != 0);
                    format!("{} realImp={} adopted={} fallback=YES", fallback, if real_invoked { "YES" } else { "NO" }, adopted)
                } else {
                    format!(
                        "cocos removeChild parent={} child={} cleanup={} realImp={} adopted={} linkedAfter={}",
                        self.describe_ptr(receiver),
                        self.describe_ptr(arg2),
                        if arg3 != 0 { "YES" } else { "NO" },
                        if real_invoked { "YES" } else { "NO" },
                        adopted,
                        if still_linked { "YES" } else { "NO" },
                    )
                };
                let _ = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, note))
            }
            "onEnter" if on_cocos_node => {
                self.ensure_synthetic_sprite_state(receiver).entered = true;
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, format!("cocos node onEnter state=[{}] revision={}", self.describe_node_graph_state(receiver), revision)))
            }
            "onExit" if on_cocos_node => {
                self.ensure_synthetic_sprite_state(receiver).entered = false;
                let revision = self.invalidate_synthetic_widget_content(receiver, selector);
                Some((receiver, format!("cocos node onExit state=[{}] revision={}", self.describe_node_graph_state(receiver), revision)))
            }
            "registerWithTouchDispatcher" if on_cocos_node => {
                self.ensure_synthetic_sprite_state(receiver).touch_enabled = true;
                Some((receiver, format!("cocos node registerWithTouchDispatcher state=[{}]", self.describe_node_graph_state(receiver))))
            }
            "visit" if on_cocos_node => {
                let mut trace_budget = SceneVisitTraceBudget::new(SCENE_VISIT_TRACE_EVENT_LIMIT);
                let draws = self.visit_synthetic_node_recursive(receiver, 0, &mut trace_budget);
                Some((receiver, format!(
                    "cocos node visit rendered={} traced={} state=[{}]",
                    draws,
                    trace_budget.emitted(),
                    self.describe_node_graph_state(receiver),
                )))
            }
            "draw" if on_cocos_node => {
                let mut trace_budget = SceneVisitTraceBudget::new(SCENE_VISIT_TRACE_EVENT_LIMIT);
                let rendered = self.render_synthetic_node_into_framebuffer(receiver, selector, 0, &mut trace_budget);
                Some((receiver, format!(
                    "cocos node draw rendered={} traced={} state=[{}]",
                    if rendered { "YES" } else { "NO" },
                    trace_budget.emitted(),
                    self.describe_node_graph_state(receiver),
                )))
            }
            "sharedManager" if on_audio_manager || class_str.contains("CDAudioManager") => {
                let manager_hint = self.runtime.graphics.cocos_audio_manager_object;
                if self.maybe_defer_real_cocos_audio_dispatch(selector, receiver, "CDAudioManager", manager_hint) {
                    return None;
                }
                let mgr = if receiver != 0 { receiver } else { self.ensure_cdaudio_manager_object() };
                self.runtime.audio_trace.objc_audio_last_result = Some(self.describe_ptr(mgr));
                self.audio_trace_note_objc_audio_selector("CDAudioManager", selector, None, false);
                self.audio_trace_push_event(format!(
                    "objc.audio.fastpath class=CDAudioManager selector={} result={}",
                    selector,
                    self.describe_ptr(mgr),
                ));
                Some((mgr, format!("audio sharedManager -> {}", self.describe_ptr(mgr))))
            }
            "soundEngine" if on_audio_manager => {
                let manager_hint = self.runtime.graphics.cocos_audio_manager_object;
                if self.maybe_defer_real_cocos_audio_dispatch(selector, receiver, "CDAudioManager", manager_hint) {
                    return None;
                }
                let engine = self.ensure_cdsound_engine_object();
                self.runtime.audio_trace.objc_audio_last_result = Some(self.describe_ptr(engine));
                if engine == 0 {
                    self.runtime.audio_trace.objc_audio_manager_soundengine_nil_results = self.runtime.audio_trace.objc_audio_manager_soundengine_nil_results.saturating_add(1);
                }
                self.audio_trace_note_objc_audio_selector("CDAudioManager", selector, None, false);
                self.audio_trace_push_event(format!(
                    "objc.audio.fastpath class=CDAudioManager selector=soundEngine result={}",
                    self.describe_ptr(engine),
                ));
                Some((engine, format!("audio soundEngine -> {}", self.describe_ptr(engine))))
            }
            "configure:channelGroupDefinitions:channelGroupTotal:" if on_audio_manager => {
                let manager_hint = self.runtime.graphics.cocos_audio_manager_object;
                if self.maybe_defer_real_cocos_audio_dispatch(selector, receiver, "CDAudioManager", manager_hint) {
                    return None;
                }
                let mgr = if receiver != 0 { receiver } else { self.ensure_cdaudio_manager_object() };
                let engine = self.ensure_cdsound_engine_object();
                self.runtime.audio_trace.objc_audio_last_result = Some(self.describe_ptr(mgr));
                self.audio_trace_note_objc_audio_selector("CDAudioManager", selector, None, false);
                self.audio_trace_push_event(format!(
                    "objc.audio.fastpath class=CDAudioManager selector={} result={} engine={}",
                    selector,
                    self.describe_ptr(mgr),
                    self.describe_ptr(engine),
                ));
                Some((mgr, format!("audio configure(groups={}, defs={}) -> ok engine={}", arg2, self.describe_ptr(arg3), self.describe_ptr(engine))))
            }
            "init:channelGroupDefinitions:channelGroupTotal:" if on_audio_manager => {
                let manager_hint = self.runtime.graphics.cocos_audio_manager_object;
                if self.maybe_defer_real_cocos_audio_dispatch(selector, receiver, "CDAudioManager", manager_hint) {
                    return None;
                }
                let mgr = if receiver != 0 { receiver } else { self.ensure_cdaudio_manager_object() };
                let _ = self.ensure_cdsound_engine_object();
                self.runtime.audio_trace.objc_audio_last_result = Some(self.describe_ptr(mgr));
                self.audio_trace_note_objc_audio_selector("CDAudioManager", selector, None, false);
                self.audio_trace_push_event(format!(
                    "objc.audio.fastpath class=CDAudioManager selector={} result={}",
                    selector,
                    self.describe_ptr(mgr),
                ));
                Some((mgr, format!("audio manager init(groups={}, defs={}) -> ok", arg2, self.describe_ptr(arg3))))
            }
            "preloadBackgroundMusic:" | "playBackgroundMusic:" | "playBackgroundMusic:loop:" | "stopBackgroundMusic" | "pauseBackgroundMusic" | "resumeBackgroundMusic" | "setBackgroundMusicVolume:" if on_audio_manager => {
                let manager_hint = self.runtime.graphics.cocos_audio_manager_object;
                if self.maybe_defer_real_cocos_audio_dispatch(selector, receiver, "CDAudioManager", manager_hint) {
                    return None;
                }
                let mgr = if receiver != 0 { receiver } else { self.ensure_cdaudio_manager_object() };
                let _ = self.ensure_cdsound_engine_object();
                let resource = self
                    .resolve_path_from_url_like_value(arg2, false)
                    .map(|path| path.display().to_string())
                    .or_else(|| self.guest_string_value(arg2));
                self.runtime.audio_trace.objc_audio_last_result = Some(self.describe_ptr(mgr));
                self.audio_trace_note_objc_audio_selector("CDAudioManager", selector, resource.clone(), false);
                self.audio_trace_push_event(format!(
                    "objc.audio.fastpath class=CDAudioManager selector={} resource={} result={} loopArg=0x{:08x}",
                    selector,
                    resource.clone().unwrap_or_else(|| "<none>".to_string()),
                    self.describe_ptr(mgr),
                    arg3,
                ));
                Some((mgr, format!(
                    "audio manager {} resource={} loopArg=0x{:08x} -> ok",
                    selector,
                    resource.unwrap_or_else(|| "<none>".to_string()),
                    arg3,
                )))
            }
            "init:channelGroupTotal:" | "init:channelGroupTotal:audioSessionCategory:" if on_sound_engine => {
                let engine_hint = self.runtime.graphics.cocos_sound_engine_object;
                if self.maybe_defer_real_cocos_audio_dispatch(selector, receiver, "CDSoundEngine", engine_hint) {
                    return None;
                }
                let engine = if receiver != 0 { receiver } else { self.ensure_cdsound_engine_object() };
                self.runtime.audio_trace.objc_audio_last_result = Some(self.describe_ptr(engine));
                self.audio_trace_push_event(format!(
                    "objc.audio.fastpath class=CDSoundEngine selector={} result={} defs={} groups=0x{:08x}",
                    selector,
                    self.describe_ptr(engine),
                    self.describe_ptr(arg2),
                    arg3,
                ));
                Some((engine, format!("sound engine {} defs={} groups={} -> ok", selector, self.describe_ptr(arg2), arg3)))
            }
            "asynchLoadProgress" if on_sound_engine => {
                let engine_hint = self.runtime.graphics.cocos_sound_engine_object;
                if self.maybe_defer_real_cocos_audio_dispatch(selector, receiver, "CDSoundEngine", engine_hint) {
                    return None;
                }
                let progress = 1.0f32.to_bits();
                self.runtime.audio_trace.objc_audio_last_result = Some(format!("0x{progress:08x}/1.000"));
                self.audio_trace_note_objc_audio_selector("CDSoundEngine", selector, None, false);
                self.audio_trace_push_event("objc.audio.fastpath class=CDSoundEngine selector=asynchLoadProgress result=0x3f800000/1.000".to_string());
                Some((progress, "sound engine asynchLoadProgress -> 1.000".to_string()))
            }
            "playSound:channelGroupId:pitch:pan:gain:loop:" if on_sound_engine => {
                let engine_hint = self.runtime.graphics.cocos_sound_engine_object;
                if self.maybe_defer_real_cocos_audio_dispatch(selector, receiver, "CDSoundEngine", engine_hint) {
                    return None;
                }
                self.runtime.audio_trace.next_objc_audio_effect_id = self.runtime.audio_trace.next_objc_audio_effect_id.saturating_add(1).max(1);
                let sound_id = self.runtime.audio_trace.next_objc_audio_effect_id;
                let resource = self
                    .resolve_path_from_url_like_value(arg2, false)
                    .map(|path| path.display().to_string())
                    .or_else(|| self.guest_string_value(arg2));
                self.runtime.audio_trace.objc_audio_last_result = Some(format!("0x{sound_id:08x}"));
                self.audio_trace_note_objc_audio_selector("CDSoundEngine", selector, resource.clone(), false);
                self.audio_trace_push_event(format!(
                    "objc.audio.fastpath class=CDSoundEngine selector={} resource={} result=0x{:08x} group=0x{:08x}",
                    selector,
                    resource.clone().unwrap_or_else(|| "<none>".to_string()),
                    sound_id,
                    arg3,
                ));
                Some((sound_id, format!(
                    "sound engine playSound resource={} group=0x{:08x} -> id=0x{:08x}",
                    resource.unwrap_or_else(|| "<none>".to_string()),
                    arg3,
                    sound_id,
                )))
            }
            _ => None,
        }
    }

    fn drive_runloop_animation_sources(&mut self, origin: &str) {
        if self.runtime.ui_cocos.display_link_armed {
            let (display_target, display_selector, invoked) = self.dispatch_synthetic_display_link_tick(origin);
            self.diag.trace.push(format!(
                "     ↳ hle CADisplayLink.tick {} target={} selector={} invoked={} frameDtBits=0x{:08x} frameDt={:.6}",
                self.describe_ptr(self.runtime.ui_cocos.synthetic_display_link),
                self.describe_ptr(display_target),
                display_selector,
                if invoked { "YES" } else { "NO" },
                self.runtime.ui_cocos.animation_interval_bits,
                if self.runtime.ui_cocos.animation_interval_bits != 0 {
                    Self::f32_from_bits(self.runtime.ui_cocos.animation_interval_bits)
                } else {
                    1.0f32 / 60.0f32
                },
            ));
            if !invoked {
                self.simulate_graphics_tick();
            }
        }
        self.fire_due_cocos_scheduled_selectors(origin);
        self.drive_active_synthetic_cocos_interval_actions(origin);
        self.fire_due_foundation_timers(origin);
        let _ = self.materialize_passive_loading_action_plan(origin, false);
        self.fire_due_delayed_selectors(origin);
    }

}
