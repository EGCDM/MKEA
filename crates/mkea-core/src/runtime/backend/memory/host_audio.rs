use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Debug, Clone)]
struct HostPcmPlaybackPlan {
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    loop_forever: bool,
    gain: f32,
    chunks: Vec<Vec<u8>>,
}

impl MemoryArm32Backend {
    fn objc_infer_audio_receiver_class_name(&mut self, receiver: u32, selector: &str) -> Option<String> {
        if receiver == 0 || !Self::audio_is_objc_audio_selector(selector) {
            return None;
        }
        if self.runtime.ui_runtime.audio_players.contains_key(&receiver) {
            return Some("AVAudioPlayer".to_string());
        }
        if receiver == self.runtime.graphics.cocos_sound_engine_object {
            return Some("CDSoundEngine".to_string());
        }
        let manager = self.runtime.graphics.cocos_audio_manager_object;
        if manager != 0 {
            let snapshot = self.objc_collect_audio_receiver_ivar_snapshot(manager);
            for entry in snapshot {
                if entry.value != receiver {
                    continue;
                }
                let lower = entry.name.to_ascii_lowercase();
                if lower.contains("backgroundmusic") {
                    return Some("AVAudioPlayer".to_string());
                }
                if lower.contains("soundengine") {
                    return Some("CDSoundEngine".to_string());
                }
            }
        }
        None
    }

    fn objc_bind_audio_related_ivars(
        &mut self,
        snapshot: &[PendingAudioIvarSnapshot],
        reason: &str,
    ) -> Vec<String> {
        let mut repairs = Vec::new();
        for entry in snapshot {
            if entry.value == 0 || entry.value_class.is_some() {
                continue;
            }
            let lower = entry.name.to_ascii_lowercase();
            let expected = if lower.contains("backgroundmusic") {
                Some("AVAudioPlayer")
            } else if lower.contains("soundengine") {
                Some("CDSoundEngine")
            } else {
                None
            };
            let Some(expected) = expected else {
                continue;
            };
            let Some(class_ptr) = self.objc_lookup_class_by_name(expected) else {
                continue;
            };
            self.objc_attach_receiver_class(entry.value, class_ptr, expected);
            if expected == "CDSoundEngine" {
                self.objc_register_guest_singleton_object(class_ptr, expected, entry.value, reason);
            }
            repairs.push(format!(
                "bind-ivar:{}::{}->{}/{}",
                entry.owner_class,
                entry.name,
                self.describe_ptr(entry.value),
                expected,
            ));
        }
        repairs
    }

    fn host_audio_stop_openal_source(&mut self, source_id: u32) {
        if let Some(source) = self.runtime.openal.sources.get_mut(&source_id) {
            if let Some(token) = source.host_stop_token.take() {
                token.store(true, Ordering::SeqCst);
                self.audio_trace_push_event(format!(
                    "hostaudio.openal.stop source={} token=signalled",
                    source_id
                ));
            }
        }
    }

    fn host_audio_openal_plan_for_source(&self, source_id: u32) -> Option<HostPcmPlaybackPlan> {
        let source = self.runtime.openal.sources.get(&source_id)?;
        let mut buffer_ids: Vec<u32> = source.queued_buffers.iter().copied().collect();
        if buffer_ids.is_empty() {
            if let Some(buffer_id) = source.ints.get(&AL_BUFFER).copied().filter(|id| *id > 0) {
                buffer_ids.push(buffer_id as u32);
            }
        }
        if buffer_ids.is_empty() {
            return None;
        }
        let first = self.runtime.openal.buffers.get(&buffer_ids[0])?;
        let (channels, bits_per_sample) = match first.format {
            AL_FORMAT_MONO8 => (1u16, 8u16),
            AL_FORMAT_MONO16 => (1u16, 16u16),
            AL_FORMAT_STEREO8 => (2u16, 8u16),
            AL_FORMAT_STEREO16 => (2u16, 16u16),
            _ => return None,
        };
        if first.frequency == 0 || first.pcm_data.is_empty() {
            return None;
        }
        let mut chunks = Vec::new();
        for buffer_id in buffer_ids {
            let buffer = self.runtime.openal.buffers.get(&buffer_id)?;
            if buffer.format != first.format || buffer.frequency != first.frequency || buffer.pcm_data.is_empty() {
                return None;
            }
            chunks.push(buffer.pcm_data.clone());
        }
        let source_gain = source.floats.get(&AL_GAIN).copied().unwrap_or(1.0);
        let listener_gain = self.runtime.openal.listener_floats.get(&AL_GAIN).copied().unwrap_or(1.0);
        let gain = (source_gain * listener_gain).clamp(0.0, 4.0);
        let loop_forever = source.ints.get(&AL_LOOPING).copied().unwrap_or(AL_FALSE as i32) != 0;
        Some(HostPcmPlaybackPlan {
            sample_rate: first.frequency,
            channels,
            bits_per_sample,
            loop_forever,
            gain,
            chunks,
        })
    }

