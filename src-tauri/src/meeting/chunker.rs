//! 把麦克风与系统音频两路 PCM 对齐、混音、重采样到 16 kHz 单声道，并按静音边界切成段。
//!
//! 内存里只保留当前这一段（两路 f32），全会不累积。切段规则：
//! - 达到目标时长且末尾 600 ms 混音 RMS 低于阈值 → 在静音处切
//! - 超过硬上限（目标 + 90 s）→ 强切
//! - 结束时剩余 ≥ 3 s 才出最后一段

use super::capture::{PcmChunk, Track};
use crate::audio::{mix_to_mono, resample};

pub const TARGET_RATE: u32 = 16_000;
const SILENCE_WINDOW_SAMPLES: usize = TARGET_RATE as usize * 6 / 10;
const SILENCE_RMS: f32 = 0.005;
const MIN_FLUSH_SAMPLES: usize = TARGET_RATE as usize * 3;
const HARD_MAX_EXTRA_SECS: usize = 90;
const MIX_GAIN: f32 = 0.85;

/// 切好的一段 PCM（16 kHz mono i16），尚未编码。
pub struct PcmSegment {
    pub index: u32,
    pub start_offset_secs: u64,
    pub duration_secs: f32,
    pub pcm: Vec<i16>,
}

/// 已 FLAC 编码、准备送去转写的一段。
pub struct ReadyChunk {
    pub index: u32,
    pub start_offset_secs: u64,
    pub duration_secs: f32,
    pub flac: Vec<u8>,
}

#[derive(Default)]
struct TrackBuf {
    /// 原采样率单声道，累积到 ≥ 1 s 再重采样，减少线性插值的边界伪影
    pending: Vec<f32>,
    rate_in: u32,
    /// 16 kHz 单声道
    buf: Vec<f32>,
}

impl TrackBuf {
    fn push(&mut self, frame: &PcmChunk) {
        if frame.samples.is_empty() || frame.channels == 0 || frame.sample_rate == 0 {
            return;
        }
        if self.rate_in != frame.sample_rate {
            self.drain_pending();
            self.rate_in = frame.sample_rate;
        }
        if frame.channels > 1 {
            self.pending
                .extend(mix_to_mono(&frame.samples, frame.channels));
        } else {
            self.pending.extend_from_slice(&frame.samples);
        }
        if self.pending.len() >= self.rate_in as usize {
            self.drain_pending();
        }
    }

    fn drain_pending(&mut self) {
        if self.pending.is_empty() || self.rate_in == 0 {
            return;
        }
        let resampled = resample(&self.pending, self.rate_in, TARGET_RATE);
        self.buf.extend(resampled);
        self.pending.clear();
    }

    /// 取出前 n 个样本，不足的补零（另一路更长时对齐用）。
    fn take(&mut self, n: usize) -> Vec<f32> {
        let available = n.min(self.buf.len());
        let mut out: Vec<f32> = self.buf.drain(..available).collect();
        out.resize(n, 0.0);
        out
    }
}

pub struct Chunker {
    target: usize,
    hard_max: usize,
    mic: TrackBuf,
    sys: TrackBuf,
    index: u32,
    emitted: u64,
}

impl Chunker {
    pub fn new(target_secs: u32) -> Self {
        let target = target_secs.max(30) as usize * TARGET_RATE as usize;
        Self {
            target,
            hard_max: target + HARD_MAX_EXTRA_SECS * TARGET_RATE as usize,
            mic: TrackBuf::default(),
            sys: TrackBuf::default(),
            index: 0,
            emitted: 0,
        }
    }

    pub fn push(&mut self, track: Track, frame: &PcmChunk) {
        match track {
            Track::Mic => self.mic.push(frame),
            Track::System => self.sys.push(frame),
        }
    }

    fn available(&self) -> usize {
        self.mic.buf.len().max(self.sys.buf.len())
    }

    /// 到点且末尾静音（或超硬上限）时切出一段。
    pub fn poll_cut(&mut self) -> Option<PcmSegment> {
        let len = self.available();
        if len < self.target {
            return None;
        }
        if len < self.hard_max && !self.tail_is_silent(len) {
            return None;
        }
        Some(self.cut(len))
    }

    /// 结束时把剩余音频切出来；太短（< 3 s）就丢掉。
    pub fn flush(&mut self) -> Option<PcmSegment> {
        self.mic.drain_pending();
        self.sys.drain_pending();
        let len = self.available();
        if len < MIN_FLUSH_SAMPLES {
            return None;
        }
        Some(self.cut(len))
    }

    fn tail_is_silent(&self, len: usize) -> bool {
        let start = len.saturating_sub(SILENCE_WINDOW_SAMPLES);
        let mut acc = 0f32;
        let mut n = 0usize;
        for i in start..len {
            let m = self.mic.buf.get(i).copied().unwrap_or(0.0);
            let s = self.sys.buf.get(i).copied().unwrap_or(0.0);
            let v = mix_sample(m, s);
            acc += v * v;
            n += 1;
        }
        n > 0 && (acc / n as f32).sqrt() < SILENCE_RMS
    }

