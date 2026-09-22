import type { AppConfig } from './types'

/** 只有 AWS 两个协议：Bedrock 做文本 / 图像，Transcribe 做语音 */
export type ModelProtocol = 'bedrock' | 'aws-transcribe'

export interface ModelEntry {
  id: string
  provider: string
  model: string
  protocol: ModelProtocol
  builtin: boolean
  supportsAudio: boolean
  supportsText: boolean
  supportsVision: boolean
}

/** 与 src-tauri/src/ai/models.rs 的 BUILTIN_MODELS 是两份手动镜像，改一处必须同步另一处 */
export const BUILTIN_MODELS: ModelEntry[] = [
  {
    id: 'builtin-bedrock-claude-sonnet-5',
    provider: 'Amazon Bedrock',
    model: 'global.anthropic.claude-sonnet-5',
    protocol: 'bedrock',
    builtin: true,
    supportsAudio: false,
    supportsText: true,
    supportsVision: true,
  },
  {
    id: 'builtin-bedrock-claude-opus-5',
    provider: 'Amazon Bedrock',
    model: 'global.anthropic.claude-opus-5',
    protocol: 'bedrock',
    builtin: true,
    supportsAudio: false,
    supportsText: true,
    supportsVision: true,
  },
  {
    id: 'builtin-bedrock-claude-haiku-4-5',
    provider: 'Amazon Bedrock',
    model: 'global.anthropic.claude-haiku-4-5-20251001-v1:0',
    protocol: 'bedrock',
    builtin: true,
    supportsAudio: false,
    supportsText: true,
    supportsVision: true,
  },
  {
    id: 'builtin-aws-transcribe',
    provider: 'Amazon Transcribe',
    model: 'streaming',
    protocol: 'aws-transcribe',
    builtin: true,
    supportsAudio: true,
    supportsText: false,
    supportsVision: false,
  },
  // 高准确度转写：协议是 bedrock 但只吃音频（唯一一个这种组合），
  // 不支持流式输入，录完整段再传。术语识别比 Transcribe 明显更准。
  {
    id: 'builtin-bedrock-voxtral',
    provider: 'Amazon Bedrock',
    model: 'mistral.voxtral-small-24b-2507',
    protocol: 'bedrock',
    builtin: true,
    supportsAudio: true,
    supportsText: false,
    supportsVision: false,
  },
]

export const DEFAULT_TRANSCRIBE_MODEL = 'builtin-aws-transcribe'
export const DEFAULT_TEXT_MODEL = 'builtin-bedrock-claude-sonnet-5'

export function getAllModels(config: AppConfig): ModelEntry[] {
  const customs: ModelEntry[] = config.models.custom.map(c => ({
    id: c.id,
    provider: c.provider || 'Amazon Bedrock',
    model: c.model,
    protocol: 'bedrock',
    builtin: false,
    supportsAudio: false,
    supportsText: c.supportsText ?? true,
    supportsVision: c.supportsVision ?? true,
  }))
  return [...BUILTIN_MODELS, ...customs]
}

export function getAudioModels(config: AppConfig): ModelEntry[] {
  return getAllModels(config).filter(m => m.supportsAudio)
}

export function getTextModels(config: AppConfig): ModelEntry[] {
  return getAllModels(config).filter(m => m.supportsText)
}

export function getVisionModels(config: AppConfig): ModelEntry[] {
  return getAllModels(config).filter(m => m.supportsVision)
}

export function findModel(config: AppConfig, modelId: string): ModelEntry | undefined {
  return getAllModels(config).find(m => m.id === modelId)
}
