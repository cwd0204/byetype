//! 模型目录：只有 AWS 两个协议。
//! - `bedrock`：Claude（文本 + 图像），经 global.* 跨区推理配置调用
//! - `aws-transcribe`：Amazon Transcribe 流式语音转写（只做音频）
//!
//! 与 src/core/models.ts 的 BUILTIN_MODELS 是两份手动镜像，改一处必须同步另一处。

use crate::config::types::AppConfig;

pub struct BuiltinModel {
    pub id: &'static str,
    pub provider: &'static str,
    pub model: &'static str,
    pub protocol: &'static str,
    pub supports_audio: bool,
    pub supports_text: bool,
    /// 与前端镜像保持同构；前端据此过滤图像识别的模型下拉框
    #[allow(dead_code)]
    pub supports_vision: bool,
}

pub const PROTOCOL_BEDROCK: &str = "bedrock";
pub const PROTOCOL_AWS_TRANSCRIBE: &str = "aws-transcribe";

/// 新装与迁移时的默认模型。
pub const DEFAULT_TRANSCRIBE_MODEL: &str = "builtin-aws-transcribe";
pub const DEFAULT_TEXT_MODEL: &str = "builtin-bedrock-claude-sonnet-5";

pub static BUILTIN_MODELS: &[BuiltinModel] = &[
    BuiltinModel {
        id: "builtin-bedrock-claude-sonnet-5",
        provider: "Amazon Bedrock",
        model: "global.anthropic.claude-sonnet-5",
        protocol: PROTOCOL_BEDROCK,
        supports_audio: false,
        supports_text: true,
        supports_vision: true,
    },
    BuiltinModel {
        id: "builtin-bedrock-claude-opus-5",
        provider: "Amazon Bedrock",
        model: "global.anthropic.claude-opus-5",
        protocol: PROTOCOL_BEDROCK,
        supports_audio: false,
        supports_text: true,
        supports_vision: true,
    },
    BuiltinModel {
        id: "builtin-bedrock-claude-haiku-4-5",
        provider: "Amazon Bedrock",
        model: "global.anthropic.claude-haiku-4-5-20251001-v1:0",
        protocol: PROTOCOL_BEDROCK,
        supports_audio: false,
        supports_text: true,
        supports_vision: true,
    },
    BuiltinModel {
        id: "builtin-aws-transcribe",
        provider: "Amazon Transcribe",
        model: "streaming",
        protocol: PROTOCOL_AWS_TRANSCRIBE,
        supports_audio: true,
        supports_text: false,
        supports_vision: false,
    },
    // 高准确度转写：Bedrock 上唯一中文可用的语音转文字模型。协议是 bedrock 但只吃音频，
    // 是「bedrock + audio」这个组合的唯一实例，所以下游不能再用协议等价于能力。
    // 不支持流式输入，录完整段再转，延迟随音频长度增长。
    BuiltinModel {
        id: "builtin-bedrock-voxtral",
        provider: "Amazon Bedrock",
        model: "mistral.voxtral-small-24b-2507",
        protocol: PROTOCOL_BEDROCK,
        supports_audio: true,
        supports_text: false,
        supports_vision: false,
    },
];

#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub protocol: String,
    /// 发给服务商的模型 id（Bedrock 为模型 / 推理配置 id）
    pub model: String,
    /// 服务商显示名（内置模型取预设名，自定义模型取用户填写名），用于用量统计展示。
    pub provider_label: String,
    /// 能否吃音频。协议不足以判断：Bedrock 下既有文本模型也有 Voxtral 这种只吃音频的。
    pub supports_audio: bool,
}

