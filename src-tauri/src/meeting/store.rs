//! 会议数据落盘：`<app_data>/meetings/<id>/{meta.json, transcript.jsonl, transcript.md, summary.md, audio/}`
//! 以及把纪要导出到用户指定的笔记目录（可指向 Obsidian vault）。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

pub const MEETINGS_DIR: &str = "meetings";
pub const DEFAULT_NOTES_DIR: &str = "meetings-notes";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingMeta {
    pub id: String,
    pub title: String,
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub duration_secs: u64,
    /// auto | manual
    pub source: String,
    /// recording | finalizing | done | summary_failed | failed | interrupted
    pub status: String,
    #[serde(default)]
    pub chunk_count: u32,
    #[serde(default)]
    pub failed_chunks: u32,
    #[serde(default)]
    pub transcribe_model: String,
    #[serde(default)]
    pub summary_model: String,
    #[serde(default)]
    pub notes_path: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub microphone: bool,
    #[serde(default)]
    pub system_audio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegment {
    pub index: u32,
    pub start_secs: u64,
    pub end_secs: u64,
    pub text: String,
    #[serde(default)]
    pub failed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingDetail {
    pub meta: MeetingMeta,
    pub transcript: String,
    pub summary: Option<String>,
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Clone)]
pub struct MeetingStore {
    root: PathBuf,
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content).map_err(|e| format!("写入 {} 失败: {}", tmp.display(), e))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("写入 {} 失败: {}", path.display(), e)
    })
}

pub fn format_hms(secs: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// 文件名里去掉路径分隔符与 Finder / Windows 不接受的字符。
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\t' => ' ',
            _ => c,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .chars()
        .take(60)
        .collect::<String>()
        .trim()
        .to_string()
}

impl MeetingStore {
    pub fn new(data_dir: &Path) -> Self {
        let root = data_dir.join(MEETINGS_DIR);
        let _ = fs::create_dir_all(&root);
        Self { root }
    }

