//! 会议音频采集线程：麦克风（cpal）+ 系统音频（平台后端）→ Chunker 切段 → FLAC → 转写队列。
//!
//! cpal::Stream 和 Core Audio 对象都在这个专用线程里创建与销毁，所以后端 trait 不要求 Send。
//! 采集线程退出时会 drop 转写队列的发送端，转写循环据此自然结束。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::SampleFormat;

use super::chunker::{Chunker, PcmSegment, ReadyChunk};
use crate::audio::encoder;

#[cfg(target_os = "macos")]
pub mod macos_tap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Track {
    Mic,
    System,
}

/// 一帧原始 PCM（原采样率、原声道数，f32 交织）。
pub struct PcmChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

pub type FrameSender = mpsc::Sender<(Track, PcmChunk)>;

pub trait CaptureBackend {
    fn start(&mut self, tx: FrameSender) -> Result<(), String>;
    fn stop(&mut self);
}

fn stream_error(error: cpal::StreamError) {
    eprintln!("[Meeting] mic stream error: {}", error);
}

/// 麦克风：与听写共用同一套设备选择逻辑，但是独立的输入流，不影响 F4 听写。
pub struct MicBackend {
    device_name: String,
    stream: Option<cpal::Stream>,
}

impl MicBackend {
    pub fn new(device_name: &str) -> Self {
        Self {
            device_name: device_name.to_string(),
            stream: None,
        }
    }
}

impl CaptureBackend for MicBackend {
    fn start(&mut self, tx: FrameSender) -> Result<(), String> {
        let device = crate::audio::find_input_device(&self.device_name)
            .ok_or_else(|| "没有可用的麦克风".to_string())?;
        let default_config = device
            .default_input_config()
            .map_err(|e| format!("读取麦克风配置失败: {}", e))?;
        let sample_rate = default_config.sample_rate().0;
        let channels = default_config.channels();
        let sample_format = default_config.sample_format();
        let config: cpal::StreamConfig = default_config.into();

        let stream = match sample_format {
            SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let _ = tx.send((
                        Track::Mic,
                        PcmChunk {
                            samples: data.to_vec(),
                            sample_rate,
                            channels,
                        },
                    ));
                },
                stream_error,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let _ = tx.send((
                        Track::Mic,
                        PcmChunk {
                            samples: data.iter().map(|&s| s as f32 / 32768.0).collect(),
                            sample_rate,
                            channels,
                        },
                    ));
                },
                stream_error,
                None,
            ),
            other => return Err(format!("不支持的麦克风采样格式: {:?}", other)),
        }
        .map_err(|e| format!("创建麦克风输入流失败: {}", e))?;

        stream
            .play()
            .map_err(|e| format!("启动麦克风输入流失败: {}", e))?;
        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.pause();
            drop(stream);
        }
    }
}

/// 平台系统音频后端。
pub fn new_system_backend() -> Result<Box<dyn CaptureBackend>, String> {
    #[cfg(target_os = "macos")]
    {
        macos_tap::support_status()?;
        Ok(Box::new(macos_tap::ProcessTapBackend::new()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(crate::i18n::tr("err.systemAudioUnsupported").to_string())
    }
}

/// 当前系统是否支持系统音频录制；Err 里是不支持的原因。
pub fn system_audio_support() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos_tap::support_status()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(crate::i18n::tr("err.systemAudioUnsupported").to_string())
    }
}

pub struct CaptureConfig {
    pub mic_device: String,
    pub capture_mic: bool,
    pub capture_system: bool,
    pub chunk_seconds: u32,
}

/// 采集线程的输出端：切好的段送进 tokio 通道，pending 计数给状态显示用。
pub struct CaptureSink {
    pub tx: tokio::sync::mpsc::Sender<ReadyChunk>,
    pub pending: Arc<AtomicU32>,
}

/// 实际启动成功的音源与警告（例如系统音频权限被拒但麦克风可用）。
#[derive(Debug, Clone, Default)]
pub struct CaptureStarted {
    pub microphone: bool,
    pub system_audio: bool,
    pub warnings: Vec<String>,
}

