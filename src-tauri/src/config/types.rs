use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub general: GeneralConfig,
    #[serde(default)]
    pub local_api: LocalApiConfig,
    pub models: ModelsConfig,
    pub transcribe: TranscribeConfig,
    #[serde(default)]
    pub voice_learning: VoiceLearningConfig,
    #[serde(alias = "optimize")]
    pub voice_templates: VoiceTemplatesConfig,
    #[serde(default)]
    pub extract: ExtractConfig,
    pub advanced: AdvancedConfig,
    #[serde(default)]
    pub backup: BackupConfig,
    #[serde(default)]
    pub meeting: MeetingConfig,
}

/// 会议记录：检测 Zoom 会议 → 采集麦克风 + 系统音频 → 分段转写 → 会议纪要。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingConfig {
    /// 总开关；关闭时不探测、托盘手动开始仍可用
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub auto_detect: bool,
    #[serde(default = "default_true")]
    pub capture_system_audio: bool,
    #[serde(default = "default_true")]
    pub capture_microphone: bool,
    /// 空 = 跟随「转写设置」里的转写模型
    #[serde(default)]
    pub transcribe_model_id: String,
    #[serde(default = "default_meeting_summary_model")]
    pub summary_model_id: String,
    #[serde(default)]
    pub summary_thinking: ThinkingConfig,
    /// 每段音频目标时长（秒），到点后在静音处切
    #[serde(default = "default_chunk_seconds")]
    pub chunk_seconds: u32,
    #[serde(default = "default_chunk_timeout")]
    pub chunk_timeout_secs: u32,
    #[serde(default = "default_summary_timeout")]
    pub summary_timeout_secs: u32,
    #[serde(default = "default_max_meeting_minutes")]
    pub max_meeting_minutes: u32,
    #[serde(default = "default_detect_poll")]
    pub detect_poll_secs: u32,
    /// 纪要导出目录；空 = <app_data>/meetings-notes
    #[serde(default)]
    pub notes_folder: String,
    #[serde(default)]
    pub keep_audio: bool,
    #[serde(default = "default_true")]
    pub show_window_on_start: bool,
    #[serde(default = "default_true")]
    pub open_summary_when_done: bool,
    #[serde(default)]
    pub prompts: MeetingPromptsConfig,
}

/// 自定义提示词路径；空 = 内置模板
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingPromptsConfig {
    #[serde(default)]
    pub transcribe: String,
    #[serde(default)]
    pub summary: String,
}

fn default_meeting_summary_model() -> String {
    "builtin-bedrock-claude-sonnet-5".to_string()
}

fn default_chunk_seconds() -> u32 {
    240
}

fn default_chunk_timeout() -> u32 {
    120
}

fn default_summary_timeout() -> u32 {
    180
}

fn default_max_meeting_minutes() -> u32 {
    240
}

fn default_detect_poll() -> u32 {
    3
}

impl Default for MeetingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_detect: true,
            capture_system_audio: true,
            capture_microphone: true,
            transcribe_model_id: String::new(),
            summary_model_id: default_meeting_summary_model(),
            summary_thinking: ThinkingConfig::default(),
            chunk_seconds: default_chunk_seconds(),
            chunk_timeout_secs: default_chunk_timeout(),
            summary_timeout_secs: default_summary_timeout(),
            max_meeting_minutes: default_max_meeting_minutes(),
            detect_poll_secs: default_detect_poll(),
            notes_folder: String::new(),
            keep_audio: false,
            show_window_on_start: true,
            open_summary_when_done: true,
            prompts: MeetingPromptsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalApiConfig {
    pub enabled: bool,
    pub port: u16,
}

impl Default for LocalApiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 8765,
        }
    }
}

impl AppConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.local_api.port < 1024 {
            return Err("本机接口端口必须在 1024 到 65535 之间".to_string());
        }
        let meeting = &self.meeting;
        if !(60..=600).contains(&meeting.chunk_seconds) {
            return Err("会议分段时长必须在 60 到 600 秒之间".to_string());
        }
        if !(1..=30).contains(&meeting.detect_poll_secs) {
            return Err("Zoom 探测间隔必须在 1 到 30 秒之间".to_string());
        }
        if !(10..=600).contains(&meeting.max_meeting_minutes) {
            return Err("会议最长时长必须在 10 到 600 分钟之间".to_string());
        }
        if meeting.chunk_timeout_secs < 30 || meeting.summary_timeout_secs < 30 {
            return Err("会议转写 / 纪要超时不能少于 30 秒".to_string());
        }
        Ok(())
    }
}

fn default_true() -> bool {
    true
}

