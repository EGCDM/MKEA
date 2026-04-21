use std::collections::HashMap;
use std::path::Path;

use super::{
    DirectorIvarLayout, MenuEntry, MenuStackPlan, SyntheticImageChannelTransform,
    SyntheticNetworkProfile, TitleProfile,
};

const WATCHED_SPRITES: [&str; 3] = ["menu_buttons.png", "loading_bar.png", "menu_notepad.png"];

#[derive(Debug)]
pub(crate) struct AboveAndBelowProfile;

pub(crate) static PROFILE: AboveAndBelowProfile = AboveAndBelowProfile;

pub(crate) fn director_ivar_layout(bundle_root: Option<&Path>) -> Option<DirectorIvarLayout> {
    if !is_title(bundle_root) {
        return None;
    }
    Some(DirectorIvarLayout {
        open_gl_view_offset: 4,
        running_scene_offset: 68,
        next_scene_offset: 72,
        effect_scene_offset: None,
    })
}

pub(crate) fn texture_rect_origin_preference(texture_key: &str) -> Option<bool> {
    let key = texture_key.to_ascii_lowercase();
    if key.contains("loading_bar.png") {
        return Some(false);
    }
    if key.contains("menu_buttons.png") {
        return Some(true);
    }
    None
}

pub(crate) fn bundle_image_channel_transform(
    _texture_key: &str,
    _path: &Path,
    _saw_cgbi: bool,
    _used_cgbi: bool,
) -> SyntheticImageChannelTransform {
    // This profile used to compensate for a generic synthetic CgBI decode bug by
    // force-swapping all bundle PNGs. The generic decoder now performs the channel
    // normalization itself for the affected non-interlaced CgBI population, so keeping
    // the profile override would double-swap Above And Below and push the whole UI blue.
    SyntheticImageChannelTransform::None
}

pub(crate) fn should_force_top_left_low_strip(texture_key: &str, rect_x: u32, rect_y: u32, rect_h: u32) -> bool {
    let key = texture_key.to_ascii_lowercase();
    key.contains("menu_buttons.png")
        && rect_x <= 4
        && rect_y <= rect_h.saturating_mul(2).saturating_add(8)
}

pub(crate) fn should_prefer_top_left_achievements_strip(
    texture_key: &str,
    rect_x: u32,
    rect_y: u32,
    rect_h: u32,
    parent_selector_name: &str,
) -> bool {
    should_force_top_left_low_strip(texture_key, rect_x, rect_y, rect_h)
        && parent_selector_name.contains("achievementsCallback:")
}

pub(crate) fn should_skip_fullscreen_bootstrap_fill(
    parent_label: &str,
    has_fullscreen_textured_subtree: bool,
) -> bool {
    if !has_fullscreen_textured_subtree {
        return false;
    }
    is_first_scene_label(parent_label) || is_menu_layer_label(parent_label)
}


