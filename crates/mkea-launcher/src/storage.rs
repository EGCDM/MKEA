use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const MAX_RECENT_LAUNCHES: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LauncherPrefs {
    pub window_width: u32,
    pub window_height: u32,
    #[serde(default = "default_true")]
    pub auto_window_orientation: bool,
    pub max_instructions: u64,
    #[serde(default = "default_runtime_mode")]
    pub runtime_mode: String,
    #[serde(default = "default_execution_backend")]
    pub execution_backend: String,
    pub input_flip_y: bool,
    pub synthetic_network_faults: bool,
    pub runloop_ticks: u32,
    pub close_when_finished: bool,
    #[serde(default = "default_true")]
    pub prefer_release_runtime: bool,
    #[serde(default)]
    pub player_exe_override: Option<String>,
}

fn default_runtime_mode() -> String {
    "strict".to_string()
}

fn default_execution_backend() -> String {
    "unicorn".to_string()
}

impl Default for LauncherPrefs {
    fn default() -> Self {
        Self {
            window_width: 640,
            window_height: 960,
            auto_window_orientation: true,
            max_instructions: 150_000_000,
            runtime_mode: default_runtime_mode(),
            execution_backend: default_execution_backend(),
            input_flip_y: false,
            synthetic_network_faults: false,
            runloop_ticks: 1_000_000,
            close_when_finished: false,
            prefer_release_runtime: true,
            player_exe_override: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LibraryDb {
    pub version: u32,
    #[serde(default)]
    pub preferences: LauncherPrefs,
    #[serde(default)]
    pub recent_launches: Vec<RecentLaunch>,
    #[serde(default)]
    pub apps: Vec<InstalledApp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledApp {
    pub id: String,
    pub display_name: String,
    pub bundle_id: String,
    pub executable: String,
    pub minimum_ios_version: String,
    pub chosen_arch: String,
    pub source_ipa_path: String,
    pub manifest_path: String,
    pub install_dir: String,
    pub installed_at_unix: u64,
    pub last_played_at_unix: Option<u64>,
    pub last_run_log_path: Option<String>,
}

impl InstalledApp {
    pub fn manifest_pathbuf(&self) -> PathBuf {
        PathBuf::from(&self.manifest_path)
    }

    pub fn install_dir_pathbuf(&self) -> PathBuf {
        PathBuf::from(&self.install_dir)
    }

    pub fn source_ipa_pathbuf(&self) -> PathBuf {
        PathBuf::from(&self.source_ipa_path)
    }

    pub fn exists(&self) -> bool {
        self.manifest_pathbuf().is_file()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentLaunch {
    pub app_id: String,
    pub display_name: String,
    pub launched_at_unix: u64,
    pub log_path: String,
}

impl RecentLaunch {
    pub fn log_pathbuf(&self) -> PathBuf {
        PathBuf::from(&self.log_path)
    }

    pub fn exists(&self) -> bool {
        self.log_pathbuf().is_file()
    }
}

#[derive(Debug, Clone)]
pub struct DataPaths {
    pub root: PathBuf,
    pub apps_dir: PathBuf,
    pub library_file: PathBuf,
}

impl DataPaths {
    pub fn discover() -> Result<Self> {
        let root = dirs::data_local_dir()
            .unwrap_or_else(fallback_data_root)
            .join("mkEA");
        let apps_dir = root.join("apps");
        let library_file = root.join("library.json");
        Ok(Self {
            root,
            apps_dir,
            library_file,
        })
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.apps_dir)
            .with_context(|| format!("failed to create app library dir: {}", self.apps_dir.display()))?;
        Ok(())
    }

    pub fn app_dir_for(&self, app_id: &str) -> PathBuf {
        self.apps_dir.join(app_id)
    }
}

pub fn load_library(paths: &DataPaths) -> Result<LibraryDb> {
    paths.ensure()?;
    if !paths.library_file.is_file() {
        let db = LibraryDb {
            version: 3,
            preferences: LauncherPrefs::default(),
            recent_launches: Vec::new(),
            apps: Vec::new(),
        };
        save_library(paths, &db)?;
        return Ok(db);
    }

    let bytes = fs::read(&paths.library_file)
        .with_context(|| format!("failed to read library file: {}", paths.library_file.display()))?;
    let mut db: LibraryDb = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse library file: {}", paths.library_file.display()))?;

    if db.version < 3 {
        db.version = 3;
    }
    db.recent_launches.retain(|entry| !entry.log_path.trim().is_empty());
    trim_recent_launches(&mut db.recent_launches);
    Ok(db)
}

pub fn save_library(paths: &DataPaths, db: &LibraryDb) -> Result<()> {
    paths.ensure()?;
    let text = serde_json::to_vec_pretty(db)?;
    fs::write(&paths.library_file, text)
        .with_context(|| format!("failed to write library file: {}", paths.library_file.display()))?;
    Ok(())
}

pub fn push_recent_launch(db: &mut LibraryDb, launch: RecentLaunch) {
    db.recent_launches.retain(|entry| entry.log_path != launch.log_path);
    db.recent_launches.insert(0, launch);
    trim_recent_launches(&mut db.recent_launches);
}

pub fn trim_recent_launches(entries: &mut Vec<RecentLaunch>) {
    if entries.len() > MAX_RECENT_LAUNCHES {
        entries.truncate(MAX_RECENT_LAUNCHES);
    }
}

pub fn safe_app_id(bundle_id: &str, display_name: &str) -> String {
    let source = if !bundle_id.trim().is_empty() {
        bundle_id.trim()
    } else if !display_name.trim().is_empty() {
        display_name.trim()
    } else {
        "app"
    };

    let mut out = String::with_capacity(source.len());
    let mut last_dash = false;
    for ch in source.chars() {
        let keep = ch.is_ascii_alphanumeric();
        if keep {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        format!("app-{}", now_unix_secs())
    } else {
        out
    }
}

pub fn ensure_unique_app_id(paths: &DataPaths, desired: &str) -> String {
    let base = if desired.trim().is_empty() {
        format!("app-{}", now_unix_secs())
    } else {
        desired.trim().to_string()
    };
    let mut candidate = base.clone();
    let mut index = 2_u32;
    while paths.app_dir_for(&candidate).exists() {
        candidate = format!("{base}-{index}");
        index = index.saturating_add(1);
    }
    candidate
}

pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

pub fn format_unix_time(ts: Option<u64>) -> String {
    match ts {
        Some(value) if value > 0 => format!("{}", value),
        _ => "—".to_string(),
    }
}

pub fn open_in_shell(path: &Path) -> Result<()> {
    if cfg!(target_os = "windows") {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .with_context(|| format!("failed to open in Explorer: {}", path.display()))?;
        return Ok(());
    }
    if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .with_context(|| format!("failed to open in Finder: {}", path.display()))?;
        return Ok(());
    }
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .with_context(|| format!("failed to open in file manager: {}", path.display()))?;
    Ok(())
}

fn fallback_data_root() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("mkEA-data")
}

fn default_true() -> bool {
    true
}
