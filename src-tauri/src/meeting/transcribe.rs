//! 会议分段转写：按会议转写模型的协议分两路。
//! - Amazon Transcribe：直接送音频并开说话人分离，输出「说话人N：」行
//! - 多模态 LLM（Gemini / Qwen 等）：带 meeting-transcribe 提示词 + 上一段结尾 + 能量提示

use std::path::Path;

use super::chunker::ReadyChunk;
use crate::ai;
use crate::ai::models::PROTOCOL_AWS_TRANSCRIBE;
use crate::ai::prompt::{self, MeetingChunkContext};
use crate::ai::transcribe_aws::TranscribeOptions;
use crate::config::types::AppConfig;

pub const USAGE_SCENE: &str = "meeting-transcribe";
/// 传给下一段的上文长度（字符）
pub const TAIL_CHARS: usize = 300;

/// 会议转写用哪个模型：单独设置了就用它，否则跟随「转写设置」。
pub fn transcribe_model_id(config: &AppConfig) -> &str {
    if config.meeting.transcribe_model_id.trim().is_empty() {
        &config.transcribe.model_id
    } else {
        &config.meeting.transcribe_model_id
    }
}

/// 取文本结尾的若干字符作为下一段的承接上下文。
pub fn tail_of(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let start = chars.len().saturating_sub(TAIL_CHARS);
    chars[start..].iter().collect::<String>().trim().to_string()
}

pub async fn transcribe_chunk(
    client: &reqwest::Client,
    config: &AppConfig,
    prompts_dir: &Path,
    learning_rules: &str,
    chunk: &ReadyChunk,
    flac_base64: &str,
    previous_tail: &str,
) -> Result<String, String> {
    let model_id = transcribe_model_id(config);
    let protocol = ai::models::protocol_of(config, model_id)
        .ok_or_else(|| format!("会议转写模型不存在: {}", model_id))?;

    if protocol == PROTOCOL_AWS_TRANSCRIBE {
        return ai::transcribe_with_prompt(
            client,
            flac_base64,
            config,
            model_id,
            "",
            &config.transcribe.thinking,
            USAGE_SCENE,
            TranscribeOptions {
                speaker_labels: true,
            },
        )
        .await
        .map(|output| output.text);
    }

    let ctx = MeetingChunkContext {
        chunk_index: chunk.index,
        start_offset_secs: chunk.start_offset_secs,
        previous_tail,
        speaker_hints: &chunk.speaker_hints,
    };
    let system_prompt =
        prompt::build_meeting_transcribe_prompt(config, prompts_dir, learning_rules, &ctx);
    ai::transcribe_with_prompt(
        client,
        flac_base64,
        config,
        model_id,
        &system_prompt,
        &config.transcribe.thinking,
        USAGE_SCENE,
        TranscribeOptions::default(),
    )
    .await
    .map(|output| output.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_dictation_model() {
        let mut config = AppConfig::default();
        assert_eq!(transcribe_model_id(&config), config.transcribe.model_id);
        config.meeting.transcribe_model_id = "builtin-aws-transcribe".to_string();
        assert_eq!(transcribe_model_id(&config), "builtin-aws-transcribe");
    }

    #[test]
    fn tail_keeps_last_chars() {
        let text = "一".repeat(500);
        assert_eq!(tail_of(&text).chars().count(), TAIL_CHARS);
        assert_eq!(tail_of("短文本"), "短文本");
    }
}
