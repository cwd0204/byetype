import type { MessageBundle } from '../types'

// 区域：bubble。key 统一用 'bubble.xxx' 前缀；zh 与 en 的 key 必须一一对应。
// 药丸只有 130px 宽（还要放圆点和 ✕），标签务必短。
export const bubble: MessageBundle = {
  zh: {
    'bubble.thinking': 'Thinking...',
    'bubble.meetingRecording': '会议录制中',
    'bubble.meetingWarning': '转写有失败',
    'bubble.summarizing': 'Summarizing...',
  },
  en: {
    'bubble.thinking': 'Thinking...',
    'bubble.meetingRecording': 'Meeting',
    'bubble.meetingWarning': 'Transcription issue',
    'bubble.summarizing': 'Summarizing...',
  },
}
