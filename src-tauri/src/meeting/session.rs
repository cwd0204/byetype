//! 会议会话状态机：Idle → Recording → Finalizing → Idle。
//!
//! - `start` 建目录、起采集线程与转写任务
//! - 转写任务串行消费分段（保证顺序，并把上一段结尾当作下一段的上下文）
//! - `stop` 进入 Finalizing，后台任务等采集线程 flush、等转写队列排空、生成纪要、导出笔记
//! - `discard` 直接丢弃
//! - 探测器通过 `on_meeting_detected` / `on_meeting_gone` 驱动自动开始与结束；手动开始的会话不受探测影响

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::Local;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use super::capture::{run_capture, CaptureConfig, CaptureHandle, CaptureSink};
use super::chunker::ReadyChunk;
use super::store::{MeetingMeta, MeetingStore, TranscriptSegment, DEFAULT_NOTES_DIR};
use super::{summary, transcribe, window};
use crate::ai;
use crate::config::types::AppConfig;
use crate::config::ConfigManager;
use crate::i18n::{tr, tr_fmt};

/// 收尾时等待转写队列排空的上限
const DRAIN_TIMEOUT: Duration = Duration::from_secs(600);
const TICK_INTERVAL: Duration = Duration::from_secs(5);

/// 同时转写几个分段。Transcribe 整段上传按约 1.06 倍实时消费，串行消费追不上
/// 实时产出的分段，长会议在停止时会积压一大截；并发 2 个把有效吞吐压到约 0.53 倍实时。
const TRANSCRIBE_CONCURRENCY: usize = 2;

/// 分段转写的重试次数。一次尝试就是分钟级，`advanced.max_retries`（默认 3）会让
/// 单段最坏烧到 4 × 超时，把 DRAIN_TIMEOUT 撑爆、丢掉尾部分段，所以这里单独收紧。
const CHUNK_MAX_RETRIES: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Idle,
    Recording,
    Finalizing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartSource {
    Auto,
    Manual,
}

impl StartSource {
    fn as_str(self) -> &'static str {
        match self {
            StartSource::Auto => "auto",
            StartSource::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingStatus {
    pub phase: SessionPhase,
    pub meeting_id: Option<String>,
    pub started_at: Option<String>,
    pub elapsed_secs: u64,
    pub source: Option<StartSource>,
    pub chunks_done: u32,
    pub chunks_pending: u32,
    pub microphone: bool,
    pub system_audio: bool,
    pub warnings: Vec<String>,
    pub last_error: Option<String>,
    pub last_meeting_id: Option<String>,
}

struct ActiveSession {
    id: String,
    dir: PathBuf,
    started: Instant,
    started_at: String,
    source: StartSource,
    /// 取消转写任务（丢弃时用）
    cancel: CancellationToken,
    /// 停止计时刷新
    ticker: CancellationToken,
    capture: Option<CaptureHandle>,
    transcriber: Option<tauri::async_runtime::JoinHandle<()>>,
    chunks_done: Arc<AtomicU32>,
    chunks_pending: Arc<AtomicU32>,
    microphone: bool,
    system_audio: bool,
    warnings: Vec<String>,
}

pub struct MeetingManager {
    app: AppHandle,
    store: MeetingStore,
    phase: Mutex<SessionPhase>,
    active: Mutex<Option<ActiveSession>>,
    /// Finalizing 阶段 active 已被取走，这里记住正在收尾的会议
    finalizing: Mutex<Option<(String, String)>>,
    last_error: Mutex<Option<String>>,
    last_meeting_id: Mutex<Option<String>>,
    /// 自动启动是否「上膛」：触发一次后放下，等 Zoom 会议消失再上膛，避免失败后每次轮询都重试
    armed: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn expand_tilde(path: &str) -> String {
    let rest = path
        .strip_prefix("~/")
        .or_else(|| path.strip_prefix("~\\"));
    if let Some(rest) = rest {
        // Windows 上一般没有 HOME，只有 USERPROFILE。
        let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"));
        if let Ok(home) = home {
            return Path::new(home.trim_end_matches(['/', '\\']))
                .join(rest)
                .to_string_lossy()
                .into_owned();
        }
    }
    path.to_string()
}

/// 纪要导出目录：设置里填的（支持 ~），空则 <app_data>/meetings-notes。
pub fn notes_folder(app: &AppHandle, config: &AppConfig) -> PathBuf {
    let configured = config.meeting.notes_folder.trim();
    if !configured.is_empty() {
        return PathBuf::from(expand_tilde(configured));
    }
    app.path()
        .app_data_dir()
        .map(|dir| dir.join(DEFAULT_NOTES_DIR))
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_NOTES_DIR))
}

impl MeetingManager {
    pub fn new(app: &AppHandle, data_dir: &Path) -> Self {
        let store = MeetingStore::new(data_dir);
        store.recover_interrupted();
        Self {
            app: app.clone(),
            store,
            phase: Mutex::new(SessionPhase::Idle),
            active: Mutex::new(None),
            finalizing: Mutex::new(None),
            last_error: Mutex::new(None),
            last_meeting_id: Mutex::new(None),
            armed: AtomicBool::new(true),
        }
    }