pub fn resolve_model(config: &AppConfig, model_id: &str) -> Result<ResolvedModel, String> {
    if let Some(builtin) = BUILTIN_MODELS.iter().find(|m| m.id == model_id) {
        return Ok(ResolvedModel {
            protocol: builtin.protocol.to_string(),
            model: builtin.model.to_string(),
            provider_label: builtin.provider.to_string(),
            supports_audio: builtin.supports_audio,
        });
    }

    if let Some(custom) = config.models.custom.iter().find(|m| m.id == model_id) {
        if custom.protocol != PROTOCOL_BEDROCK {
            return Err(format!(
                "自定义模型 {} 使用了不支持的协议 {}，只支持 Bedrock",
                custom.model, custom.protocol
            ));
        }
        if custom.model.trim().is_empty() {
            return Err(format!("自定义模型 {} 没有填写 Bedrock 模型 id", model_id));
        }
        return Ok(ResolvedModel {
            protocol: PROTOCOL_BEDROCK.to_string(),
            model: custom.model.trim().to_string(),
            provider_label: if custom.provider.trim().is_empty() {
                "Amazon Bedrock".to_string()
            } else {
                custom.provider.trim().to_string()
            },
            // 自定义模型只能填文本 / 图像能力，UI 里没有音频开关
            supports_audio: false,
        });
    }

    Err(format!("Model not found: {}", model_id))
}

pub fn supports_text(config: &AppConfig, model_id: &str) -> Result<bool, String> {
    if let Some(builtin) = BUILTIN_MODELS.iter().find(|model| model.id == model_id) {
        return Ok(builtin.supports_text);
    }
    if let Some(custom) = config.models.custom.iter().find(|model| model.id == model_id) {
        return Ok(custom.supports_text);
    }
    Err(format!("Model not found: {}", model_id))
}

pub fn supports_audio(config: &AppConfig, model_id: &str) -> Result<bool, String> {
    if let Some(builtin) = BUILTIN_MODELS.iter().find(|model| model.id == model_id) {
        return Ok(builtin.supports_audio);
    }
    // 自定义模型只能是 Bedrock，Bedrock 不收音频
    if config.models.custom.iter().any(|model| model.id == model_id) {
        return Ok(false);
    }
    Err(format!("Model not found: {}", model_id))
}

/// 某个模型 id 对应的协议；模型不存在时返回 None。
#[allow(dead_code)]
pub fn protocol_of(config: &AppConfig, model_id: &str) -> Option<String> {
    resolve_model(config, model_id).ok().map(|m| m.protocol)
}

/// 空或全空白的覆盖值 = 跟随「转写设置」里的全局转写模型。
/// 语义与 `meeting.transcribe_model_id` 一致。
pub fn effective_transcribe_model(config: &AppConfig, override_id: &str) -> String {
    let trimmed = override_id.trim();
    if trimmed.is_empty() {
        config.transcribe.model_id.clone()
    } else {
        trimmed.to_string()
    }
}

