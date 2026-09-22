import { getCurrentWindow } from '@tauri-apps/api/window'
import { invoke } from '@tauri-apps/api/core'
import { subscribe, t } from '../../i18n'
import { bootstrapLanguage } from '../../i18n/tauri'

const bubble = document.getElementById('bubble')!
const currentWindow = getCurrentWindow()
let currentTaskId: number = 0
let currentStatus: string = ''

type Look = {
  shape: 'is-round' | 'is-pill' | 'is-wave'
  color: string
  /** 标签文案的 i18n key，渲染时用 t() 解析，切语言后重渲染即可换文案 */
  labelKey?: string
  glyph?: string
  dot?: boolean
  cancel?: boolean
  preparing?: boolean
  /** 显示麦克风电平波形（录音相关状态） */
  wave?: boolean
}

// 所有状态共用一个 DOM 元素，只换形状类和配色类。重建元素会重放
// bounceIn，看起来就是气泡在跳。
const looks: Record<string, Look> = {
  // 准备中与录音中都是波形药丸：宽度不变就没有 34px→130px 的过渡，画布不会被挤压。
  // 语义上也更顺 —— 灰色药丸加一条平线就是「还没收到声音」，遮罩淡出后同一条线开始动。
  preparing:    { shape: 'is-wave', color: 'c-recording', wave: true, preparing: true },
  recording:    { shape: 'is-wave', color: 'c-recording', wave: true },
  transcribing: { shape: 'is-pill', color: 'c-thinking', labelKey: 'bubble.thinking', cancel: true },
  extracting:   { shape: 'is-pill', color: 'c-thinking', labelKey: 'bubble.thinking', cancel: true },
  optimizing:   { shape: 'is-pill', color: 'c-optimizing', labelKey: 'bubble.thinking', cancel: true },
  retrying:     { shape: 'is-pill', color: 'c-retrying', labelKey: 'bubble.thinking', cancel: true },
  completed:    { shape: 'is-round', color: 'c-completed', glyph: '✓' },
  failed:       { shape: 'is-round', color: 'c-failed', glyph: '✕' },
  // 会议记录（独立的 bubble-meeting 窗口）：✕ 是「停止录制」而不是取消任务
  'meeting-detected':    { shape: 'is-pill', color: 'c-recording', labelKey: 'bubble.meetingRecording', dot: true, cancel: true },
  // 录制期间某个分段转写失败：橙色警示，✕ 仍然是「停止录制」
  'meeting-warning':     { shape: 'is-pill', color: 'c-retrying', labelKey: 'bubble.meetingWarning', dot: true, cancel: true },
  'meeting-summarizing': { shape: 'is-pill', color: 'c-thinking', labelKey: 'bubble.summarizing' },
  'meeting-done':        { shape: 'is-round', color: 'c-completed', glyph: '✓' },
  'meeting-failed':      { shape: 'is-round', color: 'c-failed', glyph: '✕' },
}

function classFor(look: Look): string {
  return `shape ${look.shape} ${look.color}${look.preparing ? ' is-preparing' : ''}`
}

// === 麦克风电平波形 ===
// Rust 侧每 40ms 推一条 bubble-level 事件（已经做完 dB 映射与平滑），这里只负责
// 存进环形缓冲并画出来。事件本身就是采样时钟，前端不跑轮询。
// 每列 2 CSS px，130px 药丸减去左右 12px padding = 106px → 53 列 ≈ 2.1 秒历史。
const COL_CSS_PX = 2

let waveCanvas: HTMLCanvasElement | null = null
let waveCtx: CanvasRenderingContext2D | null = null
let waveCols: number[] = []
let waveDpr = 1
let wavePending = 0

function attachWave(canvas: HTMLCanvasElement) {
  if (waveCanvas === canvas) return
  waveCanvas = canvas
  // DPR 与宽度要在进入波形状态时读，不能在模块加载时读：气泡窗口是启动时在屏幕外
  // 创建的，之后才被移到光标所在的显示器，那台显示器可能不是 2x。录音期间窗口不再移动。
  waveDpr = Math.max(1, Math.round(window.devicePixelRatio || 1))
  const cssWidth = canvas.clientWidth || 106
  const cssHeight = canvas.clientHeight || 20
  canvas.width = Math.round(cssWidth * waveDpr)
  canvas.height = Math.round(cssHeight * waveDpr)
  waveCtx = canvas.getContext('2d')
  waveCols = new Array(Math.max(1, Math.floor(cssWidth / COL_CSS_PX))).fill(0)
  drawWave()
}

function detachWave() {
  if (wavePending) {
    cancelAnimationFrame(wavePending)
    wavePending = 0
  }
  waveCanvas = null
  waveCtx = null
  waveCols = []
}

function pushLevel(level: number) {
  if (!waveCols.length) return
  waveCols.push(Math.min(1, Math.max(0, level)))
  waveCols.shift()
  // 合并同一帧内的多次推送，一帧最多画一次
  if (!wavePending) wavePending = requestAnimationFrame(() => { wavePending = 0; drawWave() })
}

function drawWave() {
  const ctx = waveCtx
  const canvas = waveCanvas
  if (!ctx || !canvas) return
  const { width: w, height: h } = canvas
  const cy = h / 2
  const pitch = COL_CSS_PX * waveDpr
  const barW = waveDpr
  const minHalf = waveDpr
  const maxHalf = h / 2

  ctx.clearRect(0, 0, w, h)
  ctx.fillStyle = 'rgba(255, 255, 255, 0.92)'
  // 先铺一条贯穿全宽的中线：这样「没声音就是一条平线」是结构保证的，
  // 不依赖 dB 门限刚好调准。
  ctx.fillRect(0, cy - minHalf / 2, w, minHalf)
  for (let i = 0; i < waveCols.length; i++) {
    const half = Math.max(minHalf, Math.round(waveCols[i] * maxHalf))
    ctx.fillRect(i * pitch, cy - half, barW, half * 2)
  }
}

function setPart(
  el: HTMLElement,
  selector: string,
  visible: boolean,
  text?: string,
  display = 'inline-flex',
) {
  const node = el.querySelector(selector) as HTMLElement
  node.style.display = visible ? display : 'none'
  if (text !== undefined) node.textContent = text
}

function applyLook(el: HTMLElement, look: Look) {
  el.className = classFor(look)
  setPart(el, '.dot', !!look.dot)
  setPart(el, '.label', !!look.labelKey, look.labelKey ? t(look.labelKey) : '')
  setPart(el, '.glyph', !!look.glyph, look.glyph ?? '')
  setPart(el, '.cancel-btn', !!look.cancel)
  // canvas 是替换元素，inline-flex 对它没意义
  setPart(el, '.wave', !!look.wave, undefined, 'block')
  if (look.wave) attachWave(el.querySelector('.wave') as HTMLCanvasElement)
  else detachWave()
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
    `<canvas class="wave"></canvas>` +
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
  // 先解绑波形：innerHTML 清空后 canvas 会脱离文档，留着 ctx 和挂起的 rAF 就是
  // 往一个已经没人看的画布上画。
  detachWave()
  bubble.innerHTML = ''
  currentTaskId = 0
  currentStatus = ''
})

// 麦克风电平：Rust 每 40ms 推一次。label_for 把 task_id 钳到 3，第 4 个任务会复用
// bubble-3，所以要校验 taskNumber，别把别人的电平画到当前任务上。
currentWindow.listen<{ taskNumber: number; level: number }>('bubble-level', (event) => {
  const { taskNumber, level } = event.payload
  if (taskNumber !== currentTaskId) return
  pushLevel(level)
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
