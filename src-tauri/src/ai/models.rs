use crate::config::types::{AppConfig, AudioInputMode};

pub struct BuiltinModel {
    pub id: &'static str,
    pub provider: &'static str,
    pub model: &'static str,
    pub protocol: &'static str,
    pub base_url: &'static str,
    pub supports_audio: bool,
    pub supports_text: bool,
    /// 与 src/core/models.ts 的镜像保持同构；前端据此过滤图像识别的模型下拉框
    #[allow(dead_code)]
    pub supports_vision: bool,
}

/// AWS 协议不用 API Key，凭证来自本机 AWS profile（见 ai/aws.rs）。
pub const PROTOCOL_BEDROCK: &str = "bedrock";
pub const PROTOCOL_AWS_TRANSCRIBE: &str = "aws-transcribe";

pub fn is_aws_protocol(protocol: &str) -> bool {
    protocol == PROTOCOL_BEDROCK || protocol == PROTOCOL_AWS_TRANSCRIBE
}

pub static BUILTIN_MODELS: &[BuiltinModel] = &[
    BuiltinModel {
        id: "builtin-qwen-omni-plus",
        provider: "阿里云百炼",
        model: "qwen3.5-omni-plus",
        protocol: "qwen-omni",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        supports_audio: true,
        supports_text: true,
        supports_vision: true,
    },
    BuiltinModel {
        id: "builtin-qwen-omni-flash",
        provider: "阿里云百炼",
        model: "qwen3.5-omni-flash",
        protocol: "qwen-omni",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        supports_audio: true,
        supports_text: true,
        supports_vision: true,
    },
    BuiltinModel {
        id: "builtin-gemini-3.8-flash",
        provider: "Google Gemini",
        model: "gemini-3.8-flash",
        protocol: "gemini",
        base_url: "https://generativelanguage.googleapis.com",
        supports_audio: true,
        supports_text: true,
        supports_vision: true,
    },
    BuiltinModel {
        id: "builtin-mimo-v2.5",
        provider: "XiaoMi",
        model: "mimo-v2.5",
        protocol: "mimo",
        base_url: "https://api.xiaomimimo.com/v1",
        supports_audio: true,
        supports_text: true,
        supports_vision: true,
    },
    BuiltinModel {
        id: "builtin-or-gemini-3.8-flash",
        provider: "OpenRouter",
        model: "google/gemini-3.8-flash",
        protocol: "openai-compat",
        base_url: "https://openrouter.ai/api/v1",
        supports_audio: true,
        supports_text: true,
        supports_vision: true,
    },
    BuiltinModel {
        id: "builtin-or-gemini-3.5-flash-lite",
        provider: "OpenRouter",
        model: "google/gemini-3.5-flash-lite",
        protocol: "openai-compat",
        base_url: "https://openrouter.ai/api/v1",
        supports_audio: true,
        supports_text: true,
        supports_vision: true,
    },
    BuiltinModel {
        id: "builtin-deepseek-flash",
        provider: "DeepSeek",
        model: "deepseek-flash",
        protocol: "openai-compat",
        base_url: "https://api.deepseek.com",
        supports_audio: false,
        supports_text: true,
        supports_vision: true,
    },
    // Amazon Bedrock 上的 Claude：经 global.* 跨区推理配置调用，文本 + 图像，不收音频。
    BuiltinModel {
        id: "builtin-bedrock-claude-sonnet-5",
        provider: "Amazon Bedrock",
        model: "global.anthropic.claude-sonnet-5",
        protocol: PROTOCOL_BEDROCK,
        base_url: "",
        supports_audio: false,
        supports_text: true,
        supports_vision: true,
    },
    BuiltinModel {
        id: "builtin-bedrock-claude-opus-5",
        provider: "Amazon Bedrock",
        model: "global.anthropic.claude-opus-5",
        protocol: PROTOCOL_BEDROCK,
        base_url: "",
        supports_audio: false,
        supports_text: true,
        supports_vision: true,
    },
    BuiltinModel {
        id: "builtin-bedrock-claude-haiku-4-5",
        provider: "Amazon Bedrock",
        model: "global.anthropic.claude-haiku-4-5-20251001-v1:0",
        protocol: PROTOCOL_BEDROCK,
        base_url: "",
        supports_audio: false,
        supports_text: true,
        supports_vision: true,
    },
    // Amazon Transcribe 流式转写：只做语音，没有提示词能力，专有词纠错交给文本优化阶段。
    BuiltinModel {
        id: "builtin-aws-transcribe",
        provider: "Amazon Transcribe",
        model: "streaming",
        protocol: PROTOCOL_AWS_TRANSCRIBE,
        base_url: "",
        supports_audio: true,
        supports_text: false,
        supports_vision: false,
    },
];

pub struct ResolvedModel {
    pub protocol: String,
    pub base_url: String,
    pub model: String,
    /// 服务商显示名（内置模型取预设名，自定义模型取用户填写名），用于用量统计展示。
    pub provider_label: String,
    pub api_key: String,
    pub audio_input_mode: AudioInputMode,
    pub chat_template_kwargs: Option<serde_json::Value>,
}

