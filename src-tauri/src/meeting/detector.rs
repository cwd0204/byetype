//! Zoom 会议探测。
//!
//! macOS 上 Zoom 只在开会期间运行 `CptHost` 进程（`zoom.us.app/Contents/Frameworks/CptHost.app`），
//! 空闲时不存在，所以 `pgrep -x CptHost` 是最省事也最可靠的信号。
//! 连续 2 次阳性才算开始、连续 3 次阴性才算结束，避免等候室 / 共享屏幕切换时的抖动。

use std::time::Duration;

use tauri::{AppHandle, Manager};

use super::MeetingManager;
use crate::config::ConfigManager;

const POSITIVE_THRESHOLD: u32 = 2;
const NEGATIVE_THRESHOLD: u32 = 3;

/// 当前是否有 Zoom 会议在进行。
pub async fn zoom_meeting_active() -> bool {
    #[cfg(target_os = "macos")]
    {
        tokio::process::Command::new("pgrep")
            .args(["-x", "CptHost"])
            .output()
            .await
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
    #[cfg(target_os = "windows")]
    {
        // 应用是 GUI 子系统进程，子进程不带 CREATE_NO_WINDOW 的话，
        // 每次轮询都会闪出一个黑色控制台窗。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        tokio::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq CptHost.exe", "/NH"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .await
            .map(|output| String::from_utf8_lossy(&output.stdout).contains("CptHost.exe"))
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// 启动后台轮询任务。配置里 `enabled && auto_detect` 都为真时才探测，间隔实时读配置。
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut positives = 0u32;
        let mut negatives = 0u32;
        let mut in_meeting = false;

        loop {
            let config = app.state::<ConfigManager>().get();
            let poll_secs = config.meeting.detect_poll_secs.clamp(1, 60) as u64;
            tokio::time::sleep(Duration::from_secs(poll_secs)).await;

            if !(config.meeting.enabled && config.meeting.auto_detect) {
                positives = 0;
                negatives = 0;
                continue;
            }

            if zoom_meeting_active().await {
                positives += 1;
                negatives = 0;
                if !in_meeting && positives >= POSITIVE_THRESHOLD {
                    in_meeting = true;
                    eprintln!("[Meeting] Zoom meeting detected");
                    // 启动会同步等设备就绪（可能弹权限框），放到阻塞线程，不占用 tokio worker
                    let app_handle = app.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        app_handle.state::<MeetingManager>().on_meeting_detected();
                    });
                }
            } else {
                negatives += 1;
                positives = 0;
                if in_meeting && negatives >= NEGATIVE_THRESHOLD {
                    in_meeting = false;
                    eprintln!("[Meeting] Zoom meeting gone");
                    app.state::<MeetingManager>().on_meeting_gone();
                }
            }
        }
    });
}
