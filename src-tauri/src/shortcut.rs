use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::ai::transcribe_live::LiveTranscribe;
use crate::audio::recorder::{AudioRecorder, LiveAudio};
use crate::config::ConfigManager;
use crate::modifier_hotkey::{self, Handler, HotkeyEvent, ModifierKey};

/// PTT 模式下，按住时间小于此阈值视为误触，丢弃录音。
const PTT_MIN_DURATION_MS: u64 = 300;

/// 等待麦克风送出首帧音频的轮询间隔。
const READY_POLL_INTERVAL_MS: u64 = 20;
/// 等待首帧音频的最大轮询次数（20ms × 150 = 3 秒兜底）。
const READY_POLL_TICKS: u32 = 150;

/// 气泡波形每拍的间隔。25 Hz 正好对应波形上的一列。
const LEVEL_TICK_MS: u64 = 40;

/// 当前录音对应的流式转写会话。每个停止点都要取走它：不取走只是丢掉加速
/// （`LiveTranscribe` 自己的看门狗会收尾），但拿不到已经识别好的文字。
type CurrentLive = Arc<Mutex<Option<LiveTranscribe>>>;

/// Register all 4 global shortcuts (2 voice + 2 image), each bound to its own template_id.
pub fn register(
    app: &AppHandle,
    recorder: Arc<AudioRecorder>,
) -> Result<(), String> {
    let cfg = app.state::<ConfigManager>().get();
    // Image shortcut 1 falls back to "F6" when empty (see the bind() call for ek1
    // below). Apply the same fallback here so conflict detection sees the actual
    // key that will be registered — otherwise an empty extract_shortcut would
    // bypass the uniqueness check and silently collide with another "F6" shortcut.
    let ek1 = if cfg.general.extract_shortcut.is_empty() { "F6".to_string() } else { cfg.general.extract_shortcut.clone() };
    let keys = [
        cfg.general.shortcut.clone(),
        cfg.general.shortcut2.clone(),
        ek1.clone(),
        cfg.general.extract_shortcut2.clone(),
    ];

    // Conflict detection: all 4 shortcuts must be unique
    for i in 0..keys.len() {
        for j in (i + 1)..keys.len() {
            if !keys[i].is_empty() && keys[i] == keys[j] {
                return Err(format!("快捷键 '{}' 重复，请设置不同的快捷键", keys[i]));
            }
        }
    }

    app.global_shortcut().unregister_all()
        .map_err(|e| format!("Failed to unregister shortcuts: {}", e))?;
    // 修饰键通道同样先清空；注册失败回滚时 register 会被再调一次，把旧绑定补回来
    modifier_hotkey::set_bindings(Vec::new());
    let mut modifier_bindings: Vec<(ModifierKey, Handler)> = Vec::new();

    // Voice shortcut 1
    if !cfg.general.shortcut.is_empty() {
        let key = &cfg.general.shortcut;
        let handler = voice_handler(
            app,
            cfg.general.shortcut_template.clone(),
            cfg.general.shortcut_transcribe_model.clone(),
            recorder.clone(),
            fires_on_release(key),
        );
        bind(app, key, "voice", handler, &mut modifier_bindings)?;
    }
    // Voice shortcut 2
    if !cfg.general.shortcut2.is_empty() {
        let key = &cfg.general.shortcut2;
        let handler = voice_handler(
            app,
            cfg.general.shortcut2_template.clone(),
            cfg.general.shortcut2_transcribe_model.clone(),
            recorder.clone(),
            fires_on_release(key),
        );
        bind(app, key, "voice", handler, &mut modifier_bindings)?;
    }
    // Image shortcut 1 (ek1 with fallback already computed above for conflict detection)
    {
        let handler = image_handler(app, cfg.general.extract_shortcut_template.clone(), fires_on_release(&ek1));
        bind(app, &ek1, "image", handler, &mut modifier_bindings)?;
    }
    // Image shortcut 2
    if !cfg.general.extract_shortcut2.is_empty() {
        let key = &cfg.general.extract_shortcut2;
        let handler = image_handler(app, cfg.general.extract_shortcut2_template.clone(), fires_on_release(key));
        bind(app, key, "image", handler, &mut modifier_bindings)?;
    }

    modifier_hotkey::set_bindings(modifier_bindings);
    Ok(())
}