    pub fn store(&self) -> &MeetingStore {
        &self.store
    }

    pub fn status(&self) -> MeetingStatus {
        let phase = *lock(&self.phase);
        let active = lock(&self.active);
        let mut status = MeetingStatus {
            phase,
            meeting_id: None,
            started_at: None,
            elapsed_secs: 0,
            source: None,
            chunks_done: 0,
            chunks_pending: 0,
            microphone: false,
            system_audio: false,
            warnings: Vec::new(),
            last_error: lock(&self.last_error).clone(),
            last_meeting_id: lock(&self.last_meeting_id).clone(),
        };
        if let Some(session) = active.as_ref() {
            status.meeting_id = Some(session.id.clone());
            status.started_at = Some(session.started_at.clone());
            status.elapsed_secs = session.started.elapsed().as_secs();
            status.source = Some(session.source);
            status.chunks_done = session.chunks_done.load(Ordering::SeqCst);
            status.chunks_pending = session.chunks_pending.load(Ordering::SeqCst);
            status.microphone = session.microphone;
            status.system_audio = session.system_audio;
            status.warnings = session.warnings.clone();
        } else if let Some((id, started_at)) = lock(&self.finalizing).as_ref() {
            status.meeting_id = Some(id.clone());
            status.started_at = Some(started_at.clone());
        }
        status
    }

    pub fn is_active(&self, id: &str) -> bool {
        lock(&self.active)
            .as_ref()
            .map(|s| s.id == id)
            .unwrap_or(false)
            || lock(&self.finalizing)
                .as_ref()
                .map(|(fid, _)| fid == id)
                .unwrap_or(false)
    }

    /// 录制期间出现问题时写入，会议窗口页脚会立刻显示。
    /// 以前 last_error 只在启动失败与收尾时写，所以分段一直失败也悄无声息。
    pub fn set_last_error(&self, error: String) {
        *lock(&self.last_error) = Some(error);
    }

    /// 广播状态给前端与托盘。
    pub fn after_change(&self) {
        let status = self.status();
        let _ = self.app.emit("meeting-status", &status);
        crate::tray::update_meeting_items(&self.app, &status);
    }

    pub fn start(&self, source: StartSource) -> Result<MeetingStatus, String> {
        {
            let mut phase = lock(&self.phase);
            if *phase != SessionPhase::Idle {
                return Err(tr("err.meetingAlreadyRunning").to_string());
            }
            *phase = SessionPhase::Recording;
        }
        let config = self.app.state::<ConfigManager>().get();
        match self.start_inner(&config, source) {
            Ok(()) => {
                *lock(&self.last_error) = None;
                self.after_change();
                Ok(self.status())
            }
            Err(error) => {
                *lock(&self.phase) = SessionPhase::Idle;
                *lock(&self.last_error) = Some(error.clone());
                self.after_change();
                Err(error)
            }
        }
    }

