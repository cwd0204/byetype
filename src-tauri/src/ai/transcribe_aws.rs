//! Amazon Transcribe 流式转写。
//!
//! 输入是 app 内部统一的 16 kHz 单声道 FLAC；Transcribe 的多语言识别只接受 PCM，
//! 所以先解码成 i16 PCM，再切成 ≤16 KiB 的事件一次性灌进去，只收 `is_partial == false`
//! 的最终结果。它没有提示词能力，专有词 / 规则 / 学习结果的纠错由文本优化阶段补上
//! （见 prompt.rs `build_optimize_prompt`）。

use aws_sdk_transcribestreaming::error::{DisplayErrorContext, ProvideErrorMetadata};
use aws_sdk_transcribestreaming::operation::start_stream_transcription::builders::StartStreamTranscriptionFluentBuilder;
use aws_sdk_transcribestreaming::operation::start_stream_transcription::StartStreamTranscriptionOutput;
use aws_sdk_transcribestreaming::primitives::Blob;
use aws_sdk_transcribestreaming::types::error::AudioStreamError;
use aws_sdk_transcribestreaming::types::{
    AudioEvent, AudioStream, Item, ItemType, LanguageCode, MediaEncoding,
    Result as TranscriptResult, TranscriptResultStream,
};
use aws_sdk_transcribestreaming::Client;

use super::aws;
use crate::config::types::AwsConfig;
use crate::i18n::{self, tr_fmt, tr_fmt_in, tr_in, Lang};

pub(crate) const LABEL: &str = "Transcribe";
/// 单个音频事件上限 32 KiB，取一半留余量。
const AUDIO_EVENT_BYTES: usize = 16 * 1024;
pub(crate) const SAMPLE_RATE_HZ: i32 = 16_000;
/// "auto" 模式下的候选语言与首选语言。
const AUTO_LANGUAGE_OPTIONS: &str = "zh-CN,en-US";

#[derive(Debug, Clone, Copy, Default)]
pub struct TranscribeOptions {
    /// 开启说话人分离，输出按「说话人N：」/「Speaker N: 」分行（会议模式用）。
    pub speaker_labels: bool,
}

fn audio_events(bytes: &[u8]) -> Vec<Result<AudioStream, AudioStreamError>> {
    bytes
        .chunks(AUDIO_EVENT_BYTES)
        .map(|chunk| {
            Ok(AudioStream::AudioEvent(
                AudioEvent::builder()
                    .audio_chunk(Blob::new(chunk.to_vec()))
                    .build(),
            ))
        })
        .collect()
}

/// 事件流里的服务端异常没有 HTTP 状态，按 Transcribe 文档的异常 → 状态码映射补上，
/// 让 retry.rs 能正确判断 4xx 不重试。
fn stream_error_status(code: &str) -> u16 {
    match code {
        "BadRequestException" => 400,
        "ConflictException" => 409,
        "LimitExceededException" => 429,
        "ServiceUnavailableException" => 503,
        _ => 500,
    }
}

pub(crate) fn format_stream_error<E>(err: &E) -> String
where
    E: ProvideErrorMetadata + std::error::Error,
{
    match err.code() {
        Some(code) => aws::with_credential_hint(format!(
            "{} API error ({}): {}: {}",
            LABEL,
            stream_error_status(code),
            code,
            err.message().unwrap_or("")
        )),
        None => aws::with_credential_hint(format!(
            "{} stream error: {}",
            LABEL,
            DisplayErrorContext(err)
        )),
    }
}

