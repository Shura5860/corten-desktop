//! Corten Acoustic Signature — Tactile Industrial Sound Architecture
//!
//! Synthesizes ultra-short (18ms–90ms), zero-dependency 44.1kHz 16-bit PCM WAV
//! micro-feedback waveforms inspired by Teenage Engineering and Braun hardware,
//! caching them in `/tmp/corten-acoustics` and playing them asynchronously via
//! PipeWire (`pw-play`), PulseAudio (`paplay`), or ALSA (`aplay`).

use std::f32::consts::PI;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const SAMPLE_RATE: u32 = 44_100;

/// Tactile industrial sound cues across Corten Desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AcousticCue {
    /// 18ms crisp mechanical relay click for Bento tiles and pill switches.
    ToggleClick,
    /// 32ms magnetic latch thud when snapping or tiling windows.
    WindowSnap,
    /// 45ms dual-curtain mechanical shutter for screenshot capture.
    Shutter,
    /// 90ms warm harmonic fifth chime on session unlock.
    Unlock,
    /// 80ms muted low double-thud on authentication failure.
    AuthFail,
    /// 70ms brushed-steel resonant ping for incoming notifications.
    Notification,
}

impl AcousticCue {
    pub fn filename(self) -> &'static str {
        match self {
            Self::ToggleClick => "toggle_click.wav",
            Self::WindowSnap => "window_snap.wav",
            Self::Shutter => "shutter.wav",
            Self::Unlock => "unlock.wav",
            Self::AuthFail => "auth_fail.wav",
            Self::Notification => "notification.wav",
        }
    }

    /// Synthesizes the complete 16-bit mono RIFF WAV byte buffer for this cue.
    pub fn synthesize_wav(self) -> Vec<u8> {
        let samples = match self {
            Self::ToggleClick => synth_toggle_click(),
            Self::WindowSnap => synth_window_snap(),
            Self::Shutter => synth_shutter(),
            Self::Unlock => synth_unlock(),
            Self::AuthFail => synth_auth_fail(),
            Self::Notification => synth_notification(),
        };
        encode_pcm16_wav(&samples, SAMPLE_RATE)
    }
}

