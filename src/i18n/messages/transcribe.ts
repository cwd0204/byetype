import type { MessageBundle } from '../types'

// 区域：transcribe。key 统一用 'transcribe.xxx' 前缀；zh 与 en 的 key 必须一一对应。
export const transcribe: MessageBundle = {
  zh: {
    'transcribe.title': '转写设置',
    'transcribe.group.model': '模型',
    'transcribe.model': '转写模型',
    'transcribe.modelDesc': 'Amazon Transcribe 流式识别；识别语言在「模型管理 → Amazon Transcribe」里设置',
    'transcribe.optimizeSection': '文本优化模型',
    'transcribe.optimizeModel': '处理模型',
    'transcribe.optimizeModelDesc': '转写后的文本优化处理使用此模型',
    'transcribe.thinkingDesc': 'Claude 扩展思考，会增加延迟与用量',
    'transcribe.otherSection': '其他',
    'transcribe.ruleBoost': '规则增强',
    'transcribe.ruleBoostDesc': 'Amazon Transcribe 没有提示词能力，转录规则、专有词汇和学习结果会始终在文本优化阶段注入，由 Claude 统一纠错',
  },
  en: {
    'transcribe.title': 'Transcription',
    'transcribe.group.model': 'Model',
    'transcribe.model': 'Transcription model',
    'transcribe.modelDesc': 'Amazon Transcribe streaming; set the recognition language under Models → Amazon Transcribe',
    'transcribe.optimizeSection': 'Text optimization model',
    'transcribe.optimizeModel': 'Processing model',
    'transcribe.optimizeModelDesc': 'Model used to optimize the text after transcription',
    'transcribe.thinkingDesc': 'Claude extended thinking; adds latency and token usage',
    'transcribe.otherSection': 'Other',
    'transcribe.ruleBoost': 'Rule injection',
    'transcribe.ruleBoostDesc': 'Amazon Transcribe has no prompt support, so transcription rules, vocabulary, and learned corrections are always injected during text optimization and applied by Claude',
  },
}
