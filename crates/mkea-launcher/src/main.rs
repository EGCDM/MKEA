#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod storage;

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use eframe::{egui, NativeOptions};
use storage::{
    ensure_unique_app_id, format_unix_time, load_library, now_unix_secs, open_in_shell,
    push_recent_launch, safe_app_id, save_library, DataPaths, InstalledApp, LibraryDb,
    RecentLaunch,
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() -> Result<()> {
    let native = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1340.0, 820.0])
            .with_min_inner_size([1120.0, 640.0])
            .with_title("mkEA Launcher"),
        ..Default::default()
    };

    let app = LauncherApp::boot()?;

    eframe::run_native(
        "mkEA Launcher",
        native,
        Box::new(move |_cc| Ok(Box::new(app))),
    )
    .map_err(|err| anyhow!(err.to_string()))
}

struct LauncherApp {
    paths: DataPaths,
    db: LibraryDb,
    selected: Option<usize>,
    status: String,
    launcher_exe_path: PathBuf,
    detected_player_exe: Option<PathBuf>,
    detected_build_flavor: String,
    log_view_path: Option<PathBuf>,
    log_view_title: String,
    log_view_text: String,
}

impl LauncherApp {
    fn boot() -> Result<Self> {
        let paths = DataPaths::discover()?;
        let mut db = load_library(&paths)?;
        sort_apps(&mut db.apps);

        let launcher_exe_path = std::env::current_exe().context("failed to resolve launcher executable path")?;
        let detected_build_flavor = detect_build_flavor(&launcher_exe_path).to_string();
        let detected_player_exe = detect_player_exe(&launcher_exe_path, &db.preferences);

        let mut status = String::from("Готово. Можно импортировать IPA в библиотеку.");
        if detected_build_flavor != "release" {
            status = format!(
                "Launcher запущен не из release ({detected_build_flavor}). Для нормального standalone используй target/release или portable bundle."
            );
        } else if detected_player_exe.is_none() {
            status = String::from(
                "Launcher собран, но mkea_player рядом не найден. Собери player bin или сделай portable bundle.",
            );
        }

        Ok(Self {
            paths,
            db,
            selected: None,
            status,
            launcher_exe_path,
            detected_player_exe,
            detected_build_flavor,
            log_view_path: None,
            log_view_title: "Встроенный просмотрщик логов".to_string(),
            log_view_text: "Лог пока не открыт. Выбери игру и нажми “Показать лог в launcher”.".to_string(),
        })
    }

    fn selected_app(&self) -> Option<&InstalledApp> {
        self.selected.and_then(|index| self.db.apps.get(index))
    }

    fn selected_app_id(&self) -> Option<String> {
        self.selected_app().map(|app| app.id.clone())
    }

    fn select_app_by_id(&mut self, app_id: &str) {
        self.selected = self.db.apps.iter().position(|app| app.id == app_id);
    }

    fn save_db(&mut self) {
        match save_library(&self.paths, &self.db) {
            Ok(()) => {}
            Err(err) => {
                self.status = format!("Не удалось сохранить library.json: {err:#}");
            }
        }
    }

    fn set_status_ok(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
    }

    fn set_status_err(&mut self, err: impl Into<String>) {
        self.status = err.into();
    }

    fn refresh_runtime_detection(&mut self) {
        self.detected_player_exe = detect_player_exe(&self.launcher_exe_path, &self.db.preferences);
        self.detected_build_flavor = detect_build_flavor(&self.launcher_exe_path).to_string();
    }

    fn refresh(&mut self) {
        let keep_app_id = self.selected_app_id();
        match load_library(&self.paths) {
            Ok(mut db) => {
                sort_apps(&mut db.apps);
                self.db = db;
                if let Some(app_id) = keep_app_id {
                    self.select_app_by_id(&app_id);
                } else if !self.db.apps.is_empty() && self.selected.is_none() {
                    self.selected = Some(0);
                }
                self.refresh_runtime_detection();
                self.set_status_ok("Библиотека обновлена.");
            }
            Err(err) => self.set_status_err(format!("Не удалось перечитать библиотеку: {err:#}")),
        }
    }

    fn import_ipa(&mut self) {
        let Some(source_ipa) = rfd::FileDialog::new()
            .add_filter("iPhone application", &["ipa"])
            .set_title("Выбери IPA для установки в mkEA Launcher")
            .pick_file()
        else {
            return;
        };

        match self.install_from_ipa(&source_ipa) {
            Ok(new_id) => {
                self.select_app_by_id(&new_id);
                self.set_status_ok(format!("Игра импортирована: {}", source_ipa.display()));
            }
            Err(err) => self.set_status_err(format!("Импорт IPA провалился: {err:#}")),
        }
    }

    fn install_from_ipa(&mut self, source_ipa: &Path) -> Result<String> {
        self.paths.ensure()?;

        let probe = mkea_loader::inspect_ipa(source_ipa)
            .with_context(|| format!("failed to inspect ipa: {}", source_ipa.display()))?;

        let desired_id = safe_app_id(&probe.manifest.bundle_id, &probe.manifest.bundle_name);
        let app_id = ensure_unique_app_id(&self.paths, &desired_id);
        let app_dir = self.paths.app_dir_for(&app_id);
        let source_dir = app_dir.join("source");
        let build_dir = app_dir.join("build");
        let runs_dir = app_dir.join("runs");

        fs::create_dir_all(&source_dir)
            .with_context(|| format!("failed to create source dir: {}", source_dir.display()))?;
        fs::create_dir_all(&runs_dir)
            .with_context(|| format!("failed to create runs dir: {}", runs_dir.display()))?;

        let source_name = source_ipa
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("game.ipa"));
        let copied_ipa = source_dir.join(source_name);
        fs::copy(source_ipa, &copied_ipa).with_context(|| {
            format!(
                "failed to copy ipa into launcher library: {} -> {}",
                source_ipa.display(),
                copied_ipa.display()
            )
        })?;

        let artifact = mkea_loader::write_build_artifacts(&copied_ipa, "armv6", &build_dir).with_context(|| {
            format!(
                "failed to generate build artifacts for copied ipa: {}",
                copied_ipa.display()
            )
        })?;

        let entry = InstalledApp {
            id: app_id.clone(),
            display_name: effective_display_name(&artifact.probe.manifest.bundle_name, &artifact.probe.manifest.executable),
            bundle_id: artifact.probe.manifest.bundle_id.clone(),
            executable: artifact.probe.manifest.executable.clone(),
            minimum_ios_version: artifact.probe.manifest.minimum_ios_version.clone(),
            chosen_arch: artifact.probe.manifest.chosen_arch.clone(),
            source_ipa_path: copied_ipa.display().to_string(),
            manifest_path: build_dir.join("manifest.json").display().to_string(),
            install_dir: app_dir.display().to_string(),
            installed_at_unix: now_unix_secs(),
            last_played_at_unix: None,
            last_run_log_path: None,
        };