    fn start_inner(&self, config: &AppConfig, source: StartSource) -> Result<(), String> {
        let transcribe_model = transcribe::transcribe_model_id(config).to_string();
        if !ai::models::supports_audio(config, &transcribe_model).unwrap_or(false) {
            return Err(tr_fmt(
                "err.meetingModelNoAudio",
                &[("model", transcribe_model.as_str())],
            ));
        }
        if !config.meeting.capture_microphone && !config.meeting.capture_system_audio {
            return Err(tr("err.meetingNoInput").to_string());
        }

        let now = Local::now();
        let (id, dir) = self.store.create_session(now)?;
        let chunks_done = Arc::new(AtomicU32::new(0));
        let chunks_pending = Arc::new(AtomicU32::new(0));
        let (tx, rx) = tokio::sync::mpsc::channel::<ReadyChunk>(32);

        let capture_config = CaptureConfig {
            mic_device: config.general.microphone.clone(),
            capture_mic: config.meeting.capture_microphone,
            // 平台不支持系统音频（Windows、旧 macOS）就直接不开，别每场会议都挂一条
            // 「系统音频不可用」的警告；设置页本来就把开关置灰并显示了原因。
            capture_system: config.meeting.capture_system_audio
                && super::capture::system_audio_support().is_ok(),
            chunk_seconds: config.meeting.chunk_seconds,
        };
        let sink = CaptureSink {
            tx,
            pending: chunks_pending.clone(),
        };
        let (capture, started) = match run_capture(capture_config, sink) {
            Ok(result) => result,
            Err(error) => {
                let _ = self.store.delete(&id);
                return Err(error);
            }
        };
        for warning in &started.warnings {
            eprintln!("[Meeting] {}", warning);
        }

        let transcribe_label = ai::models::resolve_model(config, &transcribe_model)
            .map(|m| m.model)
            .unwrap_or_else(|_| transcribe_model.clone());
        let meta = MeetingMeta {
            id: id.clone(),
            title: tr("meeting.defaultTitle").to_string(),
            started_at: now.to_rfc3339(),
            ended_at: None,
            duration_secs: 0,
            source: source.as_str().to_string(),
            status: "recording".to_string(),
            chunk_count: 0,
            failed_chunks: 0,
            transcribe_model: transcribe_label,
            summary_model: String::new(),
            notes_path: None,
            error: None,
            microphone: started.microphone,
            system_audio: started.system_audio,
        };
        self.store.write_meta(&dir, &meta)?;

        let cancel = CancellationToken::new();
        let ticker = CancellationToken::new();
        let ctx = TranscriberCtx {
            app: self.app.clone(),
            store: self.store.clone(),
            meeting_id: id.clone(),
            dir: dir.clone(),
            cancel: cancel.clone(),
            chunks_done: chunks_done.clone(),
            chunks_pending: chunks_pending.clone(),
        };
        let transcriber = tauri::async_runtime::spawn(run_transcriber(ctx, rx));

        *lock(&self.active) = Some(ActiveSession {
            id: id.clone(),
            dir,
            started: Instant::now(),
            started_at: meta.started_at.clone(),
            source,
            cancel,
            ticker: ticker.clone(),
            capture: Some(capture),
            transcriber: Some(transcriber),
            chunks_done,
            chunks_pending,
            microphone: started.microphone,
            system_audio: started.system_audio,
            warnings: started.warnings,
        });

        spawn_ticker(self.app.clone(), ticker, config.meeting.max_meeting_minutes);

        let _ = crate::bubble::show_meeting(&self.app, "meeting-detected", Some(3000));
        if config.meeting.show_window_on_start {
            let _ = window::show(&self.app, Some(&id));
        }
        let _ = self.app.emit("meeting-list-updated", ());
        Ok(())
    }

    /// 结束录制并在后台生成纪要。
    pub fn stop(&self) -> Result<MeetingStatus, String> {
        let session = {
            let mut phase = lock(&self.phase);
            if *phase != SessionPhase::Recording {
                return Err(tr("err.meetingNotRecording").to_string());
            }
            let session = match lock(&self.active).take() {
                Some(session) => session,
                None => {
                    *phase = SessionPhase::Idle;
                    return Err(tr("err.meetingStateReset").to_string());
                }
            };
            *phase = SessionPhase::Finalizing;
            *lock(&self.finalizing) = Some((session.id.clone(), session.started_at.clone()));
            session
        };
        session.ticker.cancel();
        self.after_change();
        let _ = crate::bubble::show_meeting(&self.app, "meeting-summarizing", None);

        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            finalize(app, session).await;
        });
        Ok(self.status())
    }

    /// 丢弃当前录制：停采集、取消转写、删目录。
    pub fn discard(&self) -> Result<(), String> {
        let session = {
            let mut phase = lock(&self.phase);
            if *phase != SessionPhase::Recording {
                return Err(tr("err.meetingNotRecording").to_string());
            }
            let session = lock(&self.active).take();
            *phase = SessionPhase::Idle;
            session.ok_or_else(|| tr("err.meetingStateReset").to_string())?
        };
        session.ticker.cancel();
        session.cancel.cancel();
        let ActiveSession { id, capture, .. } = session;
        let store = self.store.clone();
        let app = self.app.clone();
        std::thread::spawn(move || {
            if let Some(capture) = capture {
                capture.stop();
            }
            if let Err(error) = store.delete(&id) {
                eprintln!("[Meeting] discard cleanup failed: {}", error);
            }
            let _ = app.emit("meeting-list-updated", ());
        });
        crate::bubble::hide_meeting(&self.app);
        self.after_change();
        Ok(())
    }

    pub(crate) fn on_meeting_detected(&self) {
        if !self.armed.swap(false, Ordering::SeqCst) {
            return;
        }
        if *lock(&self.phase) != SessionPhase::Idle {
            return;
        }
        if let Err(error) = self.start(StartSource::Auto) {
            eprintln!("[Meeting] auto start failed: {}", error);
        }
    }

    pub(crate) fn on_meeting_gone(&self) {
        self.armed.store(true, Ordering::SeqCst);
        let is_auto_recording = *lock(&self.phase) == SessionPhase::Recording
            && lock(&self.active)
                .as_ref()
                .map(|s| s.source == StartSource::Auto)
                .unwrap_or(false);
        if is_auto_recording {
            if let Err(error) = self.stop() {
                eprintln!("[Meeting] auto stop failed: {}", error);
            }
        }
    }
}