/// 单独修饰键要等松开才触发（toggle / 截图），这样按住它输入组合键时才有机会取消。
/// 普通键与组合键沿用按下即触发。
fn fires_on_release(key: &str) -> bool {
    ModifierKey::parse(key).is_some()
}

/// 把 handler 挂到对应通道：单独修饰键进 `modifier_hotkey` 的轮询表，其余交给 global-shortcut。
/// `kind` 只用于错误文案（"voice" / "image"），格式保持 `Failed to register <kind> shortcut '<key>': ...`。
fn bind(
    app: &AppHandle,
    key: &str,
    kind: &str,
    handler: Handler,
    modifier_bindings: &mut Vec<(ModifierKey, Handler)>,
) -> Result<(), String> {
    if let Some(modifier) = ModifierKey::parse(key) {
        // 轮询线程不是主线程；把回调派回主线程，与 global-shortcut 的回调保持同一执行环境
        let app = app.clone();
        let on_main: Handler = Arc::new(move |event| {
            let handler = handler.clone();
            let _ = app.run_on_main_thread(move || handler(event));
        });
        modifier_bindings.push((modifier, on_main));
        return Ok(());
    }

    app.global_shortcut()
        .on_shortcut(key, move |_app, _shortcut, event| {
            let event = match event.state {
                ShortcutState::Pressed => HotkeyEvent::Pressed,
                ShortcutState::Released => HotkeyEvent::Released,
            };
            handler(event);
        })
        .map_err(|e| format!("Failed to register {} shortcut '{}': {}", kind, key, e))
}

/// Build the handler for a voice (recording) shortcut bound to the given template_id.
/// Supports both Toggle mode (default) and Push-to-Talk mode based on config.general.ptt_mode
/// at event time — switching mode does NOT require re-registering the shortcut.
fn voice_handler(
    app: &AppHandle,
    template_id: String,
    engine_id: String,
    recorder: Arc<AudioRecorder>,
    fires_on_release: bool,
) -> Handler {
    let app_handle = app.clone();
    let tmpl = template_id;
    let engine = engine_id;
    // Track the current recording's task_id between start and stop
    let current_task_id: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));
    let current_live: CurrentLive = Arc::new(Mutex::new(None));
    let recording_gen: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));

    Arc::new(move |event: HotkeyEvent| {
        let ptt = app_handle.state::<ConfigManager>().get().general.ptt_mode;

        if ptt {
            handle_ptt_event(
                &app_handle,
                event,
                &recorder,
                &current_task_id,
                &current_live,
                &recording_gen,
                &tmpl,
                &engine,
            );
        } else {
            handle_toggle_event(
                &app_handle,
                event,
                fires_on_release,
                &recorder,
                &current_task_id,
                &current_live,
                &recording_gen,
                &tmpl,
                &engine,
            );
        }
    })
}