    fn host_audio_play_openal_source(&mut self, source_id: u32) {
        self.host_audio_stop_openal_source(source_id);
        let Some(plan) = self.host_audio_openal_plan_for_source(source_id) else {
            self.audio_trace_push_event(format!(
                "hostaudio.openal.plan-miss source={} queued={} fallback=silent",
                source_id,
                self.runtime
                    .openal
                    .sources
                    .get(&source_id)
                    .map(|s| s.queued_buffers.len())
                    .unwrap_or(0)
            ));
            return;
        };
        let stop_token = Arc::new(AtomicBool::new(false));
        if let Some(source) = self.runtime.openal.sources.get_mut(&source_id) {
            source.host_stop_token = Some(stop_token.clone());
        }
        self.audio_trace_push_event(format!(
            "hostaudio.openal.play source={} chunks={} sr={} ch={} bits={} loop={} gain={:.3}",
            source_id,
            plan.chunks.len(),
            plan.sample_rate,
            plan.channels,
            plan.bits_per_sample,
            plan.loop_forever,
            plan.gain,
        ));
        Self::host_audio_spawn_pcm_thread(source_id, plan, stop_token);
    }

    fn host_audio_prepare_player_path(&mut self, player: u32) -> Option<String> {
        let state = self.runtime.ui_runtime.audio_players.get(&player)?;
        if state.content_url != 0 {
            return self
                .resolve_path_from_url_like_value(state.content_url, false)
                .map(|path| path.display().to_string())
                .or_else(|| self.guest_string_value(state.content_url));
        }
        None
    }

    fn host_audio_player_alias(player: u32) -> String {
        format!("mkea_avplayer_{player:08x}")
    }

    fn host_audio_player_prepare(&mut self, player: u32) {
        let path = self.host_audio_prepare_player_path(player);
        if let Some(state) = self.runtime.ui_runtime.audio_players.get_mut(&player) {
            state.host_cached_path = path.clone();
            if state.host_alias.is_none() {
                state.host_alias = Some(Self::host_audio_player_alias(player));
            }
        }
        self.audio_trace_push_event(format!(
            "hostaudio.player.prepare player={} path={}",
            self.describe_ptr(player),
            path.unwrap_or_else(|| "<none>".to_string())
        ));
    }

    fn host_audio_player_play(&mut self, player: u32) {
        self.host_audio_player_prepare(player);
        let Some(state) = self.runtime.ui_runtime.audio_players.get(&player).cloned() else {
            return;
        };
        let Some(path) = state.host_cached_path.clone() else {
            self.audio_trace_push_event(format!(
                "hostaudio.player.play-miss player={} reason=no-path",
                self.describe_ptr(player)
            ));
            return;
        };
        let alias = state
            .host_alias
            .clone()
            .unwrap_or_else(|| Self::host_audio_player_alias(player));
        let loop_forever = state.number_of_loops < 0;
        let volume = if state.volume <= 0.0 { 1.0 } else { state.volume };
        let ok = Self::host_audio_play_media_alias(&alias, &path, loop_forever, volume);
        self.audio_trace_push_event(format!(
            "hostaudio.player.play player={} alias={} loop={} volume={:.3} path={} ok={}",
            self.describe_ptr(player),
            alias,
            loop_forever,
            volume,
            path,
            ok,
        ));
    }

    fn host_audio_player_pause(&mut self, player: u32) {
        let alias = self
            .runtime
            .ui_runtime
            .audio_players
            .get(&player)
            .and_then(|state| state.host_alias.clone())
            .unwrap_or_else(|| Self::host_audio_player_alias(player));
        let ok = Self::host_audio_pause_media_alias(&alias);
        self.audio_trace_push_event(format!(
            "hostaudio.player.pause player={} alias={} ok={}",
            self.describe_ptr(player),
            alias,
            ok,
        ));
    }

