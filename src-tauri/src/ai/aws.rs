//! AWS SDK 公共层：按 (profile, region) 缓存 `SdkConfig`，并把 SDK 错误统一成
//! `retry.rs` 能解析的文案格式 `"<Label> API error (<status>): <code>: <message>"`。
//!
//! 凭证完全交给 SDK 默认凭证链（~/.aws/config 里的 profile、credential_process（ADA）、
//! 静态 key、SSO），app 只保存 profile 名与 region，不接触任何密钥。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use aws_config::{BehaviorVersion, Region, SdkConfig};
use aws_sdk_bedrockruntime::config::http::HttpResponse;
use aws_sdk_bedrockruntime::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};

const DEFAULT_PROFILE: &str = "default";
const DEFAULT_REGION: &str = "us-east-1";

fn cache() -> &'static Mutex<HashMap<(String, String), SdkConfig>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, String), SdkConfig>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 空值回退到 SDK 约定的默认 profile / region。
pub fn normalize(profile: &str, region: &str) -> (String, String) {
    let profile = profile.trim();
    let region = region.trim();
    (
        if profile.is_empty() {
            DEFAULT_PROFILE
        } else {
            profile
        }
        .to_string(),
        if region.is_empty() {
            DEFAULT_REGION
        } else {
            region
        }
        .to_string(),
    )
}

/// 取（或构建并缓存）指定 profile / region 的 SdkConfig。
/// SdkConfig 内部的凭证提供器会自己刷新 credential_process 返回的短期凭证，
/// 所以缓存本身不需要过期；改了 profile/region 就是新 key。
pub async fn sdk_config(profile: &str, region: &str) -> Result<SdkConfig, String> {
    let key = normalize(profile, region);
    if let Some(cfg) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Ok(cfg.clone());
    }
    let cfg = aws_config::defaults(BehaviorVersion::latest())
        .profile_name(&key.0)
        .region(Region::new(key.1.clone()))
        .load()
        .await;
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, cfg.clone());
    Ok(cfg)
}

/// 把 SDK 错误转成统一文案。
/// - 服务端错误带 HTTP 状态码，格式与其他 provider 一致，`retry.rs` 据此判断 4xx 不重试
/// - 凭证 / 网络等本地失败没有状态码，走 `"<Label> request failed: ..."`
/// - 凭证类问题追加 mwinit 提示（ADA 的短期凭证依赖 Midway 登录）
pub fn map_sdk_error<E>(label: &str, err: SdkError<E, HttpResponse>) -> String
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    match &err {
        SdkError::ServiceError(ctx) => {
            let status = ctx.raw().status().as_u16();
            let code = ctx.err().code().unwrap_or("UnknownError");
            let message = ctx.err().message().unwrap_or("");
            with_credential_hint(format!(
                "{} API error ({}): {}: {}",
                label, status, code, message
            ))
        }
        other => with_credential_hint(format!(
            "{} request failed: {}",
            label,
            DisplayErrorContext(other)
        )),
    }
}

/// 凭证相关的失败追加可操作的提示。
pub fn with_credential_hint(message: String) -> String {
    let lower = message.to_lowercase();
    let credential_issue = [
        "credential",
        "midway",
        "expired",
        "unrecognizedclient",
        "invalidsignature",
        "no providers in chain",
        "security token",
        "mwinit",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if credential_issue && !message.contains("mwinit -o") {
        format!(
            "{}（AWS 凭证不可用：请在终端运行 mwinit -o 刷新 Midway，并确认 profile 名称正确）",
            message
        )
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_falls_back_to_sdk_defaults() {
        assert_eq!(
            normalize("", "  "),
            ("default".to_string(), "us-east-1".to_string())
        );
        assert_eq!(
            normalize(" bedrock ", "ap-northeast-1"),
            ("bedrock".to_string(), "ap-northeast-1".to_string())
        );
    }

    #[test]
    fn credential_failures_get_mwinit_hint() {
        let hinted = with_credential_hint("Bedrock request failed: no credentials".to_string());
        assert!(hinted.contains("mwinit -o"));
        assert!(hinted.starts_with("Bedrock request failed: no credentials"));
    }

    #[test]
    fn non_credential_errors_are_left_alone() {
        let msg = "Bedrock API error (429): ThrottlingException: slow down".to_string();
        assert_eq!(with_credential_hint(msg.clone()), msg);
    }

    #[test]
    fn hint_is_not_duplicated() {
        let once = with_credential_hint("credential expired".to_string());
        let twice = with_credential_hint(once.clone());
        assert_eq!(once, twice);
    }
}
