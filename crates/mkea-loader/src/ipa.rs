use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use plist::Value;
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::macho::{parse_macho_slice, pick_preferred_slice, MachProbe};
use crate::manifest::{infer_bundle_display_profile, supported_orientation_from_strings, Manifest};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpaProbe {
    pub manifest: Manifest,
    pub mach: MachProbe,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LoadedIpa {
    pub probe: IpaProbe,
    pub macho_slice: Vec<u8>,
    pub bundle_root: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildArtifactManifest {
    pub ipa: String,
    pub extracted_dir: String,
    pub binary_slice_path: String,
    pub imports_path: String,
    pub probe: IpaProbe,
}

pub fn inspect_ipa(path: &Path) -> Result<IpaProbe> {
    inspect_ipa_with_arch(path, "armv6")
}

pub fn write_build_artifacts(path: &Path, prefer_arch: &str, out_dir: &Path) -> Result<BuildArtifactManifest> {
    let mut loaded = load_ipa_with_arch(path, prefer_arch)?;

    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create build dir: {}", out_dir.display()))?;

    let extracted_dir = out_dir.join("extracted");
    if extracted_dir.exists() {
        fs::remove_dir_all(&extracted_dir)
            .with_context(|| format!("failed to clear extracted dir: {}", extracted_dir.display()))?;
    }
    extract_ipa_to_dir(path, &extracted_dir)?;

    let binary_rel = PathBuf::from("binary").join(format!(
        "{}.{}.macho",
        loaded.probe.manifest.executable, loaded.probe.manifest.chosen_arch
    ));
    let binary_abs = out_dir.join(&binary_rel);
    if let Some(parent) = binary_abs.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create binary dir: {}", parent.display()))?;
    }
    fs::write(&binary_abs, &loaded.macho_slice)
        .with_context(|| format!("failed to write selected Mach-O slice: {}", binary_abs.display()))?;

    let imports_rel = PathBuf::from("imports.txt");
    let imports_abs = out_dir.join(&imports_rel);
    let mut imports_text = loaded.probe.mach.undefined_symbols.join("\n");
    if !imports_text.is_empty() {
        imports_text.push('\n');
    }
    fs::write(&imports_abs, imports_text)
        .with_context(|| format!("failed to write imports list: {}", imports_abs.display()))?;

    if let Some((head, _)) = loaded.probe.manifest.info_plist_path_in_ipa.rsplit_once('/') {
        let bundle_root = extracted_dir.join(head);
        let profile = infer_bundle_display_profile(
            &bundle_root,
            &loaded.probe.manifest.supported_interface_orientations,
        );
        if loaded.probe.manifest.preferred_orientation.is_none() {
            loaded.probe.manifest.preferred_orientation = profile.preferred_orientation;
        }
        if loaded.probe.manifest.native_surface_width.is_none() {
            loaded.probe.manifest.native_surface_width = profile.surface_width;
        }
        if loaded.probe.manifest.native_surface_height.is_none() {
            loaded.probe.manifest.native_surface_height = profile.surface_height;
        }
        if loaded.probe.manifest.orientation_inference_source.is_none() {
            loaded.probe.manifest.orientation_inference_source = profile.source.clone();
        }
        if let Some(source) = profile.source {
            loaded.probe.notes.push(format!("Display profile: {}", source));
        }
    }

    let artifact = BuildArtifactManifest {
        ipa: path.display().to_string(),
        extracted_dir: portable_manifest_path(&path_relative_to(out_dir, &extracted_dir)),
        binary_slice_path: portable_manifest_path(&binary_rel),
        imports_path: portable_manifest_path(&imports_rel),
        probe: loaded.probe,
    };

    let manifest_path = out_dir.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_vec_pretty(&artifact)?)
        .with_context(|| format!("failed to write build manifest: {}", manifest_path.display()))?;

    Ok(artifact)
}

