//! 极简双语层（简体中文 / English）。
//!
//! Rust 侧直接展示给用户的文案（托盘菜单、窗口标题、会议 Markdown 标题、说话人标签、
//! 报错）都从这里的表取。语言由 `config.general.language` 决定：`"zh-CN"` / `"en"`
//! 显式指定，`"system"` 跟随系统 locale。当前语言存在进程级原子量里，
//! 没有 `AppHandle` 的模块（store.rs、transcribe_aws.rs）也能直接 `tr(...)`。
//!
//! 深层模块（backup、learning、local_api、prompt、audio）暂未接入，仍是中文。

use std::sync::atomic::{AtomicU8, Ordering};

use tauri::{AppHandle, Manager};

use crate::config::ConfigManager;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    ZhCn,
    En,
}

impl Lang {
    /// 把设置里的 `language` 值解析成语言；`"system"` 及其他未知值按系统 locale。
    pub fn from_setting(setting: &str) -> Lang {
        match setting.trim() {
            "zh-CN" => Lang::ZhCn,
            "en" => Lang::En,
            _ => system_lang(),
        }
    }

    fn code(self) -> u8 {
        match self {
            Lang::ZhCn => 0,
            Lang::En => 1,
        }
    }

    fn from_code(code: u8) -> Lang {
        match code {
            1 => Lang::En,
            _ => Lang::ZhCn,
        }
    }
}

fn system_lang() -> Lang {
    sys_locale::get_locale()
        .map(|locale| from_locale(&locale))
        .unwrap_or(Lang::En)
}

/// locale 以 zh 开头（zh-CN / zh-Hans-CN / zh_CN.UTF-8）算中文，其余一律英文。
fn from_locale(locale: &str) -> Lang {
    if locale.trim_start().to_ascii_lowercase().starts_with("zh") {
        Lang::ZhCn
    } else {
        Lang::En
    }
}

/// 进程级当前语言。显式设置之前按中文——既是历史行为，也让单测结果不受机器 locale 影响。
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_current(lang: Lang) {
    CURRENT.store(lang.code(), Ordering::SeqCst);
}

pub fn current() -> Lang {
    Lang::from_code(CURRENT.load(Ordering::SeqCst))
}