struct TranscriberCtx {
    app: AppHandle,
    store: MeetingStore,
    meeting_id: String,
    dir: PathBuf,
    cancel: CancellationToken,
    chunks_done: Arc<AtomicU32>,
    chunks_pending: Arc<AtomicU32>,
}

/// 并发消费分段（最多 `TRANSCRIBE_CONCURRENCY` 个同时在转），失败的段记成占位并继续。
///
/// 顺序不再依赖完成顺序：`store::read_segments` 按 index 排序去重，前端实时路径也按
/// index 排序，所以乱序完成不影响 transcript.md 与界面。
async fn run_transcriber(ctx: TranscriberCtx, mut rx: tokio::sync::mpsc::Receiver<ReadyChunk>) {
    let ctx = Arc::new(ctx);
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        // 并发已满就先等一个完成，别把队列里的分段一口气全领走
        if tasks.len() >= TRANSCRIBE_CONCURRENCY {
            tasks.join_next().await;
            continue;
        }
        tokio::select! {
            next = rx.recv() => match next {
                Some(chunk) => {
                    if ctx.cancel.is_cancelled() {
                        break;
                    }
                    let ctx = ctx.clone();
                    tasks.spawn(async move { transcribe_and_record(ctx, chunk).await });
                }
                // 采集侧已收尾，等在途的转完
                None => break,
            },
            _ = ctx.cancel.cancelled() => break,
        }
    }
    while tasks.join_next().await.is_some() {}
}

/// 转写一个分段并落盘、通知前端。音频**无条件先存**，这样转写失败时还能重来。
async fn transcribe_and_record(ctx: Arc<TranscriberCtx>, chunk: ReadyChunk) {
    let config = ctx.app.state::<ConfigManager>().get();
    // 不再看 keep_audio：失败的会议必须能重新转写，所以录制期间一律留音频，
    // 由 finalize 决定去留（全部成功且用户没要求保留时才删）。
    if let Err(error) = ctx
        .store
        .save_chunk_audio(&ctx.dir, chunk.index, &chunk.flac)
    {
        eprintln!("[Meeting] save chunk audio failed: {}", error);
    }

    let (text, failed) = match transcribe_one(&ctx, &config, &chunk).await {
        Ok(text) => (text, false),
        Err(error) => {
            eprintln!("[Meeting] chunk {} failed: {}", chunk.index, error);
            // 第一段失败就让用户看见：会议窗口页脚读的是 last_error，
            // 以前只在启动失败和收尾时才写，用户要等到会议结束才知道全崩了。
            ctx.app
                .state::<MeetingManager>()
                .set_last_error(tr_fmt(
                    "err.chunkFailedLive",
                    &[("index", (chunk.index + 1).to_string().as_str()), ("error", error.as_str())],
                ));
            let _ = crate::bubble::show_meeting(&ctx.app, "meeting-warning", Some(4000));
            (tr_fmt("err.chunkFailed", &[("error", error.as_str())]), true)
        }
    };

    let segment = TranscriptSegment {
        index: chunk.index,
        start_secs: chunk.start_offset_secs,
        end_secs: chunk.start_offset_secs + chunk.duration_secs.round() as u64,
        text,
        failed,
    };
    if let Err(error) = ctx.store.append_segment(&ctx.dir, &segment) {
        eprintln!("[Meeting] append segment failed: {}", error);
    }
    ctx.chunks_done.fetch_add(1, Ordering::SeqCst);
    let _ = ctx
        .chunks_pending
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
            Some(v.saturating_sub(1))
        });

    let _ = ctx.app.emit(
        "meeting-transcript-appended",
        serde_json::json!({ "meetingId": ctx.meeting_id, "segment": segment }),
    );
    ctx.app.state::<MeetingManager>().after_change();
}

