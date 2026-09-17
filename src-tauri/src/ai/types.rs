/// 一次模型调用的 Token 用量。服务商未返回时保持 0，调用次数仍计入统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}
