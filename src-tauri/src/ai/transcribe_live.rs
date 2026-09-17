//! 边录边转写：录音期间就建立 `StartStreamTranscription` 会话，把麦克风的帧
//! 实时喂进去，松手时只等尾段最终结果。
//!
//! 为什么值得这么做：Transcribe 按实时速率消费音频，「录完再整段灌」意味着
//! 转写耗时约等于说话时长（本机实测 1.02–1.06 倍）。实时喂之后，说话的同时
//! 识别就在进行，松手后只剩约 1 秒的尾段。
//!
//! 这条路径**只服务听写**，且只是加速手段：任何一步失败（凭证、建流、流中途
//! 报错、结果为空）都由调用方静默回退到 `transcribe_aws::transcribe` 的整段
//! 路径，用户最多觉得慢一点。会议模式与本机 HTTP 接口继续走整段路径。

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use aws_sdk_transcribestreaming::primitives::Blob;
use aws_sdk_transcribestreaming::types::error::AudioStreamError;
use aws_sdk_transcribestreaming::types::{AudioEvent, AudioStream, MediaEncoding};
use aws_sdk_transcribestreaming::Client;
use futures_util::Stream;
use tauri::async_runtime::JoinHandle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use super::aws;
use super::transcribe_aws::{
    apply_language, collect_transcript, TranscribeOptions, LABEL, SAMPLE_RATE_HZ,
};
use crate::audio::recorder::{LiveAudio, PcmFrame};
use crate::config::types::AwsConfig;
use crate::i18n::Lang;

/// 每个音频事件的字节数。3200 字节 = 1600 个 i16 = 100 ms @ 16 kHz 单声道，
/// 落在 AWS「50–200 ms 均匀分块」的建议区间中段。
const CHUNK_BYTES: usize = 3200;

/// 送进 Transcribe 的音频事件流。
///
/// 手写 `Stream` 而不是用 `futures_util::stream::unfold`，是因为
/// `EventStreamSender` 要求 `S: Stream + Send + Sync + 'static`，而 unfold 出来的
/// 流里裹着 async 块产生的 future，编译器不保证它 `Sync`。这个结构体的字段都是
/// 普通数据，两个 marker 都是显然成立的。
struct AudioEventStream {
    rx: UnboundedReceiver<LiveAudio>,
    /// 原采样率的单声道样本，攒够 1 秒再重采样。逐帧重采样会在块边界留下杂音，
    /// 这条规则与 `meeting/chunker.rs` 的 `TrackBuf` 一致。
    pending: Vec<f32>,
    rate_in: u32,
    /// 已重采样成 16 kHz 的 i16 小端字节，按 `CHUNK_BYTES` 切成事件发出去。
    out: Vec<u8>,
    /// 已收到结束信号，`out` 排空后流就结束。
    done: bool,
}

impl AudioEventStream {
    fn new(rx: UnboundedReceiver<LiveAudio>) -> Self {
        Self {
            rx,
            pending: Vec::new(),
            rate_in: 0,
            out: Vec::new(),
            done: false,
        }
    }