async fn transcribe_one(
    ctx: &TranscriberCtx,
    config: &AppConfig,
    chunk: &ReadyChunk,
) -> Result<String, String> {
    let attempt = || transcribe::transcribe_chunk(config, chunk);
    tokio::select! {
        result = ai::retry::with_retry(
            attempt,
            CHUNK_MAX_RETRIES,
            config.meeting.chunk_timeout_secs,
            |attempt| eprintln!("[Meeting] chunk {} retry #{}", chunk.index, attempt),
        ) => result,
        _ = ctx.cancel.cancelled() => Err(tr("err.taskCancelled").to_string()),
    }
}

/// 每 5 秒刷新一次状态（计时），并在达到最长时长时自动停止。
fn spawn_ticker(app: AppHandle, ticker: CancellationToken, max_minutes: u32) {
    tauri::async_runtime::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(max_minutes.max(1) as u64 * 60);
        loop {
            tokio::select! {
                _ = ticker.cancelled() => break,
                _ = tokio::time::sleep(TICK_INTERVAL) => {}
            }
            let manager = app.state::<MeetingManager>();
            manager.after_change();
            if Instant::now() >= deadline {
                eprintln!("[Meeting] max duration reached, stopping");
                let _ = manager.stop();
                break;
            }
        }
    });
}

/// 收尾：停采集 → 等转写排空 → 纪要 → 落盘 → 回到 Idle。
async fn finalize(app: AppHandle, session: ActiveSession) {
    let ActiveSession {
        id,
        dir,
        started,
        cancel,
        capture,
        transcriber,
        chunks_done,
        ..
    } = session;

    if let Some(capture) = capture {
        let _ = tauri::async_runtime::spawn_blocking(move || capture.stop()).await;
    }
    if let Some(handle) = transcriber {
        if tokio::time::timeout(DRAIN_TIMEOUT, handle).await.is_err() {
            eprintln!("[Meeting] transcriber drain timed out");
        }
    }
    cancel.cancel();

    let result = finalize_inner(
        &app,
        &dir,
        started.elapsed().as_secs(),
        chunks_done.load(Ordering::SeqCst),
    )
    .await;

    let manager = app.state::<MeetingManager>();
    {
        *lock(&manager.phase) = SessionPhase::Idle;
        *lock(&manager.finalizing) = None;
        *lock(&manager.last_meeting_id) = Some(id.clone());
        *lock(&manager.last_error) = result.as_ref().err().cloned();
    }
    let ok = result.is_ok();
    let notes_path = result.as_ref().ok().cloned().flatten();
    manager.after_change();
    let _ = app.emit(
        "meeting-finished",
        serde_json::json!({
            "meetingId": id,
            "ok": ok,
            "error": result.as_ref().err(),
            "notesPath": notes_path,
        }),
    );
    let _ = app.emit("meeting-list-updated", ());
    let _ = crate::bubble::show_meeting(
        &app,
        if ok { "meeting-done" } else { "meeting-failed" },
        Some(2500),
    );

    let config = app.state::<ConfigManager>().get();
    if ok && config.meeting.open_summary_when_done {
        let _ = window::show(&app, Some(&id));
    }
}

