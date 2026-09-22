//! Amazon Bedrock（Converse API）：Claude 做文本优化、图像取字、通用文本补全，
//! Voxtral 做「高准确度」语音转写（`transcribe_audio`，只吃 wav / mp3，不支持流式输入）。
//! 默认的听写转写走 transcribe_aws.rs 的流式通道。
//! 凭证与 region 来自 config.models.aws，见 aws.rs。

use std::collections::HashMap;

use aws_sdk_bedrockruntime::primitives::Blob;
use aws_sdk_bedrockruntime::types::{
    AudioBlock, AudioFormat, AudioSource, ContentBlock, ConversationRole, ImageBlock, ImageFormat,
    ImageSource, InferenceConfiguration, Message, SystemContentBlock,
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

/// 转写输出上限。听写一段 3 分钟的话大约几百字，给 3072 已经很宽；
/// 上限本身也是退化的兜底——模型卡进重复循环时最多烧到这里就停。
const TRANSCRIBE_MAX_TOKENS: i32 = 3072;

/// Voxtral 的转写指令。**改这段之前先跑 `live_voxtral` 测试。**
///
/// 实测（2026-09-22，mistral.voxtral-small-24b-2507，均在 temperature=0 下）：
/// - 不给指令 → 它把音频当问题回答，而不是转写
/// - 指令放 system 块 → 服务端 ValidationException 拒绝
/// - 中文指令 → 能转写，但再加一行 450 字符的术语表就会切换成问答模式
/// - 把 agent / rules / vocabulary 三份文档（约 3600 字符）当指令 → 彻底跑偏，只会回答问题
/// - 英文短指令 + 一行短术语提示 → 最好：5 次调用一字不差，且保住「Amazon」不被译成「亚马逊」
///
/// **不传 temperature 会踩大坑**：Mistral 的默认采样温度不是 0，同一段音频每次转写都不同，
/// 见过「转录」变「链载」「录音」「翻译」、凭空多出「AMZN」、输出繁体。所以转写路径显式传 0。
///
/// 结论：必须是英文、必须短、术语提示要截断、不要做成用户可编辑的提示词文件。
const TRANSCRIBE_INSTRUCTION: &str = "Transcribe the audio verbatim. \
Do not answer, explain, translate, or summarize anything you hear. \
Keep English proper nouns in Latin script. Output only the transcript.";

/// 术语提示的长度上限。实测超过约 450 字符会把模型推进问答模式。
const TERM_HINT_MAX_CHARS: usize = 300;

/// WAV 体积上限。超过就别发了，交给调用方回落到流式 Transcribe。
///
/// 实测边界（2026-09-22）：1.83 MB / 60 秒可以，2.75 MB / 90 秒被服务端拒绝
/// （ValidationException: Failed to buffer the request body），所以真实上限约 2 MB。
/// 16 kHz 单声道 16 位 = 32 KB/秒，取 1.75 MB ≈ 55 秒，留一点余量。
/// 超过这个长度的录音只能走 Transcribe —— 这是 Voxtral 这条路的硬约束。
const TRANSCRIBE_MAX_WAV_BYTES: usize = 1_835_008; // 1.75 MiB

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

/// 推理块的字符总数。只数明文推理；被服务商加密的 `redactedContent` 数不了也不猜。
/// Bedrock 的 outputTokens 把推理和正文混在一起，没有这个数就解释不了
/// 「输出两千 token、正文一百字」是怎么来的。
fn reasoning_chars(blocks: &[ContentBlock]) -> u64 {
    blocks
        .iter()
        .filter_map(|block| block.as_reasoning_content().ok())
        .filter_map(|reasoning| reasoning.as_reasoning_text().ok())
        .map(|text| text.text().chars().count() as u64)
        .sum()
}

/// 本次调用的 max_tokens：预算模式必须大于 budget_tokens，自适应模式给思考留余量。
fn max_tokens_for(req: ThinkingRequest, thinking: &ThinkingConfig, base: i32) -> i32 {
    match req {
        ThinkingRequest::Absent | ThinkingRequest::Off => base,
        ThinkingRequest::On(ThinkingMode::Budget) => base + thinking_budget(thinking),
        ThinkingRequest::On(ThinkingMode::Adaptive) => base + THINKING_HEADROOM,
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_once(
    client: &Client,
    model: &str,
    system_prompt: &str,
    content: &[ContentBlock],
    thinking: &ThinkingConfig,
    req: ThinkingRequest,
    base_max_tokens: i32,
    temperature: Option<f32>,
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
        .inference_config({
            let mut cfg = InferenceConfiguration::builder()
                .max_tokens(max_tokens_for(req, thinking, base_max_tokens));
            // 不设温度就是走模型默认采样：Mistral 的默认不是 0，同一段音频每次转写结果都不一样。
            // 转写必须可复现，所以那条路径显式传 0。
            if let Some(t) = temperature {
                cfg = cfg.temperature(t);
            }
            cfg.build()
        });
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

    let blocks = response
        .output()
        .and_then(|output| output.as_message().ok())
        .map(|message| message.content())
        .unwrap_or_default();
    let text = blocks
        .iter()
        .filter_map(|block| block.as_text().ok().map(|s| s.as_str()))
        .collect::<Vec<_>>()
        .join("");
    let text = strip_thinking_tags(&text);

    let usage = TokenUsage {
        prompt_tokens: response
            .usage()
            .map(|u| u.input_tokens().max(0) as u64)
            .unwrap_or_default(),
        completion_tokens: response
            .usage()
            .map(|u| u.output_tokens().max(0) as u64)
            .unwrap_or_default(),
        reasoning_chars: reasoning_chars(blocks),
    };

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
    temperature: Option<f32>,
) -> Result<(String, TokenUsage), String> {
    let sdk = aws::sdk_config(&cfg.bedrock_profile, &cfg.bedrock_region).await?;
    let client = Client::new(&sdk);

    // 猜错思考参数形式时服务端会明确报错，用 fallback 再试一次。
    // thinking / output_config 是 Claude 专用字段，非 Claude 模型（Voxtral、Nova 等）
    // 一律什么都不发，否则会被服务端当成非法参数拒掉。
    let (first, fallback) = if thinking.enabled && is_claude(model) {
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
        temperature,
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
                    temperature,
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
        None,
    )
    .await?;
    if result.is_empty() {
        // 正文为空基本只有一种成因：推理 token 把 max_tokens 占满了。
        // 回退原文是对的（总比丢字好），但别再无声无息 —— 上一轮就是这样查了很久。
        eprintln!(
            "[Bedrock] {} returned no text (in={} out={} reasoning_chars={}), falling back to raw transcript",
            model, usage.prompt_tokens, usage.completion_tokens, usage.reasoning_chars
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
        None,
    )
    .await
}

/// 「高准确度」语音转写：把整段录音交给 Bedrock 上的 Voxtral。
///
/// 输入是管道通用的 base64 FLAC；Voxtral 只认 wav / mp3，所以解成 PCM 再套 WAV 头。
/// 指令放 user 文本块（system 块会被拒），可选带一行紧凑术语提示。
///
/// 返回 `Err(DEGENERATE_MARKER…)` 表示产出不像转写稿（模型改去回答问题、或卡进重复循环），
/// 由调用方回落到 Amazon Transcribe。
pub async fn transcribe_audio(
    cfg: &AwsConfig,
    audio_base64: &str,
    model: &str,
    term_hint: &str,
) -> Result<(String, TokenUsage), String> {
    let flac = base64::engine::general_purpose::STANDARD
        .decode(audio_base64)
        .map_err(|e| format!("{} audio decode failed: {}", LABEL, e))?;
    let pcm = super::transcribe_aws::flac_to_pcm_bytes(flac)?;
    let audio_secs = pcm.len() as f32 / (crate::audio::encoder::SAMPLE_RATE as f32 * 2.0);
    let wav = crate::audio::encoder::wrap_pcm16_as_wav(&pcm, crate::audio::encoder::SAMPLE_RATE);
    // 调试开关：把真正发给模型的 WAV 落盘，用来核对链路有没有改坏音频
    if let Ok(path) = std::env::var("BYETYPE_DUMP_TRANSCRIBE_WAV") {
        let _ = std::fs::write(&path, &wav);
        eprintln!("[transcribe] dumped {} bytes to {}", wav.len(), path);
    }
    if wav.len() > TRANSCRIBE_MAX_WAV_BYTES {
        return Err(format!(
            "{}: 录音过长（{:.0} 秒）不适合整段上传",
            NOT_A_TRANSCRIPT, audio_secs
        ));
    }

    let audio = AudioBlock::builder()
        .format(AudioFormat::Wav)
        .source(AudioSource::Bytes(Blob::new(wav)))
        .build()
        .map_err(|e| format!("{} build audio block failed: {}", LABEL, e))?;

    let mut instruction = TRANSCRIBE_INSTRUCTION.to_string();
    if !term_hint.is_empty() {
        instruction.push_str("\nLikely proper nouns: ");
        instruction.push_str(term_hint);
    }

    // system 块会被 Voxtral 拒绝，指令只能跟在音频后面当 user 文本块。
    let (text, usage) = converse(
        cfg,
        model,
        "",
        vec![ContentBlock::Audio(audio), ContentBlock::Text(instruction)],
        &ThinkingConfig {
            enabled: false,
            ..ThinkingConfig::default()
        },
        TRANSCRIBE_MAX_TOKENS,
        // 转写要可复现：温度留空会走模型默认采样，同一段音频每次结果都不同
        Some(0.0),
    )
    .await
    .map_err(|error| {
        if is_payload_too_large(&error) {
            format!("{}: 请求体过大（{:.1} 秒音频）", NOT_A_TRANSCRIPT, audio_secs)
        } else {
            error
        }
    })?;

    let text = text.trim().to_string();
    if let Some(reason) = transcript_defect(&text, audio_secs) {
        return Err(format!("{}: {}", NOT_A_TRANSCRIPT, reason));
    }
    Ok((text, usage))
}

/// 服务端因请求体过大拒绝时的特征。上面的体积守卫是第一道，这是兜底：
/// 万一实际上限比我们估的还小，也要走回落而不是把硬错误抛给用户。
fn is_payload_too_large(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("failed to buffer the request body")
        || lower.contains("request body too large")
        || lower.contains("payload too large")
        || lower.contains("(413)")
}

/// 从词汇表文档里抽出紧凑的术语提示：只取 `- ` 开头的条目、去掉括号里的说明、
/// 只保留拉丁字符条目（中文术语不需要提示，塞进去反而挤占本就很小的指令预算），
/// 并截断到 `TERM_HINT_MAX_CHARS`。
pub fn term_hint_from_vocabulary(vocabulary: &str) -> String {
    let mut terms: Vec<&str> = Vec::new();
    for line in vocabulary.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("- ") else {
            continue;
        };
        let term = rest
            .split(['（', '(', '：', ':'])
            .next()
            .unwrap_or("")
            .trim();
        let latin = !term.is_empty()
            && term
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || " /.-+&".contains(c))
            && term.chars().any(|c| c.is_ascii_alphabetic());
        if latin && !terms.contains(&term) {
            terms.push(term);
        }
    }
    let mut hint = String::new();
    for term in terms {
        let extra = if hint.is_empty() { 0 } else { 2 };
        if hint.len() + extra + term.len() > TERM_HINT_MAX_CHARS {
            break;
        }
        if !hint.is_empty() {
            hint.push_str(", ");
        }
        hint.push_str(term);
    }
    hint
}

/// 产出不像转写稿时返回原因，像则返回 None。
///
/// 三条实测来的判据：
/// 1. 长度远超音频时长能承载的字数 —— 模型改去回答问题时会长篇大论
/// 2. 出现 markdown 结构 —— 口语转写不会有 `**`、编号列表或标题
/// 3. 结尾是同一小段重复多次 —— 卡进重复循环直到撞 max_tokens
fn transcript_defect(text: &str, audio_secs: f32) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let limit = (audio_secs * 12.0).max(40.0) as usize;
    if chars.len() > limit {
        return Some(format!(
            "输出 {} 字远超 {:.1} 秒音频的合理长度（上限 {}）",
            chars.len(),
            audio_secs,
            limit
        ));
    }
    if text.contains("**") {
        return Some("输出含 markdown 粗体".to_string());
    }
    for line in text.lines() {
        let line = line.trim_start();
        if line.starts_with('#') {
            return Some("输出含 markdown 标题".to_string());
        }
        let mut it = line.chars();
        let digits: String = it.by_ref().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && line[digits.len()..].starts_with(". ") {
            return Some("输出含编号列表".to_string());
        }
    }
    // 结尾周期性重复：末 k*REPEATS 个字符正好是末 k 个重复 REPEATS 次
    const REPEATS: usize = 6;
    for k in 1..=16usize {
        if chars.len() < k * REPEATS {
            break;
        }
        let tail = &chars[chars.len() - k * REPEATS..];
        let unit = &tail[..k];
        if tail.chunks(k).all(|chunk| chunk == unit) {
            return Some(format!("输出结尾出现 {} 次重复片段", REPEATS));
        }
    }
    None
}

/// 产出形态校验失败的错误前缀。不带 `(<状态码>)`，所以 retry.rs 会当成可重试错误；
/// 调用方据此决定回落到 Amazon Transcribe。
pub const NOT_A_TRANSCRIPT: &str = "Bedrock 转写结果不像转写稿";

/// 连通性测试：最小的一次 Converse 调用，能拿到响应即视为凭证、region、模型都可用。
pub async fn test_connectivity(cfg: &AwsConfig, model: &str) -> Result<(), String> {
    converse(
        cfg,
        model,
        "",
        vec![ContentBlock::Text("hi".to_string())],
        &ThinkingConfig::default(),
        8,
        None,
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
    fn counts_only_plain_reasoning_text() {
        use aws_sdk_bedrockruntime::types::{ReasoningContentBlock, ReasoningTextBlock};

        let reasoning = |text: &str| {
            ContentBlock::ReasoningContent(ReasoningContentBlock::ReasoningText(
                ReasoningTextBlock::builder().text(text).build().unwrap(),
            ))
        };
        let blocks = vec![
            reasoning("先想一想"),
            ContentBlock::Text("正文".to_string()),
            reasoning("再想想"),
            ContentBlock::ReasoningContent(ReasoningContentBlock::RedactedContent(Blob::new(
                vec![0u8; 64],
            ))),
        ];
        assert_eq!(reasoning_chars(&blocks), 7);
        assert_eq!(reasoning_chars(&[ContentBlock::Text("只有正文".to_string())]), 0);
        assert_eq!(reasoning_chars(&[]), 0);
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

    // === 转写产出校验 ===

    #[test]
    fn a_normal_transcript_passes() {
        // 16 秒音频、约 40 字，正常
        let text = "有一个问题，目前这个转录的精确度还是不太够。除了 Amazon Transcribe，我们还有别的选项吗？";
        assert_eq!(transcript_defect(text, 16.0), None);
        // 很短的音频也不要误杀：下限是 40 字
        assert_eq!(transcript_defect("好的", 0.5), None);
        assert_eq!(transcript_defect("测试一下功能是否正常", 1.0), None);
    }

    #[test]
    fn answering_the_question_is_rejected_by_length() {
        // 实测抓到的真实失败：16 秒音频，模型改去介绍 AWS 服务
        let text = "是的，AWS 提供了多种实时语音识别服务，可以帮助你提高转录的准确性。".repeat(8);
        let defect = transcript_defect(&text, 16.0).expect("should be rejected");
        assert!(defect.contains("远超"), "{defect}");
    }

    #[test]
    fn markdown_structure_is_rejected() {
        let bold = transcript_defect("**Amazon Transcribe**：这是一个服务", 10.0);
        assert!(bold.unwrap().contains("粗体"));
        let heading = transcript_defect("# 服务列表\n第一项", 10.0);
        assert!(heading.unwrap().contains("标题"));
        let list = transcript_defect("以下是服务：\n1. Amazon Transcribe\n2. Voxtral", 10.0);
        assert!(list.unwrap().contains("编号列表"));
        // 口语里的「1」「第1步」不该被当成列表
        assert_eq!(transcript_defect("第1步先开会，2点再讨论", 10.0), None);
    }

    #[test]
    fn repetition_loop_is_rejected() {
        // 实测抓到的真实失败：卡进「有的，」的重复循环直到撞 max_tokens
        let text = format!("嗯，{}", "有的，".repeat(120));
        let defect = transcript_defect(&text, 600.0).expect("should be rejected");
        assert!(defect.contains("重复片段"), "{defect}");
        // 单字重复也要抓到
        let single = format!("测试{}", "啊".repeat(20));
        assert!(transcript_defect(&single, 600.0).is_some());
        // 正常句子里的重复词不该触发
        assert_eq!(transcript_defect("测试测试测试，功能正常", 10.0), None);
    }

    #[test]
    fn term_hint_keeps_latin_terms_only_and_caps_length() {
        let vocabulary = "# 词汇表\n\n## 术语\n\n- Amazon Transcribe（实际错例：amazon的）\n- Amazon Bedrock\n- 转录（不要写成转路）\n- ByeType\n- Amazon Transcribe\n普通正文行\n";
        let hint = term_hint_from_vocabulary(vocabulary);
        assert!(hint.contains("Amazon Transcribe"));
        assert!(hint.contains("Amazon Bedrock"));
        assert!(hint.contains("ByeType"));
        // 中文术语不进提示（挤占本就很小的指令预算）
        assert!(!hint.contains("转录"));
        // 括号说明要去掉，重复项只留一份
        assert!(!hint.contains("实际错例"));
        assert_eq!(hint.matches("Amazon Transcribe").count(), 1);
        assert!(!hint.contains("普通正文行"));
    }

    #[test]
    fn term_hint_is_truncated() {
        let many: String = (0..200).map(|i| format!("- LongTermName{}\n", i)).collect();
        let hint = term_hint_from_vocabulary(&many);
        assert!(hint.len() <= TERM_HINT_MAX_CHARS, "len={}", hint.len());
        assert!(hint.starts_with("LongTermName0"));
    }

    #[test]
    fn term_hint_is_empty_for_skeleton_vocabulary() {
        assert_eq!(
            term_hint_from_vocabulary("# 词汇表\n\n## 人名\n\n## 术语\n"),
            ""
        );
        assert_eq!(term_hint_from_vocabulary(""), "");
    }

    #[test]
    fn payload_guard_matches_measured_limit() {
        // 实测：1.83 MB / 60 秒通过，2.75 MB / 90 秒被拒。守卫要落在两者之间且偏保守。
        const BYTES_PER_SEC: usize = 16_000 * 2;
        let ok_60s = 60 * BYTES_PER_SEC + 44;
        let rejected_90s = 90 * BYTES_PER_SEC + 44;
        assert!(
            TRANSCRIBE_MAX_WAV_BYTES < rejected_90s,
            "守卫 {} 必须小于实测被拒的 {}",
            TRANSCRIBE_MAX_WAV_BYTES,
            rejected_90s
        );
        // 55 秒以内要放过，别把常见的短听写也挡掉
        let fifty_five = 55 * BYTES_PER_SEC + 44;
        assert!(TRANSCRIBE_MAX_WAV_BYTES >= fifty_five);
        // 60 秒刚好在边界外，符合「约 55 秒」的文档说明
        assert!(TRANSCRIBE_MAX_WAV_BYTES < ok_60s);
    }

    #[test]
    fn payload_too_large_errors_trigger_fallback() {
        assert!(is_payload_too_large(
            "Bedrock API error (400): ValidationException: Failed to buffer the request body"
        ));
        assert!(is_payload_too_large("Request body too large"));
        assert!(is_payload_too_large("Bedrock API error (413): too big"));
        // 别把普通错误也当成体积问题
        assert!(!is_payload_too_large(
            "Bedrock API error (403): AccessDeniedException: no permission"
        ));
        assert!(!is_payload_too_large("Request timed out"));
    }

    /// 真机联调：Voxtral 转写一段真实中文录音。
    /// `BYETYPE_TEST_WAV` 指定 wav 样本（默认 /tmp/byetype-test-zh.wav）。
    /// 运行：cargo test --lib live_voxtral -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "需要 AWS 凭证、网络与音频样本"]
    async fn live_voxtral_transcribe_chinese_sample() {
        let cfg = AwsConfig {
            bedrock_profile: std::env::var("BYETYPE_TEST_BEDROCK_PROFILE")
                .unwrap_or_else(|_| "bedrock".into()),
            bedrock_region: std::env::var("BYETYPE_TEST_BEDROCK_REGION")
                .unwrap_or_else(|_| "ap-northeast-1".into()),
            ..AwsConfig::default()
        };
        let wav_path =
            std::env::var("BYETYPE_TEST_WAV").unwrap_or_else(|_| "/tmp/byetype-test-zh.wav".into());
        let wav = std::fs::read(&wav_path).expect("test wav sample");
        // 走一遍管道的真实形态：先归一成内部的 FLAC，再交给 Voxtral
        let normalized = crate::audio::input::normalize_audio(
            wav,
            "audio/wav",
            600,
            &tokio_util::sync::CancellationToken::new(),
        )
        .expect("normalize to flac");
        let audio_base64 = crate::audio::encoder::audio_to_base64(&normalized.flac);

        let started = std::time::Instant::now();
        let (text, usage) = transcribe_audio(
            &cfg,
            &audio_base64,
            "mistral.voxtral-small-24b-2507",
            "Amazon, Amazon Transcribe, Amazon Bedrock, ByeType",
        )
        .await
        .expect("voxtral transcribe");
        eprintln!(
            "[live] voxtral ({} ms, in={} out={}) → {:?}",
            started.elapsed().as_millis(),
            usage.prompt_tokens,
            usage.completion_tokens,
            text
        );
        assert!(!text.trim().is_empty(), "空转写结果");
        // 产出形态校验必须放行正常结果
        assert_eq!(transcript_defect(&text, 600.0), None);
    }
}