pub fn load_build_artifact(path: &Path) -> Result<LoadedIpa> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read build manifest: {}", path.display()))?;
    let artifact: BuildArtifactManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse build manifest JSON: {}", path.display()))?;

    let binary_path = resolve_relative_path(path, &artifact.binary_slice_path);
    let macho_slice = fs::read(&binary_path)
        .with_context(|| format!("failed to read selected Mach-O slice: {}", binary_path.display()))?;

    let extracted_dir = resolve_relative_path(path, &artifact.extracted_dir);
    let bundle_root = artifact
        .probe
        .manifest
        .info_plist_path_in_ipa
        .rsplit_once('/')
        .map(|(head, _)| extracted_dir.join(head));

    Ok(LoadedIpa {
        probe: artifact.probe,
        macho_slice,
        bundle_root,
    })
}

pub fn inspect_ipa_with_arch(path: &Path, prefer_arch: &str) -> Result<IpaProbe> {
    Ok(load_ipa_with_arch(path, prefer_arch)?.probe)
}

pub fn load_ipa_with_arch(path: &Path, prefer_arch: &str) -> Result<LoadedIpa> {
    let file = File::open(path)
        .with_context(|| format!("failed to open ipa: {}", path.display()))?;
    let mut zip = ZipArchive::new(file)
        .with_context(|| format!("failed to open ZIP/IPA: {}", path.display()))?;

    let info_plist_path = find_info_plist(&mut zip)?;
    let info_plist_bytes = read_entry_bytes(&mut zip, &info_plist_path)?;
    let info_plist = Value::from_reader(Cursor::new(info_plist_bytes))
        .context("failed to parse Info.plist")?;
    let dict = info_plist
        .as_dictionary()
        .ok_or_else(|| anyhow!("Info.plist root is not a dictionary"))?;

    let bundle_id = get_plist_string(dict, "CFBundleIdentifier").unwrap_or_default();
    let bundle_name = get_plist_string(dict, "CFBundleDisplayName")
        .or_else(|| get_plist_string(dict, "CFBundleName"))
        .unwrap_or_default();
    let minimum_ios_version = get_plist_string(dict, "MinimumOSVersion").unwrap_or_default();
    let executable = get_plist_string(dict, "CFBundleExecutable")
        .ok_or_else(|| anyhow!("Info.plist does not contain CFBundleExecutable"))?;
    let supported_interface_orientations = plist_supported_interface_orientations(dict);

    let app_dir = info_plist_path
        .rsplit_once('/')
        .map(|(head, _)| head)
        .ok_or_else(|| anyhow!("invalid Info.plist path inside IPA: {info_plist_path}"))?;
    let executable_path_in_ipa = format!("{app_dir}/{executable}");

    let macho_bytes = read_entry_bytes(&mut zip, &executable_path_in_ipa)
        .with_context(|| format!("failed to read Mach-O: {executable_path_in_ipa}"))?;
    let (chosen_arch, chosen_slice) = pick_preferred_slice(&macho_bytes, prefer_arch)
        .with_context(|| format!("failed to choose slice for preferred arch {prefer_arch}"))?;
    let mach = parse_macho_slice(&chosen_arch, &chosen_slice)
        .with_context(|| format!("failed to parse Mach-O slice ({chosen_arch})"))?;

    let mut notes = Vec::new();
    notes.push(format!(
        "Selected architecture: {} (preferred: {})",
        chosen_arch, prefer_arch
    ));
    notes.push(format!(
        "Undefined symbols queued for HLE layer: {}",
        mach.undefined_symbols.len()
    ));
    notes.push(format!(
        "Indirect pointer slots discovered: {}",
        mach.indirect_pointers.len()
    ));
    notes.push(format!(
        "External relocations discovered: {}",
        mach.external_relocations.len()
    ));
    if mach.encryption_cryptid.unwrap_or(0) != 0 {
        notes.push(
            "cryptid != 0: binary appears encrypted; a legal unencrypted binary is required for actual execution"
                .to_string(),
        );
    }
    if mach.entry_pc.is_none() {
        notes.push(
            "Entry point was not resolved from LC_MAIN/LC_UNIXTHREAD yet; runtime bootstrap will need an explicit fallback"
                .to_string(),
        );
    }

    let probe = IpaProbe {
        manifest: Manifest {
            bundle_id,
            bundle_name,
            minimum_ios_version,
            executable,
            info_plist_path_in_ipa: info_plist_path,
            executable_path_in_ipa,
            chosen_arch,
            supported_interface_orientations: supported_interface_orientations.clone(),
            preferred_orientation: supported_orientation_from_strings(&supported_interface_orientations),
            native_surface_width: None,
            native_surface_height: None,
            orientation_inference_source: supported_orientation_from_strings(&supported_interface_orientations)
                .map(|orientation| format!("plist:{}", orientation.as_str())),
        },
        mach,
        notes,
    };

    Ok(LoadedIpa {
        probe,
        macho_slice: chosen_slice,
        bundle_root: None,
    })
}

