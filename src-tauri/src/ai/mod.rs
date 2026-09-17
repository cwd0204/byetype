//! AI 层：只有 AWS 两条路径。
//! - 音频 → Amazon Transcribe（`transcribe_aws.rs`）
//! - 文本 / 图像 → Bedrock 上的 Claude（`bedrock.rs`）
//!
//! 所有调用成功后都经 `record_usage` 记入用量统计。

pub mod aws;
pub mod bedrock;
pub mod models;
pub mod prompt;
pub mod retry;
pub mod transcribe_aws;
pub mod transcribe_live;
pub mod types;

use crate::config::types::{AppConfig, ThinkingConfig};
use crate::i18n::{tr, tr_fmt};
use crate::usage;
use base64::Engine as _;
use models::{PROTOCOL_AWS_TRANSCRIBE, PROTOCOL_BEDROCK};
use std::path::Path;
use transcribe_aws::TranscribeOptions;
use types::TokenUsage;

/// 把一次成功调用的用量写入统计。失败调用不记录，重试中的失败也不记录。
fn record_usage(scene: &str, resolved: &models::ResolvedModel, usage: TokenUsage) {
    usage::record(
        scene,
        &resolved.model,
        &resolved.provider_label,
        usage.prompt_tokens,
        usage.completion_tokens,
    );
}

/// 成功调用的输出：文本 + 实际使用的模型信息（供耗时统计按模型归集）。
pub struct AiOutput {
    pub text: String,
    pub model: String,
    pub provider: String,
}

fn ai_output(text: String, resolved: &models::ResolvedModel) -> AiOutput {
    AiOutput {
        text,
        model: resolved.model.clone(),
        provider: resolved.provider_label.clone(),
    }
}

/// 听写转写。Transcribe 吃不到提示词，所以 prompts_dir / learning_rules 在这里用不上，
/// 专有词纠错由文本优化阶段补上（prompt.rs `build_optimize_prompt`）。
/// 参数形态保留是为了和 task 管道的调用方式一致。
pub async fn transcribe(
    _client: &reqwest::Client,
    audio_base64: &str,
    config: &AppConfig,
    _prompts_dir: &Path,
    _learning_rules: &str,
) -> Result<AiOutput, String> {
    transcribe_audio(
        audio_base64,
        config,
        &config.transcribe.model_id,
        "transcribe",
        TranscribeOptions::default(),
    )
    .await
}

/// 用指定音频模型转写一段 FLAC(base64)。听写与会议分段共用，区别只在场景与说话人分离开关。
pub async fn transcribe_audio(
    audio_base64: &str,
    config: &AppConfig,
    model_id: &str,
    scene: &str,
    opts: TranscribeOptions,
) -> Result<AiOutput, String> {
    let resolved = models::resolve_model(config, model_id)?;
    if resolved.protocol != PROTOCOL_AWS_TRANSCRIBE {
        return Err(tr_fmt(
            "err.modelNoAudio",
            &[("model", resolved.model.as_str())],
        ));
    }
    let flac = base64::engine::general_purpose::STANDARD
        .decode(audio_base64)
        .map_err(|e| tr_fmt("err.audioBase64", &[("error", e.to_string().as_str())]))?;
    let text = transcribe_aws::transcribe(&config.models.aws, flac, opts).await?;
    record_usage(scene, &resolved, TokenUsage::default());
    Ok(ai_output(text, &resolved))
}

/// 听写能不能走「边说边转写」。只有解析出来的转写模型确实是 Amazon Transcribe
/// 才开流：自定义模型可能是别的协议，那条路径没有流式实现。
pub fn live_transcribe_supported(config: &AppConfig) -> bool {
    models::resolve_model(config, &config.transcribe.model_id)
        .map(|resolved| resolved.protocol == PROTOCOL_AWS_TRANSCRIBE)
        .unwrap_or(false)
}

/// 给流式转写的结果补上模型信息与用量记录。
///
/// 流式路径绕过了 `transcribe_audio`，但 `usage.jsonl` 的行数语义必须保持不变：
/// 每次录音仍然只有一条 `transcribe` 行，且模型字段与整段路径一致。
pub fn transcribe_live_output(config: &AppConfig, text: String) -> Result<AiOutput, String> {
    let resolved = models::resolve_model(config, &config.transcribe.model_id)?;
    record_usage("transcribe", &resolved, TokenUsage::default());
    Ok(ai_output(text, &resolved))
}

/// 图像识别用哪个模型的思考设置:
/// 图像识别自己没有思考开关,默认沿用「文本优化模型」那一栏的思考设置,
/// 让同一个模型在优化和取字两处表现一致。extract.thinking 单独设过时以它为准。
fn extract_thinking(config: &AppConfig) -> &ThinkingConfig {
    config
        .extract
        .thinking
        .as_ref()
        .unwrap_or(&config.voice_templates.thinking)
}

/// 图像识别用哪个模型：单独设置了就用它，否则跟随文本优化模型（转写模型是 Transcribe，看不了图）。
fn extract_model_id(config: &AppConfig) -> &str {
    match config.extract.model_id.as_deref() {
        Some(id) if !id.trim().is_empty() => id,
        _ if !config.voice_templates.model_id.trim().is_empty() => &config.voice_templates.model_id,
        _ => models::DEFAULT_TEXT_MODEL,
    }
}