/// Toggle mode: press once to start, press again to stop + transcribe.
/// 普通键在 Pressed 触发（原有行为）；单独修饰键在干净的 Released 触发，Cancelled 不动作。
#[allow(clippy::too_many_arguments)]
fn handle_toggle_event(
    app_handle: &AppHandle,
    event: HotkeyEvent,
    fires_on_release: bool,
    recorder: &Arc<AudioRecorder>,
    current_task_id: &Arc<Mutex<Option<u32>>>,
    current_live: &CurrentLive,
    recording_gen: &Arc<AtomicU32>,
    tmpl: &str,
    engine: &str,
) {
    let trigger = if fires_on_release {
        HotkeyEvent::Released
    } else {
        HotkeyEvent::Pressed
    };
    if event != trigger {
        return;
    }

    if recorder.is_recording() {
        // CAS: claim the right to stop — advance gen to invalidate timer
        let gen = recording_gen.load(Ordering::SeqCst);
        if recording_gen.compare_exchange(gen, gen + 1, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return; // Timer already stopped it
        }

        let task_id = current_task_id.lock().unwrap().take();
        // 先停录音流再取流式会话：停止后才到达的帧要排在 `Finish` 之前，
        // 否则会被丢掉，尾巴上的字就没了。
        let stopped = recorder.stop();
        let live = current_live.lock().unwrap().take();
        match stopped {
            Ok(base64_audio) => {
                update_tray_icon(app_handle, false);
                if let Some(tid) = task_id {
                    crate::task::process_recording(
                        app_handle,
                        tid,
                        base64_audio,
                        tmpl.to_string(),
                        engine.to_string(),
                        live,
                    );
                }
            }
            Err(e) => {
                eprintln!("Stop recording error: {}", e);
                if let Some(tid) = task_id {
                    crate::task::cancel_recording(app_handle, tid);
                }
                update_tray_icon(app_handle, false);
                let _ = app_handle.emit("recording-error", serde_json::json!({
                    "message": e
                }));
            }
        }
    } else {
        start_voice_recording(app_handle, recorder, current_task_id, current_live, recording_gen, tmpl, engine, false);
    }
}

/// PTT mode: press to start, release to stop + transcribe (or cancel if too short).
/// `Cancelled`（单独修饰键按住期间夹了别的键）与「按得太短」走同一条丢弃路径。
fn handle_ptt_event(
    app_handle: &AppHandle,
    event: HotkeyEvent,
    recorder: &Arc<AudioRecorder>,
    current_task_id: &Arc<Mutex<Option<u32>>>,
    current_live: &CurrentLive,
    recording_gen: &Arc<AtomicU32>,
    tmpl: &str,
    engine: &str,
) {
    match event {
        HotkeyEvent::Pressed => {
            // Debounce: some platforms repeat Pressed events while held.
            if recorder.is_recording() {
                return;
            }
            start_voice_recording(app_handle, recorder, current_task_id, current_live, recording_gen, tmpl, engine, true);
        }
        HotkeyEvent::Cancelled => {
            // CAS: race against auto-timeout timer.
            let gen = recording_gen.load(Ordering::SeqCst);
            if recording_gen.compare_exchange(gen, gen + 1, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                return; // Auto-timeout already stopped it.
            }
            let task_id = current_task_id.lock().unwrap().take();
            discard_recording(app_handle, recorder, current_live, task_id);
        }
        HotkeyEvent::Released => {
            // CAS: race against auto-timeout timer.
            let gen = recording_gen.load(Ordering::SeqCst);
            if recording_gen.compare_exchange(gen, gen + 1, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                return; // Auto-timeout already stopped it.
            }

            let task_id = current_task_id.lock().unwrap().take();
            let elapsed = recorder.elapsed_since_start().unwrap_or(Duration::ZERO);

            if elapsed < Duration::from_millis(PTT_MIN_DURATION_MS) {
                // Too short — discard recording, no transcription.
                discard_recording(app_handle, recorder, current_live, task_id);
            } else {
                // 顺序同 toggle：先停流，再取会话。
                let stopped = recorder.stop();
                let live = current_live.lock().unwrap().take();
                match stopped {
                    Ok(base64_audio) => {
                        update_tray_icon(app_handle, false);
                        if let Some(tid) = task_id {
                            crate::task::process_recording(
                        app_handle,
                        tid,
                        base64_audio,
                        tmpl.to_string(),
                        engine.to_string(),
                        live,
                    );
                        }
                    }
                    Err(e) => {
                        eprintln!("PTT stop recording error: {}", e);
                        if let Some(tid) = task_id {
                            crate::task::cancel_recording(app_handle, tid);
                        }
                        update_tray_icon(app_handle, false);
                        let _ = app_handle.emit("recording-error", serde_json::json!({
                            "message": e
                        }));
                    }
                }
            }
        }
    }
}