fn extract_ipa_to_dir(path: &Path, out_dir: &Path) -> Result<()> {
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create extracted dir: {}", out_dir.display()))?;

    let file = File::open(path)
        .with_context(|| format!("failed to open ipa for extraction: {}", path.display()))?;
    let mut zip = ZipArchive::new(file)
        .with_context(|| format!("failed to open ZIP/IPA for extraction: {}", path.display()))?;
    zip.extract(out_dir)
        .with_context(|| format!("failed to extract ipa into {}", out_dir.display()))?;
    Ok(())
}


fn plist_supported_interface_orientations(dict: &plist::Dictionary) -> Vec<String> {
    let mut out = Vec::new();
    for key in [
        "UISupportedInterfaceOrientations",
        "UISupportedInterfaceOrientations~iphone",
        "UISupportedInterfaceOrientations~ipad",
    ] {
        if let Some(values) = dict.get(key).and_then(|value| value.as_array()) {
            for value in values {
                if let Some(text) = value.as_string() {
                    let normalized = text.trim();
                    if !normalized.is_empty() && !out.iter().any(|existing| existing == normalized) {
                        out.push(normalized.to_string());
                    }
                }
            }
        }
    }
    if let Some(value) = get_plist_string(dict, "UIInterfaceOrientation") {
        if !value.trim().is_empty() && !out.iter().any(|existing| existing == &value) {
            out.push(value);
        }
    }
    out
}

fn resolve_relative_path(manifest_path: &Path, stored_path: &str) -> PathBuf {
    let candidate = PathBuf::from(stored_path);
    if candidate.is_absolute() {
        return candidate;
    }

    manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(parse_portable_relative_path(stored_path))
}

fn path_relative_to(base: &Path, target: &Path) -> PathBuf {
    target.strip_prefix(base).map(Path::to_path_buf).unwrap_or_else(|_| target.to_path_buf())
}

fn portable_manifest_path(path: &Path) -> String {
    if path.is_absolute() {
        return path.to_string_lossy().replace('\\', "/");
    }

    let mut parts = Vec::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::CurDir => {}
            Component::ParentDir => parts.push("..".to_string()),
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::RootDir | Component::Prefix(_) => {
                return path.to_string_lossy().replace('\\', "/");
            }
        }
    }
    parts.join("/")
}

fn parse_portable_relative_path(stored_path: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for part in stored_path.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        out.push(part);
    }
    out
}

fn find_info_plist<R: Read + std::io::Seek>(zip: &mut ZipArchive<R>) -> Result<String> {
    let mut candidates = Vec::new();
    for i in 0..zip.len() {
        let name = zip
            .by_index(i)
            .with_context(|| format!("failed to access zip entry #{i}"))?
            .name()
            .to_string();
        if name.starts_with("Payload/") && name.ends_with(".app/Info.plist") {
            candidates.push(name);
        }
    }

    candidates.sort();
    candidates
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("did not find Payload/*.app/Info.plist inside IPA"))
}

fn read_entry_bytes<R: Read + std::io::Seek>(zip: &mut ZipArchive<R>, path: &str) -> Result<Vec<u8>> {
    let mut entry = zip
        .by_name(path)
        .with_context(|| format!("zip entry not found: {path}"))?;
    let mut data = Vec::new();
    entry
        .read_to_end(&mut data)
        .with_context(|| format!("failed to read zip entry: {path}"))?;
    Ok(data)
}

fn get_plist_string(dict: &plist::Dictionary, key: &str) -> Option<String> {
    dict.get(key)
        .and_then(|value| value.as_string())
        .map(|s| s.to_string())
}
