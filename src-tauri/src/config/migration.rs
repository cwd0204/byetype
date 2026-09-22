use serde_json::Value;

use crate::ai::models::{BUILTIN_MODELS, DEFAULT_TEXT_MODEL, DEFAULT_TRANSCRIBE_MODEL, PROTOCOL_BEDROCK};

/// Transcribe 比原来的多模态模型慢，旧配置里 10 秒的超时不够用。
const MIN_TRANSCRIBE_TIMEOUT: u64 = 60;
const MIN_OPTIMIZE_TIMEOUT: u64 = 30;

pub fn migrate_if_needed(raw: &mut Value) -> bool {
    let mut migrated = false;

    // 迁移1：最早的 transcribe.model / optimize.* 结构 → modelId 结构
    if raw
        .get("transcribe")
        .and_then(|t| t.get("model"))
        .and_then(|m| m.as_str())
        .is_some()
        && raw
            .get("transcribe")
            .and_then(|t| t.get("modelId"))
            .is_none()
    {
        migrate_model_field_to_model_id(raw);
        migrated = true;
    }

    // 迁移2：optimize → voiceTemplates
    if raw.get("optimize").is_some() && raw.get("voiceTemplates").is_none() {
        migrate_optimize_to_voice_templates(raw);
        migrated = true;
    }

    if raw.get("voiceLearning").is_none() {
        let model_id = raw
            .get("voiceTemplates")
            .and_then(|section| section.get("modelId"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_TEXT_MODEL)
            .to_string();
        raw["voiceLearning"] = serde_json::json!({
            "modelId": model_id,
            "thinking": { "enabled": false, "level": "LOW" }
        });
        migrated = true;
    } else if raw
        .get("voiceLearning")
        .and_then(|section| section.get("thinking"))
        .is_none()
    {
        raw["voiceLearning"]["thinking"] = serde_json::json!({ "enabled": false, "level": "LOW" });
        migrated = true;
    }

    // 迁移3：只保留 AWS，其他供应商的模型选择与密钥全部落到 AWS 默认
    if migrate_to_aws_only(raw) {
        migrated = true;
    }

    // 迁移4：抬高过小的会议分段转写超时
    if migrate_chunk_timeout(raw) {
        migrated = true;
    }

    migrated
}

/// 老配置里 `meeting.chunkTimeoutSecs` 默认 120 秒，而 240 秒的分段实际要 350 秒上下
/// （Transcribe 整段上传按约 1.06 倍实时消费），于是每一段都必然超时，长会议一个字都转不出来。
/// 这里按分段长度算出下限并抬上去。条件检测、幂等。
fn migrate_chunk_timeout(raw: &mut Value) -> bool {
    let Some(meeting) = raw.get_mut("meeting") else {
        return false;
    };
    let chunk_seconds = meeting
        .get("chunkSeconds")
        .and_then(|v| v.as_u64())
        .unwrap_or(240) as u32;
    let min = crate::config::types::min_chunk_timeout(chunk_seconds) as u64;
    match meeting.get("chunkTimeoutSecs").and_then(|v| v.as_u64()) {
        Some(current) if current < min => {
            meeting["chunkTimeoutSecs"] = Value::from(min);
            true
        }
        _ => false,
    }
}

/// 最早期结构：transcribe.model / geminiApiKey / optimize.openaiCompat。
/// 供应商已全部移除，只需要把结构转成 modelId 形态，具体值交给 migrate_to_aws_only 收口。
fn migrate_model_field_to_model_id(raw: &mut Value) {
    let thinking = raw
        .get("transcribe")
        .and_then(|t| t.get("thinking"))
        .cloned()
        .unwrap_or(serde_json::json!({ "enabled": false, "level": "LOW" }));
    let prompts = raw
        .get("transcribe")
        .and_then(|t| t.get("prompts"))
        .cloned()
        .unwrap_or(serde_json::json!({ "agent": "", "rules": "", "vocabulary": "" }));
    raw["transcribe"] = serde_json::json!({
        "modelId": DEFAULT_TRANSCRIBE_MODEL,
        "thinking": thinking,
        "prompts": prompts,
    });

    let opt_enabled = raw
        .get("optimize")
        .and_then(|o| o.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let opt_thinking = raw
        .get("optimize")
        .and_then(|o| o.get("thinking"))
        .cloned()
        .unwrap_or(serde_json::json!({ "enabled": false, "level": "LOW" }));
    let opt_prompt = raw
        .get("optimize")
        .and_then(|o| o.get("prompt"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    raw["optimize"] = serde_json::json!({
        "enabled": opt_enabled,
        "modelId": DEFAULT_TEXT_MODEL,
        "thinking": opt_thinking,
        "prompt": opt_prompt,
    });

    raw["models"] = serde_json::json!({ "custom": [] });
}

fn migrate_optimize_to_voice_templates(raw: &mut Value) {
    let opt = match raw.get("optimize") {
        Some(v) => v.clone(),
        None => return,
    };

    let model_id = opt
        .get("modelId")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let thinking = opt
        .get("thinking")
        .cloned()
        .unwrap_or(serde_json::json!({ "enabled": false, "level": "LOW" }));
    let custom_prompt = opt
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let enabled = opt.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);

    let templates = serde_json::json!([
        { "id": "voice-optimize", "name": "自动换行", "prompt": custom_prompt },
        { "id": "voice-translate", "name": "翻译", "prompt": "" },
        { "id": "voice-custom", "name": "自定义", "prompt": "" },
    ]);

    raw["voiceTemplates"] = serde_json::json!({
        "modelId": model_id,
        "thinking": thinking,
        "templates": templates,
    });

    if let Some(general) = raw.get_mut("general") {
        if enabled {
            general["shortcutTemplate"] = serde_json::json!("voice-optimize");
        } else {
            general["shortcutTemplate"] = serde_json::json!("");
        }
        general["shortcut2"] = serde_json::json!("");
        general["shortcut2Template"] = serde_json::json!("voice-translate");
        general["extractShortcut2"] = serde_json::json!("");
        general["extractShortcutTemplate"] = serde_json::json!("image-extract");
        general["extractShortcut2Template"] = serde_json::json!("image-translate");
    }

    if let Some(extract) = raw.get_mut("extract") {
        if extract.get("templates").is_none() {
            extract["templates"] = serde_json::json!([
                { "id": "image-extract", "name": "文字识别", "prompt": "" },
                { "id": "image-translate", "name": "翻译", "prompt": "" },
                { "id": "image-custom", "name": "自定义", "prompt": "" },
            ]);
        }
    }

    if let Some(obj) = raw.as_object_mut() {
        obj.remove("optimize");
    }
}

fn is_builtin(id: &str) -> bool {
    BUILTIN_MODELS.iter().any(|m| m.id == id)
}

/// 把 modelId 收口到 AWS 可用的 id：合法就原样保留，否则换成 fallback。返回是否改动。
fn normalize_model_id(
    section: &mut Value,
    key: &str,
    valid: impl Fn(&str) -> bool,
    fallback: &str,
    allow_empty: bool,
) -> bool {
    let Some(current) = section.get(key) else { return false };
    if current.is_null() {
        return false;
    }
    let current = current.as_str().unwrap_or_default().to_string();
    if current.is_empty() && allow_empty {
        return false;
    }
    if valid(&current) {
        return false;
    }
    section[key] = Value::String(fallback.to_string());
    true
}

/// 只保留 AWS：
/// - `models.builtinApiKeys` 删除；`models.custom` 只留 protocol=bedrock 的条目
/// - 转写模型必须是 Transcribe；文本 / 图像 / 学习 / 纪要模型必须是 Bedrock（内置或自定义）
/// - 超时抬高到 Transcribe / Claude 能接受的下限；MINIMAL 思考档位并入 LOW
pub(crate) fn migrate_to_aws_only(raw: &mut Value) -> bool {
    let mut changed = false;

    // 1. models
    let mut custom_bedrock_ids: Vec<String> = Vec::new();
    if let Some(models) = raw.get_mut("models").and_then(|m| m.as_object_mut()) {
        if models.remove("builtinApiKeys").is_some() {
            changed = true;
        }
        if let Some(custom) = models.get_mut("custom").and_then(|c| c.as_array_mut()) {
            let before = custom.len();
            custom.retain(|entry| {
                entry.get("protocol").and_then(|p| p.as_str()) == Some(PROTOCOL_BEDROCK)
            });
            if custom.len() != before {
                changed = true;
            }
            custom_bedrock_ids = custom
                .iter()
                .filter_map(|entry| entry.get("id").and_then(|id| id.as_str()))
                .map(|id| id.to_string())
                .collect();
        }
    }

    let is_bedrock_text = |id: &str| -> bool {
        (is_builtin(id) && id != DEFAULT_TRANSCRIBE_MODEL) || custom_bedrock_ids.iter().any(|c| c == id)
    };
    let is_transcribe = |id: &str| -> bool { id == DEFAULT_TRANSCRIBE_MODEL };

    // 2. 各处模型选择
    if let Some(section) = raw.get_mut("transcribe") {
        changed |= normalize_model_id(section, "modelId", is_transcribe, DEFAULT_TRANSCRIBE_MODEL, false);
    }
    for key in ["voiceTemplates", "voiceLearning"] {
        if let Some(section) = raw.get_mut(key) {
            changed |= normalize_model_id(section, "modelId", is_bedrock_text, DEFAULT_TEXT_MODEL, false);
        }
    }
    if let Some(section) = raw.get_mut("extract") {
        // extract.modelId 为空 / null 表示跟随文本优化模型，保留
        changed |= normalize_model_id(section, "modelId", is_bedrock_text, DEFAULT_TEXT_MODEL, true);
    }
    if let Some(section) = raw.get_mut("meeting") {
        // 空表示跟随转写设置
        changed |= normalize_model_id(section, "transcribeModelId", is_transcribe, "", true);
        changed |= normalize_model_id(section, "summaryModelId", is_bedrock_text, DEFAULT_TEXT_MODEL, false);
    }

    // 3. 超时
    if let Some(advanced) = raw.get_mut("advanced") {
        for (key, min) in [
            ("transcribeTimeout", MIN_TRANSCRIBE_TIMEOUT),
            ("optimizeTimeout", MIN_OPTIMIZE_TIMEOUT),
        ] {
            if let Some(current) = advanced.get(key).and_then(|v| v.as_u64()) {
                if current < min {
                    advanced[key] = Value::from(min);
                    changed = true;
                }
            }
        }
    }

    // 4. 思考档位：Bedrock 没有 MINIMAL
    let thinking_paths: [&[&str]; 4] = [
        &["transcribe", "thinking"],
        &["voiceTemplates", "thinking"],
        &["voiceLearning", "thinking"],
        &["meeting", "summaryThinking"],
    ];
    for path in thinking_paths {
        let mut node = Some(&mut *raw);
        for key in path {
            node = node.and_then(|n| n.get_mut(*key));
        }
        if let Some(thinking) = node {
            if thinking.get("level").and_then(|l| l.as_str()) == Some("MINIMAL") {
                thinking["level"] = Value::String("LOW".to_string());
                changed = true;
            }
        }
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_provider_selections_land_on_aws_defaults() {
        let mut raw = json!({
            "models": {
                "builtinApiKeys": { "gemini": "AIza", "deepseek": "", "dashscope": "", "openrouter": "", "mimo": "" },
                "custom": []
            },
            "transcribe": { "modelId": "builtin-gemini-3.8-flash" },
            "voiceTemplates": { "modelId": "" },
            "voiceLearning": { "modelId": "builtin-mimo-v2.5", "thinking": { "enabled": true, "level": "MINIMAL" } },
            "extract": { "modelId": "builtin-deepseek-flash" },
            "advanced": { "transcribeTimeout": 10, "optimizeTimeout": 10 },
        });

        assert!(migrate_if_needed(&mut raw));
        assert!(raw["models"].get("builtinApiKeys").is_none());
        assert_eq!(raw["transcribe"]["modelId"], DEFAULT_TRANSCRIBE_MODEL);
        assert_eq!(raw["voiceTemplates"]["modelId"], DEFAULT_TEXT_MODEL);
        assert_eq!(raw["voiceLearning"]["modelId"], DEFAULT_TEXT_MODEL);
        assert_eq!(raw["voiceLearning"]["thinking"]["level"], "LOW");
        assert_eq!(raw["extract"]["modelId"], DEFAULT_TEXT_MODEL);
        assert_eq!(raw["advanced"]["transcribeTimeout"], 60);
        assert_eq!(raw["advanced"]["optimizeTimeout"], 30);
    }

    #[test]
    fn custom_bedrock_models_survive_and_others_are_dropped() {
        let mut raw = json!({
            "models": {
                "custom": [
                    { "id": "nova", "provider": "Bedrock", "model": "global.amazon.nova-2-lite-v1:0", "protocol": "bedrock", "supportsText": true, "supportsVision": true },
                    { "id": "gpt", "provider": "OpenAI", "model": "gpt-4o", "protocol": "openai-compat", "baseUrl": "https://api.openai.com/v1", "apiKey": "sk", "supportsAudio": false, "supportsText": true, "supportsVision": true }
                ]
            },
            "transcribe": { "modelId": "builtin-aws-transcribe" },
            "voiceTemplates": { "modelId": "nova" },
            "voiceLearning": { "modelId": "gpt", "thinking": { "enabled": false, "level": "LOW" } },
        });

        assert!(migrate_if_needed(&mut raw));
        let custom = raw["models"]["custom"].as_array().unwrap();
        assert_eq!(custom.len(), 1);
        assert_eq!(custom[0]["id"], "nova");
        assert_eq!(raw["voiceTemplates"]["modelId"], "nova");
        assert_eq!(raw["voiceLearning"]["modelId"], DEFAULT_TEXT_MODEL);
    }

    #[test]
    fn transcribe_cannot_be_a_bedrock_model_and_meeting_follows_rules() {
        let mut raw = json!({
            "transcribe": { "modelId": "builtin-bedrock-claude-sonnet-5" },
            "voiceLearning": { "modelId": "builtin-bedrock-claude-haiku-4-5", "thinking": { "enabled": false, "level": "LOW" } },
            "meeting": { "transcribeModelId": "builtin-gemini-3.8-flash", "summaryModelId": "builtin-qwen-omni-plus", "summaryThinking": { "enabled": true, "level": "MINIMAL" } },
        });

        assert!(migrate_if_needed(&mut raw));
        assert_eq!(raw["transcribe"]["modelId"], DEFAULT_TRANSCRIBE_MODEL);
        assert_eq!(raw["voiceLearning"]["modelId"], "builtin-bedrock-claude-haiku-4-5");
        assert_eq!(raw["meeting"]["transcribeModelId"], "");
        assert_eq!(raw["meeting"]["summaryModelId"], DEFAULT_TEXT_MODEL);
        assert_eq!(raw["meeting"]["summaryThinking"]["level"], "LOW");
    }

    #[test]
    fn raises_too_small_chunk_timeout() {
        // 老配置：240 秒分段配 120 秒超时，每段必然超时（这就是长会议转不出字的原因）
        let mut raw = serde_json::json!({
            "meeting": { "chunkSeconds": 240, "chunkTimeoutSecs": 120 }
        });
        assert!(migrate_chunk_timeout(&mut raw));
        assert_eq!(raw["meeting"]["chunkTimeoutSecs"], 426);
        // 幂等：再跑一次不再改动
        assert!(!migrate_chunk_timeout(&mut raw));
    }

    #[test]
    fn chunk_timeout_migration_respects_chunk_length() {
        let mut short = serde_json::json!({
            "meeting": { "chunkSeconds": 60, "chunkTimeoutSecs": 60 }
        });
        assert!(migrate_chunk_timeout(&mut short));
        assert_eq!(short["meeting"]["chunkTimeoutSecs"], 210);

        // 已经够大的不动
        let mut fine = serde_json::json!({
            "meeting": { "chunkSeconds": 240, "chunkTimeoutSecs": 900 }
        });
        assert!(!migrate_chunk_timeout(&mut fine));
        assert_eq!(fine["meeting"]["chunkTimeoutSecs"], 900);

        // 没有 meeting 段不能 panic
        let mut empty = serde_json::json!({});
        assert!(!migrate_chunk_timeout(&mut empty));
    }

    #[test]
    fn already_aws_config_is_untouched() {
        let mut raw = json!({
            "models": { "custom": [], "aws": { "bedrockProfile": "bedrock" } },
            "transcribe": { "modelId": "builtin-aws-transcribe", "thinking": { "enabled": false, "level": "LOW" } },
            "voiceTemplates": { "modelId": "builtin-bedrock-claude-sonnet-5", "thinking": { "enabled": true, "level": "HIGH" } },
            "voiceLearning": { "modelId": "builtin-bedrock-claude-opus-5", "thinking": { "enabled": false, "level": "LOW" } },
            "extract": { "modelId": null },
            "advanced": { "transcribeTimeout": 90, "optimizeTimeout": 45 },
        });
        let snapshot = raw.clone();

        assert!(!migrate_if_needed(&mut raw));
        assert_eq!(raw, snapshot);
    }

    #[test]
    fn oldest_layout_with_transcribe_model_field_ends_on_aws() {
        let mut raw = json!({
            "general": { "shortcut": "F4", "launchAtLogin": false, "theme": "system" },
            "transcribe": { "model": "gemini-3-flash-preview", "geminiApiKey": "AIza", "thinking": { "enabled": false, "level": "LOW" } },
            "optimize": { "enabled": true, "type": "openai-compat", "openaiCompat": { "baseUrl": "https://api.deepseek.com", "model": "deepseek-chat", "apiKey": "sk" }, "prompt": "" },
            "extract": {},
            "advanced": { "transcribeTimeout": 10, "optimizeTimeout": 10 },
        });

        assert!(migrate_if_needed(&mut raw));
        assert_eq!(raw["transcribe"]["modelId"], DEFAULT_TRANSCRIBE_MODEL);
        assert_eq!(raw["voiceTemplates"]["modelId"], DEFAULT_TEXT_MODEL);
        assert_eq!(raw["voiceLearning"]["modelId"], DEFAULT_TEXT_MODEL);
        assert!(raw.get("optimize").is_none());
        assert_eq!(raw["general"]["shortcutTemplate"], "voice-optimize");
        assert_eq!(raw["models"]["custom"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn no_panic_when_sections_missing() {
        let mut raw = json!({ "transcribe": {}, "other": 42 });
        assert!(migrate_if_needed(&mut raw));
        assert_eq!(raw["voiceLearning"]["modelId"], DEFAULT_TEXT_MODEL);

        let mut raw2 = json!({});
        migrate_if_needed(&mut raw2);
        assert_eq!(raw2["voiceLearning"]["modelId"], DEFAULT_TEXT_MODEL);
    }
}
