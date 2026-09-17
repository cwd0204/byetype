/**
 * 轻量 i18n：模块级当前语言 + `t(key, vars)` + React `useLang()` 订阅。
 *
 * - 配置里的 `general.language` 是 'system' | 'zh-CN' | 'en'，'system' 按 navigator.language 解析
 * - 文案按区域拆在 `./messages/*.ts`，由 `./messages/index.ts` 合并
 * - 英文缺失时回退中文，再回退 key 本身，方便逐步补齐
 */
import { useEffect, useState } from 'react'
import { en, zh } from './messages'
import type { Lang, MessageTable } from './types'

export type { Lang, MessageBundle, MessageTable } from './types'

const LANG_STORAGE_KEY = 'byetype.lang'

export function detectSystemLang(): Lang {
  const tag = (typeof navigator !== 'undefined' ? navigator.language : '') || ''
  return tag.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en'
}

/** 配置值 → 实际语言 */
export function resolveLang(setting: string | undefined | null): Lang {
  if (setting === 'zh-CN' || setting === 'en') return setting
  return detectSystemLang()
}

function initialLang(): Lang {
  // 上次解析结果缓存在 localStorage，让窗口首帧就用对语言，避免闪一下中文
  try {
    const cached = window.localStorage.getItem(LANG_STORAGE_KEY)
    if (cached === 'zh-CN' || cached === 'en') return cached
  } catch {
    // localStorage 不可用时忽略
  }
  return detectSystemLang()
}

let currentLang: Lang = initialLang()
const listeners = new Set<() => void>()

export function getLang(): Lang {
  return currentLang
}

/** 应用配置里的语言设置；变化时通知所有 useLang() 订阅者重渲染。返回解析后的语言。 */
export function applyLanguage(setting: string | undefined | null): Lang {
  const next = resolveLang(setting)
  if (next !== currentLang) {
    currentLang = next
    try {
      window.localStorage.setItem(LANG_STORAGE_KEY, next)
    } catch {
      // ignore
    }
    if (typeof document !== 'undefined') document.documentElement.lang = next
    listeners.forEach(fn => fn())
  }
  return next
}

function table(lang: Lang): MessageTable {
  return lang === 'en' ? en : zh
}

function interpolate(text: string, vars?: Record<string, string | number>): string {
  if (!vars) return text
  return text.replace(/\{(\w+)\}/g, (_, name: string) => (name in vars ? String(vars[name]) : `{${name}}`))
}

/** 取当前语言的文案。英文缺失回退中文，再回退 key。 */
export function t(key: string, vars?: Record<string, string | number>): string {
  const text = table(currentLang)[key] ?? zh[key] ?? key
  return interpolate(text, vars)
}

/** 按指定语言取文案（极少数需要固定语言的地方用，比如导出文件名） */
export function tIn(lang: Lang, key: string, vars?: Record<string, string | number>): string {
  const text = table(lang)[key] ?? zh[key] ?? key
  return interpolate(text, vars)
}

/** 非 React 代码（如气泡窗口的原生 TS）订阅语言变化；返回取消订阅函数 */
export function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** 在组件里订阅语言变化：任何渲染 t() 文案的组件都应调用一次，这样切换语言后会重渲染 */
export function useLang(): Lang {
  const [, force] = useState(0)
  useEffect(() => {
    const fn = () => force(n => n + 1)
    listeners.add(fn)
    return () => {
      listeners.delete(fn)
    }
  }, [])
  return currentLang
}