    fn host_audio_player_stop(&mut self, player: u32) {
        let alias = self
            .runtime
            .ui_runtime
            .audio_players
            .get(&player)
            .and_then(|state| state.host_alias.clone())
            .unwrap_or_else(|| Self::host_audio_player_alias(player));
        let ok = Self::host_audio_stop_media_alias(&alias);
        self.audio_trace_push_event(format!(
            "hostaudio.player.stop player={} alias={} ok={}",
            self.describe_ptr(player),
            alias,
            ok,
        ));
    }

    fn host_audio_player_set_volume(&mut self, player: u32, volume: f32) {
        let alias = self
            .runtime
            .ui_runtime
            .audio_players
            .get(&player)
            .and_then(|state| state.host_alias.clone())
            .unwrap_or_else(|| Self::host_audio_player_alias(player));
        let ok = Self::host_audio_set_media_volume(&alias, volume);
        self.audio_trace_push_event(format!(
            "hostaudio.player.set-volume player={} alias={} volume={:.3} ok={}",
            self.describe_ptr(player),
            alias,
            volume,
            ok,
        ));
    }

    fn host_audio_bgm_preload(&mut self, path: Option<String>) {
        if let Some(path) = path {
            self.runtime.ui_runtime.bgm_cached_path = Some(path.clone());
            self.audio_trace_push_event(format!("hostaudio.bgm.preload path={}", path));
        }
        if self.runtime.ui_runtime.bgm_alias.is_none() {
            self.runtime.ui_runtime.bgm_alias = Some("mkea_bgm".to_string());
        }
    }

    fn host_audio_bgm_play(&mut self, path: Option<String>, loop_forever: bool) {
        if let Some(path) = path {
            self.runtime.ui_runtime.bgm_cached_path = Some(path);
        }
        if self.runtime.ui_runtime.bgm_alias.is_none() {
            self.runtime.ui_runtime.bgm_alias = Some("mkea_bgm".to_string());
        }
        let alias = self.runtime.ui_runtime.bgm_alias.clone().unwrap_or_else(|| "mkea_bgm".to_string());
        let Some(path) = self.runtime.ui_runtime.bgm_cached_path.clone() else {
            self.audio_trace_push_event("hostaudio.bgm.play-miss reason=no-path".to_string());
            return;
        };
        let volume = if self.runtime.ui_runtime.bgm_volume <= 0.0 {
            1.0
        } else {
            self.runtime.ui_runtime.bgm_volume
        };
        let ok = Self::host_audio_play_media_alias(&alias, &path, loop_forever, volume);
        self.runtime.ui_runtime.bgm_loop = loop_forever;
        self.runtime.ui_runtime.bgm_is_playing = ok;
        self.runtime.ui_runtime.bgm_is_paused = false;
        self.audio_trace_push_event(format!(
            "hostaudio.bgm.play alias={} loop={} volume={:.3} path={} ok={}",
            alias, loop_forever, volume, path, ok
        ));
    }

    fn host_audio_bgm_pause(&mut self) {
        let alias = self.runtime.ui_runtime.bgm_alias.clone().unwrap_or_else(|| "mkea_bgm".to_string());
        let ok = Self::host_audio_pause_media_alias(&alias);
        self.runtime.ui_runtime.bgm_is_paused = ok;
        self.runtime.ui_runtime.bgm_is_playing = !ok;
        self.audio_trace_push_event(format!("hostaudio.bgm.pause alias={} ok={}", alias, ok));
    }

    fn host_audio_bgm_resume(&mut self) {
        let alias = self.runtime.ui_runtime.bgm_alias.clone().unwrap_or_else(|| "mkea_bgm".to_string());
        let ok = Self::host_audio_resume_media_alias(&alias);
        self.runtime.ui_runtime.bgm_is_paused = !ok;
        self.runtime.ui_runtime.bgm_is_playing = ok;
        self.audio_trace_push_event(format!("hostaudio.bgm.resume alias={} ok={}", alias, ok));
    }

    fn host_audio_bgm_stop(&mut self) {
        let alias = self.runtime.ui_runtime.bgm_alias.clone().unwrap_or_else(|| "mkea_bgm".to_string());
        let ok = Self::host_audio_stop_media_alias(&alias);
        self.runtime.ui_runtime.bgm_is_paused = false;
        self.runtime.ui_runtime.bgm_is_playing = false;
        self.audio_trace_push_event(format!("hostaudio.bgm.stop alias={} ok={}", alias, ok));
    }

