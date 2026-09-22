//! 会议记录的 Tauri 命令。前端封装统一放在 src/lib/tauri-api.ts。

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use super::session::{self, MeetingManager, MeetingStatus, StartSource};
use super::store::{MeetingDetail, MeetingMeta};
use super::{capture, summary, window};
use crate::config::ConfigManager;
use crate::i18n::{tr, tr_fmt};

#[tauri::command]
pub fn meeting_get_status(manager: State<'_, MeetingManager>) -> MeetingStatus {
    manager.status()
}

/// 启动会带起采集线程并等设备就绪（可能触发系统权限弹窗），放到阻塞线程里做。
#[tauri::command]
pub async fn meeting_start(app: AppHandle) -> Result<MeetingStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<MeetingManager>().start(StartSource::Manual)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn meeting_stop(manager: State<'_, MeetingManager>) -> Result<MeetingStatus, String> {
    manager.stop()
}

#[tauri::command]
pub fn meeting_discard(manager: State<'_, MeetingManager>) -> Result<(), String> {
    manager.discard()
}

#[tauri::command]
pub fn meeting_list(manager: State<'_, MeetingManager>) -> Vec<MeetingMeta> {
    manager.store().list()
}

#[tauri::command]
pub fn meeting_get(
    manager: State<'_, MeetingManager>,
    id: String,
) -> Result<MeetingDetail, String> {
    manager.store().read_detail(&id)
}

#[tauri::command]
pub fn meeting_delete(
    app: AppHandle,
    manager: State<'_, MeetingManager>,
    id: String,
) -> Result<(), String> {
    if manager.is_active(&id) {
        return Err(tr("err.meetingDeleteActive").to_string());
    }
    manager.store().delete(&id)?;
    let _ = app.emit("meeting-list-updated", ());
    Ok(())
}

/// 用户在窗口里编辑纪要后保存：写回 summary.md、刷新标题、重新导出笔记。
#[tauri::command]
pub fn meeting_save_summary(
    app: AppHandle,
    manager: State<'_, MeetingManager>,
    id: String,
    content: String,
) -> Result<(), String> {
    let store = manager.store();
    let dir = store.dir_of(&id);
    let mut meta = store
        .read_meta(&dir)
        .ok_or_else(|| tr_fmt("err.meetingNotFound", &[("id", id.as_str())]))?;
    store.write_summary(&dir, &content)?;
    if let Some(title) = summary::parse_title(&content) {
        meta.title = title;
    }
    let config = app.state::<ConfigManager>().get();
    let folder = session::notes_folder(&app, &config);
    if let Ok(path) = store.export_to_notes(&folder, &meta, &content, &store.transcript_body(&dir))
    {
        meta.notes_path = Some(path.to_string_lossy().to_string());
    }
    store.write_meta(&dir, &meta)?;
    let _ = app.emit("meeting-list-updated", ());
    Ok(())
}

#[tauri::command]
pub async fn meeting_regenerate_summary(app: AppHandle, id: String) -> Result<(), String> {
    session::regenerate_summary(&app, &id).await
}

/// 拿保留的分段音频重新转写（转写失败的会议用）。
#[tauri::command]
pub async fn meeting_retranscribe(app: AppHandle, id: String) -> Result<(), String> {
    session::retranscribe(&app, &id).await
}

#[tauri::command]
pub async fn meeting_pick_notes_folder(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |folder| {
        let _ = tx.send(folder);
    });
    let folder = rx
        .await
        .map_err(|_| tr("err.pickFolderCancelled").to_string())?;
    match folder {
        Some(path) => path
            .into_path()
            .map(|p| Some(p.to_string_lossy().to_string()))
            .map_err(|e| tr_fmt("err.pickFolderInvalid", &[("error", e.to_string().as_str())])),
        None => Ok(None),
    }
}

/// 当前生效的纪要导出目录（给设置页显示默认值）。
#[tauri::command]
pub fn meeting_notes_folder(app: AppHandle) -> String {
    let config = app.state::<ConfigManager>().get();
    session::notes_folder(&app, &config)
        .to_string_lossy()
        .to_string()
}

#[tauri::command]
pub fn meeting_reveal(manager: State<'_, MeetingManager>, id: String) -> Result<(), String> {
    let dir = manager.store().dir_of(&id);
    let target = if dir.join("summary.md").exists() {
        dir.join("summary.md")
    } else {
        dir
    };
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(&target)
            .spawn()
            .map_err(|e| tr_fmt("err.openFinderFailed", &[("error", e.to_string().as_str())]))?;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // 路径含空格时 std 会给整个参数加引号，Explorer 对此处理不一致；
        // 用 raw_arg 自己控制引号只包住路径。
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", target.display()))
            .spawn()
            .map_err(|e| {
                tr_fmt("err.openExplorerFailed", &[("error", e.to_string().as_str())])
            })?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = target;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSupport {
    pub system_audio_supported: bool,
    pub reason: Option<String>,
    pub platform: String,
}

#[tauri::command]
pub fn meeting_check_support() -> MeetingSupport {
    let (supported, reason) = match capture::system_audio_support() {
        Ok(()) => (true, None),
        Err(reason) => (false, Some(reason)),
    };
    MeetingSupport {
        system_audio_supported: supported,
        reason,
        platform: std::env::consts::OS.to_string(),
    }
}

/// 主动建一次系统音频 tap，触发 macOS「仅系统音频录制」授权弹窗。
#[tauri::command]
pub async fn meeting_request_permissions() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        tauri::async_runtime::spawn_blocking(|| capture::macos_tap::probe(800))
            .await
            .map_err(|e| e.to_string())?
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(tr("err.systemAudioUnsupported").to_string())
    }
}

#[tauri::command]
pub fn meeting_open_window(app: AppHandle, meeting_id: Option<String>) -> Result<(), String> {
    window::show(&app, meeting_id.as_deref())
}

#[tauri::command]
pub fn meeting_close_window(app: AppHandle) -> Result<(), String> {
    window::hide(&app)
}