pub(crate) fn is_title(bundle_root: Option<&Path>) -> bool {
    let bundle_root = bundle_root
        .map(|path| path.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    bundle_root.contains("aboveandbelow")
        || bundle_root.contains("above-and-below")
        || bundle_root.contains("above_and_below")
        || bundle_root.contains("aboveandbelow-eng")
}

pub(crate) fn is_first_scene_label(label: &str) -> bool {
    label.contains("FirstScene")
}

pub(crate) fn is_menu_layer_label(label: &str) -> bool {
    label.contains("MenuLayer")
}

pub(crate) fn is_loading_scene_label(label: &str) -> bool {
    label.contains("LoadingMissionScene") || label.contains("LoadingMenuScene")
}

pub(crate) fn is_loading_mission_scene_label(label: &str) -> bool {
    label.contains("LoadingMissionScene")
}

pub(crate) fn is_loading_scene_or_manager_label(label: &str) -> bool {
    is_loading_scene_label(label) || label.contains("MissionManager")
}

pub(crate) fn loading_continue_prompt_matches(text: &str) -> bool {
    let normalized = text.replace('\0', "").to_ascii_lowercase();
    normalized.contains("tap to continue") || normalized == "continue"
}

pub(crate) fn should_dedupe_loading_bmfont_parent(parent_label: &str) -> bool {
    is_loading_mission_scene_label(parent_label)
}

pub(crate) fn is_achievements_selector(selector_name: &str) -> bool {
    selector_name.contains("achievementsCallback:")
}

pub(crate) fn is_title_scene_like_label(label: &str) -> bool {
    is_first_scene_label(label) || is_loading_scene_label(label)
}

pub(crate) fn synthetic_splash_auto_advance_age_threshold(live_host_mode: bool) -> u32 {
    // In the live host path the synthetic splash scene already receives a real-ish runloop,
    // input nudges, and completed synthetic bootstrap traffic. Keeping the older 90-tick / 45-idle
    // thresholds makes the Pastel / publisher logos linger for multiple wall-clock seconds whenever
    // the host tick rate dips below an ideal 60 Hz. Prefer a noticeably tighter gate here so splash
    // progression stays close to the original feel even when the live presenter is not perfectly paced.
    if live_host_mode { 18 } else { 24 }
}

pub(crate) fn synthetic_splash_auto_advance_idle_threshold(live_host_mode: bool) -> u32 {
    if live_host_mode { 8 } else { 8 }
}

pub(crate) fn synthetic_network_profile() -> SyntheticNetworkProfile {
    SyntheticNetworkProfile {
        url: "https://bootstrap.forever-entertainment.local/aboveandbelow/bootstrap.txt",
        host: "bootstrap.forever-entertainment.local",
        path: "/aboveandbelow/bootstrap.txt",
        method: "GET",
        bundle_id: "forever-entertainment.aboveandbelow",
    }
}

pub(crate) fn synthetic_payload(connection_state_name: &str, retry: bool, delivered: usize) -> Vec<u8> {
    let profile = synthetic_network_profile();
    let mut bytes = format!(
        "bootstrap=ok\nbundle={}\nstate={}\ntransport=synthetic-hle\nretry={}\n",
        profile.bundle_id,
        connection_state_name,
        if retry { "YES" } else { "NO" },
    )
    .into_bytes();
    if bytes.len() < delivered {
        bytes.resize(delivered, b' ');
    }
    bytes.truncate(delivered);
    bytes
}

pub(crate) fn watched_sprite_match(candidate: &str) -> Option<&'static str> {
    let lower = candidate.to_ascii_lowercase();
    WATCHED_SPRITES
        .iter()
        .copied()
        .find(|watch| lower.contains(watch))
}

pub(crate) fn plan_menu_stack(menu_parent_label: &str, entries: &[MenuEntry], force: bool) -> Option<MenuStackPlan> {
    if entries.len() < 3 {
        return None;
    }

    let ys: Vec<f32> = entries.iter().map(|entry| (entry.y * 10.0).round() / 10.0).collect();
    let mut uniq_y_desc = ys.clone();
    uniq_y_desc.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    uniq_y_desc.dedup_by(|a, b| (*a - *b).abs() <= 0.1);

    let mut counts: HashMap<i32, usize> = HashMap::new();
    for y in &ys {
        let key = (y * 10.0).round() as i32;
        *counts.entry(key).or_insert(0) += 1;
    }

    let mut broken = uniq_y_desc.len() <= 2;
    if !broken {
        let max_dup = counts.values().copied().max().unwrap_or(0);
        if max_dup >= usize::max(2, entries.len().saturating_sub(1)) {
            broken = true;
        } else if entries.len() >= 4 {
            let trailing = &ys[1..];
            if !trailing.is_empty() && trailing.iter().all(|v| (*v - trailing[0]).abs() <= 0.1) {
                broken = true;
            }
        }
    }

    let main_menu_signature = menu_parent_label.starts_with("MenuLayer0") && entries.len() >= 4;
    if !broken && !(force && main_menu_signature) {
        return None;
    }

    let mut deltas = Vec::new();
    for pair in uniq_y_desc.windows(2) {
        if let [a, b] = pair {
            let d = *a - *b;
            if d.is_finite() && d > 1.0 {
                deltas.push(d);
            }
        }
    }
    let avg_h = entries.iter().map(|entry| entry.height).sum::<f32>() / (entries.len().max(1) as f32);
    let fallback_pitch = (avg_h + 24.0).max(92.0);
    deltas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut pitch = if deltas.is_empty() { fallback_pitch } else { deltas[deltas.len() / 2] };
    if !pitch.is_finite() || pitch <= 1.0 {
        pitch = fallback_pitch;
    }
    if pitch < (avg_h + 8.0) {
        pitch = fallback_pitch.max(avg_h + 8.0);
    }

    let mut top_y = entries.iter().map(|entry| entry.y).fold(f32::NEG_INFINITY, f32::max);
    if !top_y.is_finite() {
        top_y = 0.0;
    }
    if main_menu_signature {
        if !top_y.is_finite() || top_y <= (pitch * 0.75) {
            top_y += pitch;
        }
        let bottom_y = top_y - pitch * (entries.len().saturating_sub(1) as f32);
        if bottom_y < -260.0 {
            top_y += -260.0 - bottom_y;
        }
    }

    let mut xs: Vec<f32> = entries.iter().map(|entry| entry.x).filter(|value| value.is_finite()).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let anchor_x = if xs.is_empty() { 0.0 } else { xs[xs.len() / 2] };
    let ordered = entries.iter().map(|entry| entry.node).collect();

    Some(MenuStackPlan {
        ordered,
        pitch,
        top_y,
        anchor_x,
    })
}