/// 录音期间把麦克风电平推给气泡，驱动波形动画。
///
/// 为什么是 Rust 主动推而不是前端轮询：停止录音的权威状态是 `recording_gen`，每条
/// 停止路径都会 CAS 递增它，所以这里一句世代校验就能干净退出；而 requestAnimationFrame
/// 在隐藏或被遮挡的窗口里不执行，气泡平时正是 hide 且停在屏幕外，前端循环没法当活性依据。
fn spawn_level_pump(
    app: &AppHandle,
    recorder: &Arc<AudioRecorder>,
    recording_gen: &Arc<AtomicU32>,
    gen: u32,
    task_id: u32,
) {
    let app = app.clone();
    let recorder = recorder.clone();
    let gen_src = recording_gen.clone();
    let label = crate::bubble::label_for(task_id);
    std::thread::spawn(move || {
        let mut disp = 0f32;
        loop {
            if gen_src.load(Ordering::SeqCst) != gen {
                return; // 录音已结束
            }
            let target = crate::audio::recorder::level_from_rms(recorder.take_level());
            disp = crate::audio::recorder::smooth(disp, target);
            // 带上 taskNumber：label_for 把 task_id 钳到 MAX_BUBBLES，第 4 个任务会复用
            // bubble-3，前端据此丢弃不属于当前任务的电平。
            let _ = app.emit_to(
                &label,
                "bubble-level",
                serde_json::json!({ "taskNumber": task_id, "level": disp }),
            );
            std::thread::sleep(Duration::from_millis(LEVEL_TICK_MS));
        }
    });
}

/// 丢弃一次不该转写的录音（PTT 按得太短，或单独修饰键按住期间夹了别的键）：
/// 取消采集与流式会话、还原托盘图标、释放任务槽位。
fn discard_recording(
    app_handle: &AppHandle,
    recorder: &Arc<AudioRecorder>,
    current_live: &CurrentLive,
    task_id: Option<u32>,
) {
    let _ = recorder.cancel();
    if let Some(live) = current_live.lock().unwrap().take() {
        live.abort();
    }
    update_tray_icon(app_handle, false);
    if let Some(tid) = task_id {
        crate::task::cancel_recording(app_handle, tid);
    }
}

