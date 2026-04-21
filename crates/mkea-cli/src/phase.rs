use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

pub fn load_json(path: &Path) -> Result<Value> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read json file: {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse json file: {}", path.display()))?;
    Ok(value)
}

pub fn save_json(path: &Path, value: &Value) -> Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    fs::write(path, text).with_context(|| format!("failed to write json file: {}", path.display()))?;
    Ok(())
}

pub fn audit_phase(value: &Value) -> Value {
    let undefined = array_at(value, &["probe", "mach", "undefined_symbols"]);
    let dylibs = array_at(value, &["probe", "mach", "dylibs"]);
    let warnings = array_at(value, &["bootstrap", "warnings"]);

    let top_stub_prefixes = top_symbol_buckets(undefined);
    let top_dylibs = top_string_counts(dylibs.iter().filter_map(Value::as_str).map(dylib_bucket));
    let runtime = collect_runtime_summary(value);

    let mut out = Map::new();
    insert_opt_str(
        &mut out,
        "app",
        str_at(value, &["bootstrap", "app"]).or_else(|| str_at(value, &["probe", "manifest", "bundle_name"])),
    );
    insert_opt_str(
        &mut out,
        "bundle_id",
        str_at(value, &["bootstrap", "bundle_id"]).or_else(|| str_at(value, &["probe", "manifest", "bundle_id"])),
    );
    insert_opt_str(
        &mut out,
        "arch",
        str_at(value, &["bootstrap", "arch"]).or_else(|| str_at(value, &["probe", "mach", "arch"])),
    );
    insert_opt_str(
        &mut out,
        "minimum_ios_version",
        str_at(value, &["bootstrap", "minimum_ios_version"])
            .or_else(|| str_at(value, &["probe", "manifest", "minimum_ios_version"])),
    );
    insert_opt_u64(
        &mut out,
        "entry_pc",
        u64_at(value, &["runtime", "entry_pc"]).or_else(|| u64_at(value, &["bootstrap", "entry", "pc"])),
    );
    insert_opt_u64(
        &mut out,
        "initial_sp",
        u64_at(value, &["runtime", "initial_sp"]).or_else(|| u64_at(value, &["bootstrap", "entry", "sp"])),
    );
    insert_opt_u64(
        &mut out,
        "mapped_regions",
        u64_at(value, &["runtime", "mapped_regions"]).or_else(|| u64_at(value, &["bootstrap", "mapped_regions"])),
    );
    insert_opt_u64(
        &mut out,
        "registered_stubs",
        u64_at(value, &["runtime", "registered_stubs"])
            .or_else(|| u64_at(value, &["bootstrap", "image_load", "stubbed_symbols"])),
    );

    out.insert("undefined_symbols".to_string(), json!(undefined.len()));
    out.insert(
        "indirect_pointers".to_string(),
        json!(array_at(value, &["probe", "mach", "indirect_pointers"]).len()),
    );
    out.insert(
        "external_relocations".to_string(),
        json!(array_at(value, &["probe", "mach", "external_relocations"]).len()),
    );

    insert_opt_u64(&mut out, "segments_written", u64_at(value, &["bootstrap", "image_load", "segments_written"]));
    insert_opt_u64(&mut out, "bytes_written", u64_at(value, &["bootstrap", "image_load", "bytes_written"]));
    insert_opt_u64(&mut out, "stack_writes", u64_at(value, &["bootstrap", "image_load", "stack_writes"]));
    insert_opt_u64(&mut out, "trampoline_writes", u64_at(value, &["bootstrap", "image_load", "trampoline_writes"]));
    insert_opt_u64(
        &mut out,
        "indirect_pointer_writes",
        u64_at(value, &["bootstrap", "image_load", "indirect_pointer_writes"]),
    );
    insert_opt_u64(
        &mut out,
        "external_relocation_writes",
        u64_at(value, &["bootstrap", "image_load", "external_relocation_writes"]),
    );
    insert_opt_str(&mut out, "first_instruction_text", str_at(value, &["runtime", "first_instruction_text"]));
    insert_opt_bool(&mut out, "entry_bytes_present", bool_at(value, &["runtime", "entry_bytes_present"]));
    insert_opt_u64(&mut out, "executed_instructions", u64_at(value, &["runtime", "executed_instructions"]));
    insert_opt_u64(&mut out, "backend_total_steps", u64_at(value, &["runtime", "backend_execution", "total_steps"]));
    insert_opt_u64(&mut out, "backend_native_steps", u64_at(value, &["runtime", "backend_execution", "native_steps"]));
    insert_opt_u64(&mut out, "backend_shadow_steps", u64_at(value, &["runtime", "backend_execution", "shadow_steps"]));
    insert_opt_u64(&mut out, "backend_shadow_trap_steps", u64_at(value, &["runtime", "backend_execution", "shadow_trap_steps"]));
    insert_opt_u64(&mut out, "backend_shadow_fallback_steps", u64_at(value, &["runtime", "backend_execution", "shadow_fallback_steps"]));
    insert_opt_u64(&mut out, "backend_shadow_handoff_steps", u64_at(value, &["runtime", "backend_execution", "shadow_handoff_steps"]));
    insert_opt_u64(&mut out, "backend_trap_dispatches", u64_at(value, &["runtime", "backend_execution", "trap_dispatches"]));
    insert_opt_u64(&mut out, "backend_fallback_dispatches", u64_at(value, &["runtime", "backend_execution", "fallback_dispatches"]));
    insert_opt_u64(&mut out, "backend_handoff_count", u64_at(value, &["runtime", "backend_execution", "handoff_count"]));
    insert_opt_u64(&mut out, "backend_native_share_milli", u64_at(value, &["runtime", "backend_execution", "native_share_milli"]));
    insert_opt_u64(&mut out, "backend_shadow_share_milli", u64_at(value, &["runtime", "backend_execution", "shadow_share_milli"]));
    insert_opt_str(&mut out, "backend_last_trap_class", str_at(value, &["runtime", "backend_execution", "last_trap_class"]));
    insert_opt_str(&mut out, "backend_last_handoff_reason", str_at(value, &["runtime", "backend_execution", "last_handoff_reason"]));
    let backend_top_stop_sites = array_at(value, &["runtime", "backend_execution", "top_stop_sites"]);
    let backend_semantics_candidates = array_at(value, &["runtime", "backend_execution", "semantics_candidates"]);
    out.insert("backend_top_stop_sites".to_string(), Value::Array(backend_top_stop_sites.to_vec()));
    out.insert(
        "backend_semantics_candidates".to_string(),
        Value::Array(backend_semantics_candidates.to_vec()),
    );
    if let Some(primary_site) = backend_top_stop_sites.first() {
        insert_opt_str(&mut out, "backend_primary_stop_site_class", str_at(primary_site, &["trap_class"]));
        insert_opt_u64(&mut out, "backend_primary_stop_site_pc", u64_at(primary_site, &["pc"]));
        insert_opt_u64(&mut out, "backend_primary_stop_site_count", u64_at(primary_site, &["count"]));
        insert_opt_str(&mut out, "backend_primary_stop_site_symbol", str_at(primary_site, &["symbol"]));
    }
    if let Some(primary_candidate) = backend_semantics_candidates.first() {
        insert_opt_str(&mut out, "backend_primary_semantics_area", str_at(primary_candidate, &["area"]));
        insert_opt_u64(&mut out, "backend_primary_semantics_hits", u64_at(primary_candidate, &["total_hits"]));
        insert_opt_u64(&mut out, "backend_primary_semantics_score", u64_at(primary_candidate, &["weighted_score"]));
    }
    insert_opt_str(&mut out, "status", str_at(value, &["runtime", "status"]));
    insert_opt_str(&mut out, "stop_reason", str_at(value, &["runtime", "stop_reason"]));

    out.insert("warning_count".to_string(), json!(warnings.len()));
    out.insert("warnings".to_string(), Value::Array(warnings.to_vec()));
    out.insert("top_stub_prefixes".to_string(), Value::Array(top_stub_prefixes));
    out.insert("top_dylibs".to_string(), Value::Array(top_dylibs));

    insert_opt_string(&mut out, "runtime_state_source", runtime.source);
    insert_opt_string(&mut out, "conn_state", runtime.conn_state);
    insert_opt_u64(&mut out, "conn_state_code", runtime.conn_state_code);
    insert_opt_u64(&mut out, "fault_events", runtime.fault_events);
    out.insert(
        "fault_modes".to_string(),
        Value::Array(runtime.fault_modes.into_iter().map(Value::String).collect()),
    );
    insert_opt_string(&mut out, "last_error_domain", runtime.last_error_domain);
    insert_opt_i64(&mut out, "last_error_code", runtime.last_error_code);
    insert_opt_string(&mut out, "last_error_kind", runtime.last_error_kind);
    insert_opt_string(&mut out, "last_error_description", runtime.last_error_description);
    insert_opt_bool(&mut out, "retained_response", runtime.retained_response);
    insert_opt_bool(&mut out, "retained_data", runtime.retained_data);
    insert_opt_bool(&mut out, "retained_error", runtime.retained_error);
    insert_opt_u64(&mut out, "runloop_ticks", runtime.runloop_ticks);
    insert_opt_u64(&mut out, "runloop_sources", runtime.runloop_sources);
    insert_opt_u64(&mut out, "idle_after_completion", runtime.idle_after_completion);
    insert_opt_u64(&mut out, "input_consumed", runtime.input_consumed);
    insert_opt_u64(&mut out, "input_ignored", runtime.input_ignored);
    insert_opt_bool(&mut out, "input_pointer_down", runtime.input_pointer_down);
    insert_opt_u64(&mut out, "input_last_target", runtime.input_last_target);
    insert_opt_string(&mut out, "input_last_phase", runtime.input_last_phase);
    insert_opt_string(&mut out, "input_last_dispatch", runtime.input_last_dispatch);
    insert_opt_u64(&mut out, "input_cocos_attempts", runtime.input_cocos_attempts);
    insert_opt_u64(&mut out, "input_cocos_dispatched", runtime.input_cocos_dispatched);
    insert_opt_u64(&mut out, "net_events", runtime.net_events);
    insert_opt_u64(&mut out, "delegate_callbacks", runtime.delegate_callbacks);
    insert_opt_bool(&mut out, "retry_recommended", runtime.retry_recommended);
    insert_opt_bool(
        &mut out,
        "synthetic_network_fault_probes",
        runtime.synthetic_network_fault_probes,
    );
    insert_opt_u64(
        &mut out,
        "synthetic_runloop_tick_budget",
        runtime.synthetic_runloop_tick_budget,
    );
    insert_opt_u64(&mut out, "ui_cocos_director", runtime.ui_cocos_director);
    insert_opt_u64(&mut out, "ui_opengl_view", runtime.ui_opengl_view);
    insert_opt_u64(&mut out, "ui_running_scene", runtime.ui_running_scene);
    insert_opt_u64(&mut out, "ui_director_type", runtime.ui_director_type);
    insert_opt_bool(&mut out, "ui_animation_running", runtime.ui_animation_running);
    insert_opt_bool(&mut out, "ui_display_fps_enabled", runtime.ui_display_fps_enabled);
    insert_opt_u64(&mut out, "network_stage", runtime.network_stage);
    insert_opt_u64(&mut out, "network_bytes_delivered", runtime.network_bytes_delivered);
    insert_opt_u64(&mut out, "network_payload_len", runtime.network_payload_len);
    insert_opt_bool(&mut out, "network_source_closed", runtime.network_source_closed);
    insert_opt_bool(&mut out, "foundation_string_backing", runtime.foundation_string_backing);
    insert_opt_bool(&mut out, "foundation_data_backing", runtime.foundation_data_backing);
    insert_opt_u64(&mut out, "data_bytes_ptr", runtime.data_bytes_ptr);
    insert_opt_string(&mut out, "payload_preview_ascii", runtime.payload_preview_ascii);
    insert_opt_bool(&mut out, "reachability_scheduled", runtime.reachability_scheduled);
    insert_opt_bool(&mut out, "reachability_callback_set", runtime.reachability_callback_set);
    insert_opt_u64(&mut out, "reachability_flags", runtime.reachability_flags);
    insert_opt_string(&mut out, "read_stream_state", runtime.read_stream_state);
    insert_opt_u64(&mut out, "read_stream_state_code", runtime.read_stream_state_code);
    insert_opt_bool(&mut out, "read_stream_open", runtime.read_stream_open);
    insert_opt_bool(&mut out, "read_stream_scheduled", runtime.read_stream_scheduled);
    insert_opt_u64(&mut out, "read_stream_bytes_consumed", runtime.read_stream_bytes_consumed);
    insert_opt_bool(&mut out, "read_stream_has_bytes_available", runtime.read_stream_has_bytes_available);
    insert_opt_string(&mut out, "write_stream_state", runtime.write_stream_state);
    insert_opt_u64(&mut out, "write_stream_state_code", runtime.write_stream_state_code);
    insert_opt_bool(&mut out, "write_stream_open", runtime.write_stream_open);
    insert_opt_bool(&mut out, "write_stream_scheduled", runtime.write_stream_scheduled);
    insert_opt_u64(&mut out, "write_stream_bytes_written", runtime.write_stream_bytes_written);
    insert_opt_bool(&mut out, "write_stream_can_accept_bytes", runtime.write_stream_can_accept_bytes);
    insert_opt_bool(&mut out, "graphics_context_current", runtime.graphics_context_current);
    insert_opt_bool(&mut out, "graphics_layer_attached", runtime.graphics_layer_attached);
    insert_opt_bool(&mut out, "graphics_surface_ready", runtime.graphics_surface_ready);
    insert_opt_bool(&mut out, "graphics_framebuffer_complete", runtime.graphics_framebuffer_complete);
    insert_opt_bool(&mut out, "graphics_viewport_ready", runtime.graphics_viewport_ready);
    insert_opt_bool(&mut out, "graphics_presented", runtime.graphics_presented);
    insert_opt_bool(&mut out, "graphics_readback_ready", runtime.graphics_readback_ready);
    insert_opt_u64(&mut out, "graphics_frame_index", runtime.graphics_frame_index);
    insert_opt_u64(&mut out, "graphics_present_calls", runtime.graphics_present_calls);
    insert_opt_u64(&mut out, "graphics_draw_calls", runtime.graphics_draw_calls);
    insert_opt_u64(&mut out, "graphics_clear_calls", runtime.graphics_clear_calls);
    insert_opt_u64(&mut out, "graphics_readback_calls", runtime.graphics_readback_calls);
    insert_opt_u64(&mut out, "graphics_gl_calls", runtime.graphics_gl_calls);
    insert_opt_u64(&mut out, "graphics_surface_width", runtime.graphics_surface_width);
    insert_opt_u64(&mut out, "graphics_surface_height", runtime.graphics_surface_height);
    insert_opt_u64(&mut out, "graphics_viewport_width", runtime.graphics_viewport_width);
    insert_opt_u64(&mut out, "graphics_viewport_height", runtime.graphics_viewport_height);
    insert_opt_u64(&mut out, "graphics_framebuffer_bytes", runtime.graphics_framebuffer_bytes);
    insert_opt_u64(&mut out, "graphics_last_readback_bytes", runtime.graphics_last_readback_bytes);
    insert_opt_u64(&mut out, "graphics_last_readback_checksum", runtime.graphics_last_readback_checksum);
    insert_opt_string(&mut out, "graphics_last_readback_origin", runtime.graphics_last_readback_origin);
    insert_opt_string(&mut out, "graphics_last_present_source", runtime.graphics_last_present_source);
    insert_opt_string(&mut out, "graphics_last_present_decision", runtime.graphics_last_present_decision);
    insert_opt_u64(&mut out, "graphics_retained_present_calls", runtime.graphics_retained_present_calls);
    insert_opt_u64(&mut out, "graphics_synthetic_fallback_present_calls", runtime.graphics_synthetic_fallback_present_calls);
    insert_opt_u64(&mut out, "graphics_auto_scene_present_calls", runtime.graphics_auto_scene_present_calls);
    insert_opt_u64(&mut out, "graphics_guest_draw_calls", runtime.graphics_guest_draw_calls);
    insert_opt_u64(&mut out, "graphics_guest_vertex_fetches", runtime.graphics_guest_vertex_fetches);
    insert_opt_u64(&mut out, "graphics_last_draw_mode", runtime.graphics_last_draw_mode);
    insert_opt_string(&mut out, "graphics_last_draw_mode_label", runtime.graphics_last_draw_mode_label);
    insert_opt_u64(&mut out, "graphics_last_guest_draw_checksum", runtime.graphics_last_guest_draw_checksum);
    insert_opt_bool(&mut out, "graphics_uikit_context_current", runtime.graphics_uikit_context_current);
    insert_opt_u64(&mut out, "graphics_uikit_contexts_created", runtime.graphics_uikit_contexts_created);
    insert_opt_u64(&mut out, "graphics_uikit_images_created", runtime.graphics_uikit_images_created);
    insert_opt_u64(&mut out, "graphics_uikit_draw_ops", runtime.graphics_uikit_draw_ops);
    insert_opt_u64(&mut out, "graphics_uikit_present_ops", runtime.graphics_uikit_present_ops);
    insert_opt_string(&mut out, "graphics_last_ui_source", runtime.graphics_last_ui_source);
    insert_opt_bool(&mut out, "graphics_dump_frames_enabled", runtime.graphics_dump_frames_enabled);
    insert_opt_u64(&mut out, "graphics_dump_every", runtime.graphics_dump_every);
    insert_opt_u64(&mut out, "graphics_dump_limit", runtime.graphics_dump_limit);
    insert_opt_u64(&mut out, "graphics_dumps_saved", runtime.graphics_dumps_saved);
    insert_opt_string(&mut out, "graphics_last_dump_path", runtime.graphics_last_dump_path);
    insert_opt_bool(&mut out, "filesystem_bundle_available", runtime.filesystem_bundle_available);
    insert_opt_string(&mut out, "filesystem_bundle_root", runtime.filesystem_bundle_root);
    insert_opt_u64(&mut out, "filesystem_indexed_files", runtime.filesystem_indexed_files);
    insert_opt_u64(&mut out, "filesystem_cached_images", runtime.filesystem_cached_images);
    insert_opt_u64(&mut out, "filesystem_bundle_objects_created", runtime.filesystem_bundle_objects_created);
    insert_opt_u64(&mut out, "filesystem_bundle_scoped_hits", runtime.filesystem_bundle_scoped_hits);
    insert_opt_u64(&mut out, "filesystem_bundle_scoped_misses", runtime.filesystem_bundle_scoped_misses);
    insert_opt_u64(&mut out, "filesystem_png_cgbi_detected", runtime.filesystem_png_cgbi_detected);
    insert_opt_u64(&mut out, "filesystem_png_cgbi_decoded", runtime.filesystem_png_cgbi_decoded);
    insert_opt_u64(&mut out, "filesystem_png_decode_failures", runtime.filesystem_png_decode_failures);
    insert_opt_u64(&mut out, "filesystem_image_named_hits", runtime.filesystem_image_named_hits);
    insert_opt_u64(&mut out, "filesystem_image_named_misses", runtime.filesystem_image_named_misses);
    insert_opt_u64(&mut out, "filesystem_file_open_hits", runtime.filesystem_file_open_hits);
    insert_opt_u64(&mut out, "filesystem_file_open_misses", runtime.filesystem_file_open_misses);
    insert_opt_u64(&mut out, "filesystem_file_read_ops", runtime.filesystem_file_read_ops);
    insert_opt_u64(&mut out, "filesystem_file_bytes_read", runtime.filesystem_file_bytes_read);
    insert_opt_u64(&mut out, "filesystem_open_file_handles", runtime.filesystem_open_file_handles);
    insert_opt_string(&mut out, "filesystem_last_resource_name", runtime.filesystem_last_resource_name);
    insert_opt_string(&mut out, "filesystem_last_resource_path", runtime.filesystem_last_resource_path);
    insert_opt_string(&mut out, "filesystem_last_file_path", runtime.filesystem_last_file_path);
    insert_opt_string(&mut out, "filesystem_last_file_mode", runtime.filesystem_last_file_mode);
    insert_opt_u64(&mut out, "heap_base", runtime.heap_base);
    insert_opt_u64(&mut out, "heap_end", runtime.heap_end);
    insert_opt_u64(&mut out, "heap_cursor", runtime.heap_cursor);
    insert_opt_u64(&mut out, "heap_allocations_total", runtime.heap_allocations_total);
    insert_opt_u64(&mut out, "heap_allocations_active", runtime.heap_allocations_active);
    insert_opt_u64(&mut out, "heap_frees", runtime.heap_frees);
    insert_opt_u64(&mut out, "heap_reallocs", runtime.heap_reallocs);
    insert_opt_u64(&mut out, "heap_bytes_active", runtime.heap_bytes_active);
    insert_opt_u64(&mut out, "heap_bytes_peak", runtime.heap_bytes_peak);
    insert_opt_u64(&mut out, "heap_bytes_reserved", runtime.heap_bytes_reserved);
    insert_opt_u64(&mut out, "heap_last_alloc_ptr", runtime.heap_last_alloc_ptr);
    insert_opt_u64(&mut out, "heap_last_alloc_size", runtime.heap_last_alloc_size);
    insert_opt_u64(&mut out, "heap_last_freed_ptr", runtime.heap_last_freed_ptr);
    insert_opt_u64(&mut out, "heap_last_realloc_old_ptr", runtime.heap_last_realloc_old_ptr);
    insert_opt_u64(&mut out, "heap_last_realloc_new_ptr", runtime.heap_last_realloc_new_ptr);
    insert_opt_u64(&mut out, "heap_last_realloc_size", runtime.heap_last_realloc_size);
    insert_opt_string(&mut out, "heap_last_error", runtime.heap_last_error);
    insert_opt_u64(&mut out, "vfp_multi_ops", runtime.vfp_multi_ops);
    insert_opt_u64(&mut out, "vfp_load_multi_ops", runtime.vfp_load_multi_ops);
    insert_opt_u64(&mut out, "vfp_store_multi_ops", runtime.vfp_store_multi_ops);
    insert_opt_u64(&mut out, "vfp_pc_base_ops", runtime.vfp_pc_base_ops);
    insert_opt_u64(&mut out, "vfp_pc_base_load_ops", runtime.vfp_pc_base_load_ops);
    insert_opt_u64(&mut out, "vfp_pc_base_store_ops", runtime.vfp_pc_base_store_ops);
    insert_opt_u64(&mut out, "vfp_single_reg_capacity", runtime.vfp_single_reg_capacity);
    insert_opt_u64(&mut out, "vfp_single_range_ops", runtime.vfp_single_range_ops);
    insert_opt_u64(&mut out, "vfp_exact_opcode_hits", runtime.vfp_exact_opcode_hits);
    insert_opt_u64(&mut out, "vfp_exact_override_hits", runtime.vfp_exact_override_hits);
    insert_opt_u64(&mut out, "vfp_single_transfer_ops", runtime.vfp_single_transfer_ops);
    insert_opt_u64(&mut out, "vfp_double_transfer_ops", runtime.vfp_double_transfer_ops);
    insert_opt_u64(&mut out, "vfp_last_start_addr", runtime.vfp_last_start_addr);
    insert_opt_u64(&mut out, "vfp_last_end_addr", runtime.vfp_last_end_addr);
    insert_opt_u64(&mut out, "vfp_last_pc_base_addr", runtime.vfp_last_pc_base_addr);
    insert_opt_u64(&mut out, "vfp_last_pc_base_word", runtime.vfp_last_pc_base_word);
    insert_opt_string(&mut out, "vfp_last_op", runtime.vfp_last_op);
    insert_opt_string(&mut out, "vfp_last_single_range", runtime.vfp_last_single_range);
    insert_opt_string(&mut out, "vfp_last_exact_opcode", runtime.vfp_last_exact_opcode);
    insert_opt_string(&mut out, "vfp_last_exact_decoder_branch", runtime.vfp_last_exact_decoder_branch);
    insert_opt_string(&mut out, "vfp_last_transfer_mode", runtime.vfp_last_transfer_mode);
    insert_opt_u64(&mut out, "vfp_last_transfer_start_reg", runtime.vfp_last_transfer_start_reg);
    insert_opt_u64(&mut out, "vfp_last_transfer_end_reg", runtime.vfp_last_transfer_end_reg);
    insert_opt_u64(&mut out, "vfp_last_transfer_count", runtime.vfp_last_transfer_count);
    insert_opt_string(&mut out, "vfp_last_transfer_precision", runtime.vfp_last_transfer_precision);
    insert_opt_u64(&mut out, "vfp_last_transfer_addr", runtime.vfp_last_transfer_addr);
    insert_opt_string(&mut out, "vfp_last_exact_reason", runtime.vfp_last_exact_reason);
    insert_opt_u64(&mut out, "arm_reg_shift_operand2_ops", runtime.arm_reg_shift_operand2_ops);
    insert_opt_u64(&mut out, "arm_extra_load_store_ops", runtime.arm_extra_load_store_ops);
    insert_opt_u64(&mut out, "arm_extra_load_store_loads", runtime.arm_extra_load_store_loads);
    insert_opt_u64(&mut out, "arm_extra_load_store_stores", runtime.arm_extra_load_store_stores);
    insert_opt_string(&mut out, "arm_last_reg_shift", runtime.arm_last_reg_shift);
    insert_opt_string(&mut out, "arm_last_extra_load_store", runtime.arm_last_extra_load_store);
    insert_opt_u64(&mut out, "arm_exact_epilogue_site_hits", runtime.arm_exact_epilogue_site_hits);
    insert_opt_u64(&mut out, "arm_exact_epilogue_repairs", runtime.arm_exact_epilogue_repairs);
    insert_opt_u64(&mut out, "arm_exact_epilogue_last_pc", runtime.arm_exact_epilogue_last_pc);
    insert_opt_u64(&mut out, "arm_exact_epilogue_last_before_sp", runtime.arm_exact_epilogue_last_before_sp);
    insert_opt_u64(&mut out, "arm_exact_epilogue_last_after_sp", runtime.arm_exact_epilogue_last_after_sp);
    insert_opt_u64(&mut out, "arm_exact_epilogue_last_r0", runtime.arm_exact_epilogue_last_r0);
    insert_opt_u64(&mut out, "arm_exact_epilogue_last_r7", runtime.arm_exact_epilogue_last_r7);
    insert_opt_u64(&mut out, "arm_exact_epilogue_last_r8", runtime.arm_exact_epilogue_last_r8);
    insert_opt_u64(&mut out, "arm_exact_epilogue_last_lr", runtime.arm_exact_epilogue_last_lr);
    insert_opt_string(&mut out, "arm_exact_epilogue_last_repair", runtime.arm_exact_epilogue_last_repair);
    insert_opt_bool(&mut out, "objc_bridge_metadata_available", runtime.objc_bridge_metadata_available);
    insert_opt_bool(&mut out, "objc_bridge_classlist_present", runtime.objc_bridge_classlist_present);
    insert_opt_bool(&mut out, "objc_bridge_cfstring_present", runtime.objc_bridge_cfstring_present);
    insert_opt_u64(&mut out, "objc_bridge_parsed_classes", runtime.objc_bridge_parsed_classes);
    insert_opt_string(&mut out, "objc_bridge_delegate_name", runtime.objc_bridge_delegate_name);
    insert_opt_string(&mut out, "objc_bridge_delegate_class_name", runtime.objc_bridge_delegate_class_name);
    insert_opt_string(&mut out, "objc_bridge_inferred_class_name", runtime.objc_bridge_inferred_class_name);
    insert_opt_u64(&mut out, "objc_bridge_inferred_selector_hits", runtime.objc_bridge_inferred_selector_hits);
    insert_opt_string(&mut out, "objc_bridge_launch_selector", runtime.objc_bridge_launch_selector);
    insert_opt_u64(&mut out, "objc_bridge_launch_imp", runtime.objc_bridge_launch_imp);
    insert_opt_bool(&mut out, "objc_bridge_attempted", runtime.objc_bridge_attempted);
    insert_opt_bool(&mut out, "objc_bridge_succeeded", runtime.objc_bridge_succeeded);
    insert_opt_string(&mut out, "objc_bridge_failure_reason", runtime.objc_bridge_failure_reason);
    insert_opt_u64(&mut out, "objc_real_msgsend_dispatches", runtime.objc_real_msgsend_dispatches);
    insert_opt_string(&mut out, "objc_last_real_selector", runtime.objc_last_real_selector);
    insert_opt_u64(&mut out, "objc_super_msgsend_dispatches", runtime.objc_super_msgsend_dispatches);
    insert_opt_u64(&mut out, "objc_super_msgsend_fallback_returns", runtime.objc_super_msgsend_fallback_returns);
    insert_opt_string(&mut out, "objc_last_super_selector", runtime.objc_last_super_selector);
    insert_opt_u64(&mut out, "objc_last_super_receiver", runtime.objc_last_super_receiver);
    insert_opt_u64(&mut out, "objc_last_super_class", runtime.objc_last_super_class);
    insert_opt_u64(&mut out, "objc_last_super_imp", runtime.objc_last_super_imp);
    insert_opt_u64(&mut out, "objc_alloc_calls", runtime.objc_alloc_calls);
    insert_opt_u64(&mut out, "objc_alloc_with_zone_calls", runtime.objc_alloc_with_zone_calls);
    insert_opt_u64(&mut out, "objc_class_create_instance_calls", runtime.objc_class_create_instance_calls);
    insert_opt_u64(&mut out, "objc_init_calls", runtime.objc_init_calls);
    insert_opt_u64(&mut out, "objc_instances_materialized", runtime.objc_instances_materialized);
    insert_opt_string(&mut out, "objc_last_alloc_class", runtime.objc_last_alloc_class);
    insert_opt_u64(&mut out, "objc_last_alloc_receiver", runtime.objc_last_alloc_receiver);
    insert_opt_u64(&mut out, "objc_last_alloc_result", runtime.objc_last_alloc_result);
    insert_opt_u64(&mut out, "objc_last_init_receiver", runtime.objc_last_init_receiver);
    insert_opt_u64(&mut out, "objc_last_init_result", runtime.objc_last_init_result);
    insert_opt_u64(&mut out, "hot_objc_msgsend_calls", runtime.hot_objc_msgsend_calls);
    insert_opt_u64(&mut out, "hot_objc_unique_selectors", runtime.hot_objc_unique_selectors);
    out.insert(
        "hot_recent_objc_selectors".to_string(),
        Value::Array(runtime.hot_recent_objc_selectors.into_iter().map(Value::String).collect()),
    );
    out.insert("hot_top_objc_selectors".to_string(), runtime.hot_top_objc_selectors);
    insert_opt_bool(&mut out, "hot_saw_draw_rect", runtime.hot_saw_draw_rect);
    insert_opt_bool(&mut out, "hot_saw_set_needs_display", runtime.hot_saw_set_needs_display);
    insert_opt_bool(&mut out, "hot_saw_layout_subviews", runtime.hot_saw_layout_subviews);
    insert_opt_bool(&mut out, "hot_saw_image_named", runtime.hot_saw_image_named);
    insert_opt_bool(&mut out, "hot_saw_present_renderbuffer", runtime.hot_saw_present_renderbuffer);
    insert_opt_u64(&mut out, "hot_gl_calls_seen", runtime.hot_gl_calls_seen);
    out.insert(
        "hot_recent_gl_calls".to_string(),
        Value::Array(runtime.hot_recent_gl_calls.into_iter().map(Value::String).collect()),
    );
    out.insert("hot_top_gl_calls".to_string(), runtime.hot_top_gl_calls);
    insert_opt_bool(&mut out, "hot_saw_gl_bind_texture", runtime.hot_saw_gl_bind_texture);
    insert_opt_bool(&mut out, "hot_saw_gl_teximage2d", runtime.hot_saw_gl_teximage2d);
    insert_opt_bool(&mut out, "hot_saw_gl_draw_arrays", runtime.hot_saw_gl_draw_arrays);
    insert_opt_bool(&mut out, "hot_saw_gl_draw_elements", runtime.hot_saw_gl_draw_elements);

    Value::Object(out)
}

