use std::fmt::Debug;
use std::path::Path;

mod default;
pub(crate) mod above_and_below;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyntheticNetworkProfile {
    pub url: &'static str,
    pub host: &'static str,
    pub path: &'static str,
    pub method: &'static str,
    pub bundle_id: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectorIvarLayout {
    pub open_gl_view_offset: u32,
    pub running_scene_offset: u32,
    pub next_scene_offset: u32,
    pub effect_scene_offset: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MenuEntry {
    pub node: u32,
    pub width: f32,
    pub height: f32,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MenuStackPlan {
    pub ordered: Vec<u32>,
    pub pitch: f32,
    pub top_y: f32,
    pub anchor_x: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyntheticImageChannelTransform {
    None,
    SwapRedBlue,
}

pub(crate) trait TitleProfile: Debug + Sync {
    fn profile_id(&self) -> &'static str;

    fn matches_bundle_root(&self, _bundle_root: Option<&Path>) -> bool {
        false
    }

    fn is_default(&self) -> bool {
        false
    }

    fn director_ivar_layout(&self) -> Option<DirectorIvarLayout> {
        None
    }

    fn texture_rect_origin_preference(&self, _texture_key: &str) -> Option<bool> {
        None
    }

    fn bundle_image_channel_transform(
        &self,
        _texture_key: &str,
        _path: &Path,
        _saw_cgbi: bool,
        _used_cgbi: bool,
    ) -> SyntheticImageChannelTransform {
        SyntheticImageChannelTransform::None
    }

    fn should_force_top_left_low_strip(
        &self,
        _texture_key: &str,
        _rect_x: u32,
        _rect_y: u32,
        _rect_h: u32,
    ) -> bool {
        false
    }

    fn should_prefer_top_left_achievements_strip(
        &self,
        _texture_key: &str,
        _rect_x: u32,
        _rect_y: u32,
        _rect_h: u32,
        _parent_selector_name: &str,
    ) -> bool {
        false
    }

    fn should_skip_fullscreen_bootstrap_fill(
        &self,
        _parent_label: &str,
        _has_fullscreen_textured_subtree: bool,
    ) -> bool {
        false
    }

    fn is_first_scene_label(&self, _label: &str) -> bool {
        false
    }

    fn is_menu_layer_label(&self, _label: &str) -> bool {
        false
    }

    fn is_loading_scene_label(&self, _label: &str) -> bool {
        false
    }

    fn is_loading_mission_scene_label(&self, _label: &str) -> bool {
        false
    }

    fn is_loading_scene_or_manager_label(&self, _label: &str) -> bool {
        false
    }

    fn loading_continue_prompt_matches(&self, _text: &str) -> bool {
        false
    }

    fn should_dedupe_loading_bmfont_parent(&self, _parent_label: &str) -> bool {
        false
    }

    fn is_achievements_selector(&self, _selector_name: &str) -> bool {
        false
    }

    fn synthetic_splash_auto_advance_age_threshold(&self, _live_host_mode: bool) -> Option<u32> {
        None
    }

    fn synthetic_splash_auto_advance_idle_threshold(&self, _live_host_mode: bool) -> Option<u32> {
        None
    }

    fn synthetic_network_profile(&self) -> SyntheticNetworkProfile {
        SyntheticNetworkProfile {
            url: "https://bootstrap.generic.local/bootstrap.txt",
            host: "bootstrap.generic.local",
            path: "/bootstrap.txt",
            method: "GET",
            bundle_id: "generic.synthetic.title",
        }
    }

    fn synthetic_payload(&self, connection_state_name: &str, retry: bool, delivered: usize) -> Vec<u8> {
        let profile = self.synthetic_network_profile();
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

    fn watched_sprite_match(&self, _candidate: &str) -> Option<&'static str> {
        None
    }

    fn menu_stack_plan(&self, _menu_parent_label: &str, _entries: &[MenuEntry], _force: bool) -> Option<MenuStackPlan> {
        None
    }

    fn phase69_rect_builder_trace(
        &self,
        _current_pc: u32,
        _sp: u32,
        _regs3: u32,
        _addr: u32,
        _value: u32,
        _width: u32,
        _read_u32: &mut dyn FnMut(u32) -> Option<u32>,
    ) -> Option<String> {
        None
    }
}

pub(crate) fn detect_title_profile(bundle_root: Option<&Path>) -> &'static dyn TitleProfile {
    if above_and_below::PROFILE.matches_bundle_root(bundle_root) {
        &above_and_below::PROFILE
    } else {
        &default::PROFILE
    }
}
