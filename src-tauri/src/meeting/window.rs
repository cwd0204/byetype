//! 会议窗口（label "meeting"，tauri.conf.json 里预声明、启动时隐藏）。
//! 打开 / 关闭的做法与 learning.rs 一致：切 ActivationPolicy 让 macOS 真正激活窗口。

use tauri::{AppHandle, Emitter, Manager};

pub const LABEL: &str = "meeting";

pub fn show(app: &AppHandle, meeting_id: Option<&str>) -> Result<(), String> {
    let window = app
        .get_webview_window(LABEL)
        .ok_or_else(|| "会议窗口未初始化".to_string())?;
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    #[cfg(target_os = "windows")]
    let _ = window.set_skip_taskbar(false);
    if !window.is_visible().unwrap_or(false) {
        let _ = window.center();
    }
    window
        .show()
        .map_err(|e| format!("显示会议窗口失败：{}", e))?;
    window
        .set_focus()
        .map_err(|e| format!("聚焦会议窗口失败：{}", e))?;
    let _ = app.emit_to(
        LABEL,
        "meeting-open",
        serde_json::json!({ "meetingId": meeting_id }),
    );
    Ok(())
}

pub fn hide(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window(LABEL)
        .ok_or_else(|| "会议窗口未初始化".to_string())?;
    window
        .hide()
        .map_err(|e| format!("关闭会议窗口失败：{}", e))?;
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    #[cfg(target_os = "windows")]
    let _ = window.set_skip_taskbar(true);
    Ok(())
}
