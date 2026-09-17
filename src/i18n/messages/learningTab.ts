import type { MessageBundle } from '../types'

// 区域：learningTab。key 统一用 'learningTab.xxx' 前缀；zh 与 en 的 key 必须一一对应。
export const learningTab: MessageBundle = {
  zh: {
    'learningTab.file.prompt': '学习提示词',
    'learningTab.file.result': '学习结果',
    'learningTab.title': '自动学习',
    'learningTab.group.model': '模型',
    'learningTab.model': '学习模型',
    'learningTab.modelDesc': '用于对比原始转写与用户修改文本，并归纳纠错规则',
    'learningTab.thinkingDesc': '让模型在归纳纠错规则前先进行推理',
    'learningTab.docsSection': '学习文档',
  },
  en: {
    'learningTab.file.prompt': 'Learning prompt',
    'learningTab.file.result': 'Learned rules',
    'learningTab.title': 'Auto learning',
    'learningTab.group.model': 'Model',
    'learningTab.model': 'Learning model',
    'learningTab.modelDesc': 'Compares the raw transcript with your edits and derives correction rules',
    'learningTab.thinkingDesc': 'Let the model reason before deriving correction rules',
    'learningTab.docsSection': 'Learning documents',
  },
}