fn default_language() -> String {
    "system".to_string()
}

fn default_max_recording_seconds() -> u32 {
    180
}

fn default_microphone() -> String {
    "system-default".to_string()
}

fn default_extract_shortcut() -> String {
    "F6".to_string()
}

fn default_shortcut2() -> String {
    String::new()
}

fn default_extract_shortcut2() -> String {
    String::new()
}

fn default_shortcut_template() -> String {
    "voice-optimize".to_string()
}

fn default_extract_template() -> String {
    "image-extract".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneralConfig {
    pub shortcut: String,
    pub launch_at_login: bool,
    pub theme: String,
    /// 界面语言："system"（跟随系统）| "zh-CN" | "en"
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_max_recording_seconds")]
    pub max_recording_seconds: u32,
    #[serde(default = "default_microphone")]
    pub microphone: String,
    #[serde(default = "default_extract_shortcut")]
    pub extract_shortcut: String,
    #[serde(default = "default_shortcut2")]
    pub shortcut2: String,
    #[serde(default = "default_extract_shortcut2")]
    pub extract_shortcut2: String,
    #[serde(default = "default_shortcut_template")]
    pub shortcut_template: String,
    #[serde(default)]
    pub shortcut2_template: String,
    #[serde(default = "default_extract_template")]
    pub extract_shortcut_template: String,
    #[serde(default)]
    pub extract_shortcut2_template: String,
    #[serde(default)]
    pub shortcut_label: Option<String>,
    #[serde(default)]
    pub shortcut2_label: Option<String>,
    #[serde(default)]
    pub extract_shortcut_label: Option<String>,
    #[serde(default)]
    pub extract_shortcut2_label: Option<String>,
    #[serde(default)]
    pub ptt_mode: bool,
    #[serde(default = "default_true")]
    pub overwrite_clipboard: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsConfig {
    /// 用户自建的 Bedrock 模型（任意模型 / 推理配置 id）
    #[serde(default)]
    pub custom: Vec<CustomModelEntry>,
    #[serde(default)]
    pub aws: AwsConfig,
}

/// AWS 接入：只存 profile 名与 region，凭证由本机 ~/.aws/config 的凭证链提供
/// （ADA credential_process、静态 key、SSO 都行），app 不接触任何密钥。
/// Bedrock 与 Transcribe 分开配，因为同一个角色可能只授权其中一个服务。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwsConfig {
    #[serde(default = "default_aws_profile")]
    pub bedrock_profile: String,
    #[serde(default = "default_aws_region")]
    pub bedrock_region: String,
    #[serde(default = "default_aws_profile")]
    pub transcribe_profile: String,
    #[serde(default = "default_aws_region")]
    pub transcribe_region: String,
    /// "auto" = zh-CN + en-US 多语言识别（首选 zh-CN）；否则为单一语言码，如 "zh-CN" / "en-US"
    #[serde(default = "default_transcribe_language")]
    pub transcribe_language: String,
}

fn default_aws_profile() -> String {
    "default".to_string()
}

fn default_aws_region() -> String {
    "us-east-1".to_string()
}

fn default_transcribe_language() -> String {
    "auto".to_string()
}

impl Default for AwsConfig {
    fn default() -> Self {
        Self {
            bedrock_profile: default_aws_profile(),
            bedrock_region: default_aws_region(),
            transcribe_profile: default_aws_profile(),
            transcribe_region: default_aws_region(),
            transcribe_language: default_transcribe_language(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomModelEntry {
    pub id: String,
    /// 显示名（用量统计里的服务商列）
    pub provider: String,
    /// Bedrock 模型 id 或跨区推理配置 id，如 global.amazon.nova-2-lite-v1:0
    pub model: String,
    /// 只允许 "bedrock"；旧配置里其他协议的条目在迁移时被丢弃
    pub protocol: String,
    #[serde(default = "default_true")]
    pub supports_text: bool,
    #[serde(default = "default_true")]
    pub supports_vision: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    pub enabled: bool,
    pub level: String,
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self { enabled: false, level: "LOW".to_string() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptsConfig {
    pub agent: String,
    pub rules: String,
    pub vocabulary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscribeConfig {
    pub model_id: String,
    pub thinking: ThinkingConfig,
    pub prompts: PromptsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceLearningConfig {
    pub model_id: String,
    #[serde(default)]
    pub thinking: ThinkingConfig,
}

impl Default for VoiceLearningConfig {
    fn default() -> Self {
        Self {
            model_id: crate::ai::models::DEFAULT_TEXT_MODEL.to_string(),
            thinking: ThinkingConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTemplatesConfig {
    pub model_id: String,
    pub thinking: ThinkingConfig,
    #[serde(default = "default_voice_templates")]
    pub templates: Vec<TemplateEntry>,
    /// 优化阶段是否再带一遍转写参考(规则/专有词汇/自动学习结果)做二次纠错。
    /// 适合转写纠错较弱的模型;强模型转写阶段已纠对,关闭可省 token 并避免过度改写。
    #[serde(default)]
    pub reuse_transcribe_references: bool,
}

fn default_voice_templates() -> Vec<TemplateEntry> {
    vec![
        TemplateEntry { id: "voice-optimize".to_string(), name: "自动换行".to_string(), prompt: String::new() },
        TemplateEntry { id: "voice-translate".to_string(), name: "翻译".to_string(), prompt: String::new() },
        TemplateEntry { id: "voice-custom".to_string(), name: "自定义".to_string(), prompt: String::new() },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractConfig {
    pub model_id: Option<String>,
    pub thinking: Option<ThinkingConfig>,
    pub prompt: String,
    #[serde(default = "default_image_templates")]
    pub templates: Vec<TemplateEntry>,
}

fn default_image_templates() -> Vec<TemplateEntry> {
    vec![
        TemplateEntry { id: "image-extract".to_string(), name: "文字识别".to_string(), prompt: String::new() },
        TemplateEntry { id: "image-translate".to_string(), name: "翻译".to_string(), prompt: String::new() },
        TemplateEntry { id: "image-custom".to_string(), name: "自定义".to_string(), prompt: String::new() },
    ]
}

impl Default for ExtractConfig {
    fn default() -> Self {
        Self {
            model_id: None,
            thinking: None,
            prompt: String::new(),
            templates: default_image_templates(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedConfig {
    pub transcribe_timeout: u32,
    pub optimize_timeout: u32,
    pub max_retries: u32,
    pub max_parallel: u32,
    #[serde(default = "default_true")]
    pub proxy_enabled: bool,
    pub proxy_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct S3Config {
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub access_key: String,
    #[serde(default)]
    pub secret_key: String,
    #[serde(default = "default_s3_prefix")]
    pub prefix: String,
}

fn default_s3_prefix() -> String {
    "byetype/backups".to_string()
}

impl Default for S3Config {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            region: String::new(),
            bucket: String::new(),
            access_key: String::new(),
            secret_key: String::new(),
            prefix: default_s3_prefix(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackupConfig {
    #[serde(default)]
    pub s3: S3Config,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig {
                shortcut: "F4".to_string(),
                launch_at_login: false,
                theme: "system".to_string(),
                language: default_language(),
                max_recording_seconds: 180,
                microphone: "system-default".to_string(),
                extract_shortcut: "F6".to_string(),
                shortcut2: String::new(),
                extract_shortcut2: String::new(),
                shortcut_template: "voice-optimize".to_string(),
                shortcut2_template: "voice-translate".to_string(),
                extract_shortcut_template: "image-extract".to_string(),
                extract_shortcut2_template: "image-translate".to_string(),
                shortcut_label: None,
                shortcut2_label: None,
                extract_shortcut_label: None,
                extract_shortcut2_label: None,
                ptt_mode: false,
                overwrite_clipboard: true,
            },
            local_api: LocalApiConfig::default(),
            models: ModelsConfig::default(),
            transcribe: TranscribeConfig {
                model_id: crate::ai::models::DEFAULT_TRANSCRIBE_MODEL.to_string(),
                thinking: ThinkingConfig {
                    enabled: false,
                    level: "LOW".to_string(),
                },
                prompts: PromptsConfig {
                    agent: String::new(),
                    rules: String::new(),
                    vocabulary: String::new(),
                },
            },
            voice_learning: VoiceLearningConfig::default(),
            voice_templates: VoiceTemplatesConfig {
                model_id: crate::ai::models::DEFAULT_TEXT_MODEL.to_string(),
                thinking: ThinkingConfig {
                    enabled: false,
                    level: "LOW".to_string(),
                },
                templates: default_voice_templates(),
                reuse_transcribe_references: false,
            },
            extract: ExtractConfig::default(),
            advanced: AdvancedConfig {
                // Transcribe 流式识别比多模态模型慢，Claude 开思考时也需要更长时间
                transcribe_timeout: 60,
                optimize_timeout: 30,
                max_retries: 3,
                max_parallel: 3,
                proxy_enabled: true,
                proxy_url: String::new(),
            },
            backup: BackupConfig::default(),
            meeting: MeetingConfig::default(),
        }
    }
}

#[cfg(test)]
mod meeting_tests {
    use super::*;

    #[test]
    fn existing_config_without_meeting_uses_defaults() {
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        value.as_object_mut().unwrap().remove("meeting");

        let config: AppConfig = serde_json::from_value(value).unwrap();

        assert!(!config.meeting.enabled);
        assert!(config.meeting.auto_detect);
        assert_eq!(config.meeting.chunk_seconds, 240);
        assert_eq!(config.meeting.summary_model_id, "builtin-bedrock-claude-sonnet-5");
        assert!(config.meeting.transcribe_model_id.is_empty());
    }

    #[test]
    fn meeting_validation_rejects_out_of_range_values() {
        let mut config = AppConfig::default();
        config.meeting.chunk_seconds = 10;
        assert!(config.validate().is_err());

        let mut config = AppConfig::default();
        config.meeting.detect_poll_secs = 0;
        assert!(config.validate().is_err());

        let config = AppConfig::default();
        assert!(config.validate().is_ok());
    }
}

#[cfg(test)]
mod local_api_tests {
    use super::*;

    #[test]
    fn local_api_is_opt_in_on_the_stable_default_port() {
        let config = AppConfig::default();

        assert!(!config.local_api.enabled);
        assert_eq!(config.local_api.port, 8765);
    }

    #[test]
    fn local_api_rejects_privileged_ports() {
        let mut config = AppConfig::default();
        config.local_api.enabled = true;
        config.local_api.port = 80;

        assert_eq!(
            config.validate(),
            Err("本机接口端口必须在 1024 到 65535 之间".to_string())
        );
    }

    #[test]
    fn existing_config_without_local_api_uses_safe_defaults() {
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        value.as_object_mut().unwrap().remove("localApi");

        let config: AppConfig = serde_json::from_value(value).unwrap();

        assert!(!config.local_api.enabled);
        assert_eq!(config.local_api.port, 8765);
    }

    #[test]
    fn existing_config_without_aws_uses_defaults() {
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        value["models"].as_object_mut().unwrap().remove("aws");

        let config: AppConfig = serde_json::from_value(value).unwrap();

        assert_eq!(config.models.aws.bedrock_profile, "default");
        assert_eq!(config.models.aws.bedrock_region, "us-east-1");
        assert_eq!(config.models.aws.transcribe_profile, "default");
        assert_eq!(config.models.aws.transcribe_language, "auto");
    }

    #[test]
    fn partial_aws_config_fills_missing_fields() {
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        value["models"]["aws"] = serde_json::json!({ "bedrockProfile": "bedrock", "bedrockRegion": "ap-northeast-1" });

        let config: AppConfig = serde_json::from_value(value).unwrap();

        assert_eq!(config.models.aws.bedrock_profile, "bedrock");
        assert_eq!(config.models.aws.bedrock_region, "ap-northeast-1");
        assert_eq!(config.models.aws.transcribe_region, "us-east-1");
    }

    #[test]
    fn existing_config_without_voice_learning_uses_default_model() {
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        value.as_object_mut().unwrap().remove("voiceLearning");

        let config: AppConfig = serde_json::from_value(value).unwrap();

        assert_eq!(config.voice_learning.model_id, crate::ai::models::DEFAULT_TEXT_MODEL);
        assert!(!config.voice_learning.thinking.enabled);
    }

    #[test]
    fn custom_model_ignores_legacy_provider_fields() {
        let value = serde_json::json!({
            "id": "legacy-model",
            "provider": "custom",
            "model": "global.amazon.nova-2-lite-v1:0",
            "protocol": "bedrock",
            "baseUrl": "https://example.com/v1",
            "apiKey": "test",
            "audioInputMode": "audio_url",
            "chatTemplateKwargs": {"enable_thinking": false},
            "supportsAudio": true
        });

        let model: CustomModelEntry = serde_json::from_value(value).unwrap();

        assert_eq!(model.protocol, "bedrock");
        assert!(model.supports_text);
        assert!(model.supports_vision);
    }

    #[test]
    fn defaults_are_aws_only_with_relaxed_timeouts() {
        let config = AppConfig::default();
        assert_eq!(config.transcribe.model_id, crate::ai::models::DEFAULT_TRANSCRIBE_MODEL);
        assert_eq!(config.voice_templates.model_id, crate::ai::models::DEFAULT_TEXT_MODEL);
        assert_eq!(config.advanced.transcribe_timeout, 60);
        assert_eq!(config.advanced.optimize_timeout, 30);
        assert_eq!(config.general.language, "system");
        assert!(config.validate().is_ok());
    }

}