pub(crate) fn phase69_rect_builder_trace<F>(
    current_pc: u32,
    sp: u32,
    regs3: u32,
    addr: u32,
    value: u32,
    width: u32,
    mut read_u32: F,
) -> Option<String>
where
    F: FnMut(u32) -> Option<u32>,
{
    if current_pc == 0 || width != 4 {
        return None;
    }
    let branch = phase69_rect_builder_branch(current_pc)?;
    if !(addr >= sp && addr < sp.wrapping_add(0x140)) {
        return None;
    }
    let target = format!("stack+0x{:x}", addr.wrapping_sub(sp));
    let value_desc = format!("0x{value:08x}/{:.3}", f32::from_bits(value));
    let snapshot = phase69_rect_builder_snapshot(branch, sp, regs3, &mut read_u32);
    Some(format!(
        "     ↳ phase69 rect-builder branch={} pc=0x{:08x} {}={} {}",
        branch,
        current_pc,
        target,
        value_desc,
        snapshot,
    ))
}

fn phase69_rect_builder_branch(pc: u32) -> Option<&'static str> {
    match pc {
        0x0001d81c..=0x0001d840 => Some("62x202"),
        0x0001d8a8..=0x0001d8b8 => Some("201x65"),
        _ => None,
    }
}

fn phase69_rect_builder_snapshot<F>(branch: &str, sp: u32, regs3: u32, read_u32: &mut F) -> String
where
    F: FnMut(u32) -> Option<u32>,
{
    match branch {
        "62x202" => {
            let prep_x = read_u32(sp.wrapping_add(0xc4));
            let prep_y = read_u32(sp.wrapping_add(0xc8));
            let prep_w = read_u32(sp.wrapping_add(0xcc));
            let prep_h = read_u32(sp.wrapping_add(0xd0));
            let final_y = read_u32(sp);
            let final_w = read_u32(sp.wrapping_add(4));
            let final_h = read_u32(sp.wrapping_add(8));
            format!(
                "prep=({:.3},{:.3} {:.3}x{:.3}) final=(r3={:.3},{:.3} {:.3}x{:.3})",
                prep_x.map(f32::from_bits).unwrap_or(0.0),
                prep_y.map(f32::from_bits).unwrap_or(0.0),
                prep_w.map(f32::from_bits).unwrap_or(0.0),
                prep_h.map(f32::from_bits).unwrap_or(0.0),
                f32::from_bits(regs3),
                final_y.map(f32::from_bits).unwrap_or(0.0),
                final_w.map(f32::from_bits).unwrap_or(0.0),
                final_h.map(f32::from_bits).unwrap_or(0.0),
            )
        }
        "201x65" => {
            let src_y = read_u32(sp.wrapping_add(0xb8));
            let src_w = read_u32(sp.wrapping_add(0xbc));
            let src_h = read_u32(sp.wrapping_add(0xc0));
            let final_y = read_u32(sp);
            let final_w = read_u32(sp.wrapping_add(4));
            let final_h = read_u32(sp.wrapping_add(8));
            format!(
                "src=(y={:.3} w={:.3} h={:.3}) final=(r3={:.3},{:.3} {:.3}x{:.3})",
                src_y.map(f32::from_bits).unwrap_or(0.0),
                src_w.map(f32::from_bits).unwrap_or(0.0),
                src_h.map(f32::from_bits).unwrap_or(0.0),
                f32::from_bits(regs3),
                final_y.map(f32::from_bits).unwrap_or(0.0),
                final_w.map(f32::from_bits).unwrap_or(0.0),
                final_h.map(f32::from_bits).unwrap_or(0.0),
            )
        }
        _ => "snapshot=<none>".to_string(),
    }
}