/// 文案表：每个 key 对应 (中文, English)，元组类型保证两种语言不会漏填。
/// 带参数的条目用 `{name}` 占位，由 `tr_fmt` 替换。
fn entry(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        // 托盘菜单
        "tray.settings" => ("设置", "Settings"),
        "tray.history" => ("历史记录", "History"),
        "tray.autoLearning" => ("自动学习", "Auto-learning"),
        "tray.meetingStart" => ("开始会议录制", "Start Meeting Recording"),
        "tray.meetingStop" => ("停止会议录制（{elapsed}）", "Stop Meeting Recording ({elapsed})"),
        "tray.meetingFinalizing" => ("正在生成会议纪要…", "Generating meeting notes…"),
        "tray.meetingOpen" => ("会议记录…", "Meeting Notes…"),
        "tray.meetingAuto" => ("自动检测 Zoom 会议", "Auto-detect Zoom Meetings"),
        "tray.about" => ("关于", "About"),
        "tray.quit" => ("退出", "Quit"),

        // 窗口标题（也用作托盘报错对话框的标题）
        "window.settings.title" => ("ByeType 设置", "ByeType Settings"),
        "window.learning.title" => ("自动学习", "Auto-learning"),
        "window.meeting.title" => ("会议记录", "Meeting Notes"),

        // 会议记录落盘 / 导出
        "meeting.defaultTitle" => ("会议记录", "Meeting Notes"),
        "store.transcriptTitle" => ("会议转写 {date}", "Meeting Transcript {date}"),
        "store.transcriptHeading" => ("逐字转写", "Full Transcript"),
        "store.interrupted" => (
            "上次录制未正常结束",
            "The previous recording did not end properly",
        ),

        // Transcribe 说话人分离的行前缀
        "speaker.label" => ("说话人{n}", "Speaker {n}"),
        "speaker.unknown" => ("说话人", "Speaker"),
        "speaker.separator" => ("：", ": "),

        // require_bedrock 里的用途名
        "what.extract" => ("图像识别", "image text extraction"),
        "what.optimize" => ("文本优化", "text optimization"),
        "what.text" => ("文本处理", "text processing"),

        // 会议会话
        "err.meetingAlreadyRunning" => (
            "已有会议正在录制或收尾中",
            "A meeting is already being recorded or finalized",
        ),
        "err.meetingModelNoAudio" => (
            "会议转写模型不支持音频输入：{model}",
            "The meeting transcription model does not support audio input: {model}",
        ),
        "err.meetingNoInput" => (
            "麦克风和系统音频至少要开启一个",
            "Enable at least one of microphone or system audio",
        ),
        "err.meetingNotRecording" => (
            "当前没有正在录制的会议",
            "No meeting is currently being recorded",
        ),
        "err.meetingStateReset" => (
            "会话状态异常，已重置",
            "Session state was inconsistent and has been reset",
        ),
        "err.meetingStillActive" => (
            "会议仍在录制或收尾中",
            "The meeting is still being recorded or finalized",
        ),
        "err.meetingDeleteActive" => (
            "会议仍在录制或收尾中，不能删除",
            "The meeting is still being recorded or finalized and cannot be deleted",
        ),
        "err.meetingNotFound" => ("找不到会议 {id}", "Meeting {id} not found"),
        "err.meetingMetaMissing" => ("会议元数据丢失", "Meeting metadata is missing"),
        "err.noTranscript" => ("没有可用的转写内容", "No usable transcript"),
        "err.noTranscriptForSummary" => (
            "没有可用的转写内容，无法生成纪要",
            "No usable transcript, cannot generate meeting notes",
        ),
        "err.meetingNoTranscript" => (
            "这场会议没有可用的转写内容",
            "This meeting has no usable transcript",
        ),
        "err.chunkFailed" => ("[转写失败：{error}]", "[Transcription failed: {error}]"),
        "err.taskCancelled" => ("任务已取消", "Task cancelled"),
        "err.pickFolderCancelled" => ("选择目录已取消", "Folder selection cancelled"),
        "err.pickFolderInvalid" => (
            "无法解析所选目录: {error}",
            "Cannot resolve the selected folder: {error}",
        ),
        "err.openFinderFailed" => ("打开 Finder 失败: {error}", "Failed to open Finder: {error}"),
        "err.openExplorerFailed" => (
            "打开资源管理器失败: {error}",
            "Failed to open Explorer: {error}",
        ),
        "err.systemAudioUnsupported" => (
            "当前平台暂不支持录制系统音频",
            "System audio capture is not supported on this platform",
        ),

        // AI 层
        "err.modelNoAudio" => (
            "模型 {model} 不支持音频输入，语音转写请选择 Amazon Transcribe",
            "Model {model} does not support audio input; choose Amazon Transcribe for speech transcription",
        ),
        "err.modelTranscribeOnly" => (
            "模型 {model} 只能做语音转写，{what}请选择 Bedrock 上的模型",
            "Model {model} can only transcribe speech; choose a Bedrock model for {what}",
        ),
        "err.audioBase64" => (
            "Transcribe: 音频 base64 解码失败: {error}",
            "Transcribe: failed to decode audio base64: {error}",
        ),
        "err.audioDecode" => (
            "{label}: 解码音频失败: {error}",
            "{label}: failed to decode audio: {error}",
        ),
        "err.audioEmpty" => ("{label}: 音频为空", "{label}: audio is empty"),
        "err.awsCredentialHint" => (
            "（AWS 凭证不可用：请在终端运行 mwinit -o 刷新 Midway，并确认 profile 名称正确）",
            " (AWS credentials unavailable: run mwinit -o in a terminal to refresh Midway and check the profile name)",
        ),
        "err.unsupportedProtocol" => (
            "不支持的模型协议: {protocol}",
            "Unsupported model protocol: {protocol}",
        ),
        "err.awsTimeout" => ("AWS 请求超时(30 秒)", "AWS request timed out (30 s)"),

        _ => return None,
    })
}

/// 指定语言取文案；没有这个 key 时原样返回 key，方便一眼看出漏翻。
pub fn tr_in(lang: Lang, key: &str) -> &str {
    match entry(key) {
        Some((zh, en)) => match lang {
            Lang::ZhCn => zh,
            Lang::En => en,
        },
        None => key,
    }
}

