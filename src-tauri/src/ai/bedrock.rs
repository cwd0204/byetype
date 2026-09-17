//! Amazon Bedrock（Converse API）：Claude 做文本优化、图像取字、通用文本补全。
//! 不收音频——Bedrock 上能批量吃音频的只有 Voxtral（不含中文），语音走 transcribe_aws.rs。
//! 凭证与 region 来自 config.models.aws，见 aws.rs。

use std::collections::HashMap;

use aws_sdk_bedrockruntime::primitives::Blob;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, ImageBlock, ImageFormat, ImageSource, InferenceConfiguration,
    Message, SystemContentBlock,
};
use aws_sdk_bedrockruntime::Client;
use aws_smithy_types::{Document, Number};
use base64::Engine as _;

use super::aws;
use super::types::TokenUsage;
use crate::config::types::{AwsConfig, ThinkingConfig};

const LABEL: &str = "Bedrock";
/// 正文输出上限。开启思考时思考 token 也计入 max_tokens，所以再加一段余量。
const DEFAULT_MAX_TOKENS: i32 = 8192;
const THINKING_HEADROOM: i32 = 8192;

/// Claude 的两代思考参数：
/// - 4.6 及以后（含 5.x）：`thinking.type = adaptive` + `output_config.effort`
/// - 更早的（Haiku 4.5 / Sonnet 4.5 / Opus 4.5 / Sonnet 4 / 3.x）：`thinking.type = enabled` + `budget_tokens`
///
/// 先按模型名猜，猜错时服务端会明确报错，再用另一种重试一次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingMode {
    Adaptive,
    Budget,
}

impl ThinkingMode {
    fn other(self) -> Self {
        match self {
            ThinkingMode::Adaptive => ThinkingMode::Budget,
            ThinkingMode::Budget => ThinkingMode::Adaptive,
        }
    }
}

/// 一次请求实际带的思考形态。三态是必要的：`Absent`（不带任何思考字段）与
/// `Off`（显式 `thinking.type=disabled`）在服务端含义不同 —— Adaptive 代次的
/// Claude 省略字段时会按默认的自适应思考跑，effort 还取默认高档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingRequest {
    /// 不带思考字段。Budget 代次关闭思考、以及非 Claude 模型都用这个。
    Absent,
    /// 显式关闭。Adaptive 代次的 Claude 必须发，否则等于开着思考。
    Off,
    /// 开启，按代次选参数形式。
    On(ThinkingMode),
}

fn is_claude(model: &str) -> bool {
    model.to_ascii_lowercase().contains("claude")
}

/// 关闭思考时该发什么。
///
/// 两代模型省略 thinking 字段的含义是相反的：Adaptive 代次（4.6 及以后，含 5.x）
/// 省略等于开着自适应思考，必须显式发 `disabled`；Budget 代次（4.5 及更早）省略
/// 本来就不思考，保持不发。`thinking` 是 Claude 专有字段，非 Claude 模型一律不发。
fn thinking_off_request(model: &str) -> ThinkingRequest {
    if !is_claude(model) {
        return ThinkingRequest::Absent;
    }
    match preferred_thinking_mode(model) {
        ThinkingMode::Adaptive => ThinkingRequest::Off,
        ThinkingMode::Budget => ThinkingRequest::Absent,
    }
}

fn preferred_thinking_mode(model: &str) -> ThinkingMode {
    const BUDGET_GENERATIONS: [&str; 7] = [
        "claude-3",
        "claude-sonnet-4-20250514",
        "claude-opus-4-1",
        "claude-sonnet-4-5",
        "claude-opus-4-5",
        "claude-haiku-4-5",
        "claude-instant",
    ];
    if BUDGET_GENERATIONS
        .iter()
        .any(|needle| model.contains(needle))
    {
        ThinkingMode::Budget
    } else {
        ThinkingMode::Adaptive
    }
}

/// 服务端明确说思考参数形式不对时，换另一种再试。
fn suggests_other_thinking_mode(error: &str) -> bool {
    error.contains("thinking.type") || error.contains("output_config")
}

fn normalized_level(thinking: &ThinkingConfig) -> String {
    thinking.level.trim().to_uppercase()
}

/// 预算模式：思考档位 → budget_tokens。
fn thinking_budget(thinking: &ThinkingConfig) -> i32 {
    match normalized_level(thinking).as_str() {
        "MINIMAL" | "LOW" => 1024,
        "HIGH" => 16384,
        _ => 4096,
    }
}

