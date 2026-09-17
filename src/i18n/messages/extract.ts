import type { MessageBundle } from '../types'

// 区域：extract。key 统一用 'extract.xxx' 前缀；zh 与 en 的 key 必须一一对应。
export const extract: MessageBundle = {
  zh: {
    'extract.title': '图像识别设置',
    'extract.group.model': '模型',
    'extract.model': '图像识别模型',
  },
  en: {
    'extract.title': 'Image recognition',
    'extract.group.model': 'Model',
    'extract.model': 'Image recognition model',
  },
}
