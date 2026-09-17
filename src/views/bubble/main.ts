import { getCurrentWindow } from '@tauri-apps/api/window'
import { invoke } from '@tauri-apps/api/core'
import { subscribe, t } from '../../i18n'
import { bootstrapLanguage } from '../../i18n/tauri'

const bubble = document.getElementById('bubble')!
const currentWindow = getCurrentWindow()
let currentTaskId: number = 0
let currentStatus: string = ''

type Look = {
  shape: 'is-round' | 'is-pill'
  color: string
  /** 标签文案的 i18n key，渲染时用 t() 解析，切语言后重渲染即可换文案 */
  labelKey?: string
  glyph?: string
  dot?: boolean
  cancel?: boolean
  preparing?: boolean
}

// 所有状态共用一个 DOM 元素，只换形状类和配色类。重建元素会重放
// bounceIn，看起来就是气泡在跳。
const looks: Record<string, Look> = {
  preparing:    { shape: 'is-round', color: 'c-recording', dot: true, preparing: true },
  recording:    { shape: 'is-round', color: 'c-recording', dot: true },
  transcribing: { shape: 'is-pill', color: 'c-thinking', labelKey: 'bubble.thinking', cancel: true },
  extracting:   { shape: 'is-pill', color: 'c-thinking', labelKey: 'bubble.thinking', cancel: true },
  optimizing:   { shape: 'is-pill', color: 'c-optimizing', labelKey: 'bubble.thinking', cancel: true },
  retrying:     { shape: 'is-pill', color: 'c-retrying', labelKey: 'bubble.thinking', cancel: true },
  completed:    { shape: 'is-round', color: 'c-completed', glyph: '✓' },
  failed:       { shape: 'is-round', color: 'c-failed', glyph: '✕' },
  // 会议记录（独立的 bubble-meeting 窗口）：✕ 是「停止录制」而不是取消任务
  'meeting-detected':    { shape: 'is-pill', color: 'c-recording', labelKey: 'bubble.meetingRecording', dot: true, cancel: true },
  'meeting-summarizing': { shape: 'is-pill', color: 'c-thinking', labelKey: 'bubble.summarizing' },
  'meeting-done':        { shape: 'is-round', color: 'c-completed', glyph: '✓' },
  'meeting-failed':      { shape: 'is-round', color: 'c-failed', glyph: '✕' },
}

function classFor(look: Look): string {
  return `shape ${look.shape} ${look.color}${look.preparing ? ' is-preparing' : ''}`
}

function setPart(el: HTMLElement, selector: string, visible: boolean, text?: string) {
  const node = el.querySelector(selector) as HTMLElement
  node.style.display = visible ? 'inline-flex' : 'none'
  if (text !== undefined) node.textContent = text
}

function applyLook(el: HTMLElement, look: Look) {
  el.className = classFor(look)
  setPart(el, '.dot', !!look.dot)
  setPart(el, '.label', !!look.labelKey, look.labelKey ? t(look.labelKey) : '')
  setPart(el, '.glyph', !!look.glyph, look.glyph ?? '')
  setPart(el, '.cancel-btn', !!look.cancel)
}

function render(status: string) {
  const look = looks[status]
  if (!look) return

  const existing = bubble.querySelector('.shape') as HTMLElement | null
  if (existing) {
    applyLook(existing, look)
    return
  }

  // 首次创建：先在离屏节点上把类和内容设好，再挂到文档里。这样第一帧
  // 就是最终尺寸，不会从内容自然宽度过渡到 34px（那会看着像缺了半截）。
  const el = document.createElement('div')
  el.innerHTML =
    `<span class="dot"></span><span class="label"></span>` +
    `<span class="glyph"></span><span class="cancel-btn">✕</span>`
  applyLook(el, look)
  el.querySelector('.cancel-btn')!.addEventListener('mousedown', (e) => {
    e.preventDefault()
    e.stopPropagation()
    if (currentStatus.startsWith('meeting')) {
      invoke('meeting_stop').catch((err) => console.error('meeting_stop failed:', err))
      return
    }
    invoke('cancel_task', { taskId: currentTaskId }).catch((err) => console.error('cancel_task failed:', err))
  })
  bubble.appendChild(el)
}

// 语言跟随配置；切换语言时若气泡正显示着，就地重渲染换文案
bootstrapLanguage()
subscribe(() => {
  if (currentStatus) render(currentStatus)
})

// Window-scoped listeners — each bubble only receives events targeted to it
currentWindow.listen('clear-bubble', () => {
  bubble.innerHTML = ''
  currentTaskId = 0
  currentStatus = ''
})

currentWindow.listen<{ taskNumber: number; status: string }>('show-bubble', (event) => {
  const { taskNumber, status } = event.payload
  currentTaskId = taskNumber
  currentStatus = status
  render(status)
})

currentWindow.listen<{ taskNumber: number; status: string }>('update-bubble', (event) => {
  const { taskNumber, status } = event.payload
  currentTaskId = taskNumber
  currentStatus = status
  render(status)
})