/// 自适应模式：思考档位 → effort。
fn thinking_effort(thinking: &ThinkingConfig) -> &'static str {
    match normalized_level(thinking).as_str() {
        "MINIMAL" | "LOW" => "low",
        "HIGH" => "high",
        _ => "medium",
    }
}

fn thinking_fields(mode: ThinkingMode, thinking: &ThinkingConfig) -> Document {
    let mut root = HashMap::new();
    match mode {
        ThinkingMode::Adaptive => {
            let mut thinking_obj = HashMap::new();
            thinking_obj.insert("type".to_string(), Document::String("adaptive".to_string()));
            root.insert("thinking".to_string(), Document::Object(thinking_obj));
            let mut output = HashMap::new();
            output.insert(
                "effort".to_string(),
                Document::String(thinking_effort(thinking).to_string()),
            );
            root.insert("output_config".to_string(), Document::Object(output));
        }
        ThinkingMode::Budget => {
            let mut thinking_obj = HashMap::new();
            thinking_obj.insert("type".to_string(), Document::String("enabled".to_string()));
            thinking_obj.insert(
                "budget_tokens".to_string(),
                Document::Number(Number::PosInt(thinking_budget(thinking).max(0) as u64)),
            );
            root.insert("thinking".to_string(), Document::Object(thinking_obj));
        }
    }
    Document::Object(root)
}

/// 本次请求要塞进 additionalModelRequestFields 的内容，`None` 表示什么都不带。
fn request_fields(req: ThinkingRequest, thinking: &ThinkingConfig) -> Option<Document> {
    match req {
        ThinkingRequest::Absent => None,
        ThinkingRequest::Off => {
            let mut thinking_obj = HashMap::new();
            thinking_obj.insert("type".to_string(), Document::String("disabled".to_string()));
            let mut root = HashMap::new();
            root.insert("thinking".to_string(), Document::Object(thinking_obj));
            Some(Document::Object(root))
        }
        ThinkingRequest::On(mode) => Some(thinking_fields(mode, thinking)),
    }
}

