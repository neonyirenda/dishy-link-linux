//! The alert chime, synthesised as a short WAV (spec 0178). Port of
//! upstream `lib/alertSound.ts`: the same notes, envelope, and per-alert throttle.

use std::collections::HashMap;

use dishy_core::alert::Severity;

pub const SAMPLE_RATE: u32 = 22_050;
const NOTE_SECONDS: f64 = 0.26;
const NOTE_SPACING_SECONDS: f64 = 0.13;
const ATTACK_SECONDS: f64 = 0.012;
const PEAK_GAIN: f64 = 0.18;
const THROTTLE_MS: i64 = 60_000;

/// The notes for an alert opening, by severity, or clearing.
pub fn tones(severity: Severity, cleared: bool) -> &'static [f64] {
    // A falling interval reads as urgent, a rising one as benign.
    if cleared {
        return &[523.25];
    }
    match severity {
        Severity::Critical => &[880.0, 587.33],
        Severity::Warning => &[587.33, 880.0],
        Severity::Advisory => &[659.25],
    }
}

/// A short attack then an exponential fall, so a note reads as a chime and
/// never clicks.
fn envelope(t: f64) -> f64 {
    if !(0.0..NOTE_SECONDS).contains(&t) {
        0.0
    } else if t < ATTACK_SECONDS {
        PEAK_GAIN * t / ATTACK_SECONDS
    } else {
        let progress = (t - ATTACK_SECONDS) / (NOTE_SECONDS - ATTACK_SECONDS);
        PEAK_GAIN * (0.0001_f64 / PEAK_GAIN).powf(progress)
    }
}

/// 16-bit mono PCM WAV of the notes, each a decaying sine.
pub fn chime_wav(notes: &[f64]) -> Vec<u8> {
    let seconds = notes.len().saturating_sub(1) as f64 * NOTE_SPACING_SECONDS + NOTE_SECONDS;
    let count = (seconds * f64::from(SAMPLE_RATE)).round() as usize;
    let mut pcm = Vec::with_capacity(count * 2);
    for index in 0..count {
        let t = index as f64 / f64::from(SAMPLE_RATE);
        let value: f64 = notes
            .iter()
            .enumerate()
            .map(|(note, frequency)| {
                let local = t - note as f64 * NOTE_SPACING_SECONDS;
                envelope(local) * (std::f64::consts::TAU * frequency * local).sin()
            })
            .sum();
        let sample = (value.clamp(-1.0, 1.0) * f64::from(i16::MAX)).round() as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    let mut wav = Vec::with_capacity(44 + pcm.len());
    let data_len = pcm.len() as u32;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1_u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&pcm);
    wav
}

/// Play a chime through GTK's media stack. Fire and forget: a missing audio
/// backend is silent, never an error.
pub fn play(notes: &[f64]) {
    use gtk::prelude::*;
    let bytes = gtk::glib::Bytes::from_owned(chime_wav(notes));
    let stream = gtk::gio::MemoryInputStream::from_bytes(&bytes);
    let media = gtk::MediaFile::for_input_stream(&stream);
    media.play();
    // Keep the player alive until the chime has finished.
    gtk::glib::timeout_add_seconds_local_once(2, move || drop(media));
}

/// One chime per alert key per minute.
#[derive(Debug, Default)]
pub struct ChimeThrottle {
    last: HashMap<String, i64>,
}

impl ChimeThrottle {
    pub fn allow(&mut self, key: &str, now_ms: i64) -> bool {
        if self
            .last
            .get(key)
            .is_some_and(|last| now_ms - last < THROTTLE_MS)
        {
            return false;
        }
        self.last.insert(key.to_owned(), now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-SOUND-001: the tone table and a well-formed WAV of the right length.
    #[test]
    fn chimes_use_the_electron_notes_and_encode_as_wav() {
        assert_eq!(tones(Severity::Critical, false), [880.0, 587.33]);
        assert_eq!(tones(Severity::Warning, false), [587.33, 880.0]);
        assert_eq!(tones(Severity::Advisory, false), [659.25]);
        assert_eq!(tones(Severity::Critical, true), [523.25]);

        let wav = chime_wav(tones(Severity::Critical, false));
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1, "mono");
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            SAMPLE_RATE
        );
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16, "16-bit");
        let data_len = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]) as usize;
        assert_eq!(wav.len(), 44 + data_len);
        let seconds = data_len as f64 / 2.0 / f64::from(SAMPLE_RATE);
        assert!((seconds - (0.13 + 0.26)).abs() < 0.01, "{seconds}");
        let samples: Vec<i16> = wav[44..]
            .chunks(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(samples[0], 0, "starts silent, no click");
        let peak = samples
            .iter()
            .map(|sample| sample.unsigned_abs())
            .max()
            .unwrap();
        assert!(
            peak > 4_000 && peak < 7_000,
            "about 0.18 of full scale: {peak}"
        );
    }

    // SPEC-SOUND-002: one chime per alert per minute; keys are independent.
    #[test]
    fn the_throttle_spaces_each_alert() {
        let mut throttle = ChimeThrottle::default();
        assert!(throttle.allow("dish:a", 0));
        assert!(!throttle.allow("dish:a", 59_999));
        assert!(throttle.allow("dish:b", 1));
        assert!(throttle.allow("dish:a", 60_000));
    }
}
