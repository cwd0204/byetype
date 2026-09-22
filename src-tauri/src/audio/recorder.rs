use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::SampleFormat;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;

use super::encoder;

/// 判定麦克风「真的在采集」的能量阈值（RMS）。蓝牙耳机在 HFP 握手期间
/// CoreAudio 会照常回调，但送来的是全零静音，只看回调到达会误判成已就绪。
/// 真实麦克风即使在安静环境也有本底噪声，不会是精确的 0。
const READY_RMS_THRESHOLD: f32 = 0.00002;

/// 电平映射到 0..1 的 dBFS 区间与曲线。实测语音短时 RMS：安静房间 -65..-52 dBFS，
/// 正常说话 -30..-22，大声 -20..-14。地板取 -55 让室内底噪压成平线，顶值取 -12
/// 让正常说话落在中间高度而不是长期贴顶；gamma 再把安静端压扁一些。
const DB_FLOOR: f32 = -55.0;
const DB_CEIL: f32 = -12.0;
const LEVEL_GAMMA: f32 = 1.6;
/// 低于这个显示值直接归零，保证没人说话时是一条平线。
/// 配合上面的区间与 gamma，归零的边界落在约 -48.3 dBFS：安静房间（-65..-52）一定是 0，
/// 再高一点的风扇底噪即使没被归零也只有 1px 左右，和基线一样粗，看着仍是平的。
const LEVEL_GATE: f32 = 0.05;
/// 显示值的快起慢落系数（配 40ms 一拍）。上行快，看得出起音；下行略慢，不抖。
const LEVEL_ATTACK: f32 = 0.60;
const LEVEL_RELEASE: f32 = 0.35;

fn rms(data: &[f32]) -> f32 {
    if data.is_empty() {
        return 0.0;
    }
    let sum: f32 = data.iter().map(|s| s * s).sum();
    (sum / data.len() as f32).sqrt()
}

/// RMS → 0..1 的波形显示高度。
pub(crate) fn level_from_rms(rms: f32) -> f32 {
    if !rms.is_finite() || rms <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * rms.max(1e-7).log10();
    let norm = ((db - DB_FLOOR) / (DB_CEIL - DB_FLOOR)).clamp(0.0, 1.0);
    let level = norm.powf(LEVEL_GAMMA);
    if level < LEVEL_GATE {
        0.0
    } else {
        level
    }
}

/// 在音频回调线程里上报电平。用 `fetch_max` 而不是 `store`，这样一拍里的多个缓冲
/// 取峰值而不是只剩最后一个。
///
/// `is_finite` 守卫是必须的：NaN / inf 的 bit 模式大于任何有限正浮点的 bit 模式，
/// 一旦被 `fetch_max` 写进去就再也降不下来，波形会永久顶格。
fn publish_level(level: &AtomicU32, rms: f32) {
    if rms.is_finite() && rms > 0.0 {
        level.fetch_max(rms.to_bits(), Ordering::Relaxed);
    }
}