        self.db.apps.push(entry);
        sort_apps(&mut self.db.apps);
        self.save_db();
        Ok(app_id)
    }

    fn rebuild_selected(&mut self) {
        let Some(app) = self.selected_app().cloned() else {
            self.set_status_err("Сначала выбери игру в библиотеке.");
            return;
        };

        let build_dir = app.install_dir_pathbuf().join("build");
        if let Err(err) = fs::remove_dir_all(&build_dir) {
            if err.kind() != std::io::ErrorKind::NotFound {
                self.set_status_err(format!("Не удалось очистить build/: {err:#}"));
                return;
            }
        }

        match mkea_loader::write_build_artifacts(&app.source_ipa_pathbuf(), "armv6", &build_dir) {
            Ok(_) => {
                self.refresh();
                self.set_status_ok(format!("Rebuild завершён: {}", app.display_name));
            }
            Err(err) => self.set_status_err(format!("Rebuild провалился: {err:#}")),
        }
    }

    fn launch_selected(&mut self) {
        let selected_index = match self.selected {
            Some(value) => value,
            None => {
                self.set_status_err("Сначала выбери игру в библиотеке.");
                return;
            }
        };

        let app = match self.db.apps.get(selected_index).cloned() {
            Some(value) => value,
            None => {
                self.set_status_err("Выбранная запись библиотеки исчезла.");
                return;
            }
        };

        match self.spawn_player(&app) {
            Ok(log_path) => {
                let ts = now_unix_secs();
                if let Some(entry) = self.db.apps.get_mut(selected_index) {
                    entry.last_played_at_unix = Some(ts);
                    entry.last_run_log_path = Some(log_path.display().to_string());
                }
                push_recent_launch(
                    &mut self.db,
                    RecentLaunch {
                        app_id: app.id.clone(),
                        display_name: app.display_name.clone(),
                        launched_at_unix: ts,
                        log_path: log_path.display().to_string(),
                    },
                );
                self.save_db();
                let _ = self.show_log_in_viewer(&log_path, Some(format!("Лог запуска: {}", app.display_name)));
                self.set_status_ok(format!("Запуск пошёл. Лог: {}", log_path.display()));
            }
            Err(err) => self.set_status_err(format!("Не удалось запустить игру: {err:#}")),
        }
    }

    fn resolve_launch_window_dims(&self, app: &InstalledApp) -> Result<(u32, u32)> {
        let prefs = &self.db.preferences;
        let manual = (prefs.window_width.max(1), prefs.window_height.max(1));
        if !prefs.auto_window_orientation {
            return Ok(manual);
        }

        let loaded = mkea_loader::load_build_artifact(&app.manifest_pathbuf())
            .with_context(|| format!("failed to inspect build manifest for {}", app.display_name))?;
        let mut orientation = loaded.probe.manifest.plist_orientation_hint();
        let mut surface = loaded.probe.manifest.surface_size_hint();
        if (orientation.is_none() || surface.is_none()) && loaded.bundle_root.is_some() {
            if let Some(bundle_root) = loaded.bundle_root.as_ref() {
                let profile = mkea_loader::infer_bundle_display_profile(
                    bundle_root,
                    &loaded.probe.manifest.supported_interface_orientations,
                );
                if orientation.is_none() {
                    orientation = profile.preferred_orientation;
                }
                if surface.is_none() {
                    surface = profile.surface_size();
                }
            }
        }

        let Some((surface_w, surface_h)) = surface else {
            return Ok(manual);
        };
        let surface_w = surface_w.max(1);
        let surface_h = surface_h.max(1);
        let (bound_w, bound_h) = match orientation {
            Some(mkea_loader::DisplayOrientation::Landscape) if prefs.window_height > prefs.window_width => {
                (prefs.window_height.max(1), prefs.window_width.max(1))
            }
            Some(mkea_loader::DisplayOrientation::Portrait) if prefs.window_width > prefs.window_height => {
                (prefs.window_height.max(1), prefs.window_width.max(1))
            }
            _ => manual,
        };
        Ok(scale_surface_to_fit(surface_w, surface_h, bound_w, bound_h))
    }

    fn spawn_player(&mut self, app: &InstalledApp) -> Result<PathBuf> {
        let manifest = app.manifest_pathbuf();
        if !manifest.is_file() {
            return Err(anyhow!("manifest.json не найден: {}", manifest.display()));
        }

        let player_exe = self.resolve_player_exe()?;
        let runs_dir = app.install_dir_pathbuf().join("runs");
        fs::create_dir_all(&runs_dir)
            .with_context(|| format!("failed to create runs dir: {}", runs_dir.display()))?;

        let ts = now_unix_secs();
        let run_stem = format!("run-{ts}");
        let log_path = runs_dir.join(format!("{run_stem}.log"));
        let report_path = runs_dir.join(format!("{run_stem}.json"));
        let input_path = runs_dir.join(format!("{run_stem}.input.jsonl"));
        let stdout_file = File::create(&log_path)
            .with_context(|| format!("failed to create run log: {}", log_path.display()))?;
        let stderr_file = stdout_file
            .try_clone()
            .with_context(|| format!("failed to duplicate run log handle: {}", log_path.display()))?;

        let prefs = &self.db.preferences;
        let (window_width, window_height) = self.resolve_launch_window_dims(app)?;
        let mut command = Command::new(&player_exe);
        command
            .arg(&manifest)
            .arg("--title")
            .arg(&app.display_name)
            .arg("--window-width")
            .arg(window_width.to_string())
            .arg("--window-height")
            .arg(window_height.to_string())
            .arg("--max-instructions")
            .arg(prefs.max_instructions.to_string())
            .arg("--runtime-mode")
            .arg(&prefs.runtime_mode)
            .arg("--backend")
            .arg(&prefs.execution_backend)
            .arg("--runloop-ticks")
            .arg(prefs.runloop_ticks.to_string())
            .arg("--out")
            .arg(&report_path)
            .arg("--input-script")
            .arg(&input_path)
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file));

        if prefs.synthetic_network_faults {
            command.arg("--synthetic-network-faults");
        }
        if prefs.input_flip_y {
            command.arg("--input-flip-y");
        }
        if prefs.close_when_finished {
            command.arg("--close-when-finished");
        }

        #[cfg(target_os = "windows")]
        {
            command.creation_flags(CREATE_NO_WINDOW);
        }

        command.spawn().with_context(|| {
            format!(
                "failed to spawn player process: {} for manifest {}",
                player_exe.display(),
                manifest.display()
            )
        })?;

        Ok(log_path)
    }

    fn resolve_player_exe(&mut self) -> Result<PathBuf> {
        self.refresh_runtime_detection();
        if let Some(path) = &self.detected_player_exe {
            return Ok(path.clone());
        }

        let tried = candidate_player_paths(&self.launcher_exe_path, &self.db.preferences)
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");

        Err(anyhow!(
            "mkea_player не найден. Проверь portable bundle или release build.\nПроверенные пути:\n{tried}"
        ))
    }

    fn remove_selected(&mut self) {
        let Some(index) = self.selected else {
            self.set_status_err("Нечего удалять: игра не выбрана.");
            return;
        };
        let Some(app) = self.db.apps.get(index).cloned() else {
            self.set_status_err("Выбранная запись библиотеки исчезла.");
            return;
        };

        match fs::remove_dir_all(app.install_dir_pathbuf()) {
            Ok(()) => {
                self.db.apps.remove(index);
                self.db.recent_launches.retain(|entry| entry.app_id != app.id);
                if self.db.apps.is_empty() {
                    self.selected = None;
                } else if index >= self.db.apps.len() {
                    self.selected = Some(self.db.apps.len().saturating_sub(1));
                }
                self.save_db();
                self.set_status_ok(format!("Удалено: {}", app.display_name));
            }
            Err(err) => self.set_status_err(format!("Не удалось удалить игру: {err:#}")),
        }
    }

    fn open_data_dir(&mut self) {
        match open_in_shell(&self.paths.root) {
            Ok(()) => self.set_status_ok(format!("Открыл папку данных: {}", self.paths.root.display())),
            Err(err) => self.set_status_err(format!("Не удалось открыть папку данных: {err:#}")),
        }
    }

    fn open_selected_dir(&mut self) {
        let Some(app) = self.selected_app().cloned() else {
            self.set_status_err("Сначала выбери игру в библиотеке.");
            return;
        };
        match open_in_shell(&app.install_dir_pathbuf()) {
            Ok(()) => self.set_status_ok(format!("Открыл папку игры: {}", app.install_dir)),
            Err(err) => self.set_status_err(format!("Не удалось открыть папку игры: {err:#}")),
        }
    }

    fn open_selected_log(&mut self) {
        let Some(app) = self.selected_app().cloned() else {
            self.set_status_err("Сначала выбери игру в библиотеке.");
            return;
        };
        let Some(path) = app.last_run_log_path.as_ref().cloned() else {
            self.set_status_err("У этой игры ещё нет сохранённого лога запуска.");
            return;
        };
        match open_in_shell(Path::new(&path)) {
            Ok(()) => self.set_status_ok(format!("Открыл лог: {path}")),
            Err(err) => self.set_status_err(format!("Не удалось открыть лог запуска: {err:#}")),
        }
    }

    fn pick_player_override(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Выбери mkea_player.exe")
            .pick_file()
        else {
            return;
        };

        self.db.preferences.player_exe_override = Some(path.display().to_string());
        self.save_db();
        self.refresh_runtime_detection();
        self.set_status_ok(format!("Путь к player сохранён: {}", path.display()));
    }

    fn clear_player_override(&mut self) {
        self.db.preferences.player_exe_override = None;
        self.save_db();
        self.refresh_runtime_detection();
        self.set_status_ok("Ручной путь к player очищен. Снова ищем sibling/runtime рядом с launcher.".to_string());
    }

    fn show_log_in_viewer(&mut self, path: &Path, title: Option<String>) -> Result<()> {
        let mut text = fs::read_to_string(path)
            .with_context(|| format!("failed to read launcher log for viewer: {}", path.display()))?;
        if let Some(summary) = render_report_summary_for_log(path) {
            text = format!("{}

===== raw log =====
{}", summary, text);
        }
        self.log_view_path = Some(path.to_path_buf());
        self.log_view_title = title.unwrap_or_else(|| format!("Лог: {}", path.display()));
        self.log_view_text = text;
        Ok(())
    }

    fn reload_log_in_viewer(&mut self) {
        let Some(path) = self.log_view_path.clone() else {
            self.set_status_err("Во встроенном просмотрщике пока нет открытого лога.");
            return;
        };
        match self.show_log_in_viewer(&path, None) {
            Ok(()) => self.set_status_ok(format!("Лог обновлён: {}", path.display())),
            Err(err) => self.set_status_err(format!("Не удалось обновить лог: {err:#}")),
        }
    }

    fn show_selected_log_in_viewer(&mut self) {
        let Some(app) = self.selected_app().cloned() else {
            self.set_status_err("Сначала выбери игру в библиотеке.");
            return;
        };
        let Some(path) = app.last_run_log_path.clone() else {
            self.set_status_err("У этой игры ещё нет последнего лога.");
            return;
        };
        match self.show_log_in_viewer(Path::new(&path), Some(format!("Лог запуска: {}", app.display_name))) {
            Ok(()) => self.set_status_ok(format!("Лог открыт во встроенном просмотрщике: {path}")),
            Err(err) => self.set_status_err(format!("Не удалось открыть лог во встроенном просмотрщике: {err:#}")),
        }
    }
}


fn report_path_for_log(log_path: &Path) -> PathBuf {
    let file_name = log_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if let Some(stem) = file_name.strip_suffix(".log") {
        return log_path.with_file_name(format!("{stem}.json"));
    }
    log_path.with_extension("json")
}