pub fn diff_phase(baseline: &Value, candidate: &Value) -> Value {
    let base = audit_phase(baseline);
    let cand = audit_phase(candidate);

    let metrics = [
        "mapped_regions",
        "registered_stubs",
        "undefined_symbols",
        "indirect_pointers",
        "external_relocations",
        "segments_written",
        "bytes_written",
        "stack_writes",
        "trampoline_writes",
        "indirect_pointer_writes",
        "external_relocation_writes",
        "executed_instructions",
        "backend_total_steps",
        "backend_native_steps",
        "backend_shadow_steps",
        "backend_trap_dispatches",
        "backend_fallback_dispatches",
        "backend_handoff_count",
        "backend_native_share_milli",
        "backend_shadow_share_milli",
        "backend_primary_stop_site_count",
        "backend_primary_semantics_hits",
        "backend_primary_semantics_score",
        "warning_count",
        "runloop_ticks",
        "runloop_sources",
        "idle_after_completion",
        "net_events",
        "delegate_callbacks",
        "fault_events",
        "network_bytes_delivered",
        "network_payload_len",
        "reachability_flags",
        "read_stream_bytes_consumed",
        "write_stream_bytes_written",
        "graphics_frame_index",
        "graphics_present_calls",
        "graphics_draw_calls",
        "graphics_clear_calls",
        "graphics_readback_calls",
        "graphics_gl_calls",
        "graphics_surface_width",
        "graphics_surface_height",
        "graphics_viewport_width",
        "graphics_viewport_height",
        "graphics_framebuffer_bytes",
        "graphics_last_readback_bytes",
        "graphics_last_readback_checksum",
        "graphics_guest_draw_calls",
        "graphics_guest_vertex_fetches",
        "graphics_last_draw_mode",
        "graphics_last_guest_draw_checksum",
        "graphics_uikit_contexts_created",
        "graphics_uikit_images_created",
        "graphics_uikit_draw_ops",
        "graphics_uikit_present_ops",
        "graphics_retained_present_calls",
        "graphics_synthetic_fallback_present_calls",
        "graphics_auto_scene_present_calls",
        "graphics_dumps_saved",
        "filesystem_indexed_files",
        "filesystem_cached_images",
        "filesystem_bundle_objects_created",
        "filesystem_bundle_scoped_hits",
        "filesystem_bundle_scoped_misses",
        "filesystem_png_cgbi_detected",
        "filesystem_png_cgbi_decoded",
        "filesystem_png_decode_failures",
        "filesystem_image_named_hits",
        "filesystem_image_named_misses",
        "filesystem_file_open_hits",
        "filesystem_file_open_misses",
        "filesystem_file_read_ops",
        "filesystem_file_bytes_read",
        "filesystem_open_file_handles",
        "heap_base",
        "heap_end",
        "heap_cursor",
        "heap_allocations_total",
        "heap_allocations_active",
        "heap_frees",
        "heap_reallocs",
        "heap_bytes_active",
        "heap_bytes_peak",
        "heap_bytes_reserved",
        "heap_last_alloc_ptr",
        "heap_last_alloc_size",
        "heap_last_freed_ptr",
        "heap_last_realloc_old_ptr",
        "heap_last_realloc_new_ptr",
        "heap_last_realloc_size",
        "vfp_multi_ops",
        "vfp_load_multi_ops",
        "vfp_store_multi_ops",
        "vfp_pc_base_ops",
        "vfp_pc_base_load_ops",
        "vfp_pc_base_store_ops",
        "vfp_single_reg_capacity",
        "vfp_single_range_ops",
        "vfp_exact_opcode_hits",
        "vfp_exact_override_hits",
        "vfp_single_transfer_ops",
        "vfp_double_transfer_ops",
        "vfp_last_start_addr",
        "vfp_last_end_addr",
        "vfp_last_pc_base_addr",
        "vfp_last_pc_base_word",
        "vfp_last_single_range",
        "vfp_last_exact_opcode",
        "vfp_last_exact_decoder_branch",
        "vfp_last_transfer_mode",
        "vfp_last_transfer_start_reg",
        "vfp_last_transfer_end_reg",
        "vfp_last_transfer_count",
        "vfp_last_transfer_precision",
        "vfp_last_transfer_addr",
        "vfp_last_exact_reason",
        "arm_reg_shift_operand2_ops",
        "arm_extra_load_store_ops",
        "arm_extra_load_store_loads",
        "arm_extra_load_store_stores",
        "objc_bridge_parsed_classes",
        "objc_bridge_launch_imp",
        "objc_real_msgsend_dispatches",
        "hot_objc_msgsend_calls",
        "hot_objc_unique_selectors",
        "hot_gl_calls_seen",
    ];

    let mut deltas = serde_json::Map::new();
    for metric in metrics {
        if let (Some(before), Some(after)) = (number_at(&base, metric), number_at(&cand, metric)) {
            deltas.insert(metric.to_string(), json!({
                "baseline": before,
                "candidate": after,
                "delta": after - before,
            }));
        }
    }

    let mut regressions = Vec::new();
    let mut improvements = Vec::new();

    cmp_opt_u64(&base, &cand, "executed_instructions", "executed instructions", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "backend_native_steps", "native guest steps", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "backend_shadow_steps", "shadow backend steps", OrderingPref::LowerBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "backend_handoff_count", "hybrid handoff count", OrderingPref::LowerBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "registered_stubs", "registered stubs", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "indirect_pointer_writes", "indirect pointer writes", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "external_relocation_writes", "external relocation writes", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "warning_count", "warning count", OrderingPref::LowerBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "fault_events", "fault events", OrderingPref::LowerBetter, &mut regressions, &mut improvements);

    cmp_conn_state(&base, &cand, &mut regressions, &mut improvements);
    cmp_last_error(&base, &cand, &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "retained_response", "retained response", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "retained_data", "retained data", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "retained_error", "retained error", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "foundation_string_backing", "foundation string backing", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "foundation_data_backing", "foundation data backing", &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "ui_cocos_director", "cocos director object", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "ui_opengl_view", "cocos OpenGL view", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "ui_running_scene", "cocos running scene", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "ui_director_type", "cocos director type", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "ui_animation_running", "cocos animation running", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "ui_display_fps_enabled", "cocos display FPS flag", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "reachability_scheduled", "reachability scheduled", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "read_stream_open", "read stream open", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "read_stream_scheduled", "read stream scheduled", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "write_stream_open", "write stream open", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "write_stream_scheduled", "write stream scheduled", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "graphics_context_current", "graphics context current", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "graphics_layer_attached", "graphics layer attached", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "graphics_surface_ready", "graphics surface ready", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "graphics_framebuffer_complete", "graphics framebuffer complete", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "graphics_viewport_ready", "graphics viewport ready", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "graphics_presented", "graphics presented", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "graphics_readback_ready", "graphics readback ready", &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_frame_index", "graphics frame index", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_present_calls", "graphics present calls", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_draw_calls", "graphics draw calls", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_clear_calls", "graphics clear calls", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_readback_calls", "graphics readback calls", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_gl_calls", "graphics GL calls", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_framebuffer_bytes", "graphics framebuffer bytes", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_last_readback_bytes", "graphics last readback bytes", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_last_readback_checksum", "graphics last readback checksum", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_guest_draw_calls", "graphics guest draw calls", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_guest_vertex_fetches", "graphics guest vertex fetches", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_last_draw_mode", "graphics last draw mode", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_last_guest_draw_checksum", "graphics last guest draw checksum", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "graphics_uikit_context_current", "graphics UIKit context current", &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_uikit_contexts_created", "graphics UIKit contexts created", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_uikit_images_created", "graphics UIKit images created", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_uikit_draw_ops", "graphics UIKit draw ops", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_uikit_present_ops", "graphics UIKit present ops", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_retained_present_calls", "graphics retained presents", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_auto_scene_present_calls", "graphics auto-scene presents", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_synthetic_fallback_present_calls", "graphics synthetic fallback presents", OrderingPref::LowerBetter, &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "graphics_last_ui_source", "graphics last UIKit source", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "graphics_last_present_source", "graphics last present source", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "graphics_last_present_decision", "graphics last present decision", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "graphics_last_draw_mode_label", "graphics last draw mode label", &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "graphics_dumps_saved", "graphics dumps saved", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "graphics_dump_frames_enabled", "graphics dump frames enabled", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "filesystem_bundle_available", "filesystem bundle available", &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_indexed_files", "filesystem indexed files", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_cached_images", "filesystem cached images", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_bundle_objects_created", "filesystem bundle objects created", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_bundle_scoped_hits", "filesystem bundle scoped hits", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_bundle_scoped_misses", "filesystem bundle scoped misses", OrderingPref::LowerBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_png_cgbi_detected", "filesystem CgBI PNG detected", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_png_cgbi_decoded", "filesystem CgBI PNG decoded", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_png_decode_failures", "filesystem PNG decode failures", OrderingPref::LowerBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_image_named_hits", "filesystem imageNamed hits", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_image_named_misses", "filesystem imageNamed misses", OrderingPref::LowerBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_file_open_hits", "filesystem file open hits", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_file_open_misses", "filesystem file open misses", OrderingPref::LowerBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_file_read_ops", "filesystem file read ops", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_file_bytes_read", "filesystem file bytes read", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "filesystem_open_file_handles", "filesystem open file handles", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "filesystem_last_resource_path", "filesystem last resource path", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "filesystem_last_file_path", "filesystem last file path", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "filesystem_last_file_mode", "filesystem last file mode", &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "heap_cursor", "heap cursor", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "heap_allocations_total", "heap allocations total", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "heap_allocations_active", "heap allocations active", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "heap_frees", "heap frees", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "heap_reallocs", "heap reallocs", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "heap_bytes_active", "heap bytes active", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "heap_bytes_peak", "heap bytes peak", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "heap_bytes_reserved", "heap bytes reserved", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "heap_last_error", "heap last error", &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "vfp_multi_ops", "VFP multi-register ops", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "vfp_load_multi_ops", "VFP multi-register loads", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "vfp_store_multi_ops", "VFP multi-register stores", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "vfp_pc_base_ops", "VFP PC-base ops", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "vfp_pc_base_load_ops", "VFP PC-base loads", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "vfp_pc_base_store_ops", "VFP PC-base stores", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "vfp_single_range_ops", "VFP single-range ops", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "vfp_exact_opcode_hits", "VFP exact-opcode hits", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "vfp_exact_override_hits", "VFP exact override hits", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "vfp_single_transfer_ops", "VFP single-transfer ops", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "vfp_double_transfer_ops", "VFP double-transfer ops", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "vfp_last_op", "last VFP op", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "vfp_last_single_range", "last VFP single range", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "vfp_last_exact_opcode", "last exact VFP opcode decode", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "vfp_last_exact_decoder_branch", "last exact VFP decoder branch", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "vfp_last_transfer_mode", "last VFP transfer mode", &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "arm_reg_shift_operand2_ops", "ARM register-shift operand2 ops", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "arm_extra_load_store_ops", "ARM extra load/store ops", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "arm_extra_load_store_loads", "ARM extra load/store loads", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "arm_extra_load_store_stores", "ARM extra load/store stores", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "arm_last_reg_shift", "last ARM register shift", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "arm_last_extra_load_store", "last ARM extra load/store", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "objc_bridge_metadata_available", "objc bridge metadata available", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "objc_bridge_classlist_present", "objc classlist present", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "objc_bridge_cfstring_present", "objc cfstring present", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "objc_bridge_attempted", "objc UIApplicationMain bridge attempted", &mut regressions, &mut improvements);
    cmp_bool(&base, &cand, "objc_bridge_succeeded", "objc UIApplicationMain bridge succeeded", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "objc_bridge_failure_reason", "objc bridge failure reason", &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "objc_bridge_parsed_classes", "objc parsed classes", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "objc_bridge_launch_imp", "objc launch IMP", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_u64(&base, &cand, "objc_real_msgsend_dispatches", "real objc_msgSend dispatches", OrderingPref::HigherBetter, &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "objc_bridge_delegate_class_name", "objc delegate class", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "objc_bridge_launch_selector", "objc launch selector", &mut regressions, &mut improvements);
    cmp_opt_str(&base, &cand, "objc_last_real_selector", "objc last real selector", &mut regressions, &mut improvements);

    let base_entry = base.get("entry_bytes_present").and_then(Value::as_bool);
    let cand_entry = cand.get("entry_bytes_present").and_then(Value::as_bool);
    match (base_entry, cand_entry) {
        (Some(true), Some(false)) => regressions.push("entry bytes were present in baseline but missing in candidate".to_string()),
        (Some(false), Some(true)) => improvements.push("candidate now has entry bytes present".to_string()),
        _ => {}
    }

    let base_runtime_src = base.get("runtime_state_source").and_then(Value::as_str).unwrap_or_default();
    let cand_runtime_src = cand.get("runtime_state_source").and_then(Value::as_str).unwrap_or_default();
    if base_runtime_src != "runtime_state" && cand_runtime_src == "runtime_state" {
        improvements.push("candidate now exposes structured runtime_state telemetry".to_string());
    }

    let base_stop = base.get("stop_reason").and_then(Value::as_str).unwrap_or_default();
    let cand_stop = cand.get("stop_reason").and_then(Value::as_str).unwrap_or_default();
    if base_stop != cand_stop {
        if looks_bad_stop(cand_stop) && !looks_bad_stop(base_stop) {
            regressions.push(format!("candidate stop reason looks worse: {cand_stop}"));
        } else if looks_bad_stop(base_stop) && !looks_bad_stop(cand_stop) {
            improvements.push(format!("candidate stop reason looks healthier: {cand_stop}"));
        }
    }

    json!({
        "baseline": base,
        "candidate": cand,
        "deltas": deltas,
        "regressions": regressions,
        "improvements": improvements,
    })
}

#[derive(Debug, Clone, Copy)]
enum OrderingPref {
    HigherBetter,
    LowerBetter,
}

#[derive(Debug, Default, Clone)]
struct RuntimeAudit {
    source: Option<String>,
    conn_state: Option<String>,
    conn_state_code: Option<u64>,
    fault_events: Option<u64>,
    fault_modes: Vec<String>,
    last_error_domain: Option<String>,
    last_error_code: Option<i64>,
    last_error_kind: Option<String>,
    last_error_description: Option<String>,
    retained_response: Option<bool>,
    retained_data: Option<bool>,
    retained_error: Option<bool>,
    runloop_ticks: Option<u64>,
    runloop_sources: Option<u64>,
    idle_after_completion: Option<u64>,
    input_consumed: Option<u64>,
    input_ignored: Option<u64>,
    input_pointer_down: Option<bool>,
    input_last_target: Option<u64>,
    input_last_phase: Option<String>,
    input_last_dispatch: Option<String>,
    input_cocos_attempts: Option<u64>,
    input_cocos_dispatched: Option<u64>,
    net_events: Option<u64>,
    delegate_callbacks: Option<u64>,
    retry_recommended: Option<bool>,
    synthetic_network_fault_probes: Option<bool>,
    synthetic_runloop_tick_budget: Option<u64>,
    ui_cocos_director: Option<u64>,
    ui_opengl_view: Option<u64>,
    ui_running_scene: Option<u64>,
    ui_director_type: Option<u64>,
    ui_animation_running: Option<bool>,
    ui_display_fps_enabled: Option<bool>,
    network_stage: Option<u64>,
    network_bytes_delivered: Option<u64>,
    network_payload_len: Option<u64>,
    network_source_closed: Option<bool>,
    foundation_string_backing: Option<bool>,
    foundation_data_backing: Option<bool>,
    data_bytes_ptr: Option<u64>,
    payload_preview_ascii: Option<String>,
    reachability_scheduled: Option<bool>,
    reachability_callback_set: Option<bool>,
    reachability_flags: Option<u64>,
    read_stream_state: Option<String>,
    read_stream_state_code: Option<u64>,
    read_stream_open: Option<bool>,
    read_stream_scheduled: Option<bool>,
    read_stream_bytes_consumed: Option<u64>,
    read_stream_has_bytes_available: Option<bool>,
    write_stream_state: Option<String>,
    write_stream_state_code: Option<u64>,
    write_stream_open: Option<bool>,
    write_stream_scheduled: Option<bool>,
    write_stream_bytes_written: Option<u64>,
    write_stream_can_accept_bytes: Option<bool>,
    graphics_context_current: Option<bool>,
    graphics_layer_attached: Option<bool>,
    graphics_surface_ready: Option<bool>,
    graphics_framebuffer_complete: Option<bool>,
    graphics_viewport_ready: Option<bool>,
    graphics_presented: Option<bool>,
    graphics_readback_ready: Option<bool>,
    graphics_frame_index: Option<u64>,
    graphics_present_calls: Option<u64>,
    graphics_draw_calls: Option<u64>,
    graphics_clear_calls: Option<u64>,
    graphics_readback_calls: Option<u64>,
    graphics_gl_calls: Option<u64>,
    graphics_surface_width: Option<u64>,
    graphics_surface_height: Option<u64>,
    graphics_viewport_width: Option<u64>,
    graphics_viewport_height: Option<u64>,
    graphics_framebuffer_bytes: Option<u64>,
    graphics_last_readback_bytes: Option<u64>,
    graphics_last_readback_checksum: Option<u64>,
    graphics_last_readback_origin: Option<String>,
    graphics_last_present_source: Option<String>,
    graphics_last_present_decision: Option<String>,
    graphics_retained_present_calls: Option<u64>,
    graphics_synthetic_fallback_present_calls: Option<u64>,
    graphics_auto_scene_present_calls: Option<u64>,
    graphics_guest_draw_calls: Option<u64>,
    graphics_guest_vertex_fetches: Option<u64>,
    graphics_last_draw_mode: Option<u64>,
    graphics_last_draw_mode_label: Option<String>,
    graphics_last_guest_draw_checksum: Option<u64>,
    graphics_uikit_context_current: Option<bool>,
    graphics_uikit_contexts_created: Option<u64>,
    graphics_uikit_images_created: Option<u64>,
    graphics_uikit_draw_ops: Option<u64>,
    graphics_uikit_present_ops: Option<u64>,
    graphics_last_ui_source: Option<String>,
    graphics_dump_frames_enabled: Option<bool>,
    graphics_dump_every: Option<u64>,
    graphics_dump_limit: Option<u64>,
    graphics_dumps_saved: Option<u64>,
    graphics_last_dump_path: Option<String>,
    filesystem_bundle_available: Option<bool>,
    filesystem_bundle_root: Option<String>,
    filesystem_indexed_files: Option<u64>,
    filesystem_cached_images: Option<u64>,
    filesystem_bundle_objects_created: Option<u64>,
    filesystem_bundle_scoped_hits: Option<u64>,
    filesystem_bundle_scoped_misses: Option<u64>,
    filesystem_png_cgbi_detected: Option<u64>,
    filesystem_png_cgbi_decoded: Option<u64>,
    filesystem_png_decode_failures: Option<u64>,
    filesystem_image_named_hits: Option<u64>,
    filesystem_image_named_misses: Option<u64>,
    filesystem_file_open_hits: Option<u64>,
    filesystem_file_open_misses: Option<u64>,
    filesystem_file_read_ops: Option<u64>,
    filesystem_file_bytes_read: Option<u64>,
    filesystem_open_file_handles: Option<u64>,
    filesystem_last_resource_name: Option<String>,
    filesystem_last_resource_path: Option<String>,
    filesystem_last_file_path: Option<String>,
    filesystem_last_file_mode: Option<String>,
    heap_base: Option<u64>,
    heap_end: Option<u64>,
    heap_cursor: Option<u64>,
    heap_allocations_total: Option<u64>,
    heap_allocations_active: Option<u64>,
    heap_frees: Option<u64>,
    heap_reallocs: Option<u64>,
    heap_bytes_active: Option<u64>,
    heap_bytes_peak: Option<u64>,
    heap_bytes_reserved: Option<u64>,
    heap_last_alloc_ptr: Option<u64>,
    heap_last_alloc_size: Option<u64>,
    heap_last_freed_ptr: Option<u64>,
    heap_last_realloc_old_ptr: Option<u64>,
    heap_last_realloc_new_ptr: Option<u64>,
    heap_last_realloc_size: Option<u64>,
    heap_last_error: Option<String>,
    vfp_multi_ops: Option<u64>,
    vfp_load_multi_ops: Option<u64>,
    vfp_store_multi_ops: Option<u64>,
    vfp_pc_base_ops: Option<u64>,
    vfp_pc_base_load_ops: Option<u64>,
    vfp_pc_base_store_ops: Option<u64>,
    vfp_single_reg_capacity: Option<u64>,
    vfp_single_range_ops: Option<u64>,
    vfp_exact_opcode_hits: Option<u64>,
    vfp_exact_override_hits: Option<u64>,
    vfp_single_transfer_ops: Option<u64>,
    vfp_double_transfer_ops: Option<u64>,
    vfp_last_start_addr: Option<u64>,
    vfp_last_end_addr: Option<u64>,
    vfp_last_pc_base_addr: Option<u64>,
    vfp_last_pc_base_word: Option<u64>,
    vfp_last_op: Option<String>,
    vfp_last_single_range: Option<String>,
    vfp_last_exact_opcode: Option<String>,
    vfp_last_exact_decoder_branch: Option<String>,
    vfp_last_transfer_mode: Option<String>,
    vfp_last_transfer_start_reg: Option<u64>,
    vfp_last_transfer_end_reg: Option<u64>,
    vfp_last_transfer_count: Option<u64>,
    vfp_last_transfer_precision: Option<String>,
    vfp_last_transfer_addr: Option<u64>,
    vfp_last_exact_reason: Option<String>,
    arm_reg_shift_operand2_ops: Option<u64>,
    arm_extra_load_store_ops: Option<u64>,
    arm_extra_load_store_loads: Option<u64>,
    arm_extra_load_store_stores: Option<u64>,
    arm_last_reg_shift: Option<String>,
    arm_last_extra_load_store: Option<String>,
    arm_exact_epilogue_site_hits: Option<u64>,
    arm_exact_epilogue_repairs: Option<u64>,
    arm_exact_epilogue_last_pc: Option<u64>,
    arm_exact_epilogue_last_before_sp: Option<u64>,
    arm_exact_epilogue_last_after_sp: Option<u64>,
    arm_exact_epilogue_last_r0: Option<u64>,
    arm_exact_epilogue_last_r7: Option<u64>,
    arm_exact_epilogue_last_r8: Option<u64>,
    arm_exact_epilogue_last_lr: Option<u64>,
    arm_exact_epilogue_last_repair: Option<String>,
    objc_bridge_metadata_available: Option<bool>,
    objc_bridge_classlist_present: Option<bool>,
    objc_bridge_cfstring_present: Option<bool>,
    objc_bridge_parsed_classes: Option<u64>,
    objc_bridge_delegate_name: Option<String>,
    objc_bridge_delegate_class_name: Option<String>,
    objc_bridge_inferred_class_name: Option<String>,
    objc_bridge_inferred_selector_hits: Option<u64>,
    objc_bridge_launch_selector: Option<String>,
    objc_bridge_launch_imp: Option<u64>,
    objc_bridge_attempted: Option<bool>,
    objc_bridge_succeeded: Option<bool>,
    objc_bridge_failure_reason: Option<String>,
    objc_real_msgsend_dispatches: Option<u64>,
    objc_last_real_selector: Option<String>,
    objc_super_msgsend_dispatches: Option<u64>,
    objc_super_msgsend_fallback_returns: Option<u64>,
    objc_last_super_selector: Option<String>,
    objc_last_super_receiver: Option<u64>,
    objc_last_super_class: Option<u64>,
    objc_last_super_imp: Option<u64>,
    objc_alloc_calls: Option<u64>,
    objc_alloc_with_zone_calls: Option<u64>,
    objc_class_create_instance_calls: Option<u64>,
    objc_init_calls: Option<u64>,
    objc_instances_materialized: Option<u64>,
    objc_last_alloc_class: Option<String>,
    objc_last_alloc_receiver: Option<u64>,
    objc_last_alloc_result: Option<u64>,
    objc_last_init_receiver: Option<u64>,
    objc_last_init_result: Option<u64>,
    hot_objc_msgsend_calls: Option<u64>,
    hot_objc_unique_selectors: Option<u64>,
    hot_recent_objc_selectors: Vec<String>,
    hot_top_objc_selectors: Value,
    hot_saw_draw_rect: Option<bool>,
    hot_saw_set_needs_display: Option<bool>,
    hot_saw_layout_subviews: Option<bool>,
    hot_saw_image_named: Option<bool>,
    hot_saw_present_renderbuffer: Option<bool>,
    hot_gl_calls_seen: Option<u64>,
    hot_recent_gl_calls: Vec<String>,
    hot_top_gl_calls: Value,
    hot_saw_gl_bind_texture: Option<bool>,
    hot_saw_gl_teximage2d: Option<bool>,
    hot_saw_gl_draw_arrays: Option<bool>,
    hot_saw_gl_draw_elements: Option<bool>,
}

fn collect_runtime_summary(value: &Value) -> RuntimeAudit {
    if let Some(runtime_state) = path_get(value, &["runtime", "runtime_state"]) {
        let mut audit = RuntimeAudit {
            source: Some("runtime_state".to_string()),
            conn_state: str_at(runtime_state, &["network", "state"]).map(ToOwned::to_owned),
            conn_state_code: u64_at(runtime_state, &["network", "state_code"]),
            fault_events: u64_at(runtime_state, &["network", "fault_events"]),
            fault_modes: array_at(runtime_state, &["network", "fault_modes"]).iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect(),
            last_error_domain: str_at(runtime_state, &["network", "last_error_domain"]).map(ToOwned::to_owned),
            last_error_code: i64_at(runtime_state, &["network", "last_error_code"]),
            last_error_kind: str_at(runtime_state, &["network", "last_error_kind"]).map(ToOwned::to_owned),
            last_error_description: str_at(runtime_state, &["network", "last_error_description"]).map(ToOwned::to_owned),
            retained_response: bool_at(runtime_state, &["network", "retained_response"]),
            retained_data: bool_at(runtime_state, &["network", "retained_data"]),
            retained_error: bool_at(runtime_state, &["network", "retained_error"]),
            runloop_ticks: u64_at(runtime_state, &["runloop", "ticks"]),
            runloop_sources: u64_at(runtime_state, &["runloop", "sources"]),
            idle_after_completion: u64_at(runtime_state, &["runloop", "idle_ticks_after_completion"]),
            input_consumed: u64_at(runtime_state, &["input", "consumed"]),
            input_ignored: u64_at(runtime_state, &["input", "ignored"]),
            input_pointer_down: bool_at(runtime_state, &["input", "pointer_down"]),
            input_last_target: u64_at(runtime_state, &["input", "last_target"]),
            input_last_phase: str_at(runtime_state, &["input", "last_phase"]).map(ToOwned::to_owned),
            input_last_dispatch: str_at(runtime_state, &["input", "last_dispatch"]).map(ToOwned::to_owned),
            input_cocos_attempts: u64_at(runtime_state, &["input", "cocos_attempts"]),
            input_cocos_dispatched: u64_at(runtime_state, &["input", "cocos_dispatched"]),
            net_events: u64_at(runtime_state, &["network", "events"]),
            delegate_callbacks: u64_at(runtime_state, &["network", "delegate_callbacks"]),
            retry_recommended: bool_at(runtime_state, &["network", "retry_recommended"]),
            synthetic_network_fault_probes: bool_at(runtime_state, &["synthetic", "network_fault_probes"]),
            synthetic_runloop_tick_budget: u64_at(runtime_state, &["synthetic", "runloop_tick_budget"]),
            ui_cocos_director: u64_at(runtime_state, &["ui", "cocos_director"]),
            ui_opengl_view: u64_at(runtime_state, &["ui", "opengl_view"]),
            ui_running_scene: u64_at(runtime_state, &["ui", "running_scene"]),
            ui_director_type: u64_at(runtime_state, &["ui", "director_type"]),
            ui_animation_running: bool_at(runtime_state, &["ui", "animation_running"]),
            ui_display_fps_enabled: bool_at(runtime_state, &["ui", "display_fps_enabled"]),
            network_stage: u64_at(runtime_state, &["network", "stage"]),
            network_bytes_delivered: u64_at(runtime_state, &["network", "bytes_delivered"]),
            network_payload_len: u64_at(runtime_state, &["network", "payload_len"]),
            network_source_closed: bool_at(runtime_state, &["network", "source_closed"]),
            foundation_string_backing: bool_at(runtime_state, &["network", "foundation_string_backing"]),
            foundation_data_backing: bool_at(runtime_state, &["network", "foundation_data_backing"]),
            data_bytes_ptr: u64_at(runtime_state, &["network", "data_bytes_ptr"]),
            payload_preview_ascii: str_at(runtime_state, &["network", "payload_preview_ascii"]).map(ToOwned::to_owned),
            reachability_scheduled: bool_at(runtime_state, &["reachability", "scheduled"]),
            reachability_callback_set: bool_at(runtime_state, &["reachability", "callback_set"]),
            reachability_flags: u64_at(runtime_state, &["reachability", "flags"]),
            read_stream_state: str_at(runtime_state, &["streams", "read_status"]).map(ToOwned::to_owned),
            read_stream_state_code: u64_at(runtime_state, &["streams", "read_status_code"]),
            read_stream_open: bool_at(runtime_state, &["streams", "read_open"]),
            read_stream_scheduled: bool_at(runtime_state, &["streams", "read_scheduled"]),
            read_stream_bytes_consumed: u64_at(runtime_state, &["streams", "read_bytes_consumed"]),
            read_stream_has_bytes_available: bool_at(runtime_state, &["streams", "read_has_bytes_available"]),
            write_stream_state: str_at(runtime_state, &["streams", "write_status"]).map(ToOwned::to_owned),
            write_stream_state_code: u64_at(runtime_state, &["streams", "write_status_code"]),
            write_stream_open: bool_at(runtime_state, &["streams", "write_open"]),
            write_stream_scheduled: bool_at(runtime_state, &["streams", "write_scheduled"]),
            write_stream_bytes_written: u64_at(runtime_state, &["streams", "write_bytes_written"]),
            write_stream_can_accept_bytes: bool_at(runtime_state, &["streams", "write_can_accept_bytes"]),
            graphics_context_current: bool_at(runtime_state, &["graphics", "context_current"]),
            graphics_layer_attached: bool_at(runtime_state, &["graphics", "layer_attached"]),
            graphics_surface_ready: bool_at(runtime_state, &["graphics", "surface_ready"]),
            graphics_framebuffer_complete: bool_at(runtime_state, &["graphics", "framebuffer_complete"]),
            graphics_viewport_ready: bool_at(runtime_state, &["graphics", "viewport_ready"]),
            graphics_presented: bool_at(runtime_state, &["graphics", "presented"]),
            graphics_readback_ready: bool_at(runtime_state, &["graphics", "readback_ready"]),
            graphics_frame_index: u64_at(runtime_state, &["graphics", "frame_index"]),
            graphics_present_calls: u64_at(runtime_state, &["graphics", "present_calls"]),
            graphics_draw_calls: u64_at(runtime_state, &["graphics", "draw_calls"]),
            graphics_clear_calls: u64_at(runtime_state, &["graphics", "clear_calls"]),
            graphics_readback_calls: u64_at(runtime_state, &["graphics", "readback_calls"]),
            graphics_gl_calls: u64_at(runtime_state, &["graphics", "gl_calls"]),
            graphics_surface_width: u64_at(runtime_state, &["graphics", "surface_width"]),
            graphics_surface_height: u64_at(runtime_state, &["graphics", "surface_height"]),
            graphics_viewport_width: u64_at(runtime_state, &["graphics", "viewport_width"]),
            graphics_viewport_height: u64_at(runtime_state, &["graphics", "viewport_height"]),
            graphics_framebuffer_bytes: u64_at(runtime_state, &["graphics", "framebuffer_bytes"]),
            graphics_last_readback_bytes: u64_at(runtime_state, &["graphics", "last_readback_bytes"]),
            graphics_last_readback_checksum: u64_at(runtime_state, &["graphics", "last_readback_checksum"]),
            graphics_last_readback_origin: str_at(runtime_state, &["graphics", "last_readback_origin"]).map(ToOwned::to_owned),
            graphics_last_present_source: str_at(runtime_state, &["graphics", "last_present_source"]).map(ToOwned::to_owned),
            graphics_last_present_decision: str_at(runtime_state, &["graphics", "last_present_decision"]).map(ToOwned::to_owned),
            graphics_retained_present_calls: u64_at(runtime_state, &["graphics", "retained_present_calls"]),
            graphics_synthetic_fallback_present_calls: u64_at(runtime_state, &["graphics", "synthetic_fallback_present_calls"]),
            graphics_auto_scene_present_calls: u64_at(runtime_state, &["graphics", "auto_scene_present_calls"]),
            graphics_guest_draw_calls: u64_at(runtime_state, &["graphics", "guest_draw_calls"]),
            graphics_guest_vertex_fetches: u64_at(runtime_state, &["graphics", "guest_vertex_fetches"]),
            graphics_last_draw_mode: u64_at(runtime_state, &["graphics", "last_draw_mode"]),
            graphics_last_draw_mode_label: str_at(runtime_state, &["graphics", "last_draw_mode_label"]).map(ToOwned::to_owned),
            graphics_last_guest_draw_checksum: u64_at(runtime_state, &["graphics", "last_guest_draw_checksum"]),
            graphics_uikit_context_current: bool_at(runtime_state, &["graphics", "uikit_context_current"]),
            graphics_uikit_contexts_created: u64_at(runtime_state, &["graphics", "uikit_contexts_created"]),
            graphics_uikit_images_created: u64_at(runtime_state, &["graphics", "uikit_images_created"]),
            graphics_uikit_draw_ops: u64_at(runtime_state, &["graphics", "uikit_draw_ops"]),
            graphics_uikit_present_ops: u64_at(runtime_state, &["graphics", "uikit_present_ops"]),
            graphics_last_ui_source: str_at(runtime_state, &["graphics", "last_ui_source"]).map(ToOwned::to_owned),
            graphics_dump_frames_enabled: bool_at(runtime_state, &["graphics", "dump_frames_enabled"]),
            graphics_dump_every: u64_at(runtime_state, &["graphics", "dump_every"]),
            graphics_dump_limit: u64_at(runtime_state, &["graphics", "dump_limit"]),
            graphics_dumps_saved: u64_at(runtime_state, &["graphics", "dumps_saved"]),
            graphics_last_dump_path: str_at(runtime_state, &["graphics", "last_dump_path"]).map(ToOwned::to_owned),
            filesystem_bundle_available: bool_at(runtime_state, &["filesystem", "bundle_available"]),
            filesystem_bundle_root: str_at(runtime_state, &["filesystem", "bundle_root"]).map(ToOwned::to_owned),
            filesystem_indexed_files: u64_at(runtime_state, &["filesystem", "indexed_files"]),
            filesystem_cached_images: u64_at(runtime_state, &["filesystem", "cached_images"]),
            filesystem_bundle_objects_created: u64_at(runtime_state, &["filesystem", "bundle_objects_created"]),
            filesystem_bundle_scoped_hits: u64_at(runtime_state, &["filesystem", "bundle_scoped_hits"]),
            filesystem_bundle_scoped_misses: u64_at(runtime_state, &["filesystem", "bundle_scoped_misses"]),
            filesystem_png_cgbi_detected: u64_at(runtime_state, &["filesystem", "png_cgbi_detected"]),
            filesystem_png_cgbi_decoded: u64_at(runtime_state, &["filesystem", "png_cgbi_decoded"]),
            filesystem_png_decode_failures: u64_at(runtime_state, &["filesystem", "png_decode_failures"]),
            filesystem_image_named_hits: u64_at(runtime_state, &["filesystem", "image_named_hits"]),
            filesystem_image_named_misses: u64_at(runtime_state, &["filesystem", "image_named_misses"]),
            filesystem_file_open_hits: u64_at(runtime_state, &["filesystem", "file_open_hits"]),
            filesystem_file_open_misses: u64_at(runtime_state, &["filesystem", "file_open_misses"]),
            filesystem_file_read_ops: u64_at(runtime_state, &["filesystem", "file_read_ops"]),
            filesystem_file_bytes_read: u64_at(runtime_state, &["filesystem", "file_bytes_read"]),
            filesystem_open_file_handles: u64_at(runtime_state, &["filesystem", "open_file_handles"]),
            filesystem_last_resource_name: str_at(runtime_state, &["filesystem", "last_resource_name"]).map(ToOwned::to_owned),
            filesystem_last_resource_path: str_at(runtime_state, &["filesystem", "last_resource_path"]).map(ToOwned::to_owned),
            filesystem_last_file_path: str_at(runtime_state, &["filesystem", "last_file_path"]).map(ToOwned::to_owned),
            filesystem_last_file_mode: str_at(runtime_state, &["filesystem", "last_file_mode"]).map(ToOwned::to_owned),
            heap_base: u64_at(runtime_state, &["heap", "base"]),
            heap_end: u64_at(runtime_state, &["heap", "end"]),
            heap_cursor: u64_at(runtime_state, &["heap", "cursor"]),
            heap_allocations_total: u64_at(runtime_state, &["heap", "allocations_total"]),
            heap_allocations_active: u64_at(runtime_state, &["heap", "allocations_active"]),
            heap_frees: u64_at(runtime_state, &["heap", "frees"]),
            heap_reallocs: u64_at(runtime_state, &["heap", "reallocs"]),
            heap_bytes_active: u64_at(runtime_state, &["heap", "bytes_active"]),
            heap_bytes_peak: u64_at(runtime_state, &["heap", "bytes_peak"]),
            heap_bytes_reserved: u64_at(runtime_state, &["heap", "bytes_reserved"]),
            heap_last_alloc_ptr: u64_at(runtime_state, &["heap", "last_alloc_ptr"]),
            heap_last_alloc_size: u64_at(runtime_state, &["heap", "last_alloc_size"]),
            heap_last_freed_ptr: u64_at(runtime_state, &["heap", "last_freed_ptr"]),
            heap_last_realloc_old_ptr: u64_at(runtime_state, &["heap", "last_realloc_old_ptr"]),
            heap_last_realloc_new_ptr: u64_at(runtime_state, &["heap", "last_realloc_new_ptr"]),
            heap_last_realloc_size: u64_at(runtime_state, &["heap", "last_realloc_size"]),
            heap_last_error: str_at(runtime_state, &["heap", "last_error"]).map(ToOwned::to_owned),
            vfp_multi_ops: u64_at(runtime_state, &["vfp", "multi_ops"]),
            vfp_load_multi_ops: u64_at(runtime_state, &["vfp", "load_multi_ops"]),
            vfp_store_multi_ops: u64_at(runtime_state, &["vfp", "store_multi_ops"]),
            vfp_pc_base_ops: u64_at(runtime_state, &["vfp", "pc_base_ops"]),
            vfp_pc_base_load_ops: u64_at(runtime_state, &["vfp", "pc_base_load_ops"]),
            vfp_pc_base_store_ops: u64_at(runtime_state, &["vfp", "pc_base_store_ops"]),
            vfp_single_reg_capacity: u64_at(runtime_state, &["vfp", "single_reg_capacity"]),
            vfp_single_range_ops: u64_at(runtime_state, &["vfp", "single_range_ops"]),
            vfp_exact_opcode_hits: u64_at(runtime_state, &["vfp", "exact_opcode_hits"]),
            vfp_exact_override_hits: u64_at(runtime_state, &["vfp", "exact_override_hits"]),
            vfp_single_transfer_ops: u64_at(runtime_state, &["vfp", "single_transfer_ops"]),
            vfp_double_transfer_ops: u64_at(runtime_state, &["vfp", "double_transfer_ops"]),
            vfp_last_start_addr: u64_at(runtime_state, &["vfp", "last_start_addr"]),
            vfp_last_end_addr: u64_at(runtime_state, &["vfp", "last_end_addr"]),
            vfp_last_pc_base_addr: u64_at(runtime_state, &["vfp", "last_pc_base_addr"]),
            vfp_last_pc_base_word: u64_at(runtime_state, &["vfp", "last_pc_base_word"]),
            vfp_last_op: str_at(runtime_state, &["vfp", "last_op"]).map(ToOwned::to_owned),
            vfp_last_single_range: str_at(runtime_state, &["vfp", "last_single_range"]).map(ToOwned::to_owned),
            vfp_last_exact_opcode: str_at(runtime_state, &["vfp", "last_exact_opcode"]).map(ToOwned::to_owned),
            vfp_last_exact_decoder_branch: str_at(runtime_state, &["vfp", "last_exact_decoder_branch"]).map(ToOwned::to_owned),
            vfp_last_transfer_mode: str_at(runtime_state, &["vfp", "last_transfer_mode"]).map(ToOwned::to_owned),
            vfp_last_transfer_start_reg: u64_at(runtime_state, &["vfp", "last_transfer_start_reg"]),
            vfp_last_transfer_end_reg: u64_at(runtime_state, &["vfp", "last_transfer_end_reg"]),
            vfp_last_transfer_count: u64_at(runtime_state, &["vfp", "last_transfer_count"]),
            vfp_last_transfer_precision: str_at(runtime_state, &["vfp", "last_transfer_precision"]).map(ToOwned::to_owned),
            vfp_last_transfer_addr: u64_at(runtime_state, &["vfp", "last_transfer_addr"]),
            vfp_last_exact_reason: str_at(runtime_state, &["vfp", "last_exact_reason"]).map(ToOwned::to_owned),
            arm_reg_shift_operand2_ops: u64_at(runtime_state, &["arm", "reg_shift_operand2_ops"]),
            arm_extra_load_store_ops: u64_at(runtime_state, &["arm", "extra_load_store_ops"]),
            arm_extra_load_store_loads: u64_at(runtime_state, &["arm", "extra_load_store_loads"]),
            arm_extra_load_store_stores: u64_at(runtime_state, &["arm", "extra_load_store_stores"]),
            arm_last_reg_shift: str_at(runtime_state, &["arm", "last_reg_shift"]).map(ToOwned::to_owned),
            arm_last_extra_load_store: str_at(runtime_state, &["arm", "last_extra_load_store"]).map(ToOwned::to_owned),
            arm_exact_epilogue_site_hits: u64_at(runtime_state, &["arm", "exact_epilogue_site_hits"]),
            arm_exact_epilogue_repairs: u64_at(runtime_state, &["arm", "exact_epilogue_repairs"]),
            arm_exact_epilogue_last_pc: u64_at(runtime_state, &["arm", "exact_epilogue_last_pc"]),
            arm_exact_epilogue_last_before_sp: u64_at(runtime_state, &["arm", "exact_epilogue_last_before_sp"]),
            arm_exact_epilogue_last_after_sp: u64_at(runtime_state, &["arm", "exact_epilogue_last_after_sp"]),
            arm_exact_epilogue_last_r0: u64_at(runtime_state, &["arm", "exact_epilogue_last_r0"]),
            arm_exact_epilogue_last_r7: u64_at(runtime_state, &["arm", "exact_epilogue_last_r7"]),
            arm_exact_epilogue_last_r8: u64_at(runtime_state, &["arm", "exact_epilogue_last_r8"]),
            arm_exact_epilogue_last_lr: u64_at(runtime_state, &["arm", "exact_epilogue_last_lr"]),
            arm_exact_epilogue_last_repair: str_at(runtime_state, &["arm", "exact_epilogue_last_repair"]).map(ToOwned::to_owned),
            objc_bridge_metadata_available: bool_at(runtime_state, &["objc_bridge", "metadata_available"]),
            objc_bridge_classlist_present: bool_at(runtime_state, &["objc_bridge", "classlist_present"]),
            objc_bridge_cfstring_present: bool_at(runtime_state, &["objc_bridge", "cfstring_present"]),
            objc_bridge_parsed_classes: u64_at(runtime_state, &["objc_bridge", "parsed_classes"]),
            objc_bridge_delegate_name: str_at(runtime_state, &["objc_bridge", "delegate_name"]).map(ToOwned::to_owned),
            objc_bridge_delegate_class_name: str_at(runtime_state, &["objc_bridge", "delegate_class_name"]).map(ToOwned::to_owned),
            objc_bridge_inferred_class_name: str_at(runtime_state, &["objc_bridge", "inferred_class_name"]).map(ToOwned::to_owned),
            objc_bridge_inferred_selector_hits: u64_at(runtime_state, &["objc_bridge", "inferred_selector_hits"]),
            objc_bridge_launch_selector: str_at(runtime_state, &["objc_bridge", "launch_selector"]).map(ToOwned::to_owned),
            objc_bridge_launch_imp: u64_at(runtime_state, &["objc_bridge", "launch_imp"]),
            objc_bridge_attempted: bool_at(runtime_state, &["objc_bridge", "bridge_attempted"]),
            objc_bridge_succeeded: bool_at(runtime_state, &["objc_bridge", "bridge_succeeded"]),
            objc_bridge_failure_reason: str_at(runtime_state, &["objc_bridge", "failure_reason"]).map(ToOwned::to_owned),
            objc_real_msgsend_dispatches: u64_at(runtime_state, &["objc_bridge", "real_msgsend_dispatches"]),
            objc_last_real_selector: str_at(runtime_state, &["objc_bridge", "last_real_selector"]).map(ToOwned::to_owned),
            objc_super_msgsend_dispatches: u64_at(runtime_state, &["objc_bridge", "super_msgsend_dispatches"]),
            objc_super_msgsend_fallback_returns: u64_at(runtime_state, &["objc_bridge", "super_msgsend_fallback_returns"]),
            objc_last_super_selector: str_at(runtime_state, &["objc_bridge", "last_super_selector"]).map(ToOwned::to_owned),
            objc_last_super_receiver: u64_at(runtime_state, &["objc_bridge", "last_super_receiver"]),
            objc_last_super_class: u64_at(runtime_state, &["objc_bridge", "last_super_class"]),
            objc_last_super_imp: u64_at(runtime_state, &["objc_bridge", "last_super_imp"]),
            objc_alloc_calls: u64_at(runtime_state, &["objc_bridge", "alloc_calls"]),
            objc_alloc_with_zone_calls: u64_at(runtime_state, &["objc_bridge", "alloc_with_zone_calls"]),
            objc_class_create_instance_calls: u64_at(runtime_state, &["objc_bridge", "class_create_instance_calls"]),
            objc_init_calls: u64_at(runtime_state, &["objc_bridge", "init_calls"]),
            objc_instances_materialized: u64_at(runtime_state, &["objc_bridge", "instances_materialized"]),
            objc_last_alloc_class: str_at(runtime_state, &["objc_bridge", "last_alloc_class"]).map(ToOwned::to_owned),
            objc_last_alloc_receiver: u64_at(runtime_state, &["objc_bridge", "last_alloc_receiver"]),
            objc_last_alloc_result: u64_at(runtime_state, &["objc_bridge", "last_alloc_result"]),
            objc_last_init_receiver: u64_at(runtime_state, &["objc_bridge", "last_init_receiver"]),
            objc_last_init_result: u64_at(runtime_state, &["objc_bridge", "last_init_result"]),
            hot_objc_msgsend_calls: u64_at(runtime_state, &["hot_path", "objc_msgsend_calls"]),
            hot_objc_unique_selectors: u64_at(runtime_state, &["hot_path", "objc_unique_selectors"]),
            hot_recent_objc_selectors: array_at(runtime_state, &["hot_path", "recent_objc_selectors"]).iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect(),
            hot_top_objc_selectors: path_get(runtime_state, &["hot_path", "top_objc_selectors"]).cloned().unwrap_or_else(|| Value::Array(Vec::new())),
            hot_saw_draw_rect: bool_at(runtime_state, &["hot_path", "saw_draw_rect"]),
            hot_saw_set_needs_display: bool_at(runtime_state, &["hot_path", "saw_set_needs_display"]),
            hot_saw_layout_subviews: bool_at(runtime_state, &["hot_path", "saw_layout_subviews"]),
            hot_saw_image_named: bool_at(runtime_state, &["hot_path", "saw_image_named"]),
            hot_saw_present_renderbuffer: bool_at(runtime_state, &["hot_path", "saw_present_renderbuffer"]),
            hot_gl_calls_seen: u64_at(runtime_state, &["hot_path", "gl_calls_seen"]),
            hot_recent_gl_calls: array_at(runtime_state, &["hot_path", "recent_gl_calls"]).iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect(),
            hot_top_gl_calls: path_get(runtime_state, &["hot_path", "top_gl_calls"]).cloned().unwrap_or_else(|| Value::Array(Vec::new())),
            hot_saw_gl_bind_texture: bool_at(runtime_state, &["hot_path", "saw_gl_bind_texture"]),
            hot_saw_gl_teximage2d: bool_at(runtime_state, &["hot_path", "saw_gl_teximage2d"]),
            hot_saw_gl_draw_arrays: bool_at(runtime_state, &["hot_path", "saw_gl_draw_arrays"]),
            hot_saw_gl_draw_elements: bool_at(runtime_state, &["hot_path", "saw_gl_draw_elements"]),
        };
        if audit.fault_modes.is_empty() {
            audit.fault_modes = Vec::new();
        }
        return audit;
    }

    let Some(text) = str_at(value, &["runtime", "stop_reason"]).or_else(|| str_at(value, &["runtime", "status"])) else {
        return RuntimeAudit::default();
    };

    let mut audit = RuntimeAudit {
        source: Some("stop_reason".to_string()),
        conn_state: extract_token_after(text, "conn_state=").map(ToOwned::to_owned),
        conn_state_code: extract_token_after(text, "conn_state=").map(conn_state_code),
        fault_events: extract_u64_after(text, "fault_events="),
        fault_modes: extract_list_after(text, "fault_modes=[", ']'),
        runloop_ticks: extract_u64_after(text, "ticks="),
        runloop_sources: extract_u64_after(text, "sources="),
        idle_after_completion: extract_u64_after(text, "idle_after_completion="),
        net_events: extract_u64_after(text, "net_events="),
        delegate_callbacks: extract_u64_after(text, "delegate_callbacks="),
        retry_recommended: extract_yes_no_after(text, "retry="),
        graphics_surface_ready: extract_yes_no_after(text, "surfaceReady="),
        graphics_presented: extract_yes_no_after(text, "presented="),
        graphics_frame_index: extract_u64_after(text, "frames="),
        graphics_present_calls: extract_u64_after(text, "presents="),
        graphics_readback_ready: extract_yes_no_after(text, "readback="),
        ..RuntimeAudit::default()
    };

    if let Some((resp, data)) = extract_retained_response_data(text) {
        audit.retained_response = Some(resp);
        audit.retained_data = Some(data);
    }

    if let Some(last_error) = extract_token_after(text, "last_error=") {
        if !last_error.eq_ignore_ascii_case("none") {
            let mut parts = last_error.splitn(2, ' ');
            audit.last_error_code = parts.next().and_then(|n| n.parse::<i64>().ok());
            audit.last_error_kind = parts.next().map(ToOwned::to_owned);
        }
    }

    audit
}

fn looks_bad_stop(stop: &str) -> bool {
    let s = stop.to_ascii_lowercase();
    ["unmapped", "backend error", "unsupported", "fault", "panic", "invalid", "not-present"]
        .iter()
        .any(|needle| s.contains(needle))
}

fn cmp_opt_u64(
    base: &Value,
    cand: &Value,
    key: &str,
    label: &str,
    pref: OrderingPref,
    regressions: &mut Vec<String>,
    improvements: &mut Vec<String>,
) {
    let before = base.get(key).and_then(Value::as_u64);
    let after = cand.get(key).and_then(Value::as_u64);
    if let (Some(before), Some(after)) = (before, after) {
        match pref {
            OrderingPref::HigherBetter => {
                if after < before {
                    regressions.push(format!("{label} dropped: {before} -> {after}"));
                } else if after > before {
                    improvements.push(format!("{label} increased: {before} -> {after}"));
                }
            }
            OrderingPref::LowerBetter => {
                if after > before {
                    regressions.push(format!("{label} increased: {before} -> {after}"));
                } else if after < before {
                    improvements.push(format!("{label} decreased: {before} -> {after}"));
                }
            }
        }
    }
}

fn cmp_bool(
    base: &Value,
    cand: &Value,
    key: &str,
    label: &str,
    regressions: &mut Vec<String>,
    improvements: &mut Vec<String>,
) {
    let before = base.get(key).and_then(Value::as_bool);
    let after = cand.get(key).and_then(Value::as_bool);
    match (before, after) {
        (Some(false), Some(true)) => improvements.push(format!("{label} now retained/enabled")),
        (Some(true), Some(false)) => regressions.push(format!("{label} is no longer retained/enabled")),
        _ => {}
    }
}

fn cmp_opt_str(
    base: &Value,
    cand: &Value,
    key: &str,
    label: &str,
    regressions: &mut Vec<String>,
    improvements: &mut Vec<String>,
) {
    let before = base.get(key).and_then(Value::as_str);
    let after = cand.get(key).and_then(Value::as_str);
    match (before, after) {
        (Some(before), Some(after)) if before != after => improvements.push(format!("{label} changed: {before} -> {after}")),
        (None, Some(after)) => improvements.push(format!("{label} now set: {after}")),
        (Some(before), None) => regressions.push(format!("{label} cleared (was {before})")),
        _ => {}
    }
}

fn cmp_conn_state(base: &Value, cand: &Value, regressions: &mut Vec<String>, improvements: &mut Vec<String>) {
    let before = base.get("conn_state").and_then(Value::as_str);
    let after = cand.get("conn_state").and_then(Value::as_str);
    match (before, after) {
        (Some(before), Some(after)) if before != after => {
            let br = conn_state_rank(before);
            let ar = conn_state_rank(after);
            if ar > br {
                improvements.push(format!("connection state improved: {before} -> {after}"));
            } else if ar < br {
                regressions.push(format!("connection state regressed: {before} -> {after}"));
            }
        }
        _ => {}
    }
}

fn cmp_last_error(base: &Value, cand: &Value, regressions: &mut Vec<String>, improvements: &mut Vec<String>) {
    let before_code = base.get("last_error_code").and_then(Value::as_i64);
    let after_code = cand.get("last_error_code").and_then(Value::as_i64);
    let before_kind = base.get("last_error_kind").and_then(Value::as_str).unwrap_or_default();
    let after_kind = cand.get("last_error_kind").and_then(Value::as_str).unwrap_or_default();
    match (before_code, after_code) {
        (Some(before), None) => improvements.push(format!("last error cleared (was {before} {before_kind})")),
        (None, Some(after)) => regressions.push(format!("candidate now reports last error {after} {after_kind}")),
        (Some(before), Some(after)) if before != after || before_kind != after_kind => {
            improvements.push(format!("last error changed: {before} {before_kind} -> {after} {after_kind}"));
        }
        _ => {}
    }
}

fn conn_state_rank(state: &str) -> i32 {
    match state.to_ascii_lowercase().as_str() {
        "completed" => 4,
        "receiving" => 3,
        "scheduled" => 2,
        "idle" => 1,
        "faulted" => 0,
        "cancelled" => -1,
        _ => 0,
    }
}

fn conn_state_code(state: &str) -> u64 {
    match state.to_ascii_lowercase().as_str() {
        "scheduled" => 1,
        "receiving" => 2,
        "completed" => 3,
        "faulted" => 4,
        "cancelled" => 5,
        _ => 0,
    }
}

fn extract_u64_after(text: &str, marker: &str) -> Option<u64> {
    extract_numeric_token_after(text, marker)?.parse::<u64>().ok()
}

fn extract_token_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let idx = text.find(marker)?;
    let rest = &text[idx + marker.len()..];
    let end = rest.find(|c: char| matches!(c, ',' | ')' | ']')).unwrap_or(rest.len());
    Some(rest[..end].trim())
}