    fn push(&mut self, frame: PcmFrame) {
        if frame.samples.is_empty() || frame.channels == 0 || frame.sample_rate == 0 {
            return;
        }
        if self.rate_in != frame.sample_rate {
            self.drain_pending();
            self.rate_in = frame.sample_rate;
        }
        if frame.channels > 1 {
            self.pending
                .extend(crate::audio::mix_to_mono(&frame.samples, frame.channels));
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
        let resampled = crate::audio::resample(&self.pending, self.rate_in, SAMPLE_RATE_HZ as u32);
        self.out.reserve(resampled.len() * 2);
        for sample in resampled {
            let pcm = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
            self.out.extend_from_slice(&pcm.to_le_bytes());
        }
        self.pending.clear();
    }

    /// 取出一块音频事件，不足一块时把剩下的全发出去（只在结束时会走到）。
    fn take_chunk(&mut self) -> AudioStream {
        let n = self.out.len().min(CHUNK_BYTES);
        let chunk: Vec<u8> = self.out.drain(..n).collect();
        AudioStream::AudioEvent(AudioEvent::builder().audio_chunk(Blob::new(chunk)).build())
    }
}

impl Stream for AudioEventStream {
    type Item = Result<AudioStream, AudioStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // 所有字段都是 Unpin，可以安全拿到 &mut Self。
        let me = self.get_mut();
        loop {
            if me.out.len() >= CHUNK_BYTES {
                return Poll::Ready(Some(Ok(me.take_chunk())));
            }
            if me.done {
                if me.out.is_empty() {
                    return Poll::Ready(None);
                }
                return Poll::Ready(Some(Ok(me.take_chunk())));
            }
            match me.rx.poll_recv(cx) {
                Poll::Ready(Some(LiveAudio::Frame(frame))) => me.push(frame),
                // channel 关闭也当结束处理，但正常路径靠的是显式的 `Finish`
                // （原因见 `LiveAudio` 的注释：cpal 的引用循环会让 sender 永不释放）。
                Poll::Ready(Some(LiveAudio::Finish)) | Poll::Ready(None) => {
                    me.drain_pending();
                    me.done = true;
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// 看门狗：句柄一消失就掐掉后台任务。
///
/// 停止点漏了取走句柄、或者管道提前返回时，没有这层兜底的话那条 Transcribe 流
/// 会一直挂在服务端等音频，直到服务端超时。有了它，漏一处最多是丢掉加速，
/// 不会留下悬挂的流。任务已经跑完时 `abort()` 是空操作。
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// 一条正在进行的流式转写会话。
pub struct LiveTranscribe {
    /// 与录音回调共用同一个 channel，用来补发 `Finish`。
    tx: UnboundedSender<LiveAudio>,
    result: oneshot::Receiver<Result<String, String>>,
    task: AbortOnDrop,
}

impl LiveTranscribe {
    /// 告诉服务端音频到此结束，等尾段的最终结果。
    ///
    /// 调用前必须先停掉录音流（`recorder.stop()` / `cancel()`），否则暂停之后
    /// 才到达的帧会排在 `Finish` 之后被丢弃。超时按失败处理，调用方回退整段路径。
    pub async fn finish(self, timeout: Duration) -> Result<String, String> {
        // `_task` 要一直活到拿到结果为止，出了作用域看门狗才收尾。
        let LiveTranscribe {
            tx,
            result,
            task: _task,
        } = self;
        let _ = tx.send(LiveAudio::Finish);
        match tokio::time::timeout(timeout, result).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Err(format!("{} live stream ended without a result", LABEL)),
            Err(_) => Err(format!(
                "{} live stream timed out after {}s",
                LABEL,
                timeout.as_secs()
            )),
        }
    }

    /// 放弃这条流（用户取消 / PTT 误触）。不等结果，`AbortOnDrop` 负责掐掉后台任务。
    pub fn abort(self) {
        drop(self);
    }
}

/// 建立一条流式转写会话。
///
/// `tx` / `rx` 是同一个 channel 的两端：`tx` 在开始录音时就交给了
/// `AudioRecorder::start_with_feed`，所以开流之前采到的帧已经在 channel 里排着，
/// 这里接手后会一次补发，开头不会丢字。
///
/// 本函数是同步的，不会阻塞调用者：凭证获取、建流都在后台任务里做，失败通过
/// `finish()` 的 `Err` 反馈给调用方去回退。
pub fn start(
    cfg: AwsConfig,
    tx: UnboundedSender<LiveAudio>,
    rx: UnboundedReceiver<LiveAudio>,
    opts: TranscribeOptions,
    lang: Lang,
) -> LiveTranscribe {
    let (result_tx, result_rx) = oneshot::channel();
    let task = tauri::async_runtime::spawn(async move {
        let outcome = run(&cfg, AudioEventStream::new(rx), opts, lang).await;
        let _ = result_tx.send(outcome);
    });
    LiveTranscribe {
        tx,
        result: result_rx,
        task: AbortOnDrop(task),
    }
}

async fn run(
    cfg: &AwsConfig,
    input: AudioEventStream,
    opts: TranscribeOptions,
    lang: Lang,
) -> Result<String, String> {
    let sdk = aws::sdk_config(&cfg.transcribe_profile, &cfg.transcribe_region).await?;
    let client = Client::new(&sdk);

    let request = client
        .start_stream_transcription()
        .media_encoding(MediaEncoding::Pcm)
        .media_sample_rate_hertz(SAMPLE_RATE_HZ)
        .show_speaker_label(opts.speaker_labels)
        .audio_stream(input.into());

    let mut response = apply_language(request, cfg)
        .send()
        .await
        .map_err(|e| aws::map_sdk_error(LABEL, e))?;

    collect_transcript(&mut response, opts, lang).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn frame(samples: Vec<f32>, sample_rate: u32, channels: u16) -> LiveAudio {
        LiveAudio::Frame(PcmFrame {
            samples,
            sample_rate,
            channels,
        })
    }

    fn chunk_bytes(event: &AudioStream) -> usize {
        match event {
            AudioStream::AudioEvent(e) => e.audio_chunk().map(|b| b.as_ref().len()).unwrap_or(0),
            _ => 0,
        }
    }

    #[tokio::test]
    async fn emits_full_chunks_then_short_tail() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        // 16 kHz 单声道 2400 个样本 = 4800 字节 = 1 块整 + 1600 字节的尾巴。
        tx.send(frame(vec![0.5; 2400], 16_000, 1)).unwrap();
        tx.send(LiveAudio::Finish).unwrap();

        let events: Vec<_> = AudioEventStream::new(rx).collect().await;
        let sizes: Vec<usize> = events
            .iter()
            .map(|e| chunk_bytes(e.as_ref().unwrap()))
            .collect();
        assert_eq!(sizes, vec![CHUNK_BYTES, 1600]);
    }

    #[tokio::test]
    async fn downmixes_and_resamples_to_16k_mono() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        // 48 kHz 立体声 1 秒 → 16 kHz 单声道 1 秒 = 16000 个 i16 = 32000 字节。
        tx.send(frame(vec![0.25; 48_000 * 2], 48_000, 2)).unwrap();
        tx.send(LiveAudio::Finish).unwrap();

        let events: Vec<_> = AudioEventStream::new(rx).collect().await;
        let total: usize = events
            .iter()
            .map(|e| chunk_bytes(e.as_ref().unwrap()))
            .sum();
        assert_eq!(total, 32_000);
    }

    #[tokio::test]
    async fn ends_on_finish_even_without_audio() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(LiveAudio::Finish).unwrap();
        let events: Vec<_> = AudioEventStream::new(rx).collect().await;
        assert!(events.is_empty());
    }