pub struct CaptureHandle {
    stop_tx: mpsc::Sender<()>,
    join: Option<JoinHandle<()>>,
}

impl CaptureHandle {
    /// 通知采集线程停止并等它把最后一段送出去。
    pub fn stop(mut self) {
        let _ = self.stop_tx.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn deliver(sink: &CaptureSink, segment: PcmSegment) {
    match encoder::encode_flac(&segment.pcm) {
        Ok(flac) => {
            sink.pending.fetch_add(1, Ordering::SeqCst);
            let chunk = ReadyChunk {
                index: segment.index,
                start_offset_secs: segment.start_offset_secs,
                duration_secs: segment.duration_secs,
                flac,
            };
            if sink.tx.blocking_send(chunk).is_err() {
                sink.pending.fetch_sub(1, Ordering::SeqCst);
            }
        }
        Err(error) => eprintln!(
            "[Meeting] FLAC encode failed for chunk {}: {}",
            segment.index, error
        ),
    }
}

/// 启动采集线程；同步等待音源就绪（最多 8 秒），任何一路能用就算成功。
pub fn run_capture(
    cfg: CaptureConfig,
    sink: CaptureSink,
) -> Result<(CaptureHandle, CaptureStarted), String> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<CaptureStarted, String>>();

    let join = std::thread::Builder::new()
        .name("meeting-capture".to_string())
        .spawn(move || {
            let (frame_tx, frame_rx) = mpsc::channel::<(Track, PcmChunk)>();
            let mut backends: Vec<Box<dyn CaptureBackend>> = Vec::new();
            let mut started = CaptureStarted::default();

            if cfg.capture_mic {
                let mut mic = MicBackend::new(&cfg.mic_device);
                match mic.start(frame_tx.clone()) {
                    Ok(()) => {
                        started.microphone = true;
                        backends.push(Box::new(mic));
                    }
                    Err(error) => started
                        .warnings
                        .push(format!("麦克风采集启动失败：{}", error)),
                }
            }
            if cfg.capture_system {
                match new_system_backend() {
                    Ok(mut system) => match system.start(frame_tx.clone()) {
                        Ok(()) => {
                            started.system_audio = true;
                            backends.push(system);
                        }
                        Err(error) => started
                            .warnings
                            .push(format!("系统音频采集启动失败：{}", error)),
                    },
                    Err(error) => started.warnings.push(format!("系统音频不可用：{}", error)),
                }
            }
            drop(frame_tx);

            if backends.is_empty() {
                let _ = ready_tx.send(Err(format!(
                    "没有可用的音频源：{}",
                    started.warnings.join("；")
                )));
                return;
            }
            let _ = ready_tx.send(Ok(started));

            let mut chunker = Chunker::new(cfg.chunk_seconds);
            loop {
                if stop_rx.try_recv().is_ok() {
                    break;
                }
                match frame_rx.recv_timeout(Duration::from_millis(200)) {
                    Ok((track, frame)) => chunker.push(track, &frame),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                while let Some(segment) = chunker.poll_cut() {
                    deliver(&sink, segment);
                }
            }

            for backend in backends.iter_mut() {
                backend.stop();
            }
            drop(backends);

            // 收尾：排空已到达的帧，把剩余音频作为最后一段送出
            while let Ok((track, frame)) = frame_rx.try_recv() {
                chunker.push(track, &frame);
            }
            if let Some(segment) = chunker.flush() {
                deliver(&sink, segment);
            }
            // sink 在此 drop → 转写循环收到 None 结束
        })
        .map_err(|e| format!("创建采集线程失败: {}", e))?;

    match ready_rx.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(started)) => Ok((
            CaptureHandle {
                stop_tx,
                join: Some(join),
            },
            started,
        )),
        Ok(Err(error)) => {
            let _ = join.join();
            Err(error)
        }
        Err(_) => {
            let _ = stop_tx.send(());
            Err("音频采集启动超时".to_string())
        }
    }
}