fn extract_numeric_token_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let idx = text.find(marker)?;
    let rest = &text[idx + marker.len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(rest.len());
    let token = rest[..end].trim();
    if token.is_empty() { None } else { Some(token) }
}

fn extract_yes_no_after(text: &str, marker: &str) -> Option<bool> {
    match extract_token_after(text, marker)? {
        "YES" => Some(true),
        "NO" => Some(false),
        _ => None,
    }
}

fn extract_list_after(text: &str, marker: &str, terminator: char) -> Vec<String> {
    let Some(idx) = text.find(marker) else {
        return Vec::new();
    };
    let rest = &text[idx + marker.len()..];
    let end = rest.find(terminator).unwrap_or(rest.len());
    let raw = rest[..end].trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("none") {
        Vec::new()
    } else {
        raw.split(',').map(|part| part.trim().to_string()).filter(|part| !part.is_empty()).collect()
    }
}

fn extract_retained_response_data(text: &str) -> Option<(bool, bool)> {
    let idx = text.find("retained(response=")?;
    let rest = &text[idx + "retained(response=".len()..];
    let comma = rest.find(',')?;
    let response = &rest[..comma];
    let rest = &rest[comma + 1..];
    let rest = rest.strip_prefix("data=")?;
    let end = rest.find(')')?;
    let data = &rest[..end];
    Some((matches!(response.trim(), "YES"), matches!(data.trim(), "YES")))
}