    fn host_audio_bgm_set_volume(&mut self, volume: f32) {
        self.runtime.ui_runtime.bgm_volume = volume;
        let alias = self.runtime.ui_runtime.bgm_alias.clone().unwrap_or_else(|| "mkea_bgm".to_string());
        let ok = Self::host_audio_set_media_volume(&alias, volume);
        self.audio_trace_push_event(format!(
            "hostaudio.bgm.set-volume alias={} volume={:.3} ok={}",
            alias, volume, ok
        ));
    }

    #[cfg(not(windows))]
    fn host_audio_spawn_pcm_thread(_source_id: u32, _plan: HostPcmPlaybackPlan, _stop: Arc<AtomicBool>) {}

    #[cfg(not(windows))]
    fn host_audio_play_media_alias(_alias: &str, _path: &str, _loop_forever: bool, _volume: f32) -> bool {
        false
    }

    #[cfg(not(windows))]
    fn host_audio_pause_media_alias(_alias: &str) -> bool {
        false
    }

    #[cfg(not(windows))]
    fn host_audio_resume_media_alias(_alias: &str) -> bool {
        false
    }

    #[cfg(not(windows))]
    fn host_audio_stop_media_alias(_alias: &str) -> bool {
        false
    }

    #[cfg(not(windows))]
    fn host_audio_set_media_volume(_alias: &str, _volume: f32) -> bool {
        false
    }

    #[cfg(windows)]
    fn host_audio_spawn_pcm_thread(source_id: u32, plan: HostPcmPlaybackPlan, stop: Arc<AtomicBool>) {
        std::thread::Builder::new()
            .name(format!("mkea-openal-{source_id}"))
            .spawn(move || {
                Self::host_audio_run_pcm_plan(source_id, plan, stop);
            })
            .ok();
    }

