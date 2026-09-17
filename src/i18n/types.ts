/** 界面实际使用的语言（配置里的 'system' 解析之后） */
export type Lang = 'zh-CN' | 'en'

/** key → 文案。key 用 `区域.名字` 的点分形式，如 'general.title'；占位符写成 {name} */
export type MessageTable = Record<string, string>

/** 每个区域一个模块，zh 与 en 的 key 必须一一对应 */
export interface MessageBundle {
  zh: MessageTable
  en: MessageTable
}