fn number_at(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64).or_else(|| value.get(key).and_then(Value::as_u64).map(|v| v as i64))
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    path_get(value, path)?.as_str()
}

fn u64_at(value: &Value, path: &[&str]) -> Option<u64> {
    let v = path_get(value, path)?;
    v.as_u64().or_else(|| v.as_i64().map(|n| n as u64))
}

fn i64_at(value: &Value, path: &[&str]) -> Option<i64> {
    let v = path_get(value, path)?;
    v.as_i64().or_else(|| v.as_u64().map(|n| n as i64))
}

fn bool_at(value: &Value, path: &[&str]) -> Option<bool> {
    path_get(value, path)?.as_bool()
}

fn array_at<'a>(value: &'a Value, path: &[&str]) -> &'a [Value] {
    path_get(value, path)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn path_get<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = value;
    for segment in path {
        cur = cur.get(*segment)?;
    }
    Some(cur)
}

fn top_symbol_buckets(values: &[Value]) -> Vec<Value> {
    top_string_counts(values.iter().filter_map(Value::as_str).map(symbol_bucket))
}

fn top_string_counts<I>(items: I) -> Vec<Value>
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for item in items {
        *counts.entry(item.into()).or_default() += 1;
    }
    let mut pairs: Vec<_> = counts.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    pairs
        .into_iter()
        .take(12)
        .map(|(name, count)| json!({"name": name, "count": count}))
        .collect()
}

