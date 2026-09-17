//! 会议记录：检测 Zoom 会议 → 采集麦克风 + 系统音频 → 分段转写 → 生成纪要 → 落盘。
//!
//! - `detector`  轮询 Zoom 的 CptHost 进程，判断是否在开会
//! - `capture`   麦克风（cpal）与系统音频（macOS Core Audio 进程 tap）采集，切段并 FLAC 编码
//! - `chunker`   双轨对齐、混音、按静音切段、说话人能量提示
//! - `session`   MeetingManager 状态机（Idle → Recording → Finalizing → Idle）与转写 / 收尾任务
//! - `transcribe` / `summary`  复用 ai 层做分段转写与纪要
//! - `store`     meetings/<id>/ 目录读写与纪要导出
//! - `window` / `commands`  会议窗口与 Tauri 命令

pub mod capture;
pub mod chunker;
pub mod commands;
pub mod detector;
pub mod session;
pub mod store;
pub mod summary;
pub mod transcribe;
pub mod window;

pub use session::MeetingManager;