/// 一阶平滑，上行与下行用不同系数。
pub(crate) fn smooth(prev: f32, target: f32) -> f32 {
    let coeff = if target > prev {
        LEVEL_ATTACK
    } else {
        LEVEL_RELEASE
    };
    let next = prev + (target - prev) * coeff;
    if next.is_finite() {
        next.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordingState {
    Idle,
    Recording,
}

/// 录音进行中实时外送的一帧原始音频。采样率与声道数只在 `start_with_feed`
/// 内部可见，而消费端（流式转写）要自己混单声道 + 重采样，所以每帧都带上。
#[derive(Debug, Clone)]
pub struct PcmFrame {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// 实时音频通道上的消息。
///
/// `Finish` 必须由停止方显式补发，**不能靠 sender 被 drop 来表示结束**：
/// cpal 在 macOS 上给非默认输入设备注册了掉线监听（`add_disconnect_listener`），
/// 那个闭包持有 `Stream` 的 Arc 而它自己又存在同一个 `StreamInner` 里，形成引用
/// 循环。于是 `drop(Stream)` 不会释放数据回调闭包，闭包里的 sender 也就永远活着，
/// channel 不会关闭。选了具体麦克风的用户会因此永远等不到「说完了」。
#[derive(Debug)]
pub enum LiveAudio {
    Frame(PcmFrame),
    Finish,
}

struct ActiveRecording {
    stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
    channels: u16,
    /// 只是跟着录音一起存放，让 sender 的生命周期看起来完整。真正的结束信号是
    /// 停止方补发的 `LiveAudio::Finish`（原因见 `LiveAudio` 的注释）。
    _live_tx: Option<UnboundedSender<LiveAudio>>,
}

pub struct AudioRecorder {
    state: Mutex<RecordingState>,
    active: Mutex<Option<ActiveRecording>>,
    start_instant: Mutex<Option<Instant>>,
    /// 首个音频回调是否已到达。蓝牙耳机要先完成 HFP 握手才会送数据，
    /// 这段时间 CoreAudio 不回调，用这个标志告诉 UI 何时真正可以开口。
    audio_started: Arc<AtomicBool>,
    /// 最近一批音频的 RMS（f32 的 bit 表示），给气泡波形用。
    /// 回调用 `fetch_max` 累积：UI 一拍 40ms 里有约 4 个音频缓冲，取最大值才不会
    /// 丢掉其中 3 个。读取方 `take_level` 取走即清零，所以设备停止回调时读到 0。
    level: Arc<AtomicU32>,
}

// SAFETY: All fields are protected by a Mutex or are atomics. cpal::Stream is
// !Send/!Sync only due to platform marker types, but we never access the stream
// without holding the lock, so cross-thread usage is safe.
unsafe impl Send for AudioRecorder {}
unsafe impl Sync for AudioRecorder {}

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(RecordingState::Idle),
            active: Mutex::new(None),
            start_instant: Mutex::new(None),
            audio_started: Arc::new(AtomicBool::new(false)),
            level: Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn is_recording(&self) -> bool {
        *self.state.lock().unwrap() == RecordingState::Recording
    }

    pub fn elapsed_since_start(&self) -> Option<std::time::Duration> {
        self.start_instant.lock().unwrap().as_ref().map(|t| t.elapsed())
    }

    /// 本次录音是否已收到首个音频回调（即设备真正开始采集）。
    pub fn audio_started(&self) -> bool {
        self.audio_started.load(Ordering::SeqCst)
    }

    /// 取走自上次调用以来的峰值 RMS，并清零。
    /// 清零是有意的：设备不再回调时这里就返回 0，波形会掉成平线，
    /// 而不是把最后一个值永久定在屏幕上假装还有声音。
    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }

    /// 开始录音。`live_tx` 不为 `None` 时，每个音频回调都会把原始帧同时送进
    /// 那个 channel，供边录边转写消费；整段缓冲照旧累积，两条路互不影响。
    pub fn start(
        &self,
        device_name: &str,
        live_tx: Option<UnboundedSender<LiveAudio>>,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if *state == RecordingState::Recording {
            return Err("Already recording".to_string());
        }

        let device = crate::audio::find_input_device(device_name)
            .ok_or_else(|| "No input device available".to_string())?;

        let default_config = device.default_input_config()
            .map_err(|e| format!("Failed to get default input config: {}", e))?;

        let sample_rate = default_config.sample_rate().0;
        let channels = default_config.channels();
        let sample_format = default_config.sample_format();
        let config: cpal::StreamConfig = default_config.into();

        let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        self.audio_started.store(false, Ordering::SeqCst);
        self.level.store(0, Ordering::Relaxed);

        let stream = match sample_format {
            SampleFormat::F32 => {
                let sc = Arc::clone(&samples);
                let started = Arc::clone(&self.audio_started);
                let level = Arc::clone(&self.level);
                let tx = live_tx.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        // 每帧只算一次 RMS，就绪判定与波形电平共用。
                        let r = rms(data);
                        if !started.load(Ordering::Relaxed) && r > READY_RMS_THRESHOLD {
                            started.store(true, Ordering::SeqCst);
                        }
                        publish_level(&level, r);
                        // 用 lock 而不是 try_lock：try_lock 在锁竞争时会静默丢掉整帧。
                        // stop() 是先停流再取锁的，这里实际不存在竞争。
                        sc.lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .extend_from_slice(data);
                        if let Some(tx) = &tx {
                            // UnboundedSender::send 不阻塞，可以在音频回调线程里调。
                            let _ = tx.send(LiveAudio::Frame(PcmFrame {
                                samples: data.to_vec(),
                                sample_rate,
                                channels,
                            }));
                        }
                    },
                    |err| eprintln!("Audio stream error: {}", err),
                    None,
                )
            }
            SampleFormat::I16 => {
                let sc = Arc::clone(&samples);
                let started = Arc::clone(&self.audio_started);
                let level = Arc::clone(&self.level);
                let tx = live_tx.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        // 只转一次浮点，整段缓冲与实时通道共用。
                        let frame: Vec<f32> =
                            data.iter().map(|&s| s as f32 / 32768.0).collect();
                        let r = rms(&frame);
                        if !started.load(Ordering::Relaxed) && r > READY_RMS_THRESHOLD {
                            started.store(true, Ordering::SeqCst);
                        }
                        publish_level(&level, r);
                        sc.lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .extend_from_slice(&frame);
                        if let Some(tx) = &tx {
                            let _ = tx.send(LiveAudio::Frame(PcmFrame {
                                samples: frame,
                                sample_rate,
                                channels,
                            }));
                        }
                    },
                    |err| eprintln!("Audio stream error: {}", err),
                    None,
                )
            }
            _ => return Err(format!("Unsupported sample format: {:?}", sample_format)),
        }.map_err(|e| format!("Failed to build input stream: {}", e))?;

        stream.play().map_err(|e| format!("Failed to start stream: {}", e))?;

        *state = RecordingState::Recording;
        *self.start_instant.lock().unwrap() = Some(Instant::now());
        let mut active = self.active.lock().unwrap();
        *active = Some(ActiveRecording {
            stream,
            samples,
            sample_rate,
            channels,
            _live_tx: live_tx,
        });

        Ok(())
    }

    pub fn stop(&self) -> Result<String, String> {
        let (samples_data, sample_rate, channels) = {
            let mut state = self.state.lock().unwrap();
            if *state != RecordingState::Recording {
                return Err("Not recording".to_string());
            }

            let mut active_guard = self.active.lock().unwrap();
            let recording = active_guard.take()
                .ok_or_else(|| "No active recording".to_string())?;

            // Explicitly pause before drop so CoreAudio calls AudioOutputUnitStop,
            // which releases the microphone and clears the macOS orange indicator.
            let _ = recording.stream.pause();
            drop(recording.stream);
            self.level.store(0, Ordering::Relaxed);

            let mut samples = recording.samples.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *state = RecordingState::Idle;
            *self.start_instant.lock().unwrap() = None;
            // take 而不是 clone：cpal 在 macOS 上会把数据回调闭包留在一个引用
            // 循环里（见 LiveAudio 注释），闭包持有的这份 buffer 因此不会随录音
            // 结束释放。取走内容既省一次大拷贝，也把这段内存还给系统。
            (
                std::mem::take(&mut *samples),
                recording.sample_rate,
                recording.channels,
            )
        };

        if samples_data.is_empty() {
            return Err("No audio data captured".to_string());
        }

        // Mix to mono if multi-channel
        let mono = if channels > 1 {
            super::mix_to_mono(&samples_data, channels)
        } else {
            samples_data
        };

        // Resample to 16kHz
        let resampled = super::resample(&mono, sample_rate, 16_000);

        // Convert f32 [-1.0, 1.0] to i16
        let pcm: Vec<i16> = resampled.iter().map(|&s| {
            (s.clamp(-1.0, 1.0) * 32767.0) as i16
        }).collect();

        let flac_bytes = encoder::encode_flac(&pcm)?;
        Ok(encoder::audio_to_base64(&flac_bytes))
    }

    pub fn cancel(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if *state != RecordingState::Recording {
            return Err("Not recording".to_string());
        }

        let mut active_guard = self.active.lock().unwrap();
        if let Some(recording) = active_guard.take() {
            // Mirror stop()'s stream shutdown to release the mic and clear the
            // macOS orange indicator. Samples are dropped without encoding.
            let _ = recording.stream.pause();
            drop(recording.stream);
        }
        self.level.store(0, Ordering::Relaxed);

        *state = RecordingState::Idle;
        *self.start_instant.lock().unwrap() = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_recorder_is_idle() {
        let recorder = AudioRecorder::new();
        assert!(!recorder.is_recording());
    }

    #[test]
    fn test_stop_when_not_recording_returns_error() {
        let recorder = AudioRecorder::new();
        assert!(recorder.stop().is_err());
    }

    #[test]
    fn test_mix_to_mono_stereo() {
        let stereo = vec![0.5, -0.5, 1.0, 0.0];
        let mono = crate::audio::mix_to_mono(&stereo, 2);
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.0).abs() < 1e-6);
        assert!((mono[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_resample_downsample() {
        let input = vec![0.0, 0.5, 1.0, 0.5];
        let output = crate::audio::resample(&input, 48_000, 16_000);
        assert!(!output.is_empty());
        assert!(output.len() < input.len());
    }

    #[test]
    fn test_resample_same_rate() {
        let input = vec![0.1, 0.2, 0.3];
        let output = crate::audio::resample(&input, 16_000, 16_000);
        assert_eq!(output, input);
    }

    #[test]
    fn test_elapsed_since_start_is_none_when_idle() {
        let recorder = AudioRecorder::new();
        assert!(recorder.elapsed_since_start().is_none());
    }

    #[test]
    fn test_cancel_when_not_recording_returns_error() {
        let recorder = AudioRecorder::new();
        assert!(recorder.cancel().is_err());
    }

    /// dBFS → 线性 RMS，方便按分贝写断言。
    fn from_db(db: f32) -> f32 {
        10f32.powf(db / 20.0)
    }

    #[test]
    fn level_is_zero_for_silence_and_room_noise() {
        assert_eq!(level_from_rms(0.0), 0.0);
        assert_eq!(level_from_rms(1e-7), 0.0);
        // 地板以下与噪声门以内都必须是精确的 0，静音才会是一条平线。
        // 安静房间实测在 -65..-52 dBFS 这一段。
        assert_eq!(level_from_rms(from_db(-60.0)), 0.0);
        assert_eq!(level_from_rms(from_db(-52.0)), 0.0);
        assert_eq!(level_from_rms(from_db(-50.0)), 0.0);
        // 门限边界之上一点（风扇级底噪）不归零，但高度要小到和基线一样粗：
        // 0.06 × 20px 画布 ≈ 1px。
        assert!(level_from_rms(from_db(-48.0)) < 0.06);
    }

    #[test]
    fn level_puts_normal_speech_mid_height() {
        // 正常说话约 -26 dBFS，应该落在中间高度附近，而不是贴顶或贴底
        let mid = level_from_rms(from_db(-26.0));
        assert!(
            (0.40..=0.65).contains(&mid),
            "normal speech mapped to {mid}"
        );
        // 大声与满量程都到顶，且不越界
        assert_eq!(level_from_rms(from_db(-12.0)), 1.0);
        assert_eq!(level_from_rms(1.0), 1.0);
    }

    #[test]
    fn level_is_monotonic_and_bounded() {
        let mut prev = 0.0;
        for db in [-50, -45, -40, -32, -26, -22, -18, -15, -12] {
            let v = level_from_rms(from_db(db as f32));
            assert!((0.0..=1.0).contains(&v), "{db} dBFS → {v}");
            assert!(v >= prev, "not monotonic at {db} dBFS: {prev} → {v}");
            prev = v;
        }
    }

    #[test]
    fn level_rejects_non_finite_input() {
        assert_eq!(level_from_rms(f32::NAN), 0.0);
        assert_eq!(level_from_rms(f32::INFINITY), 0.0);
        assert_eq!(level_from_rms(f32::NEG_INFINITY), 0.0);
        assert_eq!(level_from_rms(-1.0), 0.0);
    }

    #[test]
    fn smooth_rises_faster_than_it_falls() {
        let up = smooth(0.0, 1.0);
        let down = smooth(1.0, 0.0);
        assert!(up > 0.5, "attack too slow: {up}");
        assert!(down > 0.5, "release too fast: {down}");
        assert!(up > 1.0 - down, "attack should outpace release");
    }

    #[test]
    fn smooth_converges_and_stays_bounded() {
        let mut v = 0.0;
        for _ in 0..20 {
            v = smooth(v, 1.0);
            assert!((0.0..=1.0).contains(&v));
        }
        assert!(v > 0.99, "did not converge up: {v}");
        for _ in 0..40 {
            v = smooth(v, 0.0);
        }
        assert!(v < 0.01, "did not converge down: {v}");
        assert_eq!(smooth(f32::NAN, 0.5), 0.0);
    }

    #[test]
    fn publish_level_keeps_peak_and_rejects_non_finite() {
        let cell = AtomicU32::new(0);
        publish_level(&cell, 0.01);
        publish_level(&cell, 0.05);
        publish_level(&cell, 0.02); // 峰值不应被较小值盖掉
        assert_eq!(f32::from_bits(cell.load(Ordering::Relaxed)), 0.05);
        // NaN / inf 的 bit 模式大于任何有限正浮点，必须挡在门外
        publish_level(&cell, f32::NAN);
        publish_level(&cell, f32::INFINITY);
        assert_eq!(f32::from_bits(cell.load(Ordering::Relaxed)), 0.05);
    }

    #[test]
    fn take_level_clears_so_a_dead_mic_reads_zero() {
        let recorder = AudioRecorder::new();
        assert_eq!(recorder.take_level(), 0.0);
        publish_level(&recorder.level, 0.2);
        assert_eq!(recorder.take_level(), 0.2);
        // 设备不再回调后，后续读取应当是 0 而不是停在上一个值
        assert_eq!(recorder.take_level(), 0.0);
    }
}
