pub mod types;
pub mod transport;
pub mod retry;
pub mod gemini;
pub mod openai_compat;
pub mod mimo;
pub mod deepseek;
pub mod prompt;
pub mod models;
pub mod aws;
pub mod bedrock;
pub mod transcribe_aws;

use crate::config::types::{AppConfig, ThinkingConfig};
use crate::usage;
use base64::Engine as _;
use models::{PROTOCOL_AWS_TRANSCRIBE, PROTOCOL_BEDROCK};
use std::path::Path;
use types::TokenUsage;

/// 判断 resolved model 是否走 DeepSeek 官方 API。
/// 条件:协议是 openai-compat 且 base_url 指向 api.deepseek.com。
/// 这样 OpenRouter 上的 `deepseek/*` 模型仍走标准 openai-compat 路径(不传 thinking)。
pub(crate) fn is_deepseek(resolved: &models::ResolvedModel) -> bool {
    resolved.protocol == "openai-compat"
        && resolved.base_url.contains("api.deepseek.com")
}

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

/// Transcribe audio using the configured provider.
pub async fn transcribe(
    client: &reqwest::Client,
    audio_base64: &str,
    config: &AppConfig,
    prompts_dir: &Path,
    learning_rules: &str,
) -> Result<AiOutput, String> {
    let system_prompt = prompt::build_transcribe_prompt(config, prompts_dir, learning_rules);
    transcribe_with_prompt(
        client,
        audio_base64,
        config,
        &config.transcribe.model_id,
        &system_prompt,
        &config.transcribe.thinking,
        "transcribe",
        transcribe_aws::TranscribeOptions::default(),
    )
    .await
}

/// 用指定模型与提示词转写一段 FLAC(base64)。听写与会议分段转写共用这条路径，
/// 区别只在 model_id / 提示词 / 用量场景；`aws_opts` 只对 Amazon Transcribe 生效。
#[allow(clippy::too_many_arguments)]
pub async fn transcribe_with_prompt(
    client: &reqwest::Client,
    audio_base64: &str,
    config: &AppConfig,
    model_id: &str,
    system_prompt: &str,
    thinking: &ThinkingConfig,
    scene: &str,
    aws_opts: transcribe_aws::TranscribeOptions,
) -> Result<AiOutput, String> {
    let resolved = models::resolve_model(config, model_id)?;

    let outcome: Result<(String, TokenUsage), String> = if is_deepseek(&resolved) {
        deepseek::transcribe(
            client,
            audio_base64,
            system_prompt,
            &resolved.api_key,
            &resolved.model,
            &resolved.base_url,
        )
        .await
    } else {
        match resolved.protocol.as_str() {
            "gemini" => {
                gemini::transcribe(
                    client,
                    audio_base64,
                    system_prompt,
                    &resolved.api_key,
                    &resolved.model,
                    &resolved.base_url,
                    thinking,
                )
                .await
            }
            "qwen-omni" => {
                openai_compat::qwen_omni_transcribe(
                    client,
                    audio_base64,
                    system_prompt,
                    &resolved.api_key,
                    &resolved.model,
                    &resolved.base_url,
                )
                .await
            }
            "mimo" => {
                mimo::transcribe(
                    client,
                    audio_base64,
                    system_prompt,
                    &resolved.api_key,
                    &resolved.model,
                    &resolved.base_url,
                )
                .await
            }
            PROTOCOL_AWS_TRANSCRIBE => {
                // Transcribe 吃不到提示词；专有词纠错由文本优化阶段补上（prompt.rs）。
                let flac = base64::engine::general_purpose::STANDARD
                    .decode(audio_base64)
                    .map_err(|e| format!("Transcribe: 音频 base64 解码失败: {}", e))?;
                transcribe_aws::transcribe(&config.models.aws, flac, aws_opts)
                    .await
                    .map(|text| (text, TokenUsage::default()))
            }
            PROTOCOL_BEDROCK => Err(
                "Bedrock 上的 Claude 不支持音频输入，语音转写请选择 Amazon Transcribe 或其他音频模型"
                    .to_string(),
            ),
            _ => {
                openai_compat::transcribe(
                    client,
                    audio_base64,
                    system_prompt,
                    &resolved.api_key,
                    &resolved.model,
                    &resolved.base_url,
                    resolved.audio_input_mode,
                    resolved.chat_template_kwargs.as_ref(),
                    Some(thinking),
                )
                .await
            }
        }
    };

    match outcome {
        Ok((text, usage)) => {
            record_usage(scene, &resolved, usage);
            Ok(ai_output(text, &resolved))
        }
        Err(e) => Err(e),
    }
}

/// 图像识别用哪个模型的思考设置:
/// 图像识别自己没有思考开关,默认沿用「文本优化模型」那一栏的思考设置,
/// 让同一个模型在优化和取字两处表现一致。extract.thinking 单独设过时以它为准。
fn extract_thinking(config: &AppConfig) -> &crate::config::types::ThinkingConfig {
    config
        .extract
        .thinking
        .as_ref()
        .unwrap_or(&config.voice_templates.thinking)
}