fn render_report_summary_for_log(log_path: &Path) -> Option<String> {
    let report_path = report_path_for_log(log_path);
    let text = fs::read_to_string(&report_path).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;

    let live = root.get("live")?;
    let runtime = root.get("runtime")?;
    let runtime_state = runtime.get("runtime_state");

    let final_state = str_at(live, &["final_state"]).unwrap_or("<unknown>");
    let close_requested = bool_at(live, &["close_requested"]).unwrap_or(false);
    let last_frame_seen = u64_at(live, &["last_frame_index_seen"]);

    let executed = u64_at(runtime, &["executed_instructions"]);
    let stop_reason = str_at(runtime, &["stop_reason"]).unwrap_or("<missing>");
    let status = str_at(runtime, &["status"]).unwrap_or("<missing>");
    let final_pc = hex_opt(u64_at(runtime, &["final_pc"]));
    let final_sp = hex_opt(u64_at(runtime, &["final_sp"]));

    let (ui_launch_count, ui_window_visible, ui_app_active, ui_scene, ui_animation_running) = if let Some(state) = runtime_state {
        (
            u64_at(state, &["ui", "launch_count"]),
            bool_at(state, &["ui", "window_visible"]),
            bool_at(state, &["ui", "app_active"]),
            hex_opt(u64_at(state, &["ui", "running_scene"])),
            bool_at(state, &["ui", "animation_running"]),
        )
    } else {
        (None, None, None, None, None)
    };

    let (runloop_ticks, runloop_sources, idle_ticks) = if let Some(state) = runtime_state {
        (
            u64_at(state, &["runloop", "ticks"]),
            u64_at(state, &["runloop", "sources"]),
            u64_at(state, &["runloop", "idle_ticks_after_completion"]),
        )
    } else {
        (None, None, None)
    };

    let (input_queued, input_consumed, input_ui_dispatched, input_cocos_dispatched, input_last_phase, input_last_dispatch, input_last_source) = if let Some(state) = runtime_state {
        (
            u64_at(state, &["input", "queued"]),
            u64_at(state, &["input", "consumed"]),
            u64_at(state, &["input", "ui_dispatched"]),
            u64_at(state, &["input", "cocos_dispatched"]),
            str_at(state, &["input", "last_phase"]),
            str_at(state, &["input", "last_dispatch"]),
            str_at(state, &["input", "last_source"]),
        )
    } else {
        (None, None, None, None, None, None, None)
    };

    let (gfx_presented, gfx_frame_index, gfx_present_calls, gfx_draw_calls, gfx_clear_calls, gfx_readback_calls, gfx_last_source, gfx_last_decision, gfx_retained, gfx_changed, gfx_stable_streak, gfx_dominant_rgba, gfx_dominant_pct_milli, gfx_unique_frames, gfx_last_unique_dump, gfx_diag_hint) = if let Some(state) = runtime_state {
        (
            bool_at(state, &["graphics", "presented"]),
            u64_at(state, &["graphics", "frame_index"]),
            u64_at(state, &["graphics", "present_calls"]),
            u64_at(state, &["graphics", "draw_calls"]),
            u64_at(state, &["graphics", "clear_calls"]),
            u64_at(state, &["graphics", "readback_calls"]),
            str_at(state, &["graphics", "last_present_source"]),
            str_at(state, &["graphics", "last_present_decision"]),
            u64_at(state, &["graphics", "retained_present_calls"]),
            bool_at(state, &["graphics", "readback_changed"]),
            u64_at(state, &["graphics", "readback_stable_streak"]),
            str_at(state, &["graphics", "dominant_rgba"]),
            u64_at(state, &["graphics", "dominant_pct_milli"]),
            u64_at(state, &["graphics", "unique_frames_saved"]),
            str_at(state, &["graphics", "last_unique_dump_path"]),
            str_at(state, &["graphics", "diagnosis_hint"]),
        )
    } else {
        (None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None)
    };

    let (scene_transitions, scene_run_with, scene_replace, scene_push, scene_on_enter, scene_on_exit, scene_on_finish, scene_ticks, scene_probe_attempts, scene_probe_fired, scene_recent) = if let Some(state) = runtime_state {
        (
            u64_at(state, &["scene", "transition_calls"]),
            u64_at(state, &["scene", "run_with_scene_calls"]),
            u64_at(state, &["scene", "replace_scene_calls"]),
            u64_at(state, &["scene", "push_scene_calls"]),
            u64_at(state, &["scene", "on_enter_events"]),
            u64_at(state, &["scene", "on_exit_events"]),
            u64_at(state, &["scene", "on_enter_transition_finish_events"]),
            u64_at(state, &["scene", "running_scene_ticks"]),
            u64_at(state, &["scene", "menu_probe_attempts"]),
            bool_at(state, &["scene", "menu_probe_fired"]),
            join_string_array(state, &["scene", "recent_events"], 5),
        )
    } else {
        (None, None, None, None, None, None, None, None, None, None, None)
    };

    let (sched_mainloop, sched_draw_scene, sched_draw_frame, sched_schedule, sched_update, sched_invalidate, sched_render_cb, sched_recent) = if let Some(state) = runtime_state {
        (
            u64_at(state, &["scheduler", "mainloop_calls"]),
            u64_at(state, &["scheduler", "draw_scene_calls"]),
            u64_at(state, &["scheduler", "draw_frame_calls"]),
            u64_at(state, &["scheduler", "schedule_calls"]),
            u64_at(state, &["scheduler", "update_calls"]),
            u64_at(state, &["scheduler", "invalidate_calls"]),
            u64_at(state, &["scheduler", "render_callback_calls"]),
            join_string_array(state, &["scheduler", "recent_events"], 5),
        )
    } else {
        (None, None, None, None, None, None, None, None)
    };

    let (
        ui_tree_running,
        ui_tree_total,
        ui_tree_reachable,
        ui_tree_visible,
        ui_tree_entered,
        ui_tree_textured,
        ui_tree_textureless_visible,
        ui_tree_menus,
        ui_tree_menu_items,
        ui_tree_labels,
        ui_tree_touch,
        ui_tree_callbacks,
        ui_tree_samples,
    ) = if let Some(state) = runtime_state {
        (
            hex_opt(u64_at(state, &["ui_tree", "running_scene"])),
            u64_at(state, &["ui_tree", "total_nodes"]),
            u64_at(state, &["ui_tree", "reachable_nodes"]),
            u64_at(state, &["ui_tree", "visible_nodes"]),
            u64_at(state, &["ui_tree", "entered_nodes"]),
            u64_at(state, &["ui_tree", "textured_nodes"]),
            u64_at(state, &["ui_tree", "visible_textureless_nodes"]),
            u64_at(state, &["ui_tree", "menu_nodes"]),
            u64_at(state, &["ui_tree", "menu_item_nodes"]),
            u64_at(state, &["ui_tree", "label_nodes"]),
            u64_at(state, &["ui_tree", "touch_enabled_nodes"]),
            u64_at(state, &["ui_tree", "callback_nodes"]),
            format_ui_tree_samples(state, &["ui_tree", "sampled_nodes"], 12),
        )
    } else {
        (None, None, None, None, None, None, None, None, None, None, None, None, None)
    };

    let (
        fs_indexed_files,
        fs_cached_images,
        fs_image_named_hits,
        fs_image_named_misses,
        fs_file_open_hits,
        fs_file_open_misses,
        fs_last_resource,
        fs_last_resource_path,
    ) = if let Some(state) = runtime_state {
        (
            u64_at(state, &["filesystem", "indexed_files"]),
            u64_at(state, &["filesystem", "cached_images"]),
            u64_at(state, &["filesystem", "image_named_hits"]),
            u64_at(state, &["filesystem", "image_named_misses"]),
            u64_at(state, &["filesystem", "file_open_hits"]),
            u64_at(state, &["filesystem", "file_open_misses"]),
            str_at(state, &["filesystem", "last_resource_name"]),
            str_at(state, &["filesystem", "last_resource_path"]),
        )
    } else {
        (None, None, None, None, None, None, None, None)
    };

    let (
        hot_objc_msgsend,
        hot_objc_unique,
        hot_recent_objc,
        hot_top_objc,
        hot_recent_gl,
        hot_top_gl,
        hot_saw_image_named,
        hot_saw_gl_draw_arrays,
        hot_saw_gl_draw_elements,
    ) = if let Some(state) = runtime_state {
        (
            u64_at(state, &["hot_path", "objc_msgsend_calls"]),
            u64_at(state, &["hot_path", "objc_unique_selectors"]),
            join_string_array(state, &["hot_path", "recent_objc_selectors"], 8),
            format_count_entries(state, &["hot_path", "top_objc_selectors"], 8),
            join_string_array(state, &["hot_path", "recent_gl_calls"], 8),
            format_count_entries(state, &["hot_path", "top_gl_calls"], 6),
            bool_at(state, &["hot_path", "saw_image_named"]),
            bool_at(state, &["hot_path", "saw_gl_draw_arrays"]),
            bool_at(state, &["hot_path", "saw_gl_draw_elements"]),
        )
    } else {
        (None, None, None, None, None, None, None, None, None)
    };

    let (obs_trace_build_id, obs_trace_banner_emitted, obs_scene_progress, obs_sprite_watch, obs_scheduler_trace, obs_scheduler_live, obs_callback_trace) = if let Some(state) = runtime_state {
        (
            str_at(state, &["observability", "trace_build_id"]),
            bool_at(state, &["observability", "trace_banner_emitted"]),
            join_string_array(state, &["observability", "scene_progress_trace"], 16),
            join_string_array(state, &["observability", "sprite_watch_trace"], 16),
            join_string_array(state, &["observability", "scheduler_trace"], 16),
            str_at(state, &["observability", "scheduler_trace_live_snapshot"]),
            join_string_array(state, &["observability", "callback_trace"], 16),
        )
    } else {
        (None, None, None, None, None, None, None)
    };

    let (net_state, net_stage, net_events, net_callbacks, net_completed, net_faulted, net_retry, net_error, net_bindings, net_births, net_slots, net_owner_candidates, net_first_binding, net_last_owner) = if let Some(state) = runtime_state {
        (
            str_at(state, &["network", "state"]),
            u64_at(state, &["network", "stage"]),
            u64_at(state, &["network", "events"]),
            u64_at(state, &["network", "delegate_callbacks"]),
            bool_at(state, &["network", "completed"]),
            bool_at(state, &["network", "faulted"]),
            bool_at(state, &["network", "retry_recommended"]),
            str_at(state, &["network", "last_error_description"])
                .or_else(|| str_at(state, &["network", "last_error_kind"])),
            join_string_array(state, &["network", "delegate_binding_trace"], 12),
            join_string_array(state, &["network", "connection_birth_trace"], 12),
            join_string_array(state, &["network", "slot_trace"], 16),
            join_string_array(state, &["network", "owner_candidate_trace"], 12),
            str_at(state, &["network", "first_app_delegate_binding"]),
            str_at(state, &["network", "last_owner_candidate"]),
        )
    } else {
        (None, None, None, None, None, None, None, None, None, None, None, None, None, None)
    };

    let (
        audio_openal_device_open_calls,
        audio_openal_context_create_calls,
        audio_openal_make_current_calls,
        audio_openal_buffers_generated,
        audio_openal_sources_generated,
        audio_openal_buffer_upload_calls,
        audio_openal_bytes_uploaded,
        audio_openal_queue_calls,
        audio_openal_unqueue_calls,
        audio_openal_play_calls,
        audio_openal_stop_calls,
        audio_openal_last_buffer_format,
        audio_openal_last_source_state,
        audio_audioqueue_create_calls,
        audio_audioqueue_allocate_calls,
        audio_audioqueue_enqueue_calls,
        audio_audioqueue_enqueued_bytes,
        audio_audioqueue_prime_calls,
        audio_audioqueue_start_calls,
        audio_audioqueue_stop_calls,
        audio_audioqueue_dispose_calls,
        audio_audioqueue_output_callback_dispatches,
        audio_audioqueue_property_callback_dispatches,
        audio_audioqueue_last_format,
        audio_audioqueue_last_queue,
        audio_audioqueue_last_buffer,
        audio_audioqueue_last_buffer_preview_hex,
        audio_audioqueue_last_buffer_preview_ascii,
        audio_audiofile_open_calls,
        audio_audiofile_read_bytes_calls,
        audio_audiofile_read_packets_calls,
        audio_audiofile_bytes_served,
        audio_systemsound_create_calls,
        audio_systemsound_play_calls,
        audio_systemsound_dispose_calls,
        audio_objc_player_alloc_calls,
        audio_objc_player_init_url_calls,
        audio_objc_player_init_data_calls,
        audio_objc_player_prepare_calls,
        audio_objc_player_play_calls,
        audio_objc_player_pause_calls,
        audio_objc_player_stop_calls,
        audio_objc_player_set_volume_calls,
        audio_objc_player_set_loops_calls,
        audio_objc_engine_shared_calls,
        audio_objc_manager_shared_calls,
        audio_objc_manager_soundengine_calls,
        audio_objc_manager_soundengine_nil_results,
        audio_objc_engine_preload_calls,
        audio_objc_bgm_preload_calls,
        audio_objc_engine_play_calls,
        audio_objc_bgm_play_calls,
        audio_objc_engine_stop_calls,
        audio_objc_engine_effect_calls,
        audio_objc_engine_async_load_progress_calls,
        audio_objc_engine_async_load_progress_nil_receivers,
        audio_objc_engine_playsound_calls,
        audio_objc_engine_playsound_nil_receivers,
        audio_objc_fallback_dispatches,
        audio_objc_last_class,
        audio_objc_last_selector,
        audio_objc_last_resource,
        audio_objc_last_result,
        audio_unsupported_events,
        audio_recent,
    ) = if let Some(state) = runtime_state {
        (
            u64_at(state, &["audio", "openal_device_open_calls"]),
            u64_at(state, &["audio", "openal_context_create_calls"]),
            u64_at(state, &["audio", "openal_make_current_calls"]),
            u64_at(state, &["audio", "openal_buffers_generated"]),
            u64_at(state, &["audio", "openal_sources_generated"]),
            u64_at(state, &["audio", "openal_buffer_upload_calls"]),
            u64_at(state, &["audio", "openal_bytes_uploaded"]),
            u64_at(state, &["audio", "openal_queue_calls"]),
            u64_at(state, &["audio", "openal_unqueue_calls"]),
            u64_at(state, &["audio", "openal_play_calls"]),
            u64_at(state, &["audio", "openal_stop_calls"]),
            str_at(state, &["audio", "openal_last_buffer_format"]),
            str_at(state, &["audio", "openal_last_source_state"]),
            u64_at(state, &["audio", "audioqueue_create_calls"]),
            u64_at(state, &["audio", "audioqueue_allocate_calls"]),
            u64_at(state, &["audio", "audioqueue_enqueue_calls"]),
            u64_at(state, &["audio", "audioqueue_enqueued_bytes"]),
            u64_at(state, &["audio", "audioqueue_prime_calls"]),
            u64_at(state, &["audio", "audioqueue_start_calls"]),
            u64_at(state, &["audio", "audioqueue_stop_calls"]),
            u64_at(state, &["audio", "audioqueue_dispose_calls"]),
            u64_at(state, &["audio", "audioqueue_output_callback_dispatches"]),
            u64_at(state, &["audio", "audioqueue_property_callback_dispatches"]),
            str_at(state, &["audio", "audioqueue_last_format"]),
            hex_opt(u64_at(state, &["audio", "audioqueue_last_queue"])),
            hex_opt(u64_at(state, &["audio", "audioqueue_last_buffer"])),
            str_at(state, &["audio", "audioqueue_last_buffer_preview_hex"]),
            str_at(state, &["audio", "audioqueue_last_buffer_preview_ascii"]),
            u64_at(state, &["audio", "audiofile_open_calls"]),
            u64_at(state, &["audio", "audiofile_read_bytes_calls"]),
            u64_at(state, &["audio", "audiofile_read_packets_calls"]),
            u64_at(state, &["audio", "audiofile_bytes_served"]),
            u64_at(state, &["audio", "systemsound_create_calls"]),
            u64_at(state, &["audio", "systemsound_play_calls"]),
            u64_at(state, &["audio", "systemsound_dispose_calls"]),
            u64_at(state, &["audio", "objc_audio_player_alloc_calls"]),
            u64_at(state, &["audio", "objc_audio_player_init_url_calls"]),
            u64_at(state, &["audio", "objc_audio_player_init_data_calls"]),
            u64_at(state, &["audio", "objc_audio_player_prepare_calls"]),
            u64_at(state, &["audio", "objc_audio_player_play_calls"]),
            u64_at(state, &["audio", "objc_audio_player_pause_calls"]),
            u64_at(state, &["audio", "objc_audio_player_stop_calls"]),
            u64_at(state, &["audio", "objc_audio_player_set_volume_calls"]),
            u64_at(state, &["audio", "objc_audio_player_set_loops_calls"]),
            u64_at(state, &["audio", "objc_audio_engine_shared_calls"]),
            u64_at(state, &["audio", "objc_audio_manager_shared_calls"]),
            u64_at(state, &["audio", "objc_audio_manager_soundengine_calls"]),
            u64_at(state, &["audio", "objc_audio_manager_soundengine_nil_results"]),
            u64_at(state, &["audio", "objc_audio_engine_preload_calls"]),
            u64_at(state, &["audio", "objc_audio_bgm_preload_calls"]),
            u64_at(state, &["audio", "objc_audio_engine_play_calls"]),
            u64_at(state, &["audio", "objc_audio_bgm_play_calls"]),
            u64_at(state, &["audio", "objc_audio_engine_stop_calls"]),
            u64_at(state, &["audio", "objc_audio_engine_effect_calls"]),
            u64_at(state, &["audio", "objc_audio_engine_async_load_progress_calls"]),
            u64_at(state, &["audio", "objc_audio_engine_async_load_progress_nil_receivers"]),
            u64_at(state, &["audio", "objc_audio_engine_playsound_calls"]),
            u64_at(state, &["audio", "objc_audio_engine_playsound_nil_receivers"]),
            u64_at(state, &["audio", "objc_audio_fallback_dispatches"]),
            str_at(state, &["audio", "objc_audio_last_class"]),
            str_at(state, &["audio", "objc_audio_last_selector"]),
            str_at(state, &["audio", "objc_audio_last_resource"]),
            str_at(state, &["audio", "objc_audio_last_result"]),
            u64_at(state, &["audio", "unsupported_events"]),
            join_string_array(state, &["audio", "recent_events"], 12),
        )
    } else {
        (
            None, None, None, None, None, None, None, None, None, None, None,
            None, None, None, None, None, None, None, None, None, None, None,
            None, None, None, None, None, None, None, None, None, None, None,
            None, None, None, None, None, None, None, None, None, None, None,
            None, None, None, None, None, None, None, None, None, None, None,
            None, None, None, None, None, None, None, None, None, None,
        )
    };

    let diagnosis = diagnose_report(
        final_state,
        runloop_ticks,
        runloop_sources,
        input_consumed,
        gfx_present_calls,
        gfx_draw_calls,
        gfx_last_decision,
        net_state,
        net_completed,
        net_faulted,
        ui_tree_menu_items,
        ui_tree_labels,
        ui_tree_textureless_visible,
        ui_tree_samples.as_deref(),
        hot_top_objc.as_deref(),
        obs_scene_progress.as_deref(),
        obs_sprite_watch.as_deref(),
        obs_scheduler_trace.as_deref(),
        obs_callback_trace.as_deref(),
    );

    let mut out = String::new();
    out.push_str("===== runtime report summary =====
");
    out.push_str(&format!("report: {}
", report_path.display()));
    out.push_str(&format!("state: {} | status: {} | close_requested: {}
", final_state, status, yes_no(close_requested)));
    out.push_str(&format!(
        "stop_reason: {}
executed: {} | final_pc: {} | final_sp: {} | last_frame_seen: {}
",
        stop_reason,
        fmt_opt_u64(executed),
        final_pc.unwrap_or_else(|| "<none>".to_string()),
        final_sp.unwrap_or_else(|| "<none>".to_string()),
        fmt_opt_u64(last_frame_seen),
    ));
    out.push_str(&format!(
        "ui: launch_count={} window_visible={} app_active={} scene={} animation_running={}
",
        fmt_opt_u64(ui_launch_count),
        yes_no_opt(ui_window_visible),
        yes_no_opt(ui_app_active),
        ui_scene.unwrap_or_else(|| "<none>".to_string()),
        yes_no_opt(ui_animation_running),
    ));
    out.push_str(&format!(
        "runloop: ticks={} sources={} idle_after_completion={}
",
        fmt_opt_u64(runloop_ticks),
        fmt_opt_u64(runloop_sources),
        fmt_opt_u64(idle_ticks),
    ));
    out.push_str(&format!(
        "input: queued={} consumed={} ui_dispatched={} cocos_dispatched={} last_phase={} last_dispatch={} last_source={}
",
        fmt_opt_u64(input_queued),
        fmt_opt_u64(input_consumed),
        fmt_opt_u64(input_ui_dispatched),
        fmt_opt_u64(input_cocos_dispatched),
        input_last_phase.unwrap_or("<none>"),
        input_last_dispatch.unwrap_or("<none>"),
        input_last_source.unwrap_or("<none>"),
    ));
    out.push_str(&format!(
        "graphics: presented={} frame_index={} presents={} draws={} clears={} readbacks={} last_source={} last_decision={} retained={} changed={} stable_streak={} dominant={} dominant_pct={}‰ unique_frames={}
",
        yes_no_opt(gfx_presented),
        fmt_opt_u64(gfx_frame_index),
        fmt_opt_u64(gfx_present_calls),
        fmt_opt_u64(gfx_draw_calls),
        fmt_opt_u64(gfx_clear_calls),
        fmt_opt_u64(gfx_readback_calls),
        gfx_last_source.unwrap_or("<none>"),
        gfx_last_decision.unwrap_or("<none>"),
        fmt_opt_u64(gfx_retained),
        yes_no_opt(gfx_changed),
        fmt_opt_u64(gfx_stable_streak),
        gfx_dominant_rgba.unwrap_or("<none>"),
        fmt_opt_u64(gfx_dominant_pct_milli),
        fmt_opt_u64(gfx_unique_frames),
    ));
    out.push_str(&format!(
        "graphics_diag: hint={} unique_dump={}
",
        gfx_diag_hint.unwrap_or("<none>"),
        gfx_last_unique_dump.unwrap_or("<none>"),
    ));
    out.push_str(&format!(
        "scene: transitions={} runWith={} replace={} push={} onEnter={} onExit={} onFinish={} ticks={} menuProbe={} fired={} recent={}
",
        fmt_opt_u64(scene_transitions),
        fmt_opt_u64(scene_run_with),
        fmt_opt_u64(scene_replace),
        fmt_opt_u64(scene_push),
        fmt_opt_u64(scene_on_enter),
        fmt_opt_u64(scene_on_exit),
        fmt_opt_u64(scene_on_finish),
        fmt_opt_u64(scene_ticks),
        fmt_opt_u64(scene_probe_attempts),
        yes_no_opt(scene_probe_fired),
        scene_recent.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!(
        "scheduler: mainLoop={} drawScene={} drawFrame={} schedule={} update={} invalidate={} renderCb={} recent={}
",
        fmt_opt_u64(sched_mainloop),
        fmt_opt_u64(sched_draw_scene),
        fmt_opt_u64(sched_draw_frame),
        fmt_opt_u64(sched_schedule),
        fmt_opt_u64(sched_update),
        fmt_opt_u64(sched_invalidate),
        fmt_opt_u64(sched_render_cb),
        sched_recent.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!(
        "ui_tree: running={} total={} reachable={} visible={} entered={} textured={} textureless_visible={} menus={} menuItems={} labels={} touch={} callbacks={} samples={}
",
        ui_tree_running.unwrap_or_else(|| "<none>".to_string()),
        fmt_opt_u64(ui_tree_total),
        fmt_opt_u64(ui_tree_reachable),
        fmt_opt_u64(ui_tree_visible),
        fmt_opt_u64(ui_tree_entered),
        fmt_opt_u64(ui_tree_textured),
        fmt_opt_u64(ui_tree_textureless_visible),
        fmt_opt_u64(ui_tree_menus),
        fmt_opt_u64(ui_tree_menu_items),
        fmt_opt_u64(ui_tree_labels),
        fmt_opt_u64(ui_tree_touch),
        fmt_opt_u64(ui_tree_callbacks),
        ui_tree_samples.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!(
        "fs: indexed_files={} cached_images={} imageNamed(h/m)={}/{} fileOpen(h/m)={}/{} last_resource={} last_path={}
",
        fmt_opt_u64(fs_indexed_files),
        fmt_opt_u64(fs_cached_images),
        fmt_opt_u64(fs_image_named_hits),
        fmt_opt_u64(fs_image_named_misses),
        fmt_opt_u64(fs_file_open_hits),
        fmt_opt_u64(fs_file_open_misses),
        fs_last_resource.unwrap_or("<none>"),
        fs_last_resource_path.unwrap_or("<none>"),
    ));
    out.push_str(&format!(
        "trace_build_id: {} banner_emitted={}
",
        obs_trace_build_id.unwrap_or("<none>"),
        yes_no_opt(obs_trace_banner_emitted),
    ));
    out.push_str(&format!(
        "hot_path: objc_msgSend={} unique_selectors={} saw_imageNamed={} saw_glDrawArrays={} saw_glDrawElements={} top_objc={} recent_objc={} top_gl={} recent_gl={}
",
        fmt_opt_u64(hot_objc_msgsend),
        fmt_opt_u64(hot_objc_unique),
        yes_no_opt(hot_saw_image_named),
        yes_no_opt(hot_saw_gl_draw_arrays),
        yes_no_opt(hot_saw_gl_draw_elements),
        hot_top_objc.unwrap_or_else(|| "<none>".to_string()),
        hot_recent_objc.unwrap_or_else(|| "<none>".to_string()),
        hot_top_gl.unwrap_or_else(|| "<none>".to_string()),
        hot_recent_gl.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!(
        "scene_progress_trace: {}
",
        obs_scene_progress.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!(
        "sprite_watch_trace: {}
",
        obs_sprite_watch.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!(
        "scheduler_trace: {}
",
        obs_scheduler_trace.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!(
        "scheduler_trace_live: {}
",
        obs_scheduler_live.unwrap_or("<none>"),
    ));
    out.push_str(&format!(
        "callback_trace: {}
",
        obs_callback_trace.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!(
        "network: state={} stage={} events={} delegate_callbacks={} completed={} faulted={} retry={} last_error={}
",
        net_state.unwrap_or("<none>"),
        fmt_opt_u64(net_stage),
        fmt_opt_u64(net_events),
        fmt_opt_u64(net_callbacks),
        yes_no_opt(net_completed),
        yes_no_opt(net_faulted),
        yes_no_opt(net_retry),
        net_error.unwrap_or("<none>"),
    ));
    out.push_str(&format!(
        "network_bindings: {}
",
        net_bindings.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!(
        "network_births: {}
",
        net_births.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!(
        "network_slots: {}
",
        net_slots.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!(
        "network_owner_candidates: {}
",
        net_owner_candidates.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!(
        "network_first_binding: {}
",
        net_first_binding.unwrap_or("<none>"),
    ));
    out.push_str(&format!(
        "network_last_owner: {}
",
        net_last_owner.unwrap_or("<none>"),
    ));
    out.push_str(&format!(
        "audio: fileOpen={} readBytes={} readPackets={} bytesServed={} aq(create/alloc/enqueue/prime/start/stop/dispose)={}/{}/{}/{}/{}/{}/{} aq(bytesEnqueued={} cbOut/cbProp)={}/{} aq(lastQueue={},lastBuffer={},fmt={},hex=[{}],ascii='{}') openal(devOpen/ctxCreate/makeCurrent/genBuf/genSrc/bufData/bytesUploaded/queue/unqueue/play/stop)={}/{}/{}/{}/{}/{}/{}/{}/{}/{}/{} openal(lastFmt={},lastState={}) objcAudio(player alloc/initURL/initData/prepare/play/pause/stop/setVol/setLoops)={}/{}/{}/{}/{}/{}/{}/{}/{} objcAudio(engine shared/preload/play/stop/effect/fallback)={}/{}/{}/{}/{}/{} objcAudio(cda mgrShared/soundEngine/soundEngineNil/bgmPreload/bgmPlay/asyncLoad/asyncNil/playSound/playSoundNil)={}/{}/{}/{}/{}/{}/{}/{}/{} objcAudio(lastClass={},lastSel={},lastRes={},lastResult={}) systemsound(create/play/dispose)={}/{}/{} unsupported={} recent={}
",
        fmt_opt_u64(audio_audiofile_open_calls),
        fmt_opt_u64(audio_audiofile_read_bytes_calls),
        fmt_opt_u64(audio_audiofile_read_packets_calls),
        fmt_opt_u64(audio_audiofile_bytes_served),
        fmt_opt_u64(audio_audioqueue_create_calls),
        fmt_opt_u64(audio_audioqueue_allocate_calls),
        fmt_opt_u64(audio_audioqueue_enqueue_calls),
        fmt_opt_u64(audio_audioqueue_prime_calls),
        fmt_opt_u64(audio_audioqueue_start_calls),
        fmt_opt_u64(audio_audioqueue_stop_calls),
        fmt_opt_u64(audio_audioqueue_dispose_calls),
        fmt_opt_u64(audio_audioqueue_enqueued_bytes),
        fmt_opt_u64(audio_audioqueue_output_callback_dispatches),
        fmt_opt_u64(audio_audioqueue_property_callback_dispatches),
        audio_audioqueue_last_queue.unwrap_or_else(|| "<none>".to_string()),
        audio_audioqueue_last_buffer.unwrap_or_else(|| "<none>".to_string()),
        audio_audioqueue_last_format.unwrap_or("<none>"),
        audio_audioqueue_last_buffer_preview_hex.unwrap_or("<none>"),
        audio_audioqueue_last_buffer_preview_ascii.unwrap_or("<none>"),
        fmt_opt_u64(audio_openal_device_open_calls),
        fmt_opt_u64(audio_openal_context_create_calls),
        fmt_opt_u64(audio_openal_make_current_calls),
        fmt_opt_u64(audio_openal_buffers_generated),
        fmt_opt_u64(audio_openal_sources_generated),
        fmt_opt_u64(audio_openal_buffer_upload_calls),
        fmt_opt_u64(audio_openal_bytes_uploaded),
        fmt_opt_u64(audio_openal_queue_calls),
        fmt_opt_u64(audio_openal_unqueue_calls),
        fmt_opt_u64(audio_openal_play_calls),
        fmt_opt_u64(audio_openal_stop_calls),
        audio_openal_last_buffer_format.unwrap_or("<none>"),
        audio_openal_last_source_state.unwrap_or("<none>"),
        fmt_opt_u64(audio_objc_player_alloc_calls),
        fmt_opt_u64(audio_objc_player_init_url_calls),
        fmt_opt_u64(audio_objc_player_init_data_calls),
        fmt_opt_u64(audio_objc_player_prepare_calls),
        fmt_opt_u64(audio_objc_player_play_calls),
        fmt_opt_u64(audio_objc_player_pause_calls),
        fmt_opt_u64(audio_objc_player_stop_calls),
        fmt_opt_u64(audio_objc_player_set_volume_calls),
        fmt_opt_u64(audio_objc_player_set_loops_calls),
        fmt_opt_u64(audio_objc_engine_shared_calls),
        fmt_opt_u64(audio_objc_engine_preload_calls),
        fmt_opt_u64(audio_objc_engine_play_calls),
        fmt_opt_u64(audio_objc_engine_stop_calls),
        fmt_opt_u64(audio_objc_engine_effect_calls),
        fmt_opt_u64(audio_objc_fallback_dispatches),
        fmt_opt_u64(audio_objc_manager_shared_calls),
        fmt_opt_u64(audio_objc_manager_soundengine_calls),
        fmt_opt_u64(audio_objc_manager_soundengine_nil_results),
        fmt_opt_u64(audio_objc_bgm_preload_calls),
        fmt_opt_u64(audio_objc_bgm_play_calls),
        fmt_opt_u64(audio_objc_engine_async_load_progress_calls),
        fmt_opt_u64(audio_objc_engine_async_load_progress_nil_receivers),
        fmt_opt_u64(audio_objc_engine_playsound_calls),
        fmt_opt_u64(audio_objc_engine_playsound_nil_receivers),
        audio_objc_last_class.unwrap_or("<none>"),
        audio_objc_last_selector.unwrap_or("<none>"),
        audio_objc_last_resource.unwrap_or("<none>"),
        audio_objc_last_result.unwrap_or("<none>"),
        fmt_opt_u64(audio_systemsound_create_calls),
        fmt_opt_u64(audio_systemsound_play_calls),
        fmt_opt_u64(audio_systemsound_dispose_calls),
        fmt_opt_u64(audio_unsupported_events),
        audio_recent.unwrap_or_else(|| "<none>".to_string()),
    ));
    out.push_str(&format!("diagnosis: {}
", diagnosis));
    Some(out)
}

fn diagnose_report(
    final_state: &str,
    runloop_ticks: Option<u64>,
    runloop_sources: Option<u64>,
    input_consumed: Option<u64>,
    gfx_present_calls: Option<u64>,
    gfx_draw_calls: Option<u64>,
    gfx_last_decision: Option<&str>,
    net_state: Option<&str>,
    net_completed: Option<bool>,
    net_faulted: Option<bool>,
    ui_tree_menu_items: Option<u64>,
    ui_tree_labels: Option<u64>,
    ui_tree_textureless_visible: Option<u64>,
    ui_tree_samples: Option<&str>,
    hot_top_objc: Option<&str>,
    scene_progress_trace: Option<&str>,
    sprite_watch_trace: Option<&str>,
    scheduler_trace: Option<&str>,
    callback_trace: Option<&str>,
) -> &'static str {
    let saw_labelish_selector = hot_top_objc
        .map(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("label") || lower.contains("string") || lower.contains("font") || lower.contains("title")
        })
        .unwrap_or(false);
    let looks_like_loading_scene = ui_tree_samples
        .map(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("loadingmissionscene") || lower.contains("loading")
        })
        .unwrap_or(false);
    let saw_sprite_sheet_path = hot_top_objc
        .map(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("spritewithfile:rect:") || lower.contains("texture")
        })
        .unwrap_or(false);
    let samples_missing_tex_rect = ui_tree_samples
        .map(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("ccsprite") && lower.contains("texrect=<none>")
        })
        .unwrap_or(false);
    let stuck_loading_without_destination = scene_progress_trace
        .map(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("scene.watch") && lower.contains("loadingmissionscene") && lower.contains("destination=nil")
        })
        .unwrap_or(false);
    let saw_loading_sprite_watch = sprite_watch_trace
        .map(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("loading_bar.png") || lower.contains("menu_notepad.png")
        })
        .unwrap_or(false);
    let scheduler_mentions_loading = scheduler_trace
        .map(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("loadingmissionscene") || lower.contains("parent ccscene")
        })
        .unwrap_or(false);
    let scheduler_saw_schedule = scheduler_trace
        .map(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("scheduleupdatefortarget:priority:paused:")
                || lower.contains("scheduleselector:fortarget:interval:paused:")
                || lower.contains("unschedule")
        })
        .unwrap_or(false);
    let scheduler_saw_tick_update = scheduler_trace
        .map(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("sel=tick:") || lower.contains("sel=update:")
        })
        .unwrap_or(false);
    if final_state.starts_with("no-new-presents") {
        if gfx_last_decision == Some("source=retained reason=retain-last-frame reused_previous=YES") {
            return "render stalled after the last retained frame; scene changed but nothing new is being presented";
        }
        if runloop_ticks.unwrap_or(0) <= 1 {
            return "runloop starvation: almost no synthetic ticks were observed after launch";
        }
        if input_consumed.unwrap_or(0) == 0 {
            return "input never reached the runtime or was never consumed";
        }
        if gfx_present_calls.unwrap_or(0) > 0 && gfx_draw_calls.unwrap_or(0) == 0 {
            if ui_tree_menu_items.unwrap_or(0) > 0 || ui_tree_labels.unwrap_or(0) > 0 || ui_tree_textureless_visible.unwrap_or(0) > 0 {
                return "scene graph is alive, but visible menu/text nodes are still synthetic or textureless; inspect ui_tree + hot_path for missing label/menu-item support";
            }
            return "presents happened without guest draw calls; likely scene transition / compositor path problem";
        }
        return "runtime stayed alive but stopped producing fresh frames; inspect runloop/input/network summaries below";
    }
    if net_faulted == Some(true) {
        return "network fault path was hit; the title may be waiting on a callback/error branch";
    }
    if net_state == Some("completed") && net_completed == Some(true) && stuck_loading_without_destination {
        if scheduler_mentions_loading && scheduler_saw_schedule && !scheduler_saw_tick_update {
            return "scene progression blocked: watched scheduler activity reaches LoadingMissionScene/parent, but no tick:/update: callbacks fire; inspect scheduler delivery after registration";
        }
        if !scheduler_mentions_loading {
            return "scene progression blocked: LoadingMissionScene is still watched with destination=nil, and scheduler_trace never touched the loading scene/parent; inspect watcher target vs scheduler registration";
        }
        return "scene progression blocked: LoadingMissionScene is still watched with destination=nil; renderer is mostly retaining the same frame, so texture-rect issues are probably secondary";
    }
    if net_state == Some("completed") && net_completed == Some(true) && gfx_draw_calls.unwrap_or(0) == 0 {
        if looks_like_loading_scene && ui_tree_textureless_visible.unwrap_or(0) >= 3 {
            return "network is done, but the title is still parked in LoadingMissionScene; visible nodes are mostly textureless sprites, so inspect sprite-sheet / batch-node atlas support before blaming scene transition";
        }
        if ui_tree_menu_items.unwrap_or(0) > 0 || saw_labelish_selector {
            return "network finished and the scene graph exists, but the UI layer still looks synthetic; inspect ui_tree/hot_path for label rendering or menu-item texture gaps";
        }
        if looks_like_loading_scene && saw_sprite_sheet_path && samples_missing_tex_rect {
            if saw_loading_sprite_watch {
                return "loading scene is alive and sprite-watch is now capturing loading_bar/menu_notepad mutations; inspect scene_progress_trace + sprite_watch_trace before changing scene logic again";
            }
            return "loading scene is alive and atlas textures resolved, but child sprites still show texRect=<none>; inspect spriteWithFile:rect:, initWithTexture:rect:, and setTextureRect:untrimmedSize: state capture";
        }
        if saw_sprite_sheet_path && ui_tree_textureless_visible.unwrap_or(0) > 0 {
            return "scene graph is alive and sprite-file selectors fired, but many visible nodes still have no resolved texture; inspect inherited atlas textures and CCSpriteSheet/BatchNode support";
        }
        return "network finished, but gameplay scene still did not start drawing; focus on scene transition / scheduler";
    }
    "no obvious fatal condition in the summary; inspect raw log and input replay next"
}

fn join_string_array(value: &Value, path: &[&str], limit: usize) -> Option<String> {
    let mut cur = value;
    for key in path {
        cur = cur.get(*key)?;
    }
    let arr = cur.as_array()?;
    let mut parts = Vec::new();
    for item in arr.iter().rev().take(limit).rev() {
        if let Some(s) = item.as_str() {
            parts.push(s.to_string());
        }
    }
    if parts.is_empty() { None } else { Some(parts.join(" | ")) }
}

fn format_count_entries(value: &Value, path: &[&str], limit: usize) -> Option<String> {
    let mut cur = value;
    for key in path {
        cur = cur.get(*key)?;
    }
    let arr = cur.as_array()?;
    let mut parts = Vec::new();
    for item in arr.iter().take(limit) {
        let name = item.get("name").and_then(Value::as_str)?;
        let count = item.get("count").and_then(Value::as_u64)?;
        parts.push(format!("{}:{}", name, count));
    }
    if parts.is_empty() { None } else { Some(parts.join(", ")) }
}

fn format_ui_tree_samples(value: &Value, path: &[&str], limit: usize) -> Option<String> {
    let mut cur = value;
    for key in path {
        cur = cur.get(*key)?;
    }
    let arr = cur.as_array()?;
    let mut parts = Vec::new();
    for item in arr.iter().take(limit) {
        let ptr = item.get("ptr").and_then(Value::as_u64).map(|v| format!("0x{v:08x}")).unwrap_or_else(|| "<none>".to_string());
        let label = item.get("label").and_then(Value::as_str).unwrap_or("<none>");
        let raw_texture = item.get("raw_texture").and_then(Value::as_u64).unwrap_or(0);
        let texture = item.get("texture").and_then(Value::as_u64).unwrap_or(0);
        let pos_x = item.get("position_x").and_then(Value::as_f64).unwrap_or(0.0);
        let pos_y = item.get("position_y").and_then(Value::as_f64).unwrap_or(0.0);
        let world_x = item.get("world_x").and_then(Value::as_f64).unwrap_or(pos_x);
        let world_y = item.get("world_y").and_then(Value::as_f64).unwrap_or(pos_y);
        let raw_width = item.get("raw_width").and_then(Value::as_u64).unwrap_or(0);
        let raw_height = item.get("raw_height").and_then(Value::as_u64).unwrap_or(0);
        let width = item.get("width").and_then(Value::as_u64).unwrap_or(0);
        let height = item.get("height").and_then(Value::as_u64).unwrap_or(0);
        let draw_eligible = item.get("draw_eligible").and_then(Value::as_bool).unwrap_or(false);
        let draw_reason = item.get("draw_reason").and_then(Value::as_str).unwrap_or("-");
        let anchor_x = item.get("anchor_x").and_then(Value::as_f64).unwrap_or(0.0);
        let anchor_y = item.get("anchor_y").and_then(Value::as_f64).unwrap_or(0.0);
        let anchor_explicit = item.get("anchor_explicit").and_then(Value::as_bool).unwrap_or(false);
        let rect_explicit = item.get("texture_rect_explicit").and_then(Value::as_bool).unwrap_or(false);
        let rect_x = item.get("texture_rect_x").and_then(Value::as_f64).unwrap_or(0.0);
        let rect_y = item.get("texture_rect_y").and_then(Value::as_f64).unwrap_or(0.0);
        let rect_w = item.get("texture_rect_w").and_then(Value::as_f64).unwrap_or(0.0);
        let rect_h = item.get("texture_rect_h").and_then(Value::as_f64).unwrap_or(0.0);
        let callback = item.get("callback_selector").and_then(Value::as_str).unwrap_or("-");
        let text = item.get("text_backing").and_then(Value::as_str).unwrap_or("");
        let texture_key = item.get("texture_key").and_then(Value::as_str).unwrap_or("-");
        let mut fragment = format!(
            "{}<{}> pos=({:.1},{:.1}) world=({:.1},{:.1}) rawSize={}x{} size={}x{} anchor=({:.2},{:.2}{}) rawTex={} tex={} draw={}({}) cb={} key={}",
            ptr,
            label,
            pos_x,
            pos_y,
            world_x,
            world_y,
            raw_width,
            raw_height,
            width,
            height,
            anchor_x,
            anchor_y,
            if anchor_explicit { ",exp" } else { ",def" },
            raw_texture,
            texture,
            if draw_eligible { "YES" } else { "NO" },
            draw_reason,
            callback,
            texture_key,
        );
        if rect_explicit {
            fragment.push_str(&format!(" texRect=({:.1},{:.1} {:.1}x{:.1})", rect_x, rect_y, rect_w, rect_h));
        } else {
            fragment.push_str(" texRect=<none>");
        }
        if !text.is_empty() {
            fragment.push_str(&format!(" text='{}'", text));
        }
        parts.push(fragment);
    }
    if parts.is_empty() { None } else { Some(parts.join(" | ")) }
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = value;
    for key in path {
        cur = cur.get(*key)?;
    }
    cur.as_str()
}

fn bool_at(value: &Value, path: &[&str]) -> Option<bool> {
    let mut cur = value;
    for key in path {
        cur = cur.get(*key)?;
    }
    cur.as_bool()
}

fn u64_at(value: &Value, path: &[&str]) -> Option<u64> {
    let mut cur = value;
    for key in path {
        cur = cur.get(*key)?;
    }
    cur.as_u64()
}

fn fmt_opt_u64(value: Option<u64>) -> String {
    value.map(|v| v.to_string()).unwrap_or_else(|| "<none>".to_string())
}

fn yes_no(value: bool) -> &'static str {
    if value { "YES" } else { "NO" }
}

fn yes_no_opt(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "YES",
        Some(false) => "NO",
        None => "<none>",
    }
}

fn hex_opt(value: Option<u64>) -> Option<String> {
    value.map(|v| format!("0x{v:08x}"))
}

impl eframe::App for LauncherApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Импортировать IPA").clicked() {
                    self.import_ipa();
                }
                if ui.button("Обновить библиотеку").clicked() {
                    self.refresh();
                }
                if ui.button("Открыть папку данных").clicked() {
                    self.open_data_dir();
                }
                if ui.button("Проверить runtime binaries").clicked() {
                    self.refresh_runtime_detection();
                    match &self.detected_player_exe {
                        Some(path) => self.set_status_ok(format!("Player найден: {}", path.display())),
                        None => self.set_status_err("Player не найден. Собери release bundle или задай override."),
                    }
                }
            });
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(&self.status);
            });
        });

        egui::SidePanel::left("games_list")
            .min_width(300.0)
            .show(ctx, |ui| {
                ui.heading("Библиотека игр");
                ui.separator();

                if self.db.apps.is_empty() {
                    ui.label("Пока пусто. Нажми “Импортировать IPA”, чтобы добавить игру.");
                    return;
                }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (index, app) in self.db.apps.iter().enumerate() {
                        let selected = self.selected == Some(index);
                        let subtitle = if app.bundle_id.is_empty() {
                            format!("{} • iOS {}", app.executable, app.minimum_ios_version)
                        } else {
                            format!("{} • iOS {}", app.bundle_id, app.minimum_ios_version)
                        };
                        ui.group(|ui| {
                            if ui.selectable_label(selected, &app.display_name).clicked() {
                                self.selected = Some(index);
                            }
                            ui.small(subtitle);
                            if !app.exists() {
                                ui.colored_label(egui::Color32::from_rgb(220, 90, 90), "manifest.json missing");
                            }
                        });
                        ui.add_space(6.0);
                    }
                });
            });

        egui::SidePanel::right("recent_launches")
            .min_width(320.0)
            .show(ctx, |ui| {
                ui.heading("Recent launches");
                ui.small("Последние реальные запуски без возврата к консольной рутине.");
                ui.separator();

                if self.db.recent_launches.is_empty() {
                    ui.label("История запусков пока пустая.");
                } else {
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        let launches = self.db.recent_launches.clone();
                        for launch in launches {
                            ui.group(|ui| {
                                ui.label(&launch.display_name);
                                ui.small(format!("launch: {}", launch.launched_at_unix));
                                ui.small(&launch.log_path);
                                ui.horizontal_wrapped(|ui| {
                                    if ui.add_enabled(launch.exists(), egui::Button::new("Показать лог")).clicked() {
                                        let title = format!("Лог запуска: {}", launch.display_name);
                                        match self.show_log_in_viewer(&launch.log_pathbuf(), Some(title)) {
                                            Ok(()) => self.set_status_ok("Лог открытия из recent launches готов."),
                                            Err(err) => self.set_status_err(format!("Не удалось открыть лог: {err:#}")),
                                        }
                                    }
                                    if ui.button("Выбрать игру").clicked() {
                                        self.select_app_by_id(&launch.app_id);
                                    }
                                });
                            });
                            ui.add_space(6.0);
                        }
                    });
                }

                ui.separator();
                ui.heading(&self.log_view_title);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Обновить лог в окне").clicked() {
                        self.reload_log_in_viewer();
                    }
                    if let Some(path) = self.log_view_path.clone() {
                        if ui.button("Открыть лог снаружи").clicked() {
                            match open_in_shell(&path) {
                                Ok(()) => self.set_status_ok(format!("Открыл лог: {}", path.display())),
                                Err(err) => self.set_status_err(format!("Не удалось открыть лог: {err:#}")),
                            }
                        }
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut self.log_view_text)
                        .desired_width(f32::INFINITY)
                        .desired_rows(22)
                        .code_editor(),
                );
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Standalone launcher");
            ui.label("Desktop-поток без обязательного cargo run: launcher — главная точка входа, player — внутренний runtime-бэкенд, логи и игры лежат в персистентном хранилище.");
            ui.separator();

            ui.group(|ui| {
                ui.heading("Runtime / release-only статус");
                ui.monospace(format!("Launcher exe: {}", self.launcher_exe_path.display()));
                ui.monospace(format!("Build flavor: {}", self.detected_build_flavor));
                match &self.detected_player_exe {
                    Some(path) => ui.monospace(format!("Player exe: {}", path.display())),
                    None => ui.colored_label(
                        egui::Color32::from_rgb(220, 90, 90),
                        "Player exe не найден. Собери release binaries или используй override.",
                    ),
                };
                if let Some(path) = self.db.preferences.player_exe_override.as_ref() {
                    ui.monospace(format!("Override: {path}"));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.db.preferences.prefer_release_runtime, "Предпочитать target/release runtime");
                    if ui.button("Выбрать mkea_player.exe вручную").clicked() {
                        self.pick_player_override();
                    }
                    if ui.button("Сбросить override").clicked() {
                        self.clear_player_override();
                    }
                });
                if ui.button("Сохранить runtime-настройки").clicked() {
                    self.save_db();
                    self.refresh_runtime_detection();
                    self.set_status_ok("Runtime-настройки сохранены.");
                }
            });

            ui.add_space(10.0);
            ui.group(|ui| {
                ui.heading("Настройки запуска по умолчанию");
                ui.horizontal(|ui| {
                    ui.label("Ширина окна");
                    ui.add(egui::DragValue::new(&mut self.db.preferences.window_width).range(160..=4096));
                    ui.label("Высота окна");
                    ui.add(egui::DragValue::new(&mut self.db.preferences.window_height).range(160..=4096));
                });
                ui.checkbox(
                    &mut self.db.preferences.auto_window_orientation,
                    "Авто-ориентация окна по профилю игры",
                );
                ui.horizontal(|ui| {
                    ui.label("Макс. инструкций");
                    ui.add(egui::DragValue::new(&mut self.db.preferences.max_instructions).speed(100_000.0).range(1_000_000..=20_000_000_000_u64));
                });
                ui.horizontal(|ui| {
                    ui.label("Runloop ticks");
                    ui.add(egui::DragValue::new(&mut self.db.preferences.runloop_ticks).range(1..=10_000_000));
                });
                ui.horizontal(|ui| {
                    ui.label("Runtime mode");
                    egui::ComboBox::from_id_salt("runtime_mode_pref")
                        .selected_text(self.db.preferences.runtime_mode.clone())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.db.preferences.runtime_mode, "strict".to_string(), "strict");
                            ui.selectable_value(&mut self.db.preferences.runtime_mode, "hybrid".to_string(), "hybrid");
                            ui.selectable_value(&mut self.db.preferences.runtime_mode, "bring-up".to_string(), "bring-up");
                        });
                    ui.label("Backend");
                    egui::ComboBox::from_id_salt("execution_backend_pref")
                        .selected_text(self.db.preferences.execution_backend.clone())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.db.preferences.execution_backend, "memory".to_string(), "memory (synthetic/HLE)");
                            ui.selectable_value(&mut self.db.preferences.execution_backend, "dry-run".to_string(), "dry-run (probe only)");
                            ui.selectable_value(&mut self.db.preferences.execution_backend, "unicorn".to_string(), "unicorn (real CPU + shadow fallback)");
                        });
                });
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.db.preferences.input_flip_y, "input_flip_y");
                    ui.checkbox(&mut self.db.preferences.synthetic_network_faults, "synthetic_network_faults");
                    ui.checkbox(&mut self.db.preferences.close_when_finished, "close_when_finished");
                });
                if self.db.preferences.execution_backend == "memory" {
                    ui.colored_label(egui::Color32::from_rgb(220, 160, 60), "memory backend = synthetic runtime path, not real CPU execution");
                }
                if ui.button("Сохранить настройки launcher").clicked() {
                    self.save_db();
                    self.set_status_ok("Настройки запуска сохранены.");
                }
            });

            ui.add_space(10.0);
            let Some(app) = self.selected_app().cloned() else {
                ui.label("Выбери игру слева, чтобы увидеть детали и кнопки запуска.");
                ui.separator();
                ui.label("Для portable standalone используй scripts/package_windows_release.cmd или .ps1 после release-сборки.");
                return;
            };

            ui.group(|ui| {
                ui.heading(&app.display_name);
                ui.monospace(format!("Bundle ID: {}", if app.bundle_id.is_empty() { "<empty>" } else { &app.bundle_id }));
                ui.monospace(format!("Executable: {}", app.executable));
                ui.monospace(format!("Minimum iOS: {}", app.minimum_ios_version));
                ui.monospace(format!("Arch: {}", app.chosen_arch));
                ui.monospace(format!("Manifest: {}", app.manifest_path));
                ui.monospace(format!("Install dir: {}", app.install_dir));
                ui.monospace(format!("Source IPA: {}", app.source_ipa_path));
                ui.monospace(format!("Installed at (unix): {}", format_unix_time(Some(app.installed_at_unix))));
                ui.monospace(format!("Last played (unix): {}", format_unix_time(app.last_played_at_unix)));
                if let Some(log_path) = app.last_run_log_path.as_ref() {
                    ui.monospace(format!("Last log: {log_path}"));
                }
                if !app.exists() {
                    ui.colored_label(egui::Color32::from_rgb(220, 90, 90), "manifest.json missing — rebuild or reinstall this IPA.");
                }
            });

            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                let can_launch = app.exists() && self.detected_player_exe.is_some();
                if ui.add_enabled(can_launch, egui::Button::new("Играть")).clicked() {
                    self.launch_selected();
                }
                if ui.button("Rebuild / reinstall app").clicked() {
                    self.rebuild_selected();
                }
                if ui.button("Показать лог в launcher").clicked() {
                    self.show_selected_log_in_viewer();
                }
                if ui.button("Открыть папку игры").clicked() {
                    self.open_selected_dir();
                }
                if ui.button("Открыть последний лог").clicked() {
                    self.open_selected_log();
                }
                if ui.button("Удалить из библиотеки").clicked() {
                    self.remove_selected();
                }
            });

            ui.add_space(10.0);
            ui.label("Что даёт эта версия прямо сейчас:");
            ui.label("• один launcher.exe — главная точка входа для обычного использования;");
            ui.label("• mkea_player.exe становится отдельным скрытым runtime-процессом, а не режимом того же GUI-exe;");
            ui.label("• recent launches и встроенный просмотрщик логов остаются прямо в окне launcher’а;");
            ui.label("• portable bundle можно собрать отдельным скриптом без ручной возни по папкам.");
        });
    }
}