/// Shared start logic for both Toggle and PTT modes.
/// Starts the recorder, allocates a task_id, shows the bubble, and spawns the
/// max-duration auto-stop timer (which races with manual stop via `recording_gen` CAS).
#[allow(clippy::too_many_arguments)]
fn start_voice_recording(
    app_handle: &AppHandle,
    recorder: &Arc<AudioRecorder>,
    current_task_id: &Arc<Mutex<Option<u32>>>,
    current_live: &CurrentLive,
    recording_gen: &Arc<AtomicU32>,
    tmpl: &str,
    engine: &str,
    ptt: bool,
) {
    // Allocate the new generation BEFORE starting the recorder, so that any
    // Release event arriving while `recorder.start()` is still in progress
    // will see the up-to-date generation when it does its CAS — preventing
    // the race where Release loads a stale `gen` (the value from before this
    // Press) and its CAS succeeds with the wrong version, dropping the recording.
    let gen = recording_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let mic = app_handle.state::<ConfigManager>().get().general.microphone.clone();
    // Allocate the task_id and publish it BEFORE starting the recorder, so that
    // any Release event (or auto-stop timer) arriving between recorder.start()
    // returning Ok and the task_id being stored will still find a task_id in
    // place — preventing the race where stop succeeds but process_recording is
    // skipped because task_id was still None, silently dropping the recording.
    let tid = match crate::task::start_recording(app_handle) {
        Some(tid) => tid,
        None => {
            // Max parallel reached — roll back the generation we allocated above
            // so a racing Release's CAS fails cleanly instead of consuming it.
            recording_gen.fetch_add(1, Ordering::SeqCst);
            return;
        }
    };
    *current_task_id.lock().unwrap() = Some(tid);

    // 边说边转写：录音一开始就把音频帧往 channel 里送，但流要等麦克风真的
    // 出声了才开（见下面的就绪线程）。这段时间的帧在 channel 里排着，开流时
    // 一次补发，开头不会丢字。协议不支持流式的转写模型直接不开这条路。
    // 只有 Amazon Transcribe 有流式通道；Voxtral 那条路要录完整段再传，不开流。
    let feed = {
        let cfg = app_handle.state::<ConfigManager>().get();
        let model = crate::ai::models::effective_transcribe_model(&cfg, engine);
        crate::ai::live_transcribe_supported(&cfg, &model)
            .then(tokio::sync::mpsc::unbounded_channel::<LiveAudio>)
    };
    let live_tx = feed.as_ref().map(|(tx, _)| tx.clone());

    match recorder.start(&mic, live_tx) {
        Ok(()) => {
            update_tray_icon(app_handle, true);

            // 蓝牙耳机要先完成 HFP 握手才会送出第一帧音频，这段时间麦克风
            // 其实没在采集。气泡先停在 preparing，收到首个回调后再转 recording，
            // 用户看到红点再开口，避免开头几秒白说。有线/内置麦克风几乎瞬间
            // 回调，观感上和以前一样。
            {
                let w_recorder = recorder.clone();
                let w_app = app_handle.clone();
                let w_gen = recording_gen.clone();
                let w_live = current_live.clone();
                std::thread::spawn(move || {
                    // 兜底 3 秒：设备异常一直不送有效音频时也让气泡转红，不卡在准备中。
                    for _ in 0..READY_POLL_TICKS {
                        if w_gen.load(Ordering::SeqCst) != gen {
                            return; // 录音已结束，不要再改气泡
                        }
                        if w_recorder.audio_started() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(READY_POLL_INTERVAL_MS));
                    }
                    if w_gen.load(Ordering::SeqCst) != gen {
                        return;
                    }
                    let _ = crate::bubble::update(&w_app, tid, "recording");
                    // 波形要在 open_live_stream 之前开：PTT 模式下那个函数会阻塞到
                    // PTT_MIN_DURATION_MS，放在它后面的话每次按住说话的头 300ms 波形是平的。
                    spawn_level_pump(&w_app, &w_recorder, &w_gen, gen, tid);
                    if let Some((tx, rx)) = feed {
                        open_live_stream(&w_app, &w_recorder, &w_live, &w_gen, gen, ptt, tx, rx);
                    }
                });
            }

            let max_secs = app_handle.state::<ConfigManager>().get().general.max_recording_seconds;
            if max_secs > 0 {
                let t_recorder = recorder.clone();
                let t_app = app_handle.clone();
                let t_task_id = current_task_id.clone();
                let t_live = current_live.clone();
                let t_gen = recording_gen.clone();
                let t_tmpl = tmpl.to_string();
                let t_engine = engine.to_string();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(max_secs as u64));
                    if t_gen.compare_exchange(gen, gen + 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        let task_id = t_task_id.lock().unwrap().take();
                        // 顺序同 toggle：先停流，再取会话。
                        let stopped = t_recorder.stop();
                        let live = t_live.lock().unwrap().take();
                        match stopped {
                            Ok(base64_audio) => {
                                update_tray_icon(&t_app, false);
                                if let Some(tid) = task_id {
                                    crate::task::process_recording(
                                        &t_app,
                                        tid,
                                        base64_audio,
                                        t_tmpl.clone(),
                                        t_engine.clone(),
                                        live,
                                    );
                                }
                            }
                            Err(e) => {
                                eprintln!("Auto-stop recording error: {}", e);
                                if let Some(tid) = task_id {
                                    crate::task::cancel_recording(&t_app, tid);
                                }
                                update_tray_icon(&t_app, false);
                                let _ = t_app.emit("recording-error", serde_json::json!({
                                    "message": e
                                }));
                            }
                        }
                    }
                });
            }
        }
        Err(e) => {
            // Bump generation again to invalidate the just-allocated `gen` —
            // any in-flight Release racing with this failed Press will then
            // see a fresh value and its CAS will fail cleanly.
            recording_gen.fetch_add(1, Ordering::SeqCst);
            // Clean up the task_id we pre-allocated: clear it from
            // current_task_id so no later path mistakes it for a live
            // recording, and cancel the task to release its slot/bubble.
            *current_task_id.lock().unwrap() = None;
            if let Some(live) = current_live.lock().unwrap().take() {
                live.abort();
            }
            crate::task::cancel_recording(app_handle, tid);
            eprintln!("Start recording error: {}", e);
            let _ = app_handle.emit("recording-error", serde_json::json!({
                "message": e
            }));
        }
    }
}