fn symbol_bucket(symbol: &str) -> String {
    let s = symbol.trim_start_matches('_');
    for marker in ["OBJC_CLASS_$_", "OBJC_METACLASS_$_", "OBJC_IVAR_$_"] {
        if let Some(rest) = s.strip_prefix(marker) {
            return format!("objc:{}", objc_bucket(rest));
        }
    }
    if s.starts_with("CF") || s.starts_with("kCF") {
        return "cf".to_string();
    }
    if s.starts_with("CG") {
        return "coregraphics".to_string();
    }
    if s.starts_with("Audio") || s.starts_with("kAudio") || s.starts_with("AL") {
        return "audio".to_string();
    }
    if s.starts_with("gl") || s.starts_with("egl") {
        return "gles".to_string();
    }
    if s.starts_with("sqlite3") {
        return "sqlite".to_string();
    }
    if s.starts_with("Sec") || s.starts_with("kSec") {
        return "security".to_string();
    }
    if s.starts_with("UI") {
        return "uikit".to_string();
    }
    if s.starts_with("NS") {
        return "foundation".to_string();
    }
    s.split(|c: char| c == '_' || c.is_ascii_digit())
        .next()
        .unwrap_or("misc")
        .to_ascii_lowercase()
}

fn objc_bucket(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_uppercase() && !out.is_empty() {
            break;
        }
        out.push(ch.to_ascii_lowercase());
    }
    if out.is_empty() {
        "objc-misc".to_string()
    } else {
        out
    }
}