/// Extract text from an image using the configured provider.
pub async fn extract_text(
    client: &reqwest::Client,
    image_base64: &str,
    config: &AppConfig,
    prompts_dir: &Path,
    template_id: &str,
) -> Result<String, String> {
    let model_id = config.extract.model_id.as_deref().unwrap_or(&config.transcribe.model_id);
    let resolved = models::resolve_model(config, model_id)?;
    let thinking = extract_thinking(config);
    let system_prompt = prompt::build_extract_prompt(config, prompts_dir, template_id);

    let outcome: Result<(String, TokenUsage), String> = if is_deepseek(&resolved) {
        deepseek::extract_text(
            client,
            image_base64,
            &system_prompt,
            &resolved.api_key,
            &resolved.model,
            &resolved.base_url,
            thinking,
            config.voice_templates.deepseek_reasoning_effort.as_deref(),
        )
        .await
    } else {
        match resolved.protocol.as_str() {
            "gemini" => {
                gemini::extract_text(
                    client,
                    image_base64,
                    &system_prompt,
                    &resolved.api_key,
                    &resolved.model,
                    &resolved.base_url,
                    thinking,
                )
                .await
            }
            "qwen-omni" => {
                openai_compat::qwen_omni_extract_text(
                    client,
                    image_base64,
                    &system_prompt,
                    &resolved.api_key,
                    &resolved.model,
                    &resolved.base_url,
                )
                .await
            }
            "mimo" => {
                mimo::extract_text(
                    client,
                    image_base64,
                    &system_prompt,
                    &resolved.api_key,
                    &resolved.model,
                    &resolved.base_url,
                )
                .await
            }
            PROTOCOL_BEDROCK => {
                bedrock::extract_text(
                    &config.models.aws,
                    image_base64,
                    &system_prompt,
                    &resolved.model,
                    thinking,
                )
                .await
            }
            PROTOCOL_AWS_TRANSCRIBE => {
                Err("Amazon Transcribe 只能做语音转写，不能识别图像".to_string())
            }
            _ => {
                openai_compat::extract_text(
                    client,
                    image_base64,
                    &system_prompt,
                    &resolved.api_key,
                    &resolved.model,
                    &resolved.base_url,
                    resolved.chat_template_kwargs.as_ref(),
                    Some(thinking),
                )
                .await
            }
        }
    };

    if let Ok((_, usage)) = &outcome {
        record_usage("extract", &resolved, *usage);
    }
    outcome.map(|(text, _)| text)
}

/// 文本类调用的 provider 分发：优化、自动学习、会议总结都走这里。
async fn run_text(
    client: &reqwest::Client,
    text: &str,
    system_prompt: &str,
    config: &AppConfig,
    resolved: &models::ResolvedModel,
    thinking: &ThinkingConfig,
    deepseek_effort: Option<&str>,
) -> Result<(String, TokenUsage), String> {
    if is_deepseek(resolved) {
        return deepseek::optimize(
            client,
            text,
            system_prompt,
            &resolved.api_key,
            &resolved.model,
            &resolved.base_url,
            thinking,
            deepseek_effort,
        )
        .await;
    }
    match resolved.protocol.as_str() {
        "gemini" => {
            gemini::optimize(
                client,
                text,
                system_prompt,
                &resolved.api_key,
                &resolved.model,
                &resolved.base_url,
                thinking,
            )
            .await
        }
        "qwen-omni" => {
            openai_compat::qwen_omni_optimize(
                client,
                text,
                system_prompt,
                &resolved.api_key,
                &resolved.model,
                &resolved.base_url,
            )
            .await
        }
        "mimo" => {
            mimo::optimize(
                client,
                text,
                system_prompt,
                &resolved.api_key,
                &resolved.model,
                &resolved.base_url,
            )
            .await
        }
        PROTOCOL_BEDROCK => {
            bedrock::optimize(
                &config.models.aws,
                text,
                system_prompt,
                &resolved.model,
                thinking,
            )
            .await
        }
        PROTOCOL_AWS_TRANSCRIBE => {
            Err("Amazon Transcribe 只能做语音转写，不能处理文本".to_string())
        }
        _ => {
            openai_compat::optimize(
                client,
                text,
                system_prompt,
                &resolved.api_key,
                &resolved.model,
                &resolved.base_url,
                resolved.chat_template_kwargs.as_ref(),
                Some(thinking),
            )
            .await
        }
    }
}

/// Optimize text using the configured provider.
pub async fn optimize(
    client: &reqwest::Client,
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
    let outcome = run_text(
        client,
        text,
        &system_prompt,
        config,
        &resolved,
        &config.voice_templates.thinking,
        config.voice_templates.deepseek_reasoning_effort.as_deref(),
    )
    .await;

    match outcome {
        Ok((text, usage)) => {
            record_usage("optimize", &resolved, usage);
            Ok(ai_output(text, &resolved))
        }
        Err(e) => Err(e),
    }
}

/// 用指定文本模型跑一段系统提示词 + 输入，返回纯文本。自动学习、会议总结共用。
#[allow(clippy::too_many_arguments)]
pub async fn complete_text(
    client: &reqwest::Client,
    input: &str,
    system_prompt: &str,
    config: &AppConfig,
    model_id: &str,
    thinking: &ThinkingConfig,
    deepseek_effort: Option<&str>,
    scene: &str,
) -> Result<String, String> {
    let resolved = models::resolve_model(config, model_id)?;
    let outcome = run_text(
        client,
        input,
        system_prompt,
        config,
        &resolved,
        thinking,
        deepseek_effort,
    )
    .await;
    if let Ok((_, usage)) = &outcome {
        record_usage(scene, &resolved, *usage);
    }
    outcome.map(|(text, _)| text)
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
        config.voice_learning.deepseek_reasoning_effort.as_deref(),
        "learn",
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::extract_thinking;
    use crate::config::types::{AppConfig, ThinkingConfig};

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
}