/// 把带说话人标签的 items 拼成「说话人N：…」（英文「Speaker N: …」）行。
/// Transcribe 的 item 是词级（中文按字/词），英文词之间要补空格，标点直接贴上。
/// 前缀与分隔符按 `lang` 取，和 `merge_speaker_lines` 必须用同一语言。
pub(crate) fn render_speaker_lines(items: &[Item], lang: Lang) -> String {
    let separator = tr_in(lang, "speaker.separator");
    let mut lines: Vec<(String, String)> = Vec::new();
    for item in items {
        let content = item.content().unwrap_or("");
        if content.is_empty() {
            continue;
        }
        let speaker = item
            .speaker()
            .map(|s| speaker_label(lang, s))
            .unwrap_or_else(|| tr_in(lang, "speaker.unknown").to_string());
        let is_punct = item.r#type() == Some(&ItemType::Punctuation);

        match lines.last_mut() {
            Some((last_speaker, text)) if *last_speaker == speaker || is_punct => {
                if !is_punct && needs_space(text, content) {
                    text.push(' ');
                }
                text.push_str(content);
            }
            _ => lines.push((speaker, content.to_string())),
        }
    }
    lines
        .into_iter()
        .map(|(speaker, text)| format!("{}{}{}", speaker, separator, text.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 「说话人N」/「Speaker N」。
fn speaker_label(lang: Lang, raw_label: &str) -> String {
    tr_fmt_in(lang, "speaker.label", &[("n", speaker_number(raw_label).as_str())])
}

/// Transcribe 的说话人标签从 0 计数，展示时从 1 开始。
fn speaker_number(label: &str) -> String {
    label
        .trim()
        .parse::<u32>()
        .map(|n| (n + 1).to_string())
        .unwrap_or_else(|_| label.trim().to_string())
}

/// 英文词之间补空格；中文与标点不补。
/// 英文词之间、英文句末标点后补空格；中文与中文标点不补。
fn needs_space(prev: &str, next: &str) -> bool {
    let (Some(last), Some(first)) = (prev.chars().last(), next.chars().next()) else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    last.is_ascii_alphanumeric() || matches!(last, '.' | ',' | '!' | '?' | ';' | ':')
}

pub(crate) fn collect_result(
    result: &TranscriptResult,
    speaker_labels: bool,
    lang: Lang,
    out: &mut Vec<String>,
) {
    if result.is_partial() {
        return;
    }
    let Some(alternative) = result.alternatives().first() else {
        return;
    };
    let text = if speaker_labels {
        render_speaker_lines(alternative.items(), lang)
    } else {
        alternative.transcript().unwrap_or("").trim().to_string()
    };
    if !text.is_empty() {
        out.push(text);
    }
}

/// 合并相邻同一说话人的行，避免每个 result 都重复「说话人1：」前缀。
/// 按 `lang` 的分隔符（中文「：」/ 英文「: 」）切出说话人，要与 `render_speaker_lines` 一致。
pub(crate) fn merge_speaker_lines(blocks: &[String], lang: Lang) -> String {
    let separator = tr_in(lang, "speaker.separator");
    let mut merged: Vec<String> = Vec::new();
    for block in blocks {
        for line in block.lines() {
            let Some((speaker, text)) = line.split_once(separator) else {
                merged.push(line.to_string());
                continue;
            };
            match merged.last_mut() {
                Some(last) if last.starts_with(&format!("{}{}", speaker, separator)) => {
                    if needs_space(last, text) {
                        last.push(' ');
                    }
                    last.push_str(text);
                }
                _ => merged.push(line.to_string()),
            }
        }
    }
    merged.join("\n")
}

/// 按配置挑语言：`auto` 走多语言识别（首选中文），否则锁定单一语言码。
/// 整段路径与流式路径必须用同一套规则，所以抽在这里共用。
pub(crate) fn apply_language(
    request: StartStreamTranscriptionFluentBuilder,
    cfg: &AwsConfig,
) -> StartStreamTranscriptionFluentBuilder {
    let language = cfg.transcribe_language.trim();
    if language.is_empty() || language.eq_ignore_ascii_case("auto") {
        request
            .identify_multiple_languages(true)
            .language_options(AUTO_LANGUAGE_OPTIONS)
            .preferred_language(LanguageCode::ZhCn)
    } else {
        request.language_code(LanguageCode::from(language))
    }
}

/// 读完事件流，把 `is_partial == false` 的最终结果拼成文本。
/// 整段路径与流式路径共用，两边的输出格式因此保证一致。
pub(crate) async fn collect_transcript(
    response: &mut StartStreamTranscriptionOutput,
    opts: TranscribeOptions,
    lang: Lang,
) -> Result<String, String> {
    let mut blocks = Vec::new();
    loop {
        let event = response
            .transcript_result_stream
            .recv()
            .await
            .map_err(|e| format_stream_error(&e))?;
        let Some(event) = event else { break };
        if let TranscriptResultStream::TranscriptEvent(transcript_event) = event {
            if let Some(transcript) = transcript_event.transcript() {
                for result in transcript.results() {
                    collect_result(result, opts.speaker_labels, lang, &mut blocks);
                }
            }
        }
    }

    if opts.speaker_labels {
        Ok(merge_speaker_lines(&blocks, lang))
    } else {
        Ok(blocks.join("\n"))
    }
}

async fn run_stream(
    cfg: &AwsConfig,
    bytes: Vec<u8>,
    encoding: MediaEncoding,
    opts: TranscribeOptions,
    lang: Lang,
) -> Result<String, String> {
    let sdk = aws::sdk_config(&cfg.transcribe_profile, &cfg.transcribe_region).await?;
    let client = Client::new(&sdk);

    let input = futures_util::stream::iter(audio_events(&bytes));
    let request = client
        .start_stream_transcription()
        .media_encoding(encoding)
        .media_sample_rate_hertz(SAMPLE_RATE_HZ)
        .show_speaker_label(opts.speaker_labels)
        .audio_stream(input.into());

    let mut response = apply_language(request, cfg)
        .send()
        .await
        .map_err(|e| aws::map_sdk_error(LABEL, e))?;

    collect_transcript(&mut response, opts, lang).await
}

/// 单次转写的音频时长上限（会议分段最长 5.5 分钟，听写最长 3 分钟，这里只是兜底）。
const MAX_AUDIO_SECONDS: u32 = 4 * 3600;

/// 把 16 kHz 单声道 FLAC 解成 PCM 16-bit little-endian 字节流。
pub(crate) fn flac_to_pcm_bytes(flac_bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    let pcm = crate::audio::input::decode_to_pcm16(
        flac_bytes,
        "audio/flac",
        MAX_AUDIO_SECONDS,
        &tokio_util::sync::CancellationToken::new(),
    )
    .map_err(|e| tr_fmt("err.audioDecode", &[("label", LABEL), ("error", e.as_str())]))?;
    let mut bytes = Vec::with_capacity(pcm.len() * 2);
    for sample in pcm {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(bytes)
}

/// 转写一段 16 kHz 单声道 FLAC（内部解成 PCM 再发送）。
pub async fn transcribe(
    cfg: &AwsConfig,
    flac_bytes: Vec<u8>,
    opts: TranscribeOptions,
) -> Result<String, String> {
    if flac_bytes.is_empty() {
        return Err(tr_fmt("err.audioEmpty", &[("label", LABEL)]));
    }
    let pcm = flac_to_pcm_bytes(flac_bytes)?;
    // 只在公开入口读一次当前语言，内部函数都显式传 lang，方便单测指定
    run_stream(cfg, pcm, MediaEncoding::Pcm, opts, i18n::current()).await
}

/// 连通性测试：送 0.5 秒静音 PCM，能正常走完流即视为凭证、region、权限都可用。
pub async fn test_connectivity(cfg: &AwsConfig) -> Result<(), String> {
    let silence = vec![0u8; (SAMPLE_RATE_HZ as usize) * 2 / 2];
    run_stream(
        cfg,
        silence,
        MediaEncoding::Pcm,
        TranscribeOptions::default(),
        i18n::current(),
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str, speaker: Option<&str>, punct: bool) -> Item {
        let mut builder = Item::builder().content(content).r#type(if punct {
            ItemType::Punctuation
        } else {
            ItemType::Pronunciation
        });
        if let Some(s) = speaker {
            builder = builder.speaker(s);
        }
        builder.build()
    }

    #[test]
    fn splits_audio_into_bounded_events() {
        let bytes = vec![1u8; AUDIO_EVENT_BYTES * 2 + 10];
        let events = audio_events(&bytes);
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn renders_speaker_lines_with_spacing_rules() {
        let items = vec![
            item("我们", Some("0"), false),
            item("today", Some("0"), false),
            item("test", Some("0"), false),
            item("。", Some("0"), true),
            item("好的", Some("1"), false),
            item("。", Some("1"), true),
        ];
        assert_eq!(
            render_speaker_lines(&items, Lang::ZhCn),
            "说话人1：我们today test。\n说话人2：好的。"
        );
        assert_eq!(
            render_speaker_lines(&items, Lang::En),
            "Speaker 1: 我们today test。\nSpeaker 2: 好的。"
        );
    }

    #[test]
    fn unknown_speaker_gets_plain_prefix() {
        let items = vec![item("hello", None, false)];
        assert_eq!(render_speaker_lines(&items, Lang::ZhCn), "说话人：hello");
        assert_eq!(render_speaker_lines(&items, Lang::En), "Speaker: hello");
    }

    #[test]
    fn merges_adjacent_lines_of_same_speaker() {
        let blocks = vec![
            "说话人1：第一句。".to_string(),
            "说话人1：第二句。\n说话人2：回应。".to_string(),
            "说话人2：继续。".to_string(),
        ];
        assert_eq!(
            merge_speaker_lines(&blocks, Lang::ZhCn),
            "说话人1：第一句。第二句。\n说话人2：回应。继续。"
        );
    }

    #[test]
    fn spaces_english_sentences_but_not_chinese() {
        assert!(needs_space("first.", "Second"));
        assert!(needs_space("hello", "world"));
        assert!(!needs_space("你好。", "再见"));
        assert!(!needs_space("hello", "."));
        assert!(!needs_space("你好", "world"));
    }

    #[test]
    fn merges_english_speaker_lines_on_ascii_separator() {
        let blocks = vec![
            "Speaker 1: first".to_string(),
            "Speaker 1: second\nSpeaker 2: reply: yes".to_string(),
            "Speaker 2: more".to_string(),
        ];
        assert_eq!(
            merge_speaker_lines(&blocks, Lang::En),
            "Speaker 1: first second\nSpeaker 2: reply: yes more"
        );
        // 中文分隔符的行在英文模式下不会被误切
        let zh = vec!["说话人1：你好".to_string()];
        assert_eq!(merge_speaker_lines(&zh, Lang::En), "说话人1：你好");
    }

    /// 真机联调：需要本机 AWS profile 有 Transcribe 权限，以及一段 16 kHz 单声道 WAV。
    /// 样本可用 `say -v Tingting "…" -o t.aiff && afconvert -f WAVE -d LEI16@16000 -c 1 t.aiff t.wav` 生成，
    /// 路径由 `BYETYPE_TEST_WAV` 指定（默认 /tmp/byetype-test-zh.wav）。
    /// 运行：cargo test --lib live_transcribe -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "需要 AWS 凭证、网络与音频样本"]
    async fn live_transcribe_chinese_sample() {
        let mut cfg = AwsConfig {
            transcribe_profile: std::env::var("BYETYPE_TEST_TRANSCRIBE_PROFILE")
                .unwrap_or_else(|_| "bedrock-kuut".into()),
            transcribe_region: std::env::var("BYETYPE_TEST_TRANSCRIBE_REGION")
                .unwrap_or_else(|_| "us-east-1".into()),
            ..AwsConfig::default()
        };
        // 用来对比「auto 多语言识别」与固定单一语言（如 zh-CN）在中英混合语料上的差异
        if let Ok(language) = std::env::var("BYETYPE_TEST_TRANSCRIBE_LANGUAGE") {
            cfg.transcribe_language = language;
        }

        test_connectivity(&cfg).await.expect("connectivity");

        let wav_path =
            std::env::var("BYETYPE_TEST_WAV").unwrap_or_else(|_| "/tmp/byetype-test-zh.wav".into());
        let wav = std::fs::read(&wav_path).expect("test wav sample");
        let normalized = crate::audio::input::normalize_audio(
            wav,
            "audio/wav",
            60,
            &tokio_util::sync::CancellationToken::new(),
        )
        .expect("normalize to flac");

        let started = std::time::Instant::now();
        let text = transcribe(&cfg, normalized.flac.clone(), TranscribeOptions::default())
            .await
            .expect("transcribe");
        eprintln!(
            "[live] transcribe ({} ms) → {:?}",
            started.elapsed().as_millis(),
            text
        );
        assert!(
            text.contains("三点") || text.contains("开会"),
            "unexpected transcript: {text}"
        );

        let labelled = transcribe(
            &cfg,
            normalized.flac,
            TranscribeOptions {
                speaker_labels: true,
            },
        )
        .await
        .expect("transcribe with speaker labels");
        eprintln!("[live] transcribe(speaker labels) → {:?}", labelled);
        assert!(
            labelled.starts_with(tr_in(i18n::current(), "speaker.unknown")),
            "expected speaker prefix: {labelled}"
        );
    }

    #[test]
    fn maps_stream_exception_codes_to_http_status() {
        assert_eq!(stream_error_status("BadRequestException"), 400);
        assert_eq!(stream_error_status("LimitExceededException"), 429);
        assert_eq!(stream_error_status("InternalFailureException"), 500);
    }
}
