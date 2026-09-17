//! 会议分段转写：走 Amazon Transcribe 并开说话人分离，输出「说话人N：」行。

use super::chunker::ReadyChunk;
use crate::ai;
use crate::ai::transcribe_aws::TranscribeOptions;
use crate::audio::encoder;
use crate::config::types::AppConfig;

pub const USAGE_SCENE: &str = "meeting-transcribe";

/// 会议转写用哪个模型：单独设置了就用它，否则跟随「转写设置」。
pub fn transcribe_model_id(config: &AppConfig) -> &str {
    if config.meeting.transcribe_model_id.trim().is_empty() {
        &config.transcribe.model_id
    } else {
        &config.meeting.transcribe_model_id
    }
}

pub async fn transcribe_chunk(config: &AppConfig, chunk: &ReadyChunk) -> Result<String, String> {
    let flac_base64 = encoder::audio_to_base64(&chunk.flac);
    ai::transcribe_audio(
        &flac_base64,
        config,
        transcribe_model_id(config),
        USAGE_SCENE,
        TranscribeOptions {
            speaker_labels: true,
        },
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
}
