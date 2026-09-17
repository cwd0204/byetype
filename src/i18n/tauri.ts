/**
 * 各窗口的语言接线：
 * - 启动时从配置读 `general.language`
 * - 监听设置窗口广播的 `language-changed`（payload 是配置值）与后端的 `config-updated`
 * 设置窗口改语言后调用 `broadcastLanguage()` 通知其他窗口。
 */
import { emit } from '@tauri-apps/api/event'
import { getConfig, onEvent } from '../lib/tauri-api'
import { applyLanguage } from './index'

export const LANGUAGE_CHANGED_EVENT = 'language-changed'

/** 非设置窗口在入口调用一次；返回取消订阅函数。 */
export function bootstrapLanguage(): () => void {
  let cancelled = false
  const unsubs: Array<() => void> = []

  getConfig()
    .then(config => {
      if (!cancelled) applyLanguage(config.general.language)
    })
    .catch(() => {})

  onEvent<string>(LANGUAGE_CHANGED_EVENT, setting => {
    if (!cancelled) applyLanguage(setting)
  }).then(unsub => {
    if (cancelled) unsub()
    else unsubs.push(unsub)
  })

  onEvent<Record<string, never>>('config-updated', () => {
    if (cancelled) return
    getConfig()
      .then(config => {
        if (!cancelled) applyLanguage(config.general.language)
      })
      .catch(() => {})
  }).then(unsub => {
    if (cancelled) unsub()
    else unsubs.push(unsub)
  })

  return () => {
    cancelled = true
    unsubs.forEach(fn => fn())
  }
}

/** 设置窗口改了语言后广播给其他窗口 */
export function broadcastLanguage(setting: string): void {
  emit(LANGUAGE_CHANGED_EVENT, setting).catch(() => {})
}