/// 在就绪线程里开一条流式转写会话，并挂到 `current_live` 上。
///
/// PTT 模式额外等到超过误触阈值再开：快按快放本来就要丢弃录音，没必要为它
/// 真开一条流。这段时间的音频帧仍在 channel 里排着，不会丢。
#[allow(clippy::too_many_arguments)]
fn open_live_stream(
    app_handle: &AppHandle,
    recorder: &Arc<AudioRecorder>,
    current_live: &CurrentLive,
    recording_gen: &Arc<AtomicU32>,
    gen: u32,
    ptt: bool,
    tx: tokio::sync::mpsc::UnboundedSender<LiveAudio>,
    rx: tokio::sync::mpsc::UnboundedReceiver<LiveAudio>,
) {
    if ptt {
        while recorder.elapsed_since_start().unwrap_or(Duration::ZERO)
            < Duration::from_millis(PTT_MIN_DURATION_MS)
        {
            if recording_gen.load(Ordering::SeqCst) != gen {
                return; // 已经松手（或被丢弃），不用开流了
            }
            std::thread::sleep(Duration::from_millis(READY_POLL_INTERVAL_MS));
        }
    }
    if recording_gen.load(Ordering::SeqCst) != gen {
        return;
    }

    let cfg = app_handle.state::<ConfigManager>().get();
    let live = crate::ai::transcribe_live::start(
        cfg.models.aws.clone(),
        tx,
        rx,
        Default::default(),
        crate::i18n::current(),
    );

    // 持锁时再核对一次 gen。所有停止点都是先 CAS 推进 gen、再锁 current_live 取走
    // 句柄，所以「持锁期间 gen 没变」就保证了停止点还没走到取的那一步，句柄不会
    // 挂在没人管的地方。gen 已经变了说明录音结束了，直接放弃这条流。
    let mut slot = current_live.lock().unwrap();
    if recording_gen.load(Ordering::SeqCst) == gen {
        *slot = Some(live);
    } else {
        drop(slot);
        live.abort();
    }
}

/// Build the handler for an image (screenshot + OCR extraction) shortcut bound to the given template_id.
/// 普通键在 Pressed 触发；单独修饰键在干净的 Released 触发。
fn image_handler(app: &AppHandle, template_id: String, fires_on_release: bool) -> Handler {
    let extract_app = app.clone();
    let tmpl = template_id;
    let trigger = if fires_on_release {
        HotkeyEvent::Released
    } else {
        HotkeyEvent::Pressed
    };
    Arc::new(move |event: HotkeyEvent| {
        if event != trigger {
            return;
        }
        crate::task::start_extraction(&extract_app, tmpl.clone());
    })
}

/// 听写录音状态交给 tray 统一合成图标（会议录制也会点亮同一个图标）。
fn update_tray_icon(app: &AppHandle, is_recording: bool) {
    crate::tray::set_dictation_recording(app, is_recording);
}
