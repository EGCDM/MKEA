use std::path::Path;

use super::TitleProfile;

#[derive(Debug)]
pub(crate) struct DefaultTitleProfile;

pub(crate) static PROFILE: DefaultTitleProfile = DefaultTitleProfile;

impl TitleProfile for DefaultTitleProfile {
    fn profile_id(&self) -> &'static str {
        "default"
    }

    fn matches_bundle_root(&self, _bundle_root: Option<&Path>) -> bool {
        true
    }

    fn is_default(&self) -> bool {
        true
    }
}
