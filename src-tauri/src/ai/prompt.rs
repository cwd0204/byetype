use std::path::Path;

use crate::config::types::{AppConfig, TemplateEntry};

const OPTIMIZE_CONTEXT_INSTRUCTION: &str = "以下文档共同构成文本优化要求。先按 rules、vocabulary 和 voice-learning 修正转录文本，再按 text-optimize 处理输出样式。text-optimize 中限制修改原文的要求，不阻止执行上述转录修正。";

pub fn load_prompt(file_path: &str) -> String {
    if file_path.is_empty() {
        return String::new();
    }
    std::fs::read_to_string(file_path).unwrap_or_default()
}

pub fn wrap_document(name: &str, content: &str) -> String {
    if content.is_empty() {
        return String::new();
    }
    format!("<document name=\"{}\">\n{}\n</document>", name, content)
}

pub fn resolve_prompt_path(custom: &str, builtin: &str) -> String {
    if !custom.is_empty() {
        custom.to_string()
    } else {
        builtin.to_string()
    }
}

pub fn load_transcription_reference_content(
    config: &AppConfig,
    prompts_dir: &Path,
) -> Result<(String, String), String> {
    let (rules_path, vocabulary_path) = resolve_transcription_reference_paths(config, prompts_dir);
    let rules = std::fs::read_to_string(&rules_path)
        .map_err(|error| format!("读取转录规则失败：{error}"))?;
    let vocabulary = std::fs::read_to_string(&vocabulary_path)
        .map_err(|error| format!("读取专有词汇失败：{error}"))?;
    Ok((rules, vocabulary))
}

fn resolve_transcription_reference_paths(
    config: &AppConfig,
    prompts_dir: &Path,
) -> (String, String) {
    (
        resolve_prompt_path(
            &config.transcribe.prompts.rules,
            &prompts_dir.join("rules.md").to_string_lossy(),
        ),
        resolve_prompt_path(
            &config.transcribe.prompts.vocabulary,
            &prompts_dir.join("vocabulary.md").to_string_lossy(),
        ),
    )
}