fn synth_toggle_click() -> Vec<i16> {
    let duration_s = 0.018;
    let count = (SAMPLE_RATE as f32 * duration_s) as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let t = i as f32 / SAMPLE_RATE as f32;
        let env = (-t * 280.0).exp();
        let high_contact = (2.0 * PI * 1650.0 * t).sin() * 0.55;
        let body_thud = (2.0 * PI * 420.0 * t).sin() * 0.45;
        let sample = (high_contact + body_thud) * env * 0.38;
        out.push((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
    }
    out
}

fn synth_window_snap() -> Vec<i16> {
    let duration_s = 0.032;
    let count = (SAMPLE_RATE as f32 * duration_s) as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let t = i as f32 / SAMPLE_RATE as f32;
        let env = (-t * 160.0).exp();
        // Pitch-dropping magnetic thud (360Hz -> 140Hz) + subtle metallic latch
        let freq = 140.0 + 220.0 * (-t * 120.0).exp();
        let thud = (2.0 * PI * freq * t).sin() * 0.70;
        let latch = (2.0 * PI * 2100.0 * t).sin() * (-t * 340.0).exp() * 0.30;
        let sample = (thud + latch) * env * 0.42;
        out.push((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
    }
    out
}

fn synth_shutter() -> Vec<i16> {
    let duration_s = 0.045;
    let count = (SAMPLE_RATE as f32 * duration_s) as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let t = i as f32 / SAMPLE_RATE as f32;
        // First curtain at t=0
        let c1 = (-t * 320.0).exp()
            * ((2.0 * PI * 1950.0 * t).sin() * 0.6 + (2.0 * PI * 620.0 * t).sin() * 0.4);
        // Second curtain at t=18ms
        let c2 = if t >= 0.018 {
            let t2 = t - 0.018;
            (-t2 * 290.0).exp()
                * ((2.0 * PI * 1480.0 * t2).sin() * 0.6 + (2.0 * PI * 480.0 * t2).sin() * 0.4)
        } else {
            0.0
        };
        let sample = (c1 + c2 * 0.85) * 0.45;
        out.push((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
    }
    out
}

fn synth_unlock() -> Vec<i16> {
    let duration_s = 0.090;
    let count = (SAMPLE_RATE as f32 * duration_s) as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let t = i as f32 / SAMPLE_RATE as f32;
        let attack = (t / 0.004).min(1.0);
        // Note 1: C5 (523.25 Hz), Note 2 enters at 35ms: E5 (659.25 Hz)
        let n1 = (2.0 * PI * 523.25 * t).sin() * (-t * 45.0).exp();
        let n2 = if t >= 0.035 {
            let t2 = t - 0.035;
            let a2 = (t2 / 0.004).min(1.0);
            a2 * (2.0 * PI * 659.25 * t2).sin() * (-t2 * 38.0).exp()
        } else {
            0.0
        };
        let sample = attack * (n1 * 0.5 + n2 * 0.6) * 0.35;
        out.push((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
    }
    out
}

fn synth_auth_fail() -> Vec<i16> {
    let duration_s = 0.085;
    let count = (SAMPLE_RATE as f32 * duration_s) as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let t = i as f32 / SAMPLE_RATE as f32;
        let pulse1 = (2.0 * PI * 165.0 * t).sin() * (-t * 95.0).exp();
        let pulse2 = if t >= 0.040 {
            let t2 = t - 0.040;
            (2.0 * PI * 135.0 * t2).sin() * (-t2 * 95.0).exp()
        } else {
            0.0
        };
        let sample = (pulse1 + pulse2 * 0.9) * 0.42;
        out.push((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
    }
    out
}

fn synth_notification() -> Vec<i16> {
    let duration_s = 0.075;
    let count = (SAMPLE_RATE as f32 * duration_s) as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let t = i as f32 / SAMPLE_RATE as f32;
        let attack = (t / 0.003).min(1.0);
        let env = attack * (-t * 52.0).exp();
        let fundamental = (2.0 * PI * 880.0 * t).sin() * 0.65;
        let fifth = (2.0 * PI * 1318.5 * t).sin() * (-t * 80.0).exp() * 0.35;
        let sample = (fundamental + fifth) * env * 0.32;
        out.push((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
    }
    out
}

fn encode_pcm16_wav(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let riff_size = 36 + data_bytes;
    let byte_rate = sample_rate * 2;
    let mut buf = Vec::with_capacity(44 + data_bytes as usize);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&riff_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt subchunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // PCM chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // AudioFormat = 1 (PCM)
    buf.extend_from_slice(&1u16.to_le_bytes()); // NumChannels = 1 (Mono)
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes()); // BlockAlign = 2
    buf.extend_from_slice(&16u16.to_le_bytes()); // BitsPerSample = 16

    // data subchunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    buf
}

/// Ensures the WAV file for `cue` is cached in a user-private runtime/cache directory and returns its path.
pub fn ensure_cached_wav(cue: AcousticCue) -> Option<PathBuf> {
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .map(|d| PathBuf::from(d).join("corten-acoustics"))
        .unwrap_or_else(|_| {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
            std::env::temp_dir().join(format!("corten-acoustics-{}", user))
        });
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(cue.filename());
    if !path.exists() {
        let bytes = cue.synthesize_wav();
        std::fs::write(&path, bytes).ok()?;
    }
    Some(path)
}

/// Plays a tactile industrial sound cue asynchronously without blocking the UI thread.
pub fn play(cue: AcousticCue) {
    if std::env::var_os("CORTEN_MUTE_ACOUSTICS").is_some() {
        return;
    }
    std::thread::spawn(move || {
        let Some(path) = ensure_cached_wav(cue) else {
            return;
        };
        let path_str = path.to_string_lossy();
        for (bin, args) in [
            ("pw-play", vec![path_str.as_ref()]),
            ("paplay", vec![path_str.as_ref()]),
            ("aplay", vec!["-q", path_str.as_ref()]),
        ] {
            if let Ok(mut child) = Command::new(bin)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                let _ = child.wait();
                break;
            }
        }
    });
}