impl TitleProfile for AboveAndBelowProfile {
    fn profile_id(&self) -> &'static str {
        "above_and_below"
    }

    fn matches_bundle_root(&self, bundle_root: Option<&Path>) -> bool {
        is_title(bundle_root)
    }

    fn director_ivar_layout(&self) -> Option<DirectorIvarLayout> {
        director_ivar_layout(Some(Path::new("Payload/AboveAndBelow.app")))
    }

    fn texture_rect_origin_preference(&self, texture_key: &str) -> Option<bool> {
        texture_rect_origin_preference(texture_key)
    }

    fn bundle_image_channel_transform(
        &self,
        texture_key: &str,
        path: &Path,
        saw_cgbi: bool,
        used_cgbi: bool,
    ) -> SyntheticImageChannelTransform {
        bundle_image_channel_transform(texture_key, path, saw_cgbi, used_cgbi)
    }

    fn should_force_top_left_low_strip(&self, texture_key: &str, rect_x: u32, rect_y: u32, rect_h: u32) -> bool {
        should_force_top_left_low_strip(texture_key, rect_x, rect_y, rect_h)
    }

    fn should_prefer_top_left_achievements_strip(
        &self,
        texture_key: &str,
        rect_x: u32,
        rect_y: u32,
        rect_h: u32,
        parent_selector_name: &str,
    ) -> bool {
        should_prefer_top_left_achievements_strip(texture_key, rect_x, rect_y, rect_h, parent_selector_name)
    }

    fn should_skip_fullscreen_bootstrap_fill(
        &self,
        parent_label: &str,
        has_fullscreen_textured_subtree: bool,
    ) -> bool {
        should_skip_fullscreen_bootstrap_fill(parent_label, has_fullscreen_textured_subtree)
    }

    fn is_first_scene_label(&self, label: &str) -> bool {
        is_first_scene_label(label)
    }

    fn is_menu_layer_label(&self, label: &str) -> bool {
        is_menu_layer_label(label)
    }

    fn is_loading_scene_label(&self, label: &str) -> bool {
        is_loading_scene_label(label)
    }

    fn is_loading_mission_scene_label(&self, label: &str) -> bool {
        is_loading_mission_scene_label(label)
    }

    fn is_loading_scene_or_manager_label(&self, label: &str) -> bool {
        is_loading_scene_or_manager_label(label)
    }

    fn loading_continue_prompt_matches(&self, text: &str) -> bool {
        loading_continue_prompt_matches(text)
    }

    fn should_dedupe_loading_bmfont_parent(&self, parent_label: &str) -> bool {
        should_dedupe_loading_bmfont_parent(parent_label)
    }

    fn is_achievements_selector(&self, selector_name: &str) -> bool {
        is_achievements_selector(selector_name)
    }

    fn synthetic_splash_auto_advance_age_threshold(&self, live_host_mode: bool) -> Option<u32> {
        Some(synthetic_splash_auto_advance_age_threshold(live_host_mode))
    }

    fn synthetic_splash_auto_advance_idle_threshold(&self, live_host_mode: bool) -> Option<u32> {
        Some(synthetic_splash_auto_advance_idle_threshold(live_host_mode))
    }

    fn synthetic_network_profile(&self) -> SyntheticNetworkProfile {
        synthetic_network_profile()
    }

    fn synthetic_payload(&self, connection_state_name: &str, retry: bool, delivered: usize) -> Vec<u8> {
        synthetic_payload(connection_state_name, retry, delivered)
    }

    fn watched_sprite_match(&self, candidate: &str) -> Option<&'static str> {
        watched_sprite_match(candidate)
    }

    fn menu_stack_plan(&self, menu_parent_label: &str, entries: &[MenuEntry], force: bool) -> Option<MenuStackPlan> {
        plan_menu_stack(menu_parent_label, entries, force)
    }

    fn phase69_rect_builder_trace(
        &self,
        current_pc: u32,
        sp: u32,
        regs3: u32,
        addr: u32,
        value: u32,
        width: u32,
        read_u32: &mut dyn FnMut(u32) -> Option<u32>,
    ) -> Option<String> {
        phase69_rect_builder_trace(current_pc, sp, regs3, addr, value, width, read_u32)
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;

    #[test]
    fn director_ivar_layout_is_title_scoped() {
        let layout = director_ivar_layout(Some(Path::new("Payload/AboveAndBelow.app"))).unwrap();
        assert_eq!(layout.open_gl_view_offset, 4);
        assert_eq!(layout.running_scene_offset, 68);
        assert_eq!(layout.next_scene_offset, 72);
        assert!(director_ivar_layout(Some(Path::new("Payload/OtherGame.app"))).is_none());
    }

    #[test]
    fn texture_rect_origin_preferences_are_title_specific() {
        assert_eq!(texture_rect_origin_preference("Menu_Buttons.png"), Some(true));
        assert_eq!(texture_rect_origin_preference("loading_bar.png"), Some(false));
        assert_eq!(texture_rect_origin_preference("atlas.png"), None);
    }

    #[test]
    fn bootstrap_fill_skip_requires_title_parent_and_textured_subtree() {
        assert!(should_skip_fullscreen_bootstrap_fill("FirstScene.synthetic", true));
        assert!(should_skip_fullscreen_bootstrap_fill("MenuLayer0.synthetic", true));
        assert!(!should_skip_fullscreen_bootstrap_fill("CCLayer.synthetic", true));
        assert!(!should_skip_fullscreen_bootstrap_fill("MenuLayer0.synthetic", false));
    }

    #[test]
    fn title_detection_matches_known_bundle_variants() {
        assert!(is_title(Some(Path::new("Payload/AboveAndBelow.app"))));
        assert!(is_title(Some(Path::new("Payload/above_and_below-eng.app"))));
        assert!(!is_title(Some(Path::new("Payload/OtherGame.app"))));
        assert!(!is_title(None));
    }

    #[test]
    fn watched_sprite_matching_is_case_insensitive() {
        assert_eq!(watched_sprite_match("UI/Loading_Bar.PNG"), Some("loading_bar.png"));
        assert_eq!(watched_sprite_match("textures/menu_notepad.png"), Some("menu_notepad.png"));
        assert_eq!(watched_sprite_match("textures/unknown.png"), None);
    }

    #[test]
    fn title_label_helpers_cover_remaining_scene_specific_rules() {
        assert!(is_first_scene_label("FirstScene.synthetic#0"));
        assert!(is_menu_layer_label("MenuLayer0.synthetic#0"));
        assert!(is_loading_scene_label("LoadingMissionScene.synthetic#0"));
        assert!(is_loading_scene_label("LoadingMenuScene.synthetic#0"));
        assert!(is_loading_mission_scene_label("LoadingMissionScene.synthetic#0"));
        assert!(!is_loading_mission_scene_label("LoadingMenuScene.synthetic#0"));
        assert!(is_loading_scene_or_manager_label("MissionManager.singleton"));
        assert!(should_dedupe_loading_bmfont_parent("LoadingMissionScene.synthetic#0"));
        assert!(is_achievements_selector("achievementsCallback:"));
        assert!(loading_continue_prompt_matches("Tap to continue"));
        assert!(loading_continue_prompt_matches("continue"));
        assert!(!loading_continue_prompt_matches("loading"));
    }

    #[test]
    fn synthetic_network_profile_matches_bootstrap_contract() {
        let profile = synthetic_network_profile();
        assert_eq!(profile.method, "GET");
        assert!(profile.url.contains(profile.host));
        assert!(profile.url.ends_with(profile.path));
        assert_eq!(profile.bundle_id, "forever-entertainment.aboveandbelow");
    }

    #[test]
    fn synthetic_payload_tracks_state_and_delivery_budget() {
        let bytes = synthetic_payload("receiving", true, 64);
        let text = String::from_utf8(bytes.clone()).expect("payload must stay utf8");
        assert!(text.contains("state=receiving"));
        assert!(text.contains("retry=YES"));
        assert_eq!(bytes.len(), 64);
    }


    #[test]
    fn live_host_splash_thresholds_stay_tighter_than_legacy_guardrails() {
        assert_eq!(synthetic_splash_auto_advance_age_threshold(false), 24);
        assert_eq!(synthetic_splash_auto_advance_idle_threshold(false), 8);
        assert_eq!(synthetic_splash_auto_advance_age_threshold(true), 18);
        assert_eq!(synthetic_splash_auto_advance_idle_threshold(true), 8);
        assert!(synthetic_splash_auto_advance_age_threshold(true) < 24);
    }

    #[test]
    fn menu_stack_plan_detects_broken_main_menu_layout() {
        let entries = vec![
            MenuEntry { node: 1, width: 220.0, height: 65.0, x: 160.0, y: 120.0 },
            MenuEntry { node: 2, width: 220.0, height: 65.0, x: 160.0, y: 0.0 },
            MenuEntry { node: 3, width: 220.0, height: 65.0, x: 160.0, y: 0.0 },
            MenuEntry { node: 4, width: 220.0, height: 65.0, x: 160.0, y: 0.0 },
        ];

        let plan = plan_menu_stack("MenuLayer0.instance(synth)", &entries, true)
            .expect("broken main menu layout should get a recovery plan");
        assert_eq!(plan.ordered, vec![1, 2, 3, 4]);
        assert!(plan.pitch >= 89.0);
        assert!(plan.top_y > 120.0);
        assert_eq!(plan.anchor_x, 160.0);
    }

    #[test]
    fn menu_stack_plan_ignores_healthy_layouts_without_force() {
        let entries = vec![
            MenuEntry { node: 1, width: 220.0, height: 65.0, x: 160.0, y: 300.0 },
            MenuEntry { node: 2, width: 220.0, height: 65.0, x: 160.0, y: 210.0 },
            MenuEntry { node: 3, width: 220.0, height: 65.0, x: 160.0, y: 120.0 },
            MenuEntry { node: 4, width: 220.0, height: 65.0, x: 160.0, y: 30.0 },
        ];

        assert!(plan_menu_stack("MenuLayer0.instance(synth)", &entries, false).is_none());
    }

    #[test]
    fn phase69_trace_formats_branch_specific_snapshot() {
        let sp = 0x1000u32;
        let mut mem = HashMap::new();
        mem.insert(sp + 0xc4, 1.25f32.to_bits());
        mem.insert(sp + 0xc8, 2.5f32.to_bits());
        mem.insert(sp + 0xcc, 62.0f32.to_bits());
        mem.insert(sp + 0xd0, 202.0f32.to_bits());
        mem.insert(sp, 12.0f32.to_bits());
        mem.insert(sp + 4, 62.0f32.to_bits());
        mem.insert(sp + 8, 202.0f32.to_bits());

        let trace = phase69_rect_builder_trace(
            0x0001d820,
            sp,
            9.0f32.to_bits(),
            sp + 4,
            62.0f32.to_bits(),
            4,
            |addr| mem.get(&addr).copied(),
        )
        .expect("phase69 branch should emit a trace");

        assert!(trace.contains("branch=62x202"));
        assert!(trace.contains("prep=(1.250,2.500 62.000x202.000)"));
        assert!(trace.contains("final=(r3=9.000,12.000 62.000x202.000)"));
    }
}