/// 按当前语言取文案。
pub fn tr(key: &str) -> &str {
    tr_in(current(), key)
}

/// 指定语言取文案并替换 `{name}` 占位符。
pub fn tr_fmt_in(lang: Lang, key: &str, args: &[(&str, &str)]) -> String {
    let mut text = tr_in(lang, key).to_string();
    for (name, value) in args {
        text = text.replace(&format!("{{{}}}", name), value);
    }
    text
}

/// 按当前语言取文案并替换 `{name}` 占位符。
pub fn tr_fmt(key: &str, args: &[(&str, &str)]) -> String {
    tr_fmt_in(current(), key, args)
}

/// 按配置重设当前语言，并刷新三个预声明窗口的标题与托盘文案。
/// 启动时（托盘与 MeetingManager 就位后）与语言设置变更时调用。
pub fn apply_language(app: &AppHandle) {
    let setting = app.state::<ConfigManager>().get().general.language;
    let lang = Lang::from_setting(&setting);
    set_current(lang);
    for (label, key) in [
        ("settings", "window.settings.title"),
        ("learning", "window.learning.title"),
        ("meeting", "window.meeting.title"),
    ] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.set_title(tr_in(lang, key));
        }
    }
    crate::tray::apply_language(app);
}

#[cfg(test)]
mod tests {
    // 注意：这里不调用 set_current——测试并行跑，改进程级语言会影响 store.rs 等依赖默认中文的用例。
    use super::*;

    #[test]
    fn explicit_settings_select_language() {
        assert_eq!(Lang::from_setting("zh-CN"), Lang::ZhCn);
        assert_eq!(Lang::from_setting("en"), Lang::En);
        assert_eq!(Lang::from_setting(" en "), Lang::En);
    }

    #[test]
    fn locale_prefix_decides_system_language() {
        assert_eq!(from_locale("zh-Hans-CN"), Lang::ZhCn);
        assert_eq!(from_locale("zh_CN.UTF-8"), Lang::ZhCn);
        assert_eq!(from_locale("ZH-TW"), Lang::ZhCn);
        assert_eq!(from_locale("en-US"), Lang::En);
        assert_eq!(from_locale("ja-JP"), Lang::En);
        assert_eq!(from_locale(""), Lang::En);
    }

    #[test]
    fn tr_in_returns_language_specific_text() {
        assert_eq!(tr_in(Lang::ZhCn, "tray.settings"), "设置");
        assert_eq!(tr_in(Lang::En, "tray.settings"), "Settings");
        assert_eq!(tr_in(Lang::ZhCn, "window.meeting.title"), "会议记录");
        assert_eq!(tr_in(Lang::En, "window.meeting.title"), "Meeting Notes");
        assert_ne!(
            tr_in(Lang::ZhCn, "err.meetingNotFound"),
            tr_in(Lang::En, "err.meetingNotFound")
        );
    }

    #[test]
    fn unknown_key_falls_back_to_key() {
        assert_eq!(tr_in(Lang::ZhCn, "no.such.key"), "no.such.key");
        assert_eq!(tr_in(Lang::En, "no.such.key"), "no.such.key");
        assert_eq!(
            tr_fmt_in(Lang::En, "no.such.key", &[("n", "1")]),
            "no.such.key"
        );
    }

    #[test]
    fn tr_fmt_substitutes_placeholders() {
        assert_eq!(
            tr_fmt_in(Lang::ZhCn, "speaker.label", &[("n", "2")]),
            "说话人2"
        );
        assert_eq!(
            tr_fmt_in(Lang::En, "speaker.label", &[("n", "2")]),
            "Speaker 2"
        );
        assert_eq!(
            tr_fmt_in(Lang::En, "tray.meetingStop", &[("elapsed", "01:02")]),
            "Stop Meeting Recording (01:02)"
        );
        assert_eq!(
            tr_fmt_in(
                Lang::ZhCn,
                "err.modelTranscribeOnly",
                &[("model", "m"), ("what", "文本优化")]
            ),
            "模型 m 只能做语音转写，文本优化请选择 Bedrock 上的模型"
        );
    }
}