    pub fn dir_of(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// 新建会议目录，id 形如 2026-09-17_1400，冲突时追加 -2 / -3。
    pub fn create_session(&self, now: DateTime<Local>) -> Result<(String, PathBuf), String> {
        let base = now.format("%Y-%m-%d_%H%M").to_string();
        let mut id = base.clone();
        let mut n = 2;
        while self.root.join(&id).exists() {
            id = format!("{}-{}", base, n);
            n += 1;
        }
        let dir = self.root.join(&id);
        fs::create_dir_all(&dir).map_err(|e| format!("创建会议目录失败: {}", e))?;
        Ok((id, dir))
    }

    pub fn write_meta(&self, dir: &Path, meta: &MeetingMeta) -> Result<(), String> {
        let json = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
        atomic_write(&dir.join("meta.json"), json.as_bytes())
    }

    pub fn read_meta(&self, dir: &Path) -> Option<MeetingMeta> {
        let raw = fs::read_to_string(dir.join("meta.json")).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn read_segments(&self, dir: &Path) -> Vec<TranscriptSegment> {
        let Ok(raw) = fs::read_to_string(dir.join("transcript.jsonl")) else {
            return Vec::new();
        };
        raw.lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// 追加一段转写并重建 transcript.md。
    pub fn append_segment(&self, dir: &Path, segment: &TranscriptSegment) -> Result<(), String> {
        let line = serde_json::to_string(segment).map_err(|e| e.to_string())?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("transcript.jsonl"))
            .map_err(|e| format!("打开 transcript.jsonl 失败: {}", e))?;
        writeln!(file, "{}", line).map_err(|e| format!("写入 transcript.jsonl 失败: {}", e))?;
        let segments = self.read_segments(dir);
        self.rebuild_transcript_md(dir, &segments)
    }

    fn rebuild_transcript_md(
        &self,
        dir: &Path,
        segments: &[TranscriptSegment],
    ) -> Result<(), String> {
        let meta = self.read_meta(dir);
        let mut md = String::new();
        md.push_str(&format!(
            "# 会议转写 {}\n",
            meta.as_ref()
                .map(|m| m.started_at.clone())
                .unwrap_or_default()
        ));
        for segment in segments {
            md.push_str(&format!(
                "\n## [{}–{}]\n\n{}\n",
                format_hms(segment.start_secs),
                format_hms(segment.end_secs),
                segment.text.trim()
            ));
        }
        atomic_write(&dir.join("transcript.md"), md.as_bytes())
    }

    pub fn read_transcript_md(&self, dir: &Path) -> Option<String> {
        fs::read_to_string(dir.join("transcript.md")).ok()
    }

    /// 转写正文（不含标题行），给纪要模型用。
    pub fn transcript_body(&self, dir: &Path) -> String {
        let segments = self.read_segments(dir);
        segments
            .iter()
            .filter(|s| !s.failed)
            .map(|s| {
                format!(
                    "## [{}–{}]\n{}",
                    format_hms(s.start_secs),
                    format_hms(s.end_secs),
                    s.text.trim()
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub fn write_summary(&self, dir: &Path, markdown: &str) -> Result<(), String> {
        atomic_write(&dir.join("summary.md"), markdown.as_bytes())
    }

    pub fn read_summary(&self, dir: &Path) -> Option<String> {
        fs::read_to_string(dir.join("summary.md")).ok()
    }

    pub fn save_chunk_audio(&self, dir: &Path, index: u32, flac: &[u8]) -> Result<PathBuf, String> {
        let audio_dir = dir.join("audio");
        fs::create_dir_all(&audio_dir).map_err(|e| format!("创建 audio 目录失败: {}", e))?;
        let path = audio_dir.join(format!("chunk-{:03}.flac", index));
        fs::write(&path, flac).map_err(|e| format!("写入音频分段失败: {}", e))?;
        Ok(path)
    }

    /// 所有会议，按开始时间倒序。
    pub fn list(&self) -> Vec<MeetingMeta> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut metas: Vec<MeetingMeta> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| self.read_meta(&entry.path()))
            .collect();
        metas.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        metas
    }

    pub fn read_detail(&self, id: &str) -> Result<MeetingDetail, String> {
        let dir = self.dir_of(id);
        let meta = self
            .read_meta(&dir)
            .ok_or_else(|| format!("找不到会议 {}", id))?;
        Ok(MeetingDetail {
            transcript: self.read_transcript_md(&dir).unwrap_or_default(),
            summary: self.read_summary(&dir),
            segments: self.read_segments(&dir),
            meta,
        })
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let dir = self.dir_of(id);
        if !dir.starts_with(&self.root) || !dir.exists() {
            return Ok(());
        }
        fs::remove_dir_all(&dir).map_err(|e| format!("删除会议目录失败: {}", e))
    }

    /// 把纪要 + 转写导出成一份 Markdown 到笔记目录，返回文件路径。
    pub fn export_to_notes(
        &self,
        notes_folder: &Path,
        meta: &MeetingMeta,
        summary: &str,
        transcript_body: &str,
    ) -> Result<PathBuf, String> {
        fs::create_dir_all(notes_folder).map_err(|e| format!("创建笔记目录失败: {}", e))?;
        let stamp = DateTime::parse_from_rfc3339(&meta.started_at)
            .map(|t| t.with_timezone(&Local).format("%Y-%m-%d %H%M").to_string())
            .unwrap_or_else(|_| meta.id.replace('_', " "));
        let title = sanitize_filename(&meta.title);
        let filename = if title.is_empty() {
            format!("{} 会议记录.md", stamp)
        } else {
            format!("{} {}.md", stamp, title)
        };
        let path = notes_folder.join(filename);
        let content = format!(
            "{}\n\n---\n\n## 逐字转写\n\n{}\n",
            summary.trim_end(),
            transcript_body.trim()
        );
        atomic_write(&path, content.as_bytes())?;
        Ok(path)
    }

    /// 启动时把上次没收尾的会议标成「已中断」；转写 jsonl 仍在，可以事后生成纪要。
    pub fn recover_interrupted(&self) {
        for meta in self.list() {
            if meta.status == "recording" || meta.status == "finalizing" {
                let dir = self.dir_of(&meta.id);
                let mut meta = meta;
                meta.status = "interrupted".to_string();
                meta.error = Some("上次录制未正常结束".to_string());
                let _ = self.write_meta(&dir, &meta);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> (MeetingStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "byetype-meeting-store-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        (MeetingStore::new(&dir), dir)
    }

    fn meta(id: &str) -> MeetingMeta {
        MeetingMeta {
            id: id.to_string(),
            title: "项目周会".to_string(),
            started_at: "2026-09-17T14:00:00+08:00".to_string(),
            ended_at: None,
            duration_secs: 0,
            source: "manual".to_string(),
            status: "recording".to_string(),
            chunk_count: 0,
            failed_chunks: 0,
            transcribe_model: String::new(),
            summary_model: String::new(),
            notes_path: None,
            error: None,
            microphone: true,
            system_audio: false,
        }
    }

    #[test]
    fn sanitizes_filenames() {
        assert_eq!(sanitize_filename("a/b:c*d?e\"f<g>h|i"), "a b c d e f g h i");
        assert_eq!(sanitize_filename("  多  空格  "), "多 空格");
        assert_eq!(sanitize_filename(&"长".repeat(100)).chars().count(), 60);
    }

    #[test]
    fn appends_segments_and_rebuilds_markdown() {
        let (store, root) = temp_store("segments");
        let (id, dir) = store.create_session(Local::now()).unwrap();
        store.write_meta(&dir, &meta(&id)).unwrap();

        store
            .append_segment(
                &dir,
                &TranscriptSegment {
                    index: 0,
                    start_secs: 0,
                    end_secs: 240,
                    text: "我：大家好".into(),
                    failed: false,
                },
            )
            .unwrap();
        store
            .append_segment(
                &dir,
                &TranscriptSegment {
                    index: 1,
                    start_secs: 240,
                    end_secs: 300,
                    text: "[转写失败]".into(),
                    failed: true,
                },
            )
            .unwrap();

        let md = store.read_transcript_md(&dir).unwrap();
        assert!(md.contains("## [00:00:00–00:04:00]\n\n我：大家好"));
        assert!(md.contains("## [00:04:00–00:05:00]"));
        assert_eq!(store.read_segments(&dir).len(), 2);
        // 纪要输入只保留成功的段
        assert!(!store.transcript_body(&dir).contains("转写失败"));

        let detail = store.read_detail(&id).unwrap();
        assert_eq!(detail.segments.len(), 2);
        assert!(detail.summary.is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn exports_notes_with_timestamped_title() {
        let (store, root) = temp_store("export");
        let notes = root.join("notes");
        let path = store
            .export_to_notes(
                &notes,
                &meta("2026-09-17_1400"),
                "# 项目周会\n\n概述",
                "我：大家好",
            )
            .unwrap();
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("2026-09-17 1400 项目周会.md"));
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("# 项目周会"));
        assert!(content.contains("## 逐字转写"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recovers_interrupted_sessions_and_lists_newest_first() {
        let (store, root) = temp_store("recover");
        let dir_a = store.dir_of("2026-09-16_1000");
        fs::create_dir_all(&dir_a).unwrap();
        let mut a = meta("2026-09-16_1000");
        a.started_at = "2026-09-16T10:00:00+08:00".to_string();
        store.write_meta(&dir_a, &a).unwrap();

        let dir_b = store.dir_of("2026-09-17_1400");
        fs::create_dir_all(&dir_b).unwrap();
        let mut b = meta("2026-09-17_1400");
        b.status = "done".to_string();
        store.write_meta(&dir_b, &b).unwrap();

        store.recover_interrupted();
        let list = store.list();
        assert_eq!(list[0].id, "2026-09-17_1400");
        assert_eq!(list[1].status, "interrupted");
        assert_eq!(list[0].status, "done");

        store.delete("2026-09-16_1000").unwrap();
        assert_eq!(store.list().len(), 1);
        let _ = fs::remove_dir_all(root);
    }
}