/// `what_key` 是 i18n 里 `what.*` 的用途名（图像识别 / 文本优化 / 文本处理）。
fn require_bedrock(resolved: &models::ResolvedModel, what_key: &str) -> Result<(), String> {
    if resolved.protocol != PROTOCOL_BEDROCK {
        return Err(tr_fmt(
            "err.modelTranscribeOnly",
            &[("model", resolved.model.as_str()), ("what", tr(what_key))],
        ));
    }
    Ok(())
}

/// Extract text from an image using the configured Bedrock model.
pub async fn extract_text(
    _client: &reqwest::Client,
    image_base64: &str,
    config: &AppConfig,
    prompts_dir: &Path,
    template_id: &str,
) -> Result<String, String> {
    let resolved = models::resolve_model(config, extract_model_id(config))?;
    require_bedrock(&resolved, "what.extract")?;
    let system_prompt = prompt::build_extract_prompt(config, prompts_dir, template_id);
    let (text, usage) = bedrock::extract_text(
        &config.models.aws,
        image_base64,
        &system_prompt,
        &resolved.model,
        extract_thinking(config),
    )
    .await?;
    record_usage("extract", &resolved, usage);
    Ok(text)
}

/// 文本类调用：优化、自动学习、会议纪要都走这里。
async fn run_text(
    text: &str,
    system_prompt: &str,
    config: &AppConfig,
    resolved: &models::ResolvedModel,
    thinking: &ThinkingConfig,
    what_key: &str,
) -> Result<(String, TokenUsage), String> {
    require_bedrock(resolved, what_key)?;
    bedrock::optimize(
        &config.models.aws,
        text,
        system_prompt,
        &resolved.model,
        thinking,
    )
    .await
}

/// Optimize text using the configured Bedrock model.
pub async fn optimize(
    _client: &reqwest::Client,
    text: &str,
    config: &AppConfig,
    prompts_dir: &Path,
    template_id: &str,
    learning_rules: &str,
) -> Result<AiOutput, String> {
    let system_prompt =
        prompt::build_optimize_prompt(config, prompts_dir, template_id, learning_rules);
    if system_prompt.is_empty() {
        // 提示词为空时不会发起 API 调用，不计入用量
        return Ok(AiOutput {
            text: text.to_string(),
            model: String::new(),
            provider: String::new(),
        });
    }

    let resolved = models::resolve_model(config, &config.voice_templates.model_id)?;
    let (text, usage) = run_text(
        text,
        &system_prompt,
        config,
        &resolved,
        &config.voice_templates.thinking,
        "what.optimize",
    )
    .await?;
    record_usage("optimize", &resolved, usage);
    Ok(ai_output(text, &resolved))
}

/// 用指定文本模型跑一段系统提示词 + 输入，返回纯文本。自动学习、会议纪要共用。
pub async fn complete_text(
    _client: &reqwest::Client,
    input: &str,
    system_prompt: &str,
    config: &AppConfig,
    model_id: &str,
    thinking: &ThinkingConfig,
    scene: &str,
) -> Result<String, String> {
    let resolved = models::resolve_model(config, model_id)?;
    let (text, usage) = run_text(input, system_prompt, config, &resolved, thinking, "what.text").await?;
    record_usage(scene, &resolved, usage);
    Ok(text)
}

/// Analyze a user correction using the configured learning model.
pub async fn analyze_correction(
    client: &reqwest::Client,
    input: &str,
    system_prompt: &str,
    config: &AppConfig,
) -> Result<String, String> {
    complete_text(
        client,
        input,
        system_prompt,
        config,
        &config.voice_learning.model_id,
        &config.voice_learning.thinking,
        "learn",
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thinking(enabled: bool) -> ThinkingConfig {
        ThinkingConfig {
            enabled,
            level: "LOW".to_string(),
        }
    }

    #[test]
    fn extract_follows_text_optimize_thinking_when_unset() {
        let mut config = AppConfig::default();
        config.voice_templates.thinking = thinking(true);
        config.transcribe.thinking = thinking(false);

        assert!(extract_thinking(&config).enabled);
    }

    #[test]
    fn extract_thinking_setting_overrides_text_optimize() {
        let mut config = AppConfig::default();
        config.extract.thinking = Some(thinking(false));
        config.voice_templates.thinking = thinking(true);

        assert!(!extract_thinking(&config).enabled);
    }

    #[test]
    fn extract_model_falls_back_to_text_model_not_transcribe() {
        let mut config = AppConfig::default();
        config.extract.model_id = None;
        config.voice_templates.model_id = "builtin-bedrock-claude-haiku-4-5".to_string();
        assert_eq!(extract_model_id(&config), "builtin-bedrock-claude-haiku-4-5");

        config.voice_templates.model_id = String::new();
        assert_eq!(extract_model_id(&config), models::DEFAULT_TEXT_MODEL);

        config.extract.model_id = Some("builtin-bedrock-claude-opus-5".to_string());
        assert_eq!(extract_model_id(&config), "builtin-bedrock-claude-opus-5");
    }

    #[test]
    fn non_bedrock_models_are_rejected_for_text() {
        let config = AppConfig::default();
        let resolved = models::resolve_model(&config, models::DEFAULT_TRANSCRIBE_MODEL).unwrap();
        let error = require_bedrock(&resolved, "what.optimize").unwrap_err();
        // 用途名已经翻译进句子里，不会把 key 原样漏给用户
        assert!(!error.contains("what.optimize"), "{error}");
        let claude = models::resolve_model(&config, models::DEFAULT_TEXT_MODEL).unwrap();
        assert!(require_bedrock(&claude, "what.optimize").is_ok());
    }
}
