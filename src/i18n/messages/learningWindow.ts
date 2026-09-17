import type { MessageBundle } from '../types'

// 区域：learningWindow。key 统一用 'learningWindow.xxx' 前缀；zh 与 en 的 key 必须一一对应。
export const learningWindow: MessageBundle = {
  zh: {
    'learningWindow.title': '自动学习',
    'learningWindow.heading': '确认学习内容',
    'learningWindow.subtitle': '先核对前两栏，再开始学习。只有右栏会录入系统。',

    'learningWindow.panel.original.label': '原始输出',
    'learningWindow.panel.original.helper': '最近一次语音转写结果',
    'learningWindow.panel.corrected.label': '用户修订／新增要求',
    'learningWindow.panel.corrected.helper': '可粘贴修订文本，也可直接输入要学习的词汇或规则',
    'learningWindow.panel.corrected.placeholder': '例如：新增词汇：ByeType',
    'learningWindow.panel.generated.label': '学习结果',
    'learningWindow.panel.generated.helper': '每行一项，只有这里会录入系统',
    'learningWindow.panel.willSave': '将录入',

    'learningWindow.generating.title': 'Learning…',
    'learningWindow.generating.desc': 'AI正在分析并生成学习内容…',

    'learningWindow.clear': '清空',
    'learningWindow.learning': '正在学习…',
    'learningWindow.regenerate': '重新生成',
    'learningWindow.startLearning': '开始学习',
    'learningWindow.applying': '正在录入…',
    'learningWindow.applied': '已录入',
    'learningWindow.apply': '确认录入',

    'learningWindow.notice.loading': '正在读取学习结果…',
    'learningWindow.notice.noDraft': '暂无学习草稿，请从托盘菜单重新开始。',
    'learningWindow.notice.added': '已录入{count}条学习内容。',
    'learningWindow.notice.nothingAdded': '没有录入新内容，右栏内容已存在。',
    'learningWindow.notice.cleared': '当前三栏已清空，已保存的学习记录不受影响。',
  },
  en: {
    'learningWindow.title': 'Voice learning',
    'learningWindow.heading': 'Confirm learning content',
    'learningWindow.subtitle': 'Review the first two columns, then start learning. Only the right column is saved.',

    'learningWindow.panel.original.label': 'Original output',
    'learningWindow.panel.original.helper': 'Latest voice transcription result',
    'learningWindow.panel.corrected.label': 'Your corrections / new rules',
    'learningWindow.panel.corrected.helper': 'Paste the corrected text, or type the words or rules to learn',
    'learningWindow.panel.corrected.placeholder': 'e.g. New term: ByeType',
    'learningWindow.panel.generated.label': 'Learning result',
    'learningWindow.panel.generated.helper': 'One item per line; only this column is saved',
    'learningWindow.panel.willSave': 'Will be saved',

    'learningWindow.generating.title': 'Learning…',
    'learningWindow.generating.desc': 'AI is analyzing and generating learning content…',

    'learningWindow.clear': 'Clear',
    'learningWindow.learning': 'Learning…',
    'learningWindow.regenerate': 'Regenerate',
    'learningWindow.startLearning': 'Start learning',
    'learningWindow.applying': 'Saving…',
    'learningWindow.applied': 'Saved',
    'learningWindow.apply': 'Confirm and save',

    'learningWindow.notice.loading': 'Loading learning result…',
    'learningWindow.notice.noDraft': 'No learning draft. Start again from the tray menu.',
    'learningWindow.notice.added': 'Saved {count} learning items.',
    'learningWindow.notice.nothingAdded': 'Nothing new to save; the right column already exists.',
    'learningWindow.notice.cleared': 'All three columns cleared; saved learning records are unaffected.',
  },
}