fn sort_apps(apps: &mut [InstalledApp]) {
    apps.sort_by(|lhs, rhs| lhs.display_name.to_lowercase().cmp(&rhs.display_name.to_lowercase()));
}

fn effective_display_name(bundle_name: &str, executable: &str) -> String {
    if bundle_name.trim().is_empty() {
        executable.to_string()
    } else {
        bundle_name.to_string()
    }
}

fn player_binary_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "mkea_player.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "mkea_player"
    }
}


fn scale_surface_to_fit(surface_w: u32, surface_h: u32, bound_w: u32, bound_h: u32) -> (u32, u32) {
    let surface_w = surface_w.max(1);
    let surface_h = surface_h.max(1);
    let bound_w = bound_w.max(1);
    let bound_h = bound_h.max(1);
    let scale_w = bound_w as f32 / surface_w as f32;
    let scale_h = bound_h as f32 / surface_h as f32;
    let scale = scale_w.min(scale_h).max(0.1);
    let width = ((surface_w as f32) * scale).round().max(1.0) as u32;
    let height = ((surface_h as f32) * scale).round().max(1.0) as u32;
    (width.max(1), height.max(1))
}

fn candidate_player_paths(launcher_exe: &Path, prefs: &storage::LauncherPrefs) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let push_unique = |vec: &mut Vec<PathBuf>, path: PathBuf| {
        if !vec.iter().any(|existing| existing == &path) {
            vec.push(path);
        }
    };

    if let Some(override_path) = prefs.player_exe_override.as_ref().filter(|value| !value.trim().is_empty()) {
        push_unique(&mut out, PathBuf::from(override_path));
    }

    if let Some(dir) = launcher_exe.parent() {
        push_unique(&mut out, dir.join(player_binary_name()));
        if prefs.prefer_release_runtime {
            if dir.file_name().map(|name| name == "debug").unwrap_or(false) {
                if let Some(target_dir) = dir.parent() {
                    push_unique(&mut out, target_dir.join("release").join(player_binary_name()));
                }
            }
        } else if dir.file_name().map(|name| name == "release").unwrap_or(false) {
            if let Some(target_dir) = dir.parent() {
                push_unique(&mut out, target_dir.join("debug").join(player_binary_name()));
            }
        }

        push_unique(&mut out, dir.join("runtime").join(player_binary_name()));
        push_unique(&mut out, dir.join("mkEA").join(player_binary_name()));
    }

    out
}

fn detect_player_exe(launcher_exe: &Path, prefs: &storage::LauncherPrefs) -> Option<PathBuf> {
    candidate_player_paths(launcher_exe, prefs)
        .into_iter()
        .find(|path| path.is_file())
}

fn detect_build_flavor(launcher_exe: &Path) -> &'static str {
    let path = launcher_exe.to_string_lossy().to_ascii_lowercase();
    if path.contains("\\target\\release\\") || path.contains("/target/release/") {
        "release"
    } else if path.contains("\\target\\debug\\") || path.contains("/target/debug/") {
        "debug"
    } else {
        "portable/custom"
    }
}