    #[cfg(windows)]
    fn host_audio_play_media_alias(alias: &str, path: &str, loop_forever: bool, volume: f32) -> bool {
        Self::host_audio_close_media_alias(alias);
        let open_cmd = format!(r#"open "{}" alias {}"#, path, alias);
        if !Self::host_audio_mci_command(&open_cmd) {
            return false;
        }
        let _ = Self::host_audio_set_media_volume(alias, volume);
        let play_cmd = if loop_forever {
            format!("play {} repeat", alias)
        } else {
            format!("play {}", alias)
        };
        if !Self::host_audio_mci_command(&play_cmd) {
            let _ = Self::host_audio_close_media_alias(alias);
            return false;
        }
        true
    }

    #[cfg(windows)]
    fn host_audio_pause_media_alias(alias: &str) -> bool {
        Self::host_audio_mci_command(&format!("pause {}", alias))
    }

    #[cfg(windows)]
    fn host_audio_resume_media_alias(alias: &str) -> bool {
        Self::host_audio_mci_command(&format!("resume {}", alias))
    }

    #[cfg(windows)]
    fn host_audio_stop_media_alias(alias: &str) -> bool {
        let stopped = Self::host_audio_mci_command(&format!("stop {}", alias));
        let _ = Self::host_audio_close_media_alias(alias);
        stopped
    }

    #[cfg(windows)]
    fn host_audio_close_media_alias(alias: &str) -> bool {
        Self::host_audio_mci_command(&format!("close {}", alias))
    }

    #[cfg(windows)]
    fn host_audio_set_media_volume(alias: &str, volume: f32) -> bool {
        let level = (volume.clamp(0.0, 1.0) * 1000.0).round() as i32;
        Self::host_audio_mci_command(&format!("setaudio {} volume to {}", alias, level.clamp(0, 1000)))
    }

    #[cfg(windows)]
    fn host_audio_mci_command(command: &str) -> bool {
        use std::ffi::c_void;

        #[link(name = "winmm")]
        unsafe extern "system" {
            fn mciSendStringW(
                lpstrcommand: *const u16,
                lpstrreturnstring: *mut u16,
                ureturnlength: u32,
                hwndcallback: *mut c_void,
            ) -> u32;
        }

        let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { mciSendStringW(wide.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut()) == 0 }
    }

    #[cfg(windows)]
    fn host_audio_run_pcm_plan(_source_id: u32, plan: HostPcmPlaybackPlan, stop: Arc<AtomicBool>) {
        use std::mem::size_of;
        use std::thread;
        use std::time::Duration;
        use winapi::um::mmeapi::{
            waveOutClose, waveOutOpen, waveOutPrepareHeader, waveOutReset, waveOutUnprepareHeader,
            waveOutWrite,
        };
        use winapi::shared::mmreg::{WAVEFORMATEX, WAVE_FORMAT_PCM};
        use winapi::um::mmsystem::{
            HWAVEOUT, WAVEHDR, WAVE_MAPPER, CALLBACK_NULL, MMSYSERR_NOERROR,
        };
        const WHDR_DONE_FLAG: u32 = 0x0000_0001;

        let block_align = plan.channels.saturating_mul(plan.bits_per_sample / 8);
        if block_align == 0 || plan.sample_rate == 0 {
            return;
        }
        let avg_bytes_per_sec = plan.sample_rate.saturating_mul(block_align as u32);
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: plan.channels,
            nSamplesPerSec: plan.sample_rate,
            nAvgBytesPerSec: avg_bytes_per_sec,
            nBlockAlign: block_align,
            wBitsPerSample: plan.bits_per_sample,
            cbSize: 0,
        };
        let mut handle = HWAVEOUT::default();
        let mmr = unsafe {
            waveOutOpen(
                &mut handle,
                WAVE_MAPPER,
                &format,
                0,
                0,
                CALLBACK_NULL,
            )
        };
        if mmr != MMSYSERR_NOERROR {
            return;
        }
        while !stop.load(Ordering::SeqCst) {
            for chunk in &plan.chunks {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let mut pcm = chunk.clone();
                if (plan.gain - 1.0).abs() > f32::EPSILON {
                    Self::host_audio_scale_pcm_buffer(&mut pcm, plan.bits_per_sample, plan.gain);
                }
                let mut header: WAVEHDR = unsafe { std::mem::zeroed() };
                header.lpData = pcm.as_mut_ptr() as *mut i8;
                header.dwBufferLength = pcm.len() as u32;
                let prepare = unsafe {
                    waveOutPrepareHeader(handle, &mut header, size_of::<WAVEHDR>() as u32)
                };
                if prepare != MMSYSERR_NOERROR {
                    continue;
                }
                let write = unsafe {
                    waveOutWrite(handle, &mut header, size_of::<WAVEHDR>() as u32)
                };
                if write != MMSYSERR_NOERROR {
                    unsafe {
                        let _ = waveOutUnprepareHeader(handle, &mut header, size_of::<WAVEHDR>() as u32);
                    }
                    continue;
                }
                while !stop.load(Ordering::SeqCst) && (header.dwFlags & WHDR_DONE_FLAG) == 0 {
                    thread::sleep(Duration::from_millis(4));
                }
                if stop.load(Ordering::SeqCst) {
                    unsafe {
                        let _ = waveOutReset(handle);
                    }
                }
                loop {
                    let unprepare = unsafe {
                        waveOutUnprepareHeader(handle, &mut header, size_of::<WAVEHDR>() as u32)
                    };
                    if unprepare == MMSYSERR_NOERROR {
                        break;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
            }
            if !plan.loop_forever {
                break;
            }
        }
        unsafe {
            let _ = waveOutReset(handle);
            let _ = waveOutClose(handle);
        }
    }

    #[cfg(windows)]
    fn host_audio_scale_pcm_buffer(bytes: &mut [u8], bits_per_sample: u16, gain: f32) {
        if gain <= 0.0 {
            bytes.fill(0);
            return;
        }
        match bits_per_sample {
            8 => {
                for sample in bytes.iter_mut() {
                    let centered = (*sample as i16) - 128;
                    let scaled = ((centered as f32) * gain).round() as i16;
                    let clamped = scaled.clamp(-128, 127) + 128;
                    *sample = clamped as u8;
                }
            }
            16 => {
                for chunk in bytes.chunks_exact_mut(2) {
                    let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                    let scaled = ((sample as f32) * gain).round() as i32;
                    let clamped = scaled.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                    let raw = clamped.to_le_bytes();
                    chunk[0] = raw[0];
                    chunk[1] = raw[1];
                }
            }
            _ => {}
        }
    }
}
