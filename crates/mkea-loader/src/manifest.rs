use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayOrientation {
    Portrait,
    Landscape,
}

impl DisplayOrientation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Portrait => "portrait",
            Self::Landscape => "landscape",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub bundle_id: String,
    pub bundle_name: String,
    pub minimum_ios_version: String,
    pub executable: String,
    pub info_plist_path_in_ipa: String,
    pub executable_path_in_ipa: String,
    pub chosen_arch: String,
    #[serde(default)]
    pub supported_interface_orientations: Vec<String>,
    #[serde(default)]
    pub preferred_orientation: Option<DisplayOrientation>,
    #[serde(default)]
    pub native_surface_width: Option<u32>,
    #[serde(default)]
    pub native_surface_height: Option<u32>,
    #[serde(default)]
    pub orientation_inference_source: Option<String>,
}

impl Manifest {
    pub fn plist_orientation_hint(&self) -> Option<DisplayOrientation> {
        supported_orientation_from_strings(&self.supported_interface_orientations)
            .or(self.preferred_orientation)
    }

    pub fn surface_size_hint(&self) -> Option<(u32, u32)> {
        let width = self.native_surface_width?;
        let height = self.native_surface_height?;
        if width == 0 || height == 0 {
            return None;
        }
        Some((width, height))
    }
}

#[derive(Debug, Clone, Default)]
pub struct BundleDisplayProfile {
    pub preferred_orientation: Option<DisplayOrientation>,
    pub surface_width: Option<u32>,
    pub surface_height: Option<u32>,
    pub source: Option<String>,
}

impl BundleDisplayProfile {
    pub fn surface_size(&self) -> Option<(u32, u32)> {
        let width = self.surface_width?;
        let height = self.surface_height?;
        if width == 0 || height == 0 {
            return None;
        }
        Some((width, height))
    }
}

#[derive(Debug, Clone)]
struct ImageOrientationCandidate {
    width: u32,
    height: u32,
    weight: u64,
    path: PathBuf,
}

impl Default for ImageOrientationCandidate {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            weight: 0,
            path: PathBuf::new(),
        }
    }
}

impl ImageOrientationCandidate {
    fn update_if_stronger(&mut self, width: u32, height: u32, weight: u64, path: &Path) {
        if weight > self.weight {
            self.width = width;
            self.height = height;
            self.weight = weight;
            self.path = path.to_path_buf();
        }
    }
}

pub fn supported_orientation_from_strings(values: &[String]) -> Option<DisplayOrientation> {
    let mut saw_portrait = false;
    let mut saw_landscape = false;
    for value in values {
        let lower = value.to_ascii_lowercase();
        if lower.contains("landscape") {
            saw_landscape = true;
        }
        if lower.contains("portrait") {
            saw_portrait = true;
        }
    }
    match (saw_portrait, saw_landscape) {
        (true, false) => Some(DisplayOrientation::Portrait),
        (false, true) => Some(DisplayOrientation::Landscape),
        _ => None,
    }
}

