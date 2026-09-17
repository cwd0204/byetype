/**
 * 合并各区域的文案。新增区域时在这里加一行 import + 两处展开即可。
 * 各区域文件只由对应的界面文件使用，key 前缀与文件名一致，避免并行修改时冲突。
 */
import type { MessageBundle, MessageTable } from '../types'
import { common } from './common'
import { general } from './general'
import { models } from './models'
import { transcribe } from './transcribe'
import { prompts } from './prompts'
import { extract } from './extract'
import { learningTab } from './learningTab'
import { meetingTab } from './meetingTab'
import { history } from './history'
import { usage } from './usage'
import { backup } from './backup'
import { about } from './about'
import { settingsApp } from './settingsApp'
import { meetingWindow } from './meetingWindow'
import { learningWindow } from './learningWindow'
import { preview } from './preview'
import { bubble } from './bubble'

const bundles: MessageBundle[] = [
  common, general, models, transcribe, prompts, extract, learningTab, meetingTab,
  history, usage, backup, about, settingsApp, meetingWindow, learningWindow, preview, bubble,
]

function merge(pick: (bundle: MessageBundle) => MessageTable): MessageTable {
  return bundles.reduce<MessageTable>((acc, bundle) => Object.assign(acc, pick(bundle)), {})
}

export const zh: MessageTable = merge(b => b.zh)
export const en: MessageTable = merge(b => b.en)