pub fn build_transcribe_prompt(
    config: &AppConfig,
    prompts_dir: &Path,
    learning_rules: &str,
) -> String {
    let agent_path = resolve_prompt_path(
        &config.transcribe.prompts.agent,
        &prompts_dir.join("agent.md").to_string_lossy(),
    );
    let (rules_path, vocabulary_path) = resolve_transcription_reference_paths(config, prompts_dir);

    let agent_content = load_prompt(&agent_path);
    let vocabulary_content = load_prompt(&vocabulary_path);
    let rules_content = load_prompt(&rules_path);

    let parts: Vec<String> = [
        wrap_document("agent", &agent_content),
        wrap_document("vocabulary", &vocabulary_content),
        wrap_document("rules", &rules_content),
        wrap_document("voice-learning", learning_rules),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect();

    parts.join("\n\n")
}

pub fn build_optimize_prompt(
    config: &AppConfig,
    prompts_dir: &Path,
    template_id: &str,
    learning_rules: &str,
) -> String {
    let optimize_content =
        load_template_prompt(&config.voice_templates.templates, template_id, prompts_dir);
    if optimize_content.is_empty() {
        return String::new();
    }

    // 强模型转写阶段已按规则纠对,优化阶段默认只带优化模板本身;
    // 弱模型可开启 reuse_transcribe_references,优化阶段再带一遍参考文档做二次纠错。
    // Amazon Transcribe 这类纯 ASR 吃不到提示词,专有词 / 规则 / 学习结果只能在这里补,
    // 所以它选中时视同开关已开。
    let needs_references = config.voice_templates.reuse_transcribe_references
        || crate::ai::models::transcribe_needs_post_correction(config);
    if !needs_references {
        return wrap_document("text-optimize", &optimize_content);
    }

    let (rules_path, vocabulary_path) = resolve_transcription_reference_paths(config, prompts_dir);
    let rules_content = load_prompt(&rules_path);
    let vocabulary_content = load_prompt(&vocabulary_path);

    let reference_parts: Vec<String> = [
        wrap_document("rules", &rules_content),
        wrap_document("vocabulary", &vocabulary_content),
        wrap_document("voice-learning", learning_rules),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect();

    let mut parts = vec![wrap_document("text-optimize", &optimize_content)];
    if !reference_parts.is_empty() {
        parts.insert(0, OPTIMIZE_CONTEXT_INSTRUCTION.to_string());
        parts.extend(reference_parts);
    }

    parts.join("\n\n")
}

pub fn build_extract_prompt(config: &AppConfig, prompts_dir: &Path, template_id: &str) -> String {
    load_template_prompt(&config.extract.templates, template_id, prompts_dir)
}

/// 会议分段转写时附带的上下文。
pub struct MeetingChunkContext<'a> {
    pub chunk_index: u32,
    pub start_offset_secs: u64,
    /// 上一段转写的结尾，只用于承接语义
    pub previous_tail: &'a str,
    /// 「我 / 对方」能量提示，两个音轨都有时才有内容
    pub speaker_hints: &'a str,
}

fn format_offset(secs: u64) -> String {
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}

/// 会议分段转写提示词：meeting-transcribe + vocabulary + voice-learning + 本段上下文。
/// 不带 agent.md / rules.md——那两份是听写的单行输出与格式规则，和多人对话冲突。
pub fn build_meeting_transcribe_prompt(
    config: &AppConfig,
    prompts_dir: &Path,
    learning_rules: &str,
    ctx: &MeetingChunkContext<'_>,
) -> String {
    let main_path = resolve_prompt_path(
        &config.meeting.prompts.transcribe,
        &prompts_dir.join("meeting-transcribe.md").to_string_lossy(),
    );
    let (_, vocabulary_path) = resolve_transcription_reference_paths(config, prompts_dir);

    let mut context = format!(
        "这是第 {} 段，起点 {}。",
        ctx.chunk_index + 1,
        format_offset(ctx.start_offset_secs)
    );
    if !ctx.previous_tail.trim().is_empty() {
        context.push_str(&format!(
            "\n<previous-tail>\n{}\n</previous-tail>",
            ctx.previous_tail.trim()
        ));
    }
    if !ctx.speaker_hints.trim().is_empty() {
        context.push_str(&format!(
            "\n<speaker-hints>\n{}\n</speaker-hints>",
            ctx.speaker_hints.trim()
        ));
    }

    [
        wrap_document("meeting-transcribe", &load_prompt(&main_path)),
        wrap_document("vocabulary", &load_prompt(&vocabulary_path)),
        wrap_document("voice-learning", learning_rules),
        wrap_document("context", &context),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

/// 会议纪要提示词：只有 meeting-summary 一份文档。
pub fn build_meeting_summary_prompt(config: &AppConfig, prompts_dir: &Path) -> String {
    let path = resolve_prompt_path(
        &config.meeting.prompts.summary,
        &prompts_dir.join("meeting-summary.md").to_string_lossy(),
    );
    wrap_document("meeting-summary", &load_prompt(&path))
}

/// Map builtin template ID to builtin prompt filename
fn builtin_prompt_filename(template_id: &str) -> Option<&str> {
    match template_id {
        "voice-optimize" => Some("text-optimize.md"),
        "voice-translate" => Some("voice-translate.md"),
        "image-extract" => Some("text-extract.md"),
        "image-translate" => Some("image-translate.md"),
        _ => None,
    }
}

pub fn load_template_prompt(
    templates: &[TemplateEntry],
    template_id: &str,
    prompts_dir: &Path,
) -> String {
    let template = templates.iter().find(|t| t.id == template_id);

    // Prefer custom prompt path from template
    if let Some(t) = template {
        if !t.prompt.is_empty() {
            let content = load_prompt(&t.prompt);
            if !content.is_empty() {
                return content;
            }
        }
    }

    // Fall back to builtin file
    if let Some(filename) = builtin_prompt_filename(template_id) {
        let builtin_path = prompts_dir.join(filename);
        return load_prompt(&builtin_path.to_string_lossy());
    }

    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_prompts_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("byetype-prompt-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("test prompt directory should be created");
        dir
    }

    #[test]
    fn appends_learning_rules_to_transcription_prompt() {
        let config = AppConfig::default();
        let prompt = build_transcribe_prompt(
            &config,
            Path::new("/path/that/does/not/exist"),
            "- 项目技能，不是项目智能",
        );

        assert_eq!(
            prompt,
            "<document name=\"voice-learning\">\n- 项目技能，不是项目智能\n</document>"
        );
    }

    #[test]
    fn reference_content_read_failure_is_reported() {
        let config = AppConfig::default();

        let error =
            load_transcription_reference_content(&config, Path::new("/path/that/does/not/exist"))
                .expect_err("missing reference content should fail");

        assert!(error.starts_with("读取转录规则失败"));
    }

    #[test]
    fn optimize_prompt_reuses_transcription_references_without_agent_role() {
        let prompts_dir = test_prompts_dir("optimize-context");
        std::fs::write(prompts_dir.join("agent.md"), "角色定义").unwrap();
        std::fs::write(prompts_dir.join("rules.md"), "转录规则").unwrap();
        std::fs::write(prompts_dir.join("vocabulary.md"), "专有词汇").unwrap();
        std::fs::write(prompts_dir.join("text-optimize.md"), "文本优化提示词").unwrap();

        let mut config = AppConfig::default();
        config.voice_templates.reuse_transcribe_references = true;

        let prompt = build_optimize_prompt(
            &config,
            &prompts_dir,
            "voice-optimize",
            "自动学习结果",
        );

        assert_eq!(
            prompt,
            "以下文档共同构成文本优化要求。先按 rules、vocabulary 和 voice-learning 修正转录文本，再按 text-optimize 处理输出样式。text-optimize 中限制修改原文的要求，不阻止执行上述转录修正。\n\n\
<document name=\"text-optimize\">\n文本优化提示词\n</document>\n\n\
<document name=\"rules\">\n转录规则\n</document>\n\n\
<document name=\"vocabulary\">\n专有词汇\n</document>\n\n\
<document name=\"voice-learning\">\n自动学习结果\n</document>"
        );
        assert!(!prompt.contains("角色定义"));

        std::fs::remove_dir_all(prompts_dir).unwrap();
    }

    #[test]
    fn optimize_prompt_omits_transcription_references_by_default() {
        let prompts_dir = test_prompts_dir("optimize-lean");
        std::fs::write(prompts_dir.join("agent.md"), "角色定义").unwrap();
        std::fs::write(prompts_dir.join("rules.md"), "转录规则").unwrap();
        std::fs::write(prompts_dir.join("vocabulary.md"), "专有词汇").unwrap();
        std::fs::write(prompts_dir.join("text-optimize.md"), "文本优化提示词").unwrap();

        let prompt = build_optimize_prompt(
            &AppConfig::default(),
            &prompts_dir,
            "voice-optimize",
            "自动学习结果",
        );

        // 默认(开关关闭)时优化阶段只带优化模板本身,不重复带入转写参考。
        assert_eq!(
            prompt,
            "<document name=\"text-optimize\">\n文本优化提示词\n</document>"
        );
        assert!(!prompt.contains("转录规则"));
        assert!(!prompt.contains("专有词汇"));
        assert!(!prompt.contains("自动学习结果"));
        assert!(!prompt.contains("角色定义"));

        std::fs::remove_dir_all(prompts_dir).unwrap();
    }

    #[test]
    fn meeting_transcribe_prompt_carries_vocabulary_and_context() {
        let prompts_dir = test_prompts_dir("meeting-transcribe");
        std::fs::write(prompts_dir.join("meeting-transcribe.md"), "会议转写规则").unwrap();
        std::fs::write(prompts_dir.join("vocabulary.md"), "专有词汇").unwrap();
        std::fs::write(prompts_dir.join("agent.md"), "角色定义").unwrap();
        std::fs::write(prompts_dir.join("rules.md"), "听写规则").unwrap();

        let ctx = MeetingChunkContext {
            chunk_index: 2,
            start_offset_secs: 3725,
            previous_tail: "上一段结尾",
            speaker_hints: "00:00-00:10 我",
        };
        let prompt = build_meeting_transcribe_prompt(
            &AppConfig::default(),
            &prompts_dir,
            "学习结果",
            &ctx,
        );

        assert!(prompt.starts_with("<document name=\"meeting-transcribe\">\n会议转写规则"));
        assert!(prompt.contains("<document name=\"vocabulary\">\n专有词汇"));
        assert!(prompt.contains("<document name=\"voice-learning\">\n学习结果"));
        assert!(prompt.contains("这是第 3 段，起点 01:02:05。"));
        assert!(prompt.contains("<previous-tail>\n上一段结尾\n</previous-tail>"));
        assert!(prompt.contains("<speaker-hints>\n00:00-00:10 我\n</speaker-hints>"));
        assert!(!prompt.contains("角色定义"));
        assert!(!prompt.contains("听写规则"));

        std::fs::remove_dir_all(prompts_dir).unwrap();
    }

    #[test]
    fn meeting_summary_prompt_prefers_custom_path() {
        let prompts_dir = test_prompts_dir("meeting-summary");
        std::fs::write(prompts_dir.join("meeting-summary.md"), "内置纪要规则").unwrap();
        let custom = prompts_dir.join("my-summary.md");
        std::fs::write(&custom, "自定义纪要规则").unwrap();

        let mut config = AppConfig::default();
        assert_eq!(
            build_meeting_summary_prompt(&config, &prompts_dir),
            "<document name=\"meeting-summary\">\n内置纪要规则\n</document>"
        );

        config.meeting.prompts.summary = custom.to_string_lossy().to_string();
        assert_eq!(
            build_meeting_summary_prompt(&config, &prompts_dir),
            "<document name=\"meeting-summary\">\n自定义纪要规则\n</document>"
        );

        std::fs::remove_dir_all(prompts_dir).unwrap();
    }

    #[test]
    fn optimize_prompt_always_carries_references_for_aws_transcribe() {
        let prompts_dir = test_prompts_dir("optimize-aws-transcribe");
        std::fs::write(prompts_dir.join("rules.md"), "转录规则").unwrap();
        std::fs::write(prompts_dir.join("vocabulary.md"), "专有词汇").unwrap();
        std::fs::write(prompts_dir.join("text-optimize.md"), "文本优化提示词").unwrap();

        // 开关关闭,但转写模型是 Amazon Transcribe:参考文档必须带上,否则专有词无处纠正。
        let mut config = AppConfig::default();
        config.voice_templates.reuse_transcribe_references = false;
        config.transcribe.model_id = "builtin-aws-transcribe".to_string();

        let prompt = build_optimize_prompt(&config, &prompts_dir, "voice-optimize", "学习结果");

        assert!(prompt.starts_with(OPTIMIZE_CONTEXT_INSTRUCTION));
        assert!(prompt.contains("<document name=\"rules\">\n转录规则"));
        assert!(prompt.contains("<document name=\"vocabulary\">\n专有词汇"));
        assert!(prompt.contains("<document name=\"voice-learning\">\n学习结果"));

        std::fs::remove_dir_all(prompts_dir).unwrap();
    }
}