    /// sender 被 drop 也要收尾。正常路径不依赖这条（cpal 的引用循环让 sender 不会
    /// 被释放），但流不能因此挂死。
    #[tokio::test]
    async fn ends_when_sender_dropped() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(frame(vec![0.1; 800], 16_000, 1)).unwrap();
        drop(tx);
        let events: Vec<_> = AudioEventStream::new(rx).collect().await;
        assert_eq!(events.len(), 1);
        assert_eq!(chunk_bytes(events[0].as_ref().unwrap()), 1600);
    }

    /// 真机联调：把一段 16 kHz 单声道 WAV 按实时节奏喂进流式会话，量「最后一帧
    /// 发出去之后还要等多久」—— 这就是用户松手后感受到的转写延迟，整段路径下
    /// 这个数约等于音频时长。样本与 profile 的约定同 transcribe_aws.rs 的 live 测试。
    /// 运行：cargo test --lib live_stream_transcribe -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "需要 AWS 凭证、网络与音频样本"]
    async fn live_stream_transcribe_chinese_sample() {
        use std::time::Instant;

        let cfg = AwsConfig {
            transcribe_profile: std::env::var("BYETYPE_TEST_TRANSCRIBE_PROFILE")
                .unwrap_or_else(|_| "bedrock-kuut".into()),
            transcribe_region: std::env::var("BYETYPE_TEST_TRANSCRIBE_REGION")
                .unwrap_or_else(|_| "ap-northeast-1".into()),
            ..AwsConfig::default()
        };
        let wav_path =
            std::env::var("BYETYPE_TEST_WAV").unwrap_or_else(|_| "/tmp/byetype-test-zh.wav".into());
        let wav = std::fs::read(&wav_path).expect("test wav sample");
        let token = tokio_util::sync::CancellationToken::new();
        let normalized = crate::audio::input::normalize_audio(wav, "audio/wav", 60, &token)
            .expect("normalize to flac");
        let pcm = crate::audio::input::decode_to_pcm16(normalized.flac, "audio/flac", 60, &token)
            .expect("decode to pcm");
        let samples: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
        let audio_secs = samples.len() as f32 / SAMPLE_RATE_HZ as f32;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let live = start(
            cfg,
            tx.clone(),
            rx,
            TranscribeOptions::default(),
            Lang::ZhCn,
        );

        // 100 ms 一帧、按墙钟节奏发，模拟 cpal 回调的到达速率。
        let frame_len = SAMPLE_RATE_HZ as usize / 10;
        let started = Instant::now();
        for (i, chunk) in samples.chunks(frame_len).enumerate() {
            tx.send(frame(chunk.to_vec(), SAMPLE_RATE_HZ as u32, 1))
                .unwrap();
            tokio::time::sleep_until(
                (started + Duration::from_millis(100 * (i as u64 + 1))).into(),
            )
            .await;
        }
        let last_frame_sent = Instant::now();
        let text = live
            .finish(Duration::from_secs(30))
            .await
            .expect("live transcribe");
        let tail = last_frame_sent.elapsed();
        eprintln!(
            "[live] streamed {:.1}s of audio; tail latency after last frame: {} ms → {:?}",
            audio_secs,
            tail.as_millis(),
            text
        );
        assert!(
            text.contains("三点") || text.contains("开会"),
            "unexpected transcript: {text}"
        );
        assert!(
            tail < Duration::from_secs(5),
            "tail latency too high: {tail:?}"
        );
    }
}
