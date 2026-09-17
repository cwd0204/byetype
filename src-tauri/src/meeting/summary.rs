//! 会议纪要：把转写交给文本模型跑 meeting-summary 提示词。
//! 转写太长时 map-reduce：先按段界分组各出分段纪要，再合并成一份。

use std::path::Path;

use super::store::MeetingMeta;
use crate::ai;
use crate::config::types::AppConfig;

pub const USAGE_SCENE: &str = "meeting-summary";
pub const DIRECT_SUMMARY_MAX_CHARS: usize = 120_000;
pub const MAP_SECTION_MAX_CHARS: usize = 40_000;

pub struct SummaryOutput {
    pub markdown: String,
    pub title: String,
    pub model: String,
}

/// 纪要模型：设置里选的 → 文本优化模型（能处理文本时）→ 自动学习模型。
pub fn summary_model_id(config: &AppConfig) -> String {
    let configured = config.meeting.summary_model_id.trim();
    if !configured.is_empty() && ai::models::resolve_model(config, configured).is_ok() {
        return configured.to_string();
    }
    let optimize = config.voice_templates.model_id.trim();
    if !optimize.is_empty() && ai::models::supports_text(config, optimize).unwrap_or(false) {
        return optimize.to_string();
    }
    config.voice_learning.model_id.clone()
}

/// 纪要首行 `# 标题`。
pub fn parse_title(markdown: &str) -> Option<String> {
    markdown
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("# "))
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .filter(|title| !title.is_empty())
}

fn meta_tag(meta: &MeetingMeta, mode: &str, extra: &str) -> String {
    let minutes = meta.duration_secs.div_ceil(60);
    format!(
        "<meta date=\"{}\" duration=\"{} 分钟\" mode=\"{}\"{} />",
        meta.started_at,
        minutes,
        mode,
        if extra.is_empty() {
            String::new()
        } else {
            format!(" {}", extra)
        }
    )
}

/// 按「## [」段界把转写分成若干块，每块不超过 max_chars。
pub(crate) fn split_sections(transcript: &str, max_chars: usize) -> Vec<String> {
    let mut groups: Vec<String> = Vec::new();
    let mut current = String::new();
    for part in transcript.split("\n## [") {
        let piece = if current.is_empty() && groups.is_empty() {
            part.to_string()
        } else {
            format!("\n## [{}", part)
        };
        if !current.is_empty() && current.chars().count() + piece.chars().count() > max_chars {
            groups.push(std::mem::take(&mut current));
        }
        current.push_str(&piece);
    }
    if !current.trim().is_empty() {
        groups.push(current);
    }
    groups
}

async fn run(
    client: &reqwest::Client,
    config: &AppConfig,
    system_prompt: &str,
    model_id: &str,
    input: &str,
) -> Result<String, String> {
    ai::complete_text(
        client,
        input,
        system_prompt,
        config,
        model_id,
        &config.meeting.summary_thinking,
        USAGE_SCENE,
    )
    .await
}

pub async fn summarize(
    client: &reqwest::Client,
    config: &AppConfig,
    prompts_dir: &Path,
    meta: &MeetingMeta,
    transcript: &str,
) -> Result<SummaryOutput, String> {
    let system_prompt = ai::prompt::build_meeting_summary_prompt(config, prompts_dir);
    if system_prompt.is_empty() {
        return Err("会议纪要提示词为空，请检查 meeting-summary.md".to_string());
    }
    let model_id = summary_model_id(config);
    let model_label = ai::models::resolve_model(config, &model_id)
        .map(|m| m.model)
        .unwrap_or_else(|_| model_id.clone());

    let markdown = if transcript.chars().count() <= DIRECT_SUMMARY_MAX_CHARS {
        let input = format!(
            "{}\n<transcript>\n{}\n</transcript>",
            meta_tag(meta, "full", ""),
            transcript.trim()
        );
        run(client, config, &system_prompt, &model_id, &input).await?
    } else {
        let sections = split_sections(transcript, MAP_SECTION_MAX_CHARS);
        let total = sections.len();
        let mut notes = Vec::with_capacity(total);
        for (i, section) in sections.iter().enumerate() {
            let input = format!(
                "{}\n<transcript>\n{}\n</transcript>",
                meta_tag(meta, "section", &format!("part=\"{}/{}\"", i + 1, total)),
                section.trim()
            );
            let note = run(client, config, &system_prompt, &model_id, &input).await?;
            notes.push(format!(
                "<section-notes index=\"{}\">\n{}\n</section-notes>",
                i + 1,
                note.trim()
            ));
        }
        let input = format!("{}\n{}", meta_tag(meta, "merge", ""), notes.join("\n"));
        run(client, config, &system_prompt, &model_id, &input).await?
    };

    let markdown = markdown.trim().to_string();
    if markdown.is_empty() {
        return Err("纪要模型返回了空内容".to_string());
    }
    let title = parse_title(&markdown).unwrap_or_default();
    Ok(SummaryOutput {
        markdown,
        title,
        model: model_label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_title_from_first_heading() {
        assert_eq!(
            parse_title("# 项目周会\n\n概述"),
            Some("项目周会".to_string())
        );
        assert_eq!(parse_title("概述\n### 主题"), None);
        assert_eq!(parse_title("  #  带空格  \n"), Some("带空格".to_string()));
    }

    #[test]
    fn splits_transcript_on_section_boundaries() {
        let transcript = format!(
            "## [00:00:00–00:04:00]\n{}\n\n## [00:04:00–00:08:00]\n{}\n\n## [00:08:00–00:12:00]\n{}",
            "a".repeat(30), "b".repeat(30), "c".repeat(30)
        );
        let groups = split_sections(&transcript, 120);
        assert_eq!(groups.len(), 2);
        assert!(groups[0].contains("aaaa") && groups[0].contains("bbbb"));
        assert!(groups[1].starts_with("\n## [00:08:00"));
        assert_eq!(split_sections(&transcript, 10_000).len(), 1);
    }

    #[test]
    fn summary_model_prefers_configured_then_text_model() {
        let mut config = AppConfig::default();
        assert_eq!(summary_model_id(&config), "builtin-bedrock-claude-sonnet-5");

        config.meeting.summary_model_id = "not-a-model".to_string();
        config.voice_templates.model_id = "builtin-bedrock-claude-haiku-4-5".to_string();
        assert_eq!(summary_model_id(&config), "builtin-bedrock-claude-haiku-4-5");

        config.voice_templates.model_id = "builtin-aws-transcribe".to_string();
        assert_eq!(summary_model_id(&config), config.voice_learning.model_id);
    }
}