pub fn infer_bundle_display_profile(
    bundle_root: &Path,
    supported_orientations: &[String],
) -> BundleDisplayProfile {
    let plist_hint = supported_orientation_from_strings(supported_orientations);
    let mut stack = vec![bundle_root.to_path_buf()];
    let mut portrait_score = 0u64;
    let mut landscape_score = 0u64;
    let mut best_portrait = ImageOrientationCandidate::default();
    let mut best_landscape = ImageOrientationCandidate::default();
    let mut scanned_pngs = 0u32;

    while let Some(dir) = stack.pop() {
        let Ok(read_dir) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
                continue;
            };
            if !ext.eq_ignore_ascii_case("png") {
                continue;
            }
            let Some((width, height)) = read_png_dimensions(&path) else {
                continue;
            };
            scanned_pngs = scanned_pngs.saturating_add(1);
            let area = width as u64 * height as u64;
            if area < 20_000 {
                continue;
            }

            let lower = path
                .file_name()
                .and_then(|value| value.to_str())
                .map(|value| value.to_ascii_lowercase())
                .unwrap_or_default();
            if lower.contains("icon") || lower.contains("itunesartwork") || lower.contains("appicon") {
                continue;
            }

            let mut weight = area;
            if lower.contains("default")
                || lower.contains("launch")
                || lower.contains("splash")
                || lower.contains("menu")
                || lower.contains("background")
                || lower.contains("_bg")
                || lower.ends_with("bg.png")
                || lower.contains("title")
            {
                weight = weight.saturating_mul(4);
            } else if lower.contains("cover") || lower.contains("home") {
                weight = weight.saturating_mul(2);
            }

            if width.saturating_mul(6) >= height.saturating_mul(7) {
                landscape_score = landscape_score.saturating_add(weight);
                best_landscape.update_if_stronger(width, height, weight, &path);
            } else if height.saturating_mul(6) >= width.saturating_mul(7) {
                portrait_score = portrait_score.saturating_add(weight);
                best_portrait.update_if_stronger(width, height, weight, &path);
            }
        }
    }

    let chosen_orientation = plist_hint.or_else(|| {
        if landscape_score > portrait_score.saturating_mul(5) / 4 {
            Some(DisplayOrientation::Landscape)
        } else if portrait_score > landscape_score.saturating_mul(5) / 4 {
            Some(DisplayOrientation::Portrait)
        } else if best_landscape.weight > best_portrait.weight {
            Some(DisplayOrientation::Landscape)
        } else if best_portrait.weight > best_landscape.weight {
            Some(DisplayOrientation::Portrait)
        } else {
            None
        }
    });

    let (surface_width, surface_height, evidence_path) = match chosen_orientation {
        Some(DisplayOrientation::Landscape) if best_landscape.weight != 0 => (
            Some(best_landscape.width.max(best_landscape.height)),
            Some(best_landscape.width.min(best_landscape.height)),
            Some(best_landscape.path.clone()),
        ),
        Some(DisplayOrientation::Portrait) if best_portrait.weight != 0 => (
            Some(best_portrait.width.min(best_portrait.height)),
            Some(best_portrait.width.max(best_portrait.height)),
            Some(best_portrait.path.clone()),
        ),
        Some(DisplayOrientation::Landscape) => (Some(480), Some(320), None),
        Some(DisplayOrientation::Portrait) => (Some(320), Some(480), None),
        None => (None, None, None),
    };

    let source = if let Some(orientation) = chosen_orientation {
        let reason = if plist_hint == Some(orientation) {
            "plist"
        } else {
            "bundle-assets"
        };
        Some(format!(
            "{}:{} scanned_pngs={} portrait_score={} landscape_score={} evidence={}",
            reason,
            orientation.as_str(),
            scanned_pngs,
            portrait_score,
            landscape_score,
            evidence_path
                .as_ref()
                .map(|value| value.display().to_string())
                .unwrap_or_else(|| "<none>".to_string())
        ))
    } else if scanned_pngs != 0 {
        Some(format!(
            "bundle-assets:unknown scanned_pngs={} portrait_score={} landscape_score={}",
            scanned_pngs,
            portrait_score,
            landscape_score,
        ))
    } else {
        None
    };

    BundleDisplayProfile {
        preferred_orientation: chosen_orientation,
        surface_width,
        surface_height,
        source,
    }
}

fn read_png_dimensions(path: &Path) -> Option<(u32, u32)> {
    let mut file = File::open(path).ok()?;
    let mut signature = [0u8; 8];
    file.read_exact(&mut signature).ok()?;
    if &signature != b"\x89PNG\r\n\x1a\n" {
        return None;
    }

    loop {
        let mut len_buf = [0u8; 4];
        file.read_exact(&mut len_buf).ok()?;
        let chunk_len = u32::from_be_bytes(len_buf) as u64;

        let mut kind = [0u8; 4];
        file.read_exact(&mut kind).ok()?;
        if &kind == b"IHDR" {
            if chunk_len < 8 {
                return None;
            }
            let mut dims = [0u8; 8];
            file.read_exact(&mut dims).ok()?;
            let width = u32::from_be_bytes([dims[0], dims[1], dims[2], dims[3]]);
            let height = u32::from_be_bytes([dims[4], dims[5], dims[6], dims[7]]);
            return (width != 0 && height != 0).then_some((width, height));
        }
        file.seek(SeekFrom::Current(chunk_len as i64 + 4)).ok()?;
    }
}
