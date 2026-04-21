#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderFrameSource {
    UIKit,
    Guest,
    SyntheticScene,
    Retained,
    SyntheticFallback,
}

impl RenderFrameSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UIKit => "uikit",
            Self::Guest => "guest",
            Self::SyntheticScene => "synthetic-scene",
            Self::Retained => "retained",
            Self::SyntheticFallback => "synthetic-fallback",
        }
    }

    pub(crate) fn readback_origin(self) -> &'static str {
        match self {
            Self::UIKit => "present-uikit",
            Self::Guest => "present-guest",
            Self::SyntheticScene => "present-auto-scene",
            Self::Retained => "present-retained",
            Self::SyntheticFallback => "present-synthetic-fallback",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RenderFrameDecision {
    pub(crate) source: RenderFrameSource,
    pub(crate) reason: &'static str,
    pub(crate) reused_previous: bool,
}

impl RenderFrameDecision {
    pub(crate) fn summary(&self) -> String {
        format!(
            "source={} reason={} reused_previous={}",
            self.source.as_str(),
            self.reason,
            if self.reused_previous { "YES" } else { "NO" },
        )
    }
}

pub(crate) fn decide_frame_source(
    has_uikit_dirty: bool,
    has_guest_dirty: bool,
    guest_draws_since_present: usize,
    auto_scene_draws: usize,
    framebuffer_has_pixels: bool,
    presented_once: bool,
) -> RenderFrameDecision {
    if has_uikit_dirty {
        return RenderFrameDecision {
            source: RenderFrameSource::UIKit,
            reason: "uikit-dirty",
            reused_previous: false,
        };
    }
    if has_guest_dirty && guest_draws_since_present > 0 && framebuffer_has_pixels {
        return RenderFrameDecision {
            source: RenderFrameSource::Guest,
            reason: "guest-dirty-visible",
            reused_previous: false,
        };
    }
    if auto_scene_draws > 0 {
        return RenderFrameDecision {
            source: RenderFrameSource::SyntheticScene,
            reason: "auto-scene-draw",
            reused_previous: false,
        };
    }
    if has_guest_dirty && guest_draws_since_present > 0 {
        return RenderFrameDecision {
            source: RenderFrameSource::Guest,
            reason: "guest-dirty",
            reused_previous: false,
        };
    }
    if framebuffer_has_pixels && presented_once {
        return RenderFrameDecision {
            source: RenderFrameSource::Retained,
            reason: if has_guest_dirty { "guest-clear-retain" } else { "retain-last-frame" },
            reused_previous: true,
        };
    }
    if has_guest_dirty {
        return RenderFrameDecision {
            source: RenderFrameSource::Guest,
            reason: "guest-dirty-no-retained",
            reused_previous: false,
        };
    }
    RenderFrameDecision {
        source: RenderFrameSource::SyntheticFallback,
        reason: "no-dirty-source",
        reused_previous: false,
    }
}