fn dylib_bucket(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.contains("foundation") {
        "Foundation".to_string()
    } else if lower.contains("uikit") {
        "UIKit".to_string()
    } else if lower.contains("corefoundation") {
        "CoreFoundation".to_string()
    } else if lower.contains("coregraphics") || lower.contains("graphicsservices") {
        "CoreGraphics".to_string()
    } else if lower.contains("openal") || lower.contains("audio") {
        "Audio".to_string()
    } else if lower.contains("opengles") {
        "OpenGLES".to_string()
    } else if lower.contains("security") {
        "Security".to_string()
    } else if lower.contains("sqlite") {
        "SQLite".to_string()
    } else {
        name.to_string()
    }
}

fn insert_opt_str(out: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        out.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn insert_opt_string(out: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        out.insert(key.to_string(), Value::String(value));
    }
}

fn insert_opt_u64(out: &mut Map<String, Value>, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        out.insert(key.to_string(), json!(value));
    }
}

fn insert_opt_i64(out: &mut Map<String, Value>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        out.insert(key.to_string(), json!(value));
    }
}

fn insert_opt_bool(out: &mut Map<String, Value>, key: &str, value: Option<bool>) {
    if let Some(value) = value {
        out.insert(key.to_string(), Value::Bool(value));
    }
}
