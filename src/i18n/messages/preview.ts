import type { MessageBundle } from '../types'

// 区域：preview。key 统一用 'preview.xxx' 前缀；zh 与 en 的 key 必须一一对应。
export const preview: MessageBundle = {
  zh: {
    'preview.title': '识别结果',
    'preview.pin': '固定窗口',
    'preview.unpin': '取消固定',
    'preview.copied': '已复制',
  },
  en: {
    'preview.title': 'Transcription',
    'preview.pin': 'Pin window',
    'preview.unpin': 'Unpin',
    'preview.copied': 'Copied',
  },
}