    fn cut(&mut self, n: usize) -> PcmSegment {
        let mic = self.mic.take(n);
        let sys = self.sys.take(n);
        let segment = PcmSegment {
            index: self.index,
            start_offset_secs: self.emitted / TARGET_RATE as u64,
            duration_secs: n as f32 / TARGET_RATE as f32,
            pcm: mix(&mic, &sys),
        };
        self.index += 1;
        self.emitted += n as u64;
        segment
    }
}

fn mix_sample(mic: f32, sys: f32) -> f32 {
    (MIX_GAIN * mic + MIX_GAIN * sys).clamp(-1.0, 1.0)
}

/// 两路等长混音成 i16；短的一路视为静音。
pub(crate) fn mix(mic: &[f32], sys: &[f32]) -> Vec<i16> {
    let len = mic.len().max(sys.len());
    (0..len)
        .map(|i| {
            let m = mic.get(i).copied().unwrap_or(0.0);
            let s = sys.get(i).copied().unwrap_or(0.0);
            (mix_sample(m, s) * 32767.0) as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(samples: Vec<f32>, rate: u32) -> PcmChunk {
        PcmChunk {
            samples,
            sample_rate: rate,
            channels: 1,
        }
    }

    fn tone(len: usize, amp: f32) -> Vec<f32> {
        (0..len).map(|i| amp * ((i as f32) * 0.3).sin()).collect()
    }

    #[test]
    fn mix_clamps_and_pads_shorter_track() {
        let mixed = mix(&[1.0, 1.0, 0.5], &[1.0]);
        assert_eq!(mixed.len(), 3);
        assert_eq!(mixed[0], 32767);
        assert!((mixed[1] as f32 / 32767.0 - 0.85).abs() < 0.01);
        assert!((mixed[2] as f32 / 32767.0 - 0.425).abs() < 0.01);
    }

    #[test]
    fn cuts_at_target_when_tail_is_silent() {
        let mut chunker = Chunker::new(30);
        let rate = TARGET_RATE as usize;
        // 29 s 有声 + 2 s 静音 → 达到 30 s 目标且末尾静音
        chunker.push(Track::Mic, &frame(tone(rate * 29, 0.3), TARGET_RATE));
        assert!(chunker.poll_cut().is_none());
        chunker.push(Track::Mic, &frame(vec![0.0; rate * 2], TARGET_RATE));
        let seg = chunker.poll_cut().expect("should cut");
        assert_eq!(seg.index, 0);
        assert_eq!(seg.start_offset_secs, 0);
        assert!((seg.duration_secs - 31.0).abs() < 0.01);
        assert_eq!(seg.pcm.len(), rate * 31);
        // 下一段从 31 s 开始
        chunker.push(Track::Mic, &frame(vec![0.0; rate * 31], TARGET_RATE));
        let next = chunker.poll_cut().expect("second cut");
        assert_eq!(next.index, 1);
        assert_eq!(next.start_offset_secs, 31);
    }

    #[test]
    fn waits_for_silence_until_hard_max() {
        let mut chunker = Chunker::new(30);
        let rate = TARGET_RATE as usize;
        chunker.push(Track::Mic, &frame(tone(rate * 60, 0.3), TARGET_RATE));
        assert!(chunker.poll_cut().is_none(), "no silence yet");
        chunker.push(Track::Mic, &frame(tone(rate * 61, 0.3), TARGET_RATE));
        let seg = chunker.poll_cut().expect("hard max reached");
        assert!(seg.duration_secs >= 120.0);
    }

    #[test]
    fn flush_drops_very_short_remainder() {
        let mut chunker = Chunker::new(30);
        chunker.push(Track::Mic, &frame(tone(TARGET_RATE as usize * 2, 0.3), TARGET_RATE));
        assert!(chunker.flush().is_none());

        let mut chunker = Chunker::new(30);
        chunker.push(Track::Mic, &frame(tone(TARGET_RATE as usize * 4, 0.3), TARGET_RATE));
        assert!(chunker.flush().is_some());
    }

    #[test]
    fn resamples_other_rates_to_16k() {
        let mut chunker = Chunker::new(30);
        chunker.push(Track::System, &frame(tone(48_000 * 3, 0.3), 48_000));
        let seg = chunker.flush().expect("3 s of audio");
        assert!((seg.pcm.len() as i64 - 48_000).abs() < 100);
    }

    #[test]
    fn aligns_two_tracks_by_padding() {
        let mut chunker = Chunker::new(30);
        let rate = TARGET_RATE as usize;
        chunker.push(Track::Mic, &frame(tone(rate * 4, 0.3), TARGET_RATE));
        chunker.push(Track::System, &frame(tone(rate * 3, 0.3), TARGET_RATE));
        let seg = chunker.flush().expect("4 s of mixed audio");
        assert_eq!(seg.pcm.len(), rate * 4);
    }
}