/// 转写阶段吃不到 rules / vocabulary / voice-learning，需要在文本优化阶段补校正。
///
/// 两个引擎都需要：Amazon Transcribe 根本不接受提示词；Voxtral 虽然能接受提示词，
/// 但实测把这几份文档喂进去会让它从转写机变成问答助手（见 bedrock.rs 的 TRANSCRIBE_INSTRUCTION
/// 注释），所以只给它一句短英文指令，校正仍然留给优化阶段。
pub fn transcribe_needs_post_correction(config: &AppConfig, model_id: &str) -> bool {
    resolve_model(config, model_id)
        .map(|resolved| resolved.supports_audio)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::CustomModelEntry;

    fn custom(id: &str, protocol: &str, model: &str) -> CustomModelEntry {
        CustomModelEntry {
            id: id.to_string(),
            provider: "My Bedrock".to_string(),
            model: model.to_string(),
            protocol: protocol.to_string(),
            supports_text: true,
            supports_vision: false,
        }
    }

    #[test]
    fn builtin_ids_are_unique_and_aws_only() {
        let mut ids: Vec<&str> = BUILTIN_MODELS.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), BUILTIN_MODELS.len());
        assert!(BUILTIN_MODELS
            .iter()
            .all(|m| m.protocol == PROTOCOL_BEDROCK || m.protocol == PROTOCOL_AWS_TRANSCRIBE));
        assert!(BUILTIN_MODELS.iter().any(|m| m.id == DEFAULT_TRANSCRIBE_MODEL));
        assert!(BUILTIN_MODELS.iter().any(|m| m.id == DEFAULT_TEXT_MODEL));
    }

    #[test]
    fn voxtral_is_a_bedrock_model_that_only_takes_audio() {
        let config = AppConfig::default();
        let voxtral = resolve_model(&config, "builtin-bedrock-voxtral").unwrap();
        // 协议是 bedrock，所以不能再用协议判断能力
        assert_eq!(voxtral.protocol, PROTOCOL_BEDROCK);
        assert_eq!(voxtral.model, "mistral.voxtral-small-24b-2507");
        assert!(voxtral.supports_audio);
        assert!(supports_audio(&config, "builtin-bedrock-voxtral").unwrap());
        // 不能被选成文本模型
        assert!(!supports_text(&config, "builtin-bedrock-voxtral").unwrap());
    }

    #[test]
    fn text_models_are_not_audio_models() {
        let config = AppConfig::default();
        let text = resolve_model(&config, DEFAULT_TEXT_MODEL).unwrap();
        assert!(!text.supports_audio);
    }

    #[test]
    fn effective_transcribe_model_falls_back_to_global() {
        let mut config = AppConfig::default();
        config.transcribe.model_id = DEFAULT_TRANSCRIBE_MODEL.to_string();
        assert_eq!(
            effective_transcribe_model(&config, ""),
            DEFAULT_TRANSCRIBE_MODEL
        );
        assert_eq!(
            effective_transcribe_model(&config, "   "),
            DEFAULT_TRANSCRIBE_MODEL
        );
        assert_eq!(
            effective_transcribe_model(&config, "builtin-bedrock-voxtral"),
            "builtin-bedrock-voxtral"
        );
        // 前后空白要修掉，否则 resolve_model 查不到
        assert_eq!(
            effective_transcribe_model(&config, " builtin-bedrock-voxtral "),
            "builtin-bedrock-voxtral"
        );
    }

    #[test]
    fn both_engines_need_post_correction() {
        let config = AppConfig::default();
        // Transcribe 根本不吃提示词；Voxtral 吃不下那几份文档（实测会跑偏），
        // 所以两条路都要靠优化阶段补词汇表与规则
        assert!(transcribe_needs_post_correction(
            &config,
            DEFAULT_TRANSCRIBE_MODEL
        ));
        assert!(transcribe_needs_post_correction(
            &config,
            "builtin-bedrock-voxtral"
        ));
        // 文本模型不是转写引擎，不触发
        assert!(!transcribe_needs_post_correction(&config, DEFAULT_TEXT_MODEL));
        assert!(!transcribe_needs_post_correction(&config, "不存在的模型"));
    }

    #[test]
    fn defaults_resolve_to_expected_protocols() {
        let config = AppConfig::default();
        let text = resolve_model(&config, DEFAULT_TEXT_MODEL).unwrap();
        assert_eq!(text.protocol, PROTOCOL_BEDROCK);
        assert_eq!(text.model, "global.anthropic.claude-sonnet-5");

        let audio = resolve_model(&config, DEFAULT_TRANSCRIBE_MODEL).unwrap();
        assert_eq!(audio.protocol, PROTOCOL_AWS_TRANSCRIBE);
        assert!(supports_audio(&config, DEFAULT_TRANSCRIBE_MODEL).unwrap());
        assert!(!supports_text(&config, DEFAULT_TRANSCRIBE_MODEL).unwrap());
        assert!(transcribe_needs_post_correction(&config, DEFAULT_TRANSCRIBE_MODEL));
    }

    #[test]
    fn custom_bedrock_model_resolves() {
        let mut config = AppConfig::default();
        config.models.custom.push(custom("nova", "bedrock", " global.amazon.nova-2-lite-v1:0 "));

        let resolved = resolve_model(&config, "nova").expect("bedrock custom model");
        assert_eq!(resolved.protocol, PROTOCOL_BEDROCK);
        assert_eq!(resolved.model, "global.amazon.nova-2-lite-v1:0");
        assert_eq!(resolved.provider_label, "My Bedrock");
        assert!(!supports_audio(&config, "nova").unwrap());
        assert!(supports_text(&config, "nova").unwrap());
    }

    #[test]
    fn custom_non_bedrock_model_is_rejected() {
        let mut config = AppConfig::default();
        config.models.custom.push(custom("legacy", "openai-compat", "gpt-x"));
        config.models.custom.push(custom("empty", "bedrock", "  "));

        assert!(resolve_model(&config, "legacy").unwrap_err().contains("只支持 Bedrock"));
        assert!(resolve_model(&config, "empty").unwrap_err().contains("没有填写"));
        assert!(resolve_model(&config, "missing").is_err());
    }
}
