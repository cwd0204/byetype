/// 一次模型调用的 Token 用量。服务商未返回时保持 0，调用次数仍计入统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// 推理块的字符数。Bedrock 不单独返回推理 token 数，只能在过滤
    /// `reasoningContent` 块时数一下长度；0 表示没有推理块（或服务商不返回）。
    pub reasoning_chars: u64,
}
