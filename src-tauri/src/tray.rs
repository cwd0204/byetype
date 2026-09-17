use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Wry,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

use crate::config::ConfigManager;
use crate::meeting::session::{MeetingStatus, SessionPhase, StartSource};
use crate::meeting::MeetingManager;

/// 托盘图标由「听写录音中」与「会议录制中」两个状态共同决定，任一为真显示红点图标。
static DICTATION_RECORDING: AtomicBool = AtomicBool::new(false);
static MEETING_RECORDING: AtomicBool = AtomicBool::new(false);

/// 需要动态改文字 / 勾选状态的菜单项句柄。
pub struct TrayHandles {
    meeting_toggle: MenuItem<Wry>,
    meeting_auto: CheckMenuItem<Wry>,
}

/// Show the settings window by moving it back on-screen and focusing it.
fn show_settings(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        // Temporarily become a Regular app so macOS activates the window
        #[cfg(target_os = "macos")]
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
        #[cfg(target_os = "windows")]
        let _ = win.set_skip_taskbar(false);

        let _ = win.center();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn show_error(app: &AppHandle, title: &str, message: String) {
    app.dialog()
        .message(message)
        .title(title)
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

fn navigate(app: &AppHandle, tab: &str) {
    show_settings(app);
    let _ = app.emit(
        "navigate-to-tab",
        crate::updater::NavigatePayload {
            tab: tab.to_string(),
        },
    );
}

pub fn create(app: &AppHandle) -> Result<(), String> {
    let settings_item = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let history_item = MenuItem::with_id(app, "history", "历史记录", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let learning_item = MenuItem::with_id(app, "auto_learning", "自动学习", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let meeting_toggle = MenuItem::with_id(app, "meeting_toggle", "开始会议录制", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let meeting_open = MenuItem::with_id(app, "meeting_open", "会议记录…", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let meeting_config = app.state::<ConfigManager>().get().meeting;
    let meeting_auto = CheckMenuItem::with_id(
        app,
        "meeting_auto",
        "自动检测 Zoom 会议",
        true,
        meeting_config.enabled && meeting_config.auto_detect,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let separator_a = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let separator_b = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let about_item =
        MenuItem::with_id(app, "about", "关于", true, None::<&str>).map_err(|e| e.to_string())?;
    let quit_item =
        MenuItem::with_id(app, "quit", "退出", true, None::<&str>).map_err(|e| e.to_string())?;

    let menu = Menu::with_items(
        app,
        &[
            &settings_item,
            &history_item,
            &learning_item,
            &separator_a,
            &meeting_toggle,
            &meeting_open,
            &meeting_auto,
            &separator_b,
            &about_item,
            &quit_item,
        ],
    )
    .map_err(|e| e.to_string())?;

    app.manage(TrayHandles {
        meeting_toggle: meeting_toggle.clone(),
        meeting_auto: meeting_auto.clone(),
    });

    let icon_bytes = include_bytes!("../icons/tray-default.png");
    let icon = tauri::image::Image::from_bytes(icon_bytes)
        .map_err(|e| format!("Failed to load tray icon: {}", e))?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("ByeType")
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "settings" => navigate(app, "general"),
            "history" => navigate(app, "history"),
            "auto_learning" => {
                if let Err(error) = crate::learning::open_learning_review(app) {
                    show_error(app, "自动学习", error);
                }
            }
            "meeting_toggle" => {
                // 启动会等设备就绪、可能弹权限框，不在菜单回调线程里阻塞
                let app_handle = app.clone();
                std::thread::spawn(move || toggle_meeting(&app_handle));
            }
            "meeting_open" => {
                if let Err(error) = crate::meeting::window::show(app, None) {
                    show_error(app, "会议记录", error);
                }
            }
            "meeting_auto" => toggle_auto_detect(app),
            "about" => navigate(app, "about"),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_settings(tray.app_handle());
            }
        })
        .build(app)
        .map_err(|e| format!("Failed to create tray: {}", e))?;

    Ok(())
}

fn toggle_meeting(app: &AppHandle) {
    let manager = app.state::<MeetingManager>();
    let result = if manager.status().phase == SessionPhase::Recording {
        manager.stop().map(|_| ())
    } else {
        manager.start(StartSource::Manual).map(|_| ())
    };
    if let Err(error) = result {
        show_error(app, "会议记录", error);
    }
}

/// 托盘里切换自动检测：打开时顺带把会议记录总开关打开，否则勾了也不会探测。
fn toggle_auto_detect(app: &AppHandle) {
    let config_manager = app.state::<ConfigManager>();
    let mut config = config_manager.get();
    let turning_on = !(config.meeting.enabled && config.meeting.auto_detect);
    if turning_on {
        config.meeting.enabled = true;
        config.meeting.auto_detect = true;
    } else {
        config.meeting.auto_detect = false;
    }
    if let Err(error) = config_manager.update(config) {
        show_error(app, "会议记录", error);
        return;
    }
    if let Some(handles) = app.try_state::<TrayHandles>() {
        let _ = handles.meeting_auto.set_checked(turning_on);
    }
    // 设置页持有整份配置，通知它重新拉取
    let _ = app.emit("config-updated", ());
}

fn format_elapsed(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else {
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}

/// 会议状态变化时刷新菜单文字、勾选与图标。
pub fn update_meeting_items(app: &AppHandle, status: &MeetingStatus) {
    if let Some(handles) = app.try_state::<TrayHandles>() {
        let text = match status.phase {
            SessionPhase::Recording => {
                format!("停止会议录制（{}）", format_elapsed(status.elapsed_secs))
            }
            SessionPhase::Finalizing => "正在生成会议纪要…".to_string(),
            SessionPhase::Idle => "开始会议录制".to_string(),
        };
        let _ = handles.meeting_toggle.set_text(text);
        let _ = handles
            .meeting_toggle
            .set_enabled(status.phase != SessionPhase::Finalizing);
        let meeting = app.state::<ConfigManager>().get().meeting;
        let _ = handles
            .meeting_auto
            .set_checked(meeting.enabled && meeting.auto_detect);
    }
    set_meeting_recording(app, status.phase == SessionPhase::Recording);
}

pub fn set_dictation_recording(app: &AppHandle, recording: bool) {
    DICTATION_RECORDING.store(recording, Ordering::SeqCst);
    refresh_icon(app);
}

pub fn set_meeting_recording(app: &AppHandle, recording: bool) {
    MEETING_RECORDING.store(recording, Ordering::SeqCst);
    refresh_icon(app);
}

fn refresh_icon(app: &AppHandle) {
    let recording =
        DICTATION_RECORDING.load(Ordering::SeqCst) || MEETING_RECORDING.load(Ordering::SeqCst);
    if let Some(tray) = app.tray_by_id("main-tray") {
        let icon_bytes: &[u8] = if recording {
            include_bytes!("../icons/tray-recording.png")
        } else {
            include_bytes!("../icons/tray-default.png")
        };
        if let Ok(icon) = tauri::image::Image::from_bytes(icon_bytes) {
            let _ = tray.set_icon(Some(icon));
        }
    }
}