/// 万一模型把 `<thinking>…</thinking>` 漏进正文块，这段字会被直接粘进用户的文档。
/// 只剥完整成对的标签，配不上就原样留着，避免把正常文本吃掉。
fn strip_thinking_tags(text: &str) -> String {
    const OPEN: &str = "<thinking>";
    const CLOSE: &str = "</thinking>";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        // to_ascii_lowercase 不改变 UTF-8 字节长度，字节下标对 rest 仍然有效。
        let lower = rest.to_ascii_lowercase();
        let Some(start) = lower.find(OPEN) else { break };
        let Some(end) = lower[start..].find(CLOSE) else { break };
        out.push_str(&rest[..start]);
        rest = &rest[start + end + CLOSE.len()..];
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// 本次调用的 max_tokens：预算模式必须大于 budget_tokens，自适应模式给思考留余量。
fn max_tokens_for(req: ThinkingRequest, thinking: &ThinkingConfig, base: i32) -> i32 {
    match req {
        ThinkingRequest::Absent | ThinkingRequest::Off => base,
        ThinkingRequest::On(ThinkingMode::Budget) => base + thinking_budget(thinking),
        ThinkingRequest::On(ThinkingMode::Adaptive) => base + THINKING_HEADROOM,
    }
}

async fn send_once(
    client: &Client,
    model: &str,
    system_prompt: &str,
    content: &[ContentBlock],
    thinking: &ThinkingConfig,
    req: ThinkingRequest,
    base_max_tokens: i32,
) -> Result<(String, TokenUsage), String> {
    let message = Message::builder()
        .role(ConversationRole::User)
        .set_content(Some(content.to_vec()))
        .build()
        .map_err(|e| format!("{} build message failed: {}", LABEL, e))?;

    let mut request = client
        .converse()
        .model_id(model)
        .messages(message)
        .inference_config(
            InferenceConfiguration::builder()
                .max_tokens(max_tokens_for(req, thinking, base_max_tokens))
                .build(),
        );
    if !system_prompt.is_empty() {
        request = request.system(SystemContentBlock::Text(system_prompt.to_string()));
    }
    if let Some(fields) = request_fields(req, thinking) {
        request = request.additional_model_request_fields(fields);
    }

    let response = request
        .send()
        .await
        .map_err(|e| aws::map_sdk_error(LABEL, e))?;

    let text = response
        .output()
        .and_then(|output| output.as_message().ok())
        .map(|message| {
            message
                .content()
                .iter()
                .filter_map(|block| block.as_text().ok().map(|s| s.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let text = strip_thinking_tags(&text);

    let usage = response
        .usage()
        .map(|u| TokenUsage {
            prompt_tokens: u.input_tokens().max(0) as u64,
            completion_tokens: u.output_tokens().max(0) as u64,
        })
        .unwrap_or_default();

    Ok((text, usage))
}

/// 一次 Converse 调用：system 提示词 + 单条 user 消息，返回拼接后的文本与用量。
/// 开启思考时先按模型代际选参数形式，被服务端拒绝就换另一种重试一次。
async fn converse(
    cfg: &AwsConfig,
    model: &str,
    system_prompt: &str,
    content: Vec<ContentBlock>,
    thinking: &ThinkingConfig,
    base_max_tokens: i32,
) -> Result<(String, TokenUsage), String> {
    let sdk = aws::sdk_config(&cfg.bedrock_profile, &cfg.bedrock_region).await?;
    let client = Client::new(&sdk);

    // 猜错思考参数形式时服务端会明确报错，用 fallback 再试一次。
    let (first, fallback) = if thinking.enabled {
        let mode = preferred_thinking_mode(model);
        (
            ThinkingRequest::On(mode),
            Some(ThinkingRequest::On(mode.other())),
        )
    } else {
        match thinking_off_request(model) {
            // 显式关闭被拒（老模型 / 自定义模型不认这个字段）就退回什么都不发。
            ThinkingRequest::Off => (ThinkingRequest::Off, Some(ThinkingRequest::Absent)),
            other => (other, None),
        }
    };

    match send_once(
        &client,
        model,
        system_prompt,
        &content,
        thinking,
        first,
        base_max_tokens,
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(error) => match fallback {
            Some(fallback) if suggests_other_thinking_mode(&error) => {
                eprintln!(
                    "[Bedrock] {:?} rejected for {}, retrying with {:?}",
                    first, model, fallback
                );
                send_once(
                    &client,
                    model,
                    system_prompt,
                    &content,
                    thinking,
                    fallback,
                    base_max_tokens,
                )
                .await
            }
            _ => Err(error),
        },
    }
}

/// 文本优化 / 通用文本处理：与其他 provider 一样把输入包在 <voice-input> 里，空响应时回退原文。
pub async fn optimize(
    cfg: &AwsConfig,
    text: &str,
    system_prompt: &str,
    model: &str,
    thinking: &ThinkingConfig,
) -> Result<(String, TokenUsage), String> {
    let user_content = format!("<voice-input>\n{}\n</voice-input>", text);
    let (result, usage) = converse(
        cfg,
        model,
        system_prompt,
        vec![ContentBlock::Text(user_content)],
        thinking,
        DEFAULT_MAX_TOKENS,
    )
    .await?;
    if result.is_empty() {
        // 正文为空基本只有一种成因：推理 token 把 max_tokens 占满了。
        // 回退原文是对的（总比丢字好），但别再无声无息 —— 上一轮就是这样查了很久。
        eprintln!(
            "[Bedrock] {} returned no text (in={} out={}), falling back to raw transcript",
            model, usage.prompt_tokens, usage.completion_tokens
        );
        return Ok((text.to_string(), usage));
    }
    Ok((result, usage))
}

/// 图像取字：截图是 PNG base64，解码成字节交给 Converse 的 ImageBlock。
pub async fn extract_text(
    cfg: &AwsConfig,
    image_base64: &str,
    system_prompt: &str,
    model: &str,
    thinking: &ThinkingConfig,
) -> Result<(String, TokenUsage), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(image_base64)
        .map_err(|e| format!("{} image decode failed: {}", LABEL, e))?;
    let image = ImageBlock::builder()
        .format(ImageFormat::Png)
        .source(ImageSource::Bytes(Blob::new(bytes)))
        .build()
        .map_err(|e| format!("{} build image block failed: {}", LABEL, e))?;
    converse(
        cfg,
        model,
        system_prompt,
        vec![ContentBlock::Image(image)],
        thinking,
        DEFAULT_MAX_TOKENS,
    )
    .await
}

/// 连通性测试：最小的一次 Converse 调用，能拿到响应即视为凭证、region、模型都可用。
pub async fn test_connectivity(cfg: &AwsConfig, model: &str) -> Result<(), String> {
    converse(
        cfg,
        model,
        "",
        vec![ContentBlock::Text("hi".to_string())],
        &ThinkingConfig::default(),
        8,
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thinking(enabled: bool, level: &str) -> ThinkingConfig {
        ThinkingConfig {
            enabled,
            level: level.to_string(),
        }
    }

    fn object(doc: &Document) -> &HashMap<String, Document> {
        match doc {
            Document::Object(map) => map,
            _ => panic!("expected object"),
        }
    }

    #[test]
    fn newer_claude_uses_adaptive_and_older_uses_budget() {
        assert_eq!(
            preferred_thinking_mode("global.anthropic.claude-sonnet-5"),
            ThinkingMode::Adaptive
        );
        assert_eq!(
            preferred_thinking_mode("global.anthropic.claude-opus-4-6-v1"),
            ThinkingMode::Adaptive
        );
        assert_eq!(
            preferred_thinking_mode("global.anthropic.claude-haiku-4-5-20251001-v1:0"),
            ThinkingMode::Budget
        );
        assert_eq!(
            preferred_thinking_mode("anthropic.claude-3-5-sonnet-20241022-v2:0"),
            ThinkingMode::Budget
        );
    }

    #[test]
    fn thinking_levels_map_to_budget_and_effort() {
        assert_eq!(thinking_budget(&thinking(true, "MINIMAL")), 1024);
        assert_eq!(thinking_budget(&thinking(true, "low")), 1024);
        assert_eq!(thinking_budget(&thinking(true, "MEDIUM")), 4096);
        assert_eq!(thinking_budget(&thinking(true, "HIGH")), 16384);
        assert_eq!(thinking_effort(&thinking(true, "LOW")), "low");
        assert_eq!(thinking_effort(&thinking(true, "")), "medium");
        assert_eq!(thinking_effort(&thinking(true, "HIGH")), "high");
    }

    #[test]
    fn adaptive_fields_have_expected_shape() {
        let doc = thinking_fields(ThinkingMode::Adaptive, &thinking(true, "HIGH"));
        let root = object(&doc);
        assert_eq!(
            object(root.get("thinking").unwrap()).get("type"),
            Some(&Document::String("adaptive".to_string()))
        );
        assert_eq!(
            object(root.get("output_config").unwrap()).get("effort"),
            Some(&Document::String("high".to_string()))
        );
    }

    #[test]
    fn budget_fields_have_expected_shape() {
        let doc = thinking_fields(ThinkingMode::Budget, &thinking(true, "LOW"));
        let thinking_obj = object(object(&doc).get("thinking").unwrap());
        assert_eq!(
            thinking_obj.get("type"),
            Some(&Document::String("enabled".to_string()))
        );
        assert_eq!(
            thinking_obj.get("budget_tokens"),
            Some(&Document::Number(Number::PosInt(1024)))
        );
    }

    #[test]
    fn max_tokens_leaves_room_for_thinking() {
        let t = thinking(true, "HIGH");
        assert_eq!(max_tokens_for(ThinkingRequest::Absent, &t, 8192), 8192);
        assert_eq!(max_tokens_for(ThinkingRequest::Off, &t, 8192), 8192);
        assert_eq!(
            max_tokens_for(ThinkingRequest::On(ThinkingMode::Budget), &t, 8192),
            8192 + 16384
        );
        assert_eq!(
            max_tokens_for(ThinkingRequest::On(ThinkingMode::Adaptive), &t, 8192),
            8192 + THINKING_HEADROOM
        );
    }

    /// 听写慢的主因：关闭思考时对 Adaptive 代次必须显式发 disabled，
    /// 否则服务端按默认的自适应思考跑。Budget 代次省略字段本来就不思考。
    #[test]
    fn disabling_thinking_is_explicit_only_for_adaptive_claude() {
        assert_eq!(
            thinking_off_request("global.anthropic.claude-sonnet-5"),
            ThinkingRequest::Off
        );
        assert_eq!(
            thinking_off_request("global.anthropic.claude-opus-5"),
            ThinkingRequest::Off
        );
        assert_eq!(
            thinking_off_request("global.anthropic.claude-haiku-4-5-20251001-v1:0"),
            ThinkingRequest::Absent
        );
        assert_eq!(
            thinking_off_request("anthropic.claude-3-5-sonnet-20241022-v2:0"),
            ThinkingRequest::Absent
        );
        // thinking 是 Claude 专有字段，别发给 Nova / Llama 之类的自定义模型。
        assert_eq!(thinking_off_request("amazon.nova-pro-v1:0"), ThinkingRequest::Absent);
        assert_eq!(
            thinking_off_request("meta.llama3-3-70b-instruct-v1:0"),
            ThinkingRequest::Absent
        );
    }

    #[test]
    fn off_request_sends_thinking_disabled() {
        let t = thinking(false, "LOW");
        let doc = request_fields(ThinkingRequest::Off, &t).expect("off must send fields");
        let root = object(&doc);
        assert_eq!(
            object(root.get("thinking").unwrap()).get("type"),
            Some(&Document::String("disabled".to_string()))
        );
        // disabled 时不该带 output_config，也不该带 budget_tokens
        assert!(root.get("output_config").is_none());
        assert!(object(root.get("thinking").unwrap())
            .get("budget_tokens")
            .is_none());
        assert!(request_fields(ThinkingRequest::Absent, &t).is_none());
    }

    #[test]
    fn strips_only_paired_thinking_tags() {
        assert_eq!(
            strip_thinking_tags("<thinking>先想一下</thinking>这是正文"),
            "这是正文"
        );
        assert_eq!(
            strip_thinking_tags("前<thinking>a</thinking>中<THINKING>b</THINKING>后"),
            "前中后"
        );
        // 配不上的标签原样留着，别把正常文本吃掉
        assert_eq!(strip_thinking_tags("正文 <thinking> 没闭合"), "正文 <thinking> 没闭合");
        assert_eq!(strip_thinking_tags("普通中文，没有标签"), "普通中文，没有标签");
    }

    #[test]
    fn detects_thinking_shape_rejections() {
        assert!(suggests_other_thinking_mode(
            "Bedrock API error (400): ValidationException: \"thinking.type.enabled\" is not supported for this model. Use \"thinking.type.adaptive\""
        ));
        assert!(!suggests_other_thinking_mode(
            "Bedrock API error (429): ThrottlingException: slow down"
        ));
    }

    /// 真机联调：需要本机 AWS profile 有 Bedrock 权限。
    /// `BYETYPE_TEST_BEDROCK_PROFILE` / `BYETYPE_TEST_BEDROCK_REGION` 覆盖默认的 bedrock / ap-northeast-1。
    /// 运行：cargo test --lib live_bedrock -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "需要 AWS 凭证与网络"]
    async fn live_bedrock_optimize_and_connectivity() {
        let cfg = AwsConfig {
            bedrock_profile: std::env::var("BYETYPE_TEST_BEDROCK_PROFILE")
                .unwrap_or_else(|_| "bedrock".into()),
            bedrock_region: std::env::var("BYETYPE_TEST_BEDROCK_REGION")
                .unwrap_or_else(|_| "ap-northeast-1".into()),
            ..AwsConfig::default()
        };
        let model = "global.anthropic.claude-sonnet-5";

        test_connectivity(&cfg, model).await.expect("connectivity");

        let (text, usage) = optimize(
            &cfg,
            "嗯 那个 我们今天 下午三点 开会 讨论一下 bedrock 接入的方案",
            "你是文本整理工具。去掉口语填充词，补全标点，只输出整理后的一句话。",
            model,
            &thinking(false, "LOW"),
        )
        .await
        .expect("optimize");
        eprintln!("[live] optimize → {:?} usage={:?}", text, usage);
        assert!(
            text.contains("三点"),
            "expected cleaned sentence, got {text}"
        );
        assert!(usage.prompt_tokens > 0 && usage.completion_tokens > 0);

        // Claude 5 走 adaptive；Haiku 4.5 走 budget。两代都要能走通。
        let (text, _) = optimize(
            &cfg,
            "一二三",
            "把输入原样返回。",
            model,
            &thinking(true, "LOW"),
        )
        .await
        .expect("optimize with adaptive thinking");
        eprintln!("[live] optimize(sonnet-5, thinking) → {:?}", text);

        let haiku = "global.anthropic.claude-haiku-4-5-20251001-v1:0";
        let (text, _) = optimize(
            &cfg,
            "一二三",
            "把输入原样返回。",
            haiku,
            &thinking(true, "LOW"),
        )
        .await
        .expect("optimize with budget thinking");
        eprintln!("[live] optimize(haiku-4-5, thinking) → {:?}", text);
    }
}