/// 返回导出的笔记路径（可能为 None）。
async fn finalize_inner(
    app: &AppHandle,
    dir: &Path,
    duration_secs: u64,
    chunks_done: u32,
) -> Result<Option<String>, String> {
    let store = app.state::<MeetingManager>().store.clone();
    let config = app.state::<ConfigManager>().get();
    let mut meta = store
        .read_meta(dir)
        .ok_or_else(|| tr("err.meetingMetaMissing").to_string())?;
    meta.ended_at = Some(Local::now().to_rfc3339());
    meta.duration_secs = duration_secs;
    meta.chunk_count = chunks_done;
    meta.failed_chunks = store.read_segments(dir).iter().filter(|s| s.failed).count() as u32;
    meta.status = "finalizing".to_string();
    store.write_meta(dir, &meta)?;

    // 音频去留：录制期间一律留着（失败才有得救），到这里才决定删不删。
    // 有任何分段失败就保留，用户可以「重新转写」。
    if !config.meeting.keep_audio && meta.failed_chunks == 0 {
        store.remove_audio(dir);
    }

    let transcript = store.transcript_body(dir);
    if transcript.trim().is_empty() {
        meta.status = "failed".to_string();
        meta.error = Some(tr("err.noTranscript").to_string());
        store.write_meta(dir, &meta)?;
        return Err(tr("err.noTranscriptForSummary").to_string());
    }

    let result = generate_summary(app, &config, &store, dir, &mut meta, &transcript).await;
    store.write_meta(dir, &meta)?;
    result
}

/// 生成纪要、写 summary.md、导出笔记目录；结果写回 meta。返回导出路径。
pub(crate) async fn generate_summary(
    app: &AppHandle,
    config: &AppConfig,
    store: &MeetingStore,
    dir: &Path,
    meta: &mut MeetingMeta,
    transcript: &str,
) -> Result<Option<String>, String> {
    let prompts_dir = crate::commands::resolve_prompts_dir_pub(app)?;
    let client =
        crate::task::build_client(config.advanced.proxy_enabled, &config.advanced.proxy_url)?;
    let snapshot = meta.clone();
    let attempt = || summary::summarize(&client, config, &prompts_dir, &snapshot, transcript);
    let outcome = ai::retry::with_retry(
        attempt,
        config.advanced.max_retries,
        config.meeting.summary_timeout_secs,
        |attempt| eprintln!("[Meeting] summary retry #{}", attempt),
    )
    .await;

    match outcome {
        Ok(output) => {
            store.write_summary(dir, &output.markdown)?;
            if !output.title.is_empty() {
                meta.title = output.title;
            }
            meta.summary_model = output.model;
            meta.status = "done".to_string();
            meta.error = None;
            let folder = notes_folder(app, config);
            match store.export_to_notes(&folder, meta, &output.markdown, transcript) {
                Ok(path) => {
                    meta.notes_path = Some(path.to_string_lossy().to_string());
                    Ok(meta.notes_path.clone())
                }
                Err(error) => {
                    eprintln!("[Meeting] export notes failed: {}", error);
                    Ok(None)
                }
            }
        }
        Err(error) => {
            meta.status = "summary_failed".to_string();
            meta.error = Some(error.clone());
            Err(error)
        }
    }
}