pub fn resolve_model(config: &AppConfig, model_id: &str) -> Result<ResolvedModel, String> {
    if let Some(builtin) = BUILTIN_MODELS.iter().find(|m| m.id == model_id) {
        static NO_KEY: String = String::new();
        let api_key = if model_id.starts_with("builtin-or-") {
            &config.models.builtin_api_keys.openrouter
        } else {
            match builtin.protocol {
                "gemini" => &config.models.builtin_api_keys.gemini,
                "openai-compat" => &config.models.builtin_api_keys.deepseek,
                "qwen-omni" => &config.models.builtin_api_keys.dashscope,
                "mimo" => &config.models.builtin_api_keys.mimo,
                // AWS 走 profile 凭证链，不需要 API Key
                PROTOCOL_BEDROCK | PROTOCOL_AWS_TRANSCRIBE => &NO_KEY,
                _ => return Err(format!("Unknown protocol for builtin model: {}", model_id)),
            }
        };
        return Ok(ResolvedModel {
            protocol: builtin.protocol.to_string(),
            base_url: builtin.base_url.to_string(),
            model: builtin.model.to_string(),
            provider_label: builtin.provider.to_string(),
            api_key: api_key.clone(),
            audio_input_mode: AudioInputMode::InputAudio,
            chat_template_kwargs: None,
        });
    }

    if let Some(custom) = config.models.custom.iter().find(|m| m.id == model_id) {
        return Ok(ResolvedModel {
            protocol: custom.protocol.clone(),
            base_url: custom.base_url.clone(),
            model: custom.model.clone(),
            provider_label: custom.provider.clone(),
            api_key: custom.api_key.clone(),
            audio_input_mode: custom.audio_input_mode,
            chat_template_kwargs: Some(custom.chat_template_kwargs.clone()),
        });
    }

    Err(format!("Model not found: {}", model_id))
}

pub fn supports_text(config: &AppConfig, model_id: &str) -> Result<bool, String> {
    if let Some(builtin) = BUILTIN_MODELS.iter().find(|model| model.id == model_id) {
        return Ok(builtin.supports_text);
    }
    if let Some(custom) = config
        .models
        .custom
        .iter()
        .find(|model| model.id == model_id)
    {
        return Ok(custom.supports_text);
    }
    Err(format!("Model not found: {}", model_id))
}

pub fn supports_audio(config: &AppConfig, model_id: &str) -> Result<bool, String> {
    if let Some(builtin) = BUILTIN_MODELS.iter().find(|model| model.id == model_id) {
        return Ok(builtin.supports_audio);
    }
    if let Some(custom) = config.models.custom.iter().find(|model| model.id == model_id) {
        return Ok(custom.supports_audio);
    }
    Err(format!("Model not found: {}", model_id))
}

/// 某个模型 id 对应的协议；模型不存在时返回 None。
pub fn protocol_of(config: &AppConfig, model_id: &str) -> Option<String> {
    resolve_model(config, model_id).ok().map(|m| m.protocol)
}

/// 当前转写模型是否是 Amazon Transcribe：它吃不到提示词，
/// 专有词 / 规则 / 学习结果只能在文本优化阶段补上。
pub fn transcribe_needs_post_correction(config: &AppConfig) -> bool {
    protocol_of(config, &config.transcribe.model_id).as_deref() == Some(PROTOCOL_AWS_TRANSCRIBE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::CustomModelEntry;

    #[test]
    fn offers_single_deepseek_model() {
        let deepseek: Vec<&BuiltinModel> = BUILTIN_MODELS
            .iter()
            .filter(|model| model.provider == "DeepSeek")
            .collect();

        assert_eq!(deepseek.len(), 1);
        assert_eq!(deepseek[0].id, "builtin-deepseek-flash");
        assert_eq!(deepseek[0].model, "deepseek-flash");
        assert!(deepseek[0].supports_text);
    }

    #[test]
    fn reports_custom_audio_only_model_as_not_text_capable() {
        let mut config = AppConfig::default();
        config.models.custom.push(CustomModelEntry {
            id: "audio-only".to_string(),
            provider: "custom".to_string(),
            model: "audio-only".to_string(),
            protocol: "openai-compat".to_string(),
            base_url: "https://example.com".to_string(),
            api_key: "test".to_string(),
            audio_input_mode: AudioInputMode::InputAudio,
            chat_template_kwargs: serde_json::json!({}),
            supports_audio: true,
            supports_text: false,
            supports_vision: false,
        });

        assert!(!supports_text(&config, "audio-only").expect("model should resolve"));
    }

    #[test]
    fn aws_builtin_models_resolve_without_api_keys() {
        let config = AppConfig::default();

        let claude = resolve_model(&config, "builtin-bedrock-claude-sonnet-5").expect("bedrock model");
        assert_eq!(claude.protocol, PROTOCOL_BEDROCK);
        assert_eq!(claude.model, "global.anthropic.claude-sonnet-5");
        assert!(claude.api_key.is_empty());
        assert!(is_aws_protocol(&claude.protocol));

        let transcribe = resolve_model(&config, "builtin-aws-transcribe").expect("transcribe model");
        assert_eq!(transcribe.protocol, PROTOCOL_AWS_TRANSCRIBE);
        assert!(transcribe.api_key.is_empty());
        assert!(!supports_text(&config, "builtin-aws-transcribe").unwrap());
    }

    #[test]
    fn bedrock_models_are_text_and_vision_only() {
        for model in BUILTIN_MODELS.iter().filter(|m| m.protocol == PROTOCOL_BEDROCK) {
            assert!(!model.supports_audio, "{} should not accept audio", model.id);
            assert!(model.supports_text && model.supports_vision, "{}", model.id);
        }
    }

    #[test]
    fn transcribe_model_requires_post_correction() {
        let mut config = AppConfig::default();
        assert!(!transcribe_needs_post_correction(&config));
        config.transcribe.model_id = "builtin-aws-transcribe".to_string();
        assert!(transcribe_needs_post_correction(&config));
    }
}