/// 对已结束的会议重新生成纪要（首次失败、或用户改了提示词之后）。
/// 重新转写一场失败的会议：拿 `audio/` 里保存的分段重跑转写，再生成纪要。
///
/// 只补失败的段（成功的段按 index 保留），转写结果按 index 追加，
/// `store::read_segments` 的「同 index 保留最后一条」会让新结果覆盖旧的失败占位。
pub async fn retranscribe(app: &AppHandle, id: &str) -> Result<(), String> {
    let (store, active) = {
        let manager = app.state::<MeetingManager>();
        (manager.store.clone(), manager.is_active(id))
    };
    if active {
        return Err(tr("err.meetingStillActive").to_string());
    }
    let config = app.state::<ConfigManager>().get();
    let dir = store.dir_of(id);
    let mut meta = store
        .read_meta(&dir)
        .ok_or_else(|| tr_fmt("err.meetingNotFound", &[("id", id)]))?;

    let audio = store.list_chunk_audio(&dir);
    if audio.is_empty() {
        return Err(tr("err.meetingNoAudio").to_string());
    }
    // 只重转失败的段；没有分段记录（比如老会议）时全部重转
    let existing = store.read_segments(&dir);
    let failed: std::collections::BTreeSet<u32> = existing
        .iter()
        .filter(|s| s.failed)
        .map(|s| s.index)
        .collect();
    let todo: Vec<(u32, std::path::PathBuf)> = audio
        .into_iter()
        .filter(|(index, _)| existing.is_empty() || failed.contains(index))
        .collect();
    if todo.is_empty() {
        return Err(tr("err.meetingNothingToRetry").to_string());
    }

    meta.status = "finalizing".to_string();
    meta.error = None;
    store.write_meta(&dir, &meta)?;
    let _ = app.emit("meeting-list-updated", ());

    let mut still_failed = 0u32;
    for (index, path) in todo {
        let Ok(flac) = std::fs::read(&path) else {
            eprintln!("[Meeting] 读取分段音频失败: {}", path.display());
            still_failed += 1;
            continue;
        };
        // 复用原有的每段转写：同一个模型解析、超时与重试策略
        let chunk = ReadyChunk {
            index,
            start_offset_secs: existing
                .iter()
                .find(|s| s.index == index)
                .map(|s| s.start_secs)
                .unwrap_or(0),
            duration_secs: (flac.len() as f32 / 16_000.0 / 2.0).max(1.0),
            flac,
        };
        let text = match ai::retry::with_retry(
            || transcribe::transcribe_chunk(&config, &chunk),
            CHUNK_MAX_RETRIES,
            config.meeting.chunk_timeout_secs,
            |attempt| eprintln!("[Meeting] 重新转写第 {} 段 retry #{}", index, attempt),
        )
        .await
        {
            Ok(text) => text,
            Err(error) => {
                eprintln!("[Meeting] 重新转写第 {} 段失败: {}", index, error);
                still_failed += 1;
                tr_fmt("err.chunkFailed", &[("error", error.as_str())])
            }
        };
        let failed_now = still_failed > 0 && text.starts_with('[');
        let segment = TranscriptSegment {
            index,
            start_secs: chunk.start_offset_secs,
            end_secs: existing
                .iter()
                .find(|s| s.index == index)
                .map(|s| s.end_secs)
                .unwrap_or(chunk.start_offset_secs + chunk.duration_secs.round() as u64),
            text,
            failed: failed_now,
        };
        if let Err(error) = store.append_segment(&dir, &segment) {
            eprintln!("[Meeting] append segment failed: {}", error);
        }
        let _ = app.emit(
            "meeting-transcript-appended",
            serde_json::json!({ "meetingId": id, "segment": segment }),
        );
    }

    meta.failed_chunks = store.read_segments(&dir).iter().filter(|s| s.failed).count() as u32;
    let transcript = store.transcript_body(&dir);
    if transcript.trim().is_empty() {
        meta.status = "failed".to_string();
        meta.error = Some(tr("err.noTranscript").to_string());
        store.write_meta(&dir, &meta)?;
        let _ = app.emit("meeting-list-updated", ());
        return Err(tr("err.noTranscriptForSummary").to_string());
    }
    if meta.failed_chunks == 0 && !config.meeting.keep_audio {
        store.remove_audio(&dir);
    }

    let result = generate_summary(app, &config, &store, &dir, &mut meta, &transcript).await;
    store.write_meta(&dir, &meta)?;
    let _ = app.emit("meeting-list-updated", ());
    let _ = app.emit(
        "meeting-finished",
        serde_json::json!({
            "meetingId": id,
            "ok": result.is_ok(),
            "error": result.as_ref().err(),
            "notesPath": result.as_ref().ok().cloned().flatten(),
        }),
    );
    result.map(|_| ())
}

pub async fn regenerate_summary(app: &AppHandle, id: &str) -> Result<(), String> {
    let (store, active) = {
        let manager = app.state::<MeetingManager>();
        (manager.store.clone(), manager.is_active(id))
    };
    if active {
        return Err(tr("err.meetingStillActive").to_string());
    }
    let config = app.state::<ConfigManager>().get();
    let dir = store.dir_of(id);
    let mut meta = store
        .read_meta(&dir)
        .ok_or_else(|| tr_fmt("err.meetingNotFound", &[("id", id)]))?;
    let transcript = store.transcript_body(&dir);
    if transcript.trim().is_empty() {
        return Err(tr("err.meetingNoTranscript").to_string());
    }
    meta.status = "finalizing".to_string();
    store.write_meta(&dir, &meta)?;
    let _ = app.emit("meeting-list-updated", ());

    let result = generate_summary(app, &config, &store, &dir, &mut meta, &transcript).await;
    store.write_meta(&dir, &meta)?;
    let _ = app.emit("meeting-list-updated", ());
    let _ = app.emit(
        "meeting-finished",
        serde_json::json!({
            "meetingId": id,
            "ok": result.is_ok(),
            "error": result.as_ref().err(),
            "notesPath": result.as_ref().ok().cloned().flatten(),
        }),
    );
    result.map(|_| ())
}
