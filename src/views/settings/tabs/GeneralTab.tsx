import React, { useEffect, useRef, useState } from 'react'
import { AppConfig, AudioDevice, LanguageSetting, LocalApiStatus, ThemeMode } from '../../../core/types'
import {
  getLaunchAtLogin,
  getLocalApiStatus,
  onEvent,
  setLaunchAtLogin,
  listInputDevices,
} from '../../../lib/tauri-api'
import { getAudioModels } from '../../../core/models'
import { t, useLang } from '../../../i18n'
import { SettingGroup } from '../components/SettingGroup'
import { SettingRow } from '../components/SettingRow'
import { Toggle } from '../components/Toggle'
import { EditableLabel } from '../components/EditableLabel'

// 快捷键行的默认显示名（文案 key，渲染时按当前语言取）
const DEFAULT_LABEL_KEYS = {
  shortcut: 'general.defaultLabel.shortcut',
  shortcut2: 'general.defaultLabel.shortcut2',
  extractShortcut: 'general.defaultLabel.extractShortcut',
  extractShortcut2: 'general.defaultLabel.extractShortcut2',
} as const

const IS_MACOS = navigator.platform.toUpperCase().includes('MAC')

// 单独修饰键作快捷键：存储值就是 KeyboardEvent.code，Rust 侧 modifier_hotkey.rs 按同一组字面值识别
const MODIFIER_CODES = [
  'AltLeft', 'AltRight', 'MetaLeft', 'MetaRight',
  'ControlLeft', 'ControlRight', 'ShiftLeft', 'ShiftRight',
] as const
type ModifierCode = typeof MODIFIER_CODES[number]

function isModifierCode(value: string): value is ModifierCode {
  return (MODIFIER_CODES as readonly string[]).includes(value)
}

const MODIFIER_KEY_NAMES = ['Control', 'Alt', 'Shift', 'Meta']

// 修饰键在两个平台上的惯用写法（仅显示用）
const MODIFIER_SYMBOL: Record<string, { mac: string; win: string }> = {
  Alt: { mac: '⌥', win: 'Alt' },
  Meta: { mac: '⌘', win: 'Win' },
  Control: { mac: '⌃', win: 'Ctrl' },
  Shift: { mac: '⇧', win: 'Shift' },
}

// 除字母、数字、F 键、小键盘数字外，global-hotkey 解析器还认这些 KeyboardEvent.code 原名
const PASSTHROUGH_CODES = new Set([
  'Space', 'Enter', 'Tab', 'Backspace', 'Delete', 'Insert', 'Home', 'End', 'PageUp', 'PageDown',
  'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight',
  'Minus', 'Equal', 'BracketLeft', 'BracketRight', 'Backslash', 'Semicolon', 'Quote', 'Backquote',
  'Comma', 'Period', 'Slash',
  'CapsLock', 'NumLock', 'ScrollLock', 'PrintScreen', 'Pause',
  'NumpadAdd', 'NumpadSubtract', 'NumpadMultiply', 'NumpadDivide', 'NumpadDecimal', 'NumpadEnter', 'NumpadEqual',
])

/**
 * 从按键事件得到热键解析器认的主键名。用物理键码 e.code 而不是 e.key：
 * macOS 上 Option+D 的 e.key 是 ∂、Shift+1 是 !，解析器都不认。
 * 返回 null 表示这个键不能用作快捷键。
 */
function keyNameFromEvent(e: React.KeyboardEvent): string | null {
  const code = e.code
  if (!code) {
    // 拿不到物理键码时退回旧逻辑
    if (e.key === ' ') return 'Space'
    return e.key || null
  }
  const letter = /^Key([A-Z])$/.exec(code)
  if (letter) return letter[1]
  const digit = /^Digit([0-9])$/.exec(code)
  if (digit) return digit[1]
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code
  if (/^Numpad[0-9]$/.test(code)) return code
  if (PASSTHROUGH_CODES.has(code)) return code
  return null
}

function displayShortcut(combo: string): string {
  if (isModifierCode(combo)) {
    const side = combo.endsWith('Left') ? 'left' : 'right'
    const base = combo.replace(/(Left|Right)$/, '')
    const symbol = MODIFIER_SYMBOL[base]
    return t(`general.modKey.${side}`, { key: IS_MACOS ? symbol.mac : symbol.win })
  }
  // 存储值用热键解析器认的 Super，显示成 Windows 用户认的 Win
  if (!IS_MACOS) return combo.replace(/Super/g, 'Win')
  return combo
    .replace(/Command/g, '\u2318')
    .replace(/Shift/g, '\u21E7')
    .replace(/Alt/g, '\u2325')
}

interface Props {
  config: AppConfig
  onSave: (config: AppConfig) => void
}

export function GeneralTab({ config, onSave }: Props) {
  useLang()
  const [recording, setRecording] = useState(false)
  const [recording2, setRecording2] = useState(false)
  const [recordingExtract, setRecordingExtract] = useState(false)
  const [recordingExtract2, setRecordingExtract2] = useState(false)
  const [devices, setDevices] = useState<AudioDevice[]>([])
  const [conflictMsg, setConflictMsg] = useState('')
  const [localApiStatus, setLocalApiStatus] = useState<LocalApiStatus>({
    running: false,
    port: null,
    error: null,
  })
  const [curlCopied, setCurlCopied] = useState(false)

  // Load device list
  useEffect(() => {
    listInputDevices()
      .then(setDevices)
      .catch(e => console.error('Failed to list input devices:', e))
  }, [])

  useEffect(() => {
    let disposed = false
    let unsubscribe: (() => void) | undefined
    getLocalApiStatus()
      .then(status => { if (!disposed) setLocalApiStatus(status) })
      .catch(error => console.error('Failed to get local API status:', error))
    onEvent<LocalApiStatus>('local-api-status', status => {
      if (!disposed) setLocalApiStatus(status)
    }).then(fn => {
      if (disposed) fn()
      else unsubscribe = fn
    })
    return () => {
      disposed = true
      unsubscribe?.()
    }
  }, [])

  const refreshDevices = async () => {
    try {
      const deviceList = await listInputDevices()
      setDevices(deviceList)
      // If current device is gone, switch to system-default
      const currentMic = config.general.microphone
      if (currentMic !== 'system-default' && !deviceList.some(d => d.name === currentMic)) {
        update({ microphone: 'system-default' })
      }
    } catch (e) {
      console.error('Failed to refresh devices:', e)
    }
  }

  useEffect(() => {
    getLaunchAtLogin().then(enabled => {
      if (enabled !== config.general.launchAtLogin) {
        onSave({ ...config, general: { ...config.general, launchAtLogin: enabled } })
      }
    }).catch(e => console.error('Failed to get launch at login:', e))
  }, [])

  const update = (changes: Partial<AppConfig['general']>) => {
    onSave({ ...config, general: { ...config.general, ...changes } })
  }

  const updateAdvanced = (changes: Partial<AppConfig['advanced']>) => {
    onSave({ ...config, advanced: { ...config.advanced, ...changes } })
  }

  const audioModels = getAudioModels(config)

  const curlCommand = `curl -fsS -X POST --data-binary @recording.m4a -H 'Content-Type: audio/mp4' 'http://127.0.0.1:${config.localApi.port}/transcribe'`

  const defaultLabels = {
    shortcut: t(DEFAULT_LABEL_KEYS.shortcut),
    shortcut2: t(DEFAULT_LABEL_KEYS.shortcut2),
    extractShortcut: t(DEFAULT_LABEL_KEYS.extractShortcut),
    extractShortcut2: t(DEFAULT_LABEL_KEYS.extractShortcut2),
  }

  const labelOf = {
    shortcut: config.general.shortcutLabel?.trim() || defaultLabels.shortcut,
    shortcut2: config.general.shortcut2Label?.trim() || defaultLabels.shortcut2,
    extractShortcut: config.general.extractShortcutLabel?.trim() || defaultLabels.extractShortcut,
    extractShortcut2: config.general.extractShortcut2Label?.trim() || defaultLabels.extractShortcut2,
  }

  // 录制中先按下的单独修饰键：松开时如果中间没按别的键，就把它作为快捷键提交
  const pendingModifier = useRef<string | null>(null)

  const showKeyMessage = (msg: string) => {
    setConflictMsg(msg)
    setTimeout(() => setConflictMsg(''), 3000)
  }

  /**
   * 生成一个录制框的 keydown / keyup 处理器。
   * - 普通键 / 组合键在 keydown 提交，主键名取自物理键码（见 keyNameFromEvent）
   * - 单独修饰键在 keyup 提交，期间按了别的键就按组合键处理
   * - 结束录制时主动 blur，这样再点同一个框会重新触发 onFocus 进入录制态
   */
  function createKeyHandlers(
    setRec: (v: boolean) => void,
    onCapture: (combo: string) => void,
    others: { key: string; label: string }[],
  ) {
    const finish = (e: React.KeyboardEvent<HTMLInputElement>) => {
      pendingModifier.current = null
      setRec(false)
      e.currentTarget.blur()
    }

    const commit = (combo: string, e: React.KeyboardEvent<HTMLInputElement>) => {
      const conflict = others.find(o => o.key === combo)
      if (conflict) {
        showKeyMessage(t('general.conflictWith', { label: conflict.label }))
      } else {
        onCapture(combo)
      }
      finish(e)
    }

    const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
      e.preventDefault()
      if (e.key === 'Escape') {
        finish(e)
        return
      }
      if (e.key === 'Tab') return
      if (MODIFIER_KEY_NAMES.includes(e.key)) {
        pendingModifier.current = isModifierCode(e.code) ? e.code : null
        return
      }

      pendingModifier.current = null
      const key = keyNameFromEvent(e)
      if (!key) {
        // 保持录制态，让用户直接换一个键再试
        showKeyMessage(t('general.keyUnsupported'))
        return
      }
      const parts: string[] = []
      if (e.ctrlKey) parts.push('Ctrl')
      if (e.altKey) parts.push('Alt')
      if (e.shiftKey) parts.push('Shift')
      // global-hotkey 只认 Super / Command，不认 Win
      if (e.metaKey) parts.push(IS_MACOS ? 'Command' : 'Super')
      parts.push(key)
      commit(parts.join('+'), e)
    }

    const onKeyUp = (e: React.KeyboardEvent<HTMLInputElement>) => {
      e.preventDefault()
      if (pendingModifier.current && e.code === pendingModifier.current) {
        commit(e.code, e)
      } else if (MODIFIER_KEY_NAMES.includes(e.key)) {
        pendingModifier.current = null
      }
    }

    return { onKeyDown, onKeyUp }
  }

  const keys1 = createKeyHandlers(
    setRecording,
    (combo) => update({ shortcut: combo }),
    [
      { key: config.general.shortcut2, label: labelOf.shortcut2 },
      { key: config.general.extractShortcut, label: labelOf.extractShortcut },
      { key: config.general.extractShortcut2, label: labelOf.extractShortcut2 },
    ],
  )

  const keys2 = createKeyHandlers(
    setRecording2,
    (combo) => update({ shortcut2: combo }),
    [
      { key: config.general.shortcut, label: labelOf.shortcut },
      { key: config.general.extractShortcut, label: labelOf.extractShortcut },
      { key: config.general.extractShortcut2, label: labelOf.extractShortcut2 },
    ],
  )

  const extractKeys1 = createKeyHandlers(
    setRecordingExtract,
    (combo) => update({ extractShortcut: combo }),
    [
      { key: config.general.shortcut, label: labelOf.shortcut },
      { key: config.general.shortcut2, label: labelOf.shortcut2 },
      { key: config.general.extractShortcut2, label: labelOf.extractShortcut2 },
    ],
  )

  const extractKeys2 = createKeyHandlers(
    setRecordingExtract2,
    (combo) => update({ extractShortcut2: combo }),
    [
      { key: config.general.shortcut, label: labelOf.shortcut },
      { key: config.general.shortcut2, label: labelOf.shortcut2 },
      { key: config.general.extractShortcut, label: labelOf.extractShortcut },
    ],
  )

  // 录制框：点击进入录制态，显示提示文案；失焦退出
  const shortcutInputProps = (
    recording: boolean,
    setRec: (v: boolean) => void,
    value: string,
    handlers: { onKeyDown: React.KeyboardEventHandler<HTMLInputElement>; onKeyUp: React.KeyboardEventHandler<HTMLInputElement> },
  ): React.InputHTMLAttributes<HTMLInputElement> => ({
    className: `kbd${recording ? ' recording' : ''}`,
    value: recording ? t('general.pressShortcut') : displayShortcut(value),
    onKeyDown: recording ? handlers.onKeyDown : undefined,
    onKeyUp: recording ? handlers.onKeyUp : undefined,
    onFocus: () => setRec(true),
    onMouseDown: () => setRec(true),
    onBlur: () => {
      pendingModifier.current = null
      setRec(false)
    },
    readOnly: true,
    style: { width: 120, textAlign: 'center', cursor: 'pointer' },
  })

  // 转写引擎下拉：空值 = 跟随「转写设置」。只有语音快捷键需要，图像识别不转写。
  const engineSelect = (value: string | undefined, onPick: (id: string) => void) => (
    <select
      className="select"
      value={value ?? ''}
      onChange={e => onPick(e.target.value)}
      style={{ minWidth: 104 }}
      title={t('general.transcribeEngine')}
    >
      <option value="">{t('general.followTranscribeSetting')}</option>
      {audioModels.map(m => (
        <option key={m.id} value={m.id}>{m.provider}</option>
      ))}
    </select>
  )

  const usesModifierShortcut = [
    config.general.shortcut,
    config.general.shortcut2,
    config.general.extractShortcut,
    config.general.extractShortcut2,
  ].some(isModifierCode)

  const themes: { value: ThemeMode; label: string; style: React.CSSProperties }[] = [
    { value: 'light', label: t('general.theme.light'), style: { background: '#ffffff', border: '1px solid #d2d2d7' } },
    { value: 'dark', label: t('general.theme.dark'), style: { background: '#1c1c1e' } },
    { value: 'system', label: t('general.theme.system'), style: { background: 'linear-gradient(to right, #ffffff 50%, #1c1c1e 50%)' } },
  ]

  return (
    <div>
      <h2 className="content-title">{t('general.title')}</h2>

      <SettingGroup title={t('general.group.appearance')}>
        <div style={{ padding: '12px 16px' }}>
          <div className="appearance-options">
            {themes.map(theme => (
              <button
                key={theme.value}
                className={`appearance-option${config.general.theme === theme.value ? ' active' : ''}`}
                onClick={() => update({ theme: theme.value })}
              >
                <div className="appearance-preview" style={theme.style} />
                <div className="appearance-label">{theme.label}</div>
              </button>
            ))}
          </div>
        </div>
        <SettingRow label={t('general.language')} description={t('general.languageDesc')}>
          <select
            className="select"
            value={config.general.language ?? 'system'}
            onChange={e => update({ language: e.target.value as LanguageSetting })}
            style={{ width: 200 }}
          >
            <option value="system">{t('general.languageSystem')}</option>
            <option value="zh-CN">中文</option>
            <option value="en">English</option>
          </select>
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t('general.group.voice')}>
        <SettingRow label={
          <EditableLabel
            value={labelOf.shortcut}
            defaultValue={defaultLabels.shortcut}
            onChange={next => update({ shortcutLabel: next })}
          />
        }>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <span style={{ fontSize: 12, color: 'var(--text-secondary)', whiteSpace: 'nowrap' }}>{t('general.outputStyle')}</span>
            <select
              className="select"
              value={config.general.shortcutTemplate}
              onChange={e => update({ shortcutTemplate: e.target.value })}
              style={{ minWidth: 100 }}
            >
              <option value="">{t('general.none')}</option>
              {config.voiceTemplates.templates.map(tpl => (
                <option key={tpl.id} value={tpl.id}>{tpl.name}</option>
              ))}
            </select>
            {engineSelect(config.general.shortcutTranscribeModel, id => update({ shortcutTranscribeModel: id }))}
            <input {...shortcutInputProps(recording, setRecording, config.general.shortcut, keys1)} />
          </div>
        </SettingRow>
        <SettingRow label={
          <EditableLabel
            value={labelOf.shortcut2}
            defaultValue={defaultLabels.shortcut2}
            onChange={next => update({ shortcut2Label: next })}
          />
        }>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <span style={{ fontSize: 12, color: 'var(--text-secondary)', whiteSpace: 'nowrap' }}>{t('general.outputStyle')}</span>
            <select
              className="select"
              value={config.general.shortcut2Template}
              onChange={e => update({ shortcut2Template: e.target.value })}
              style={{ minWidth: 100 }}
            >
              <option value="">{t('general.none')}</option>
              {config.voiceTemplates.templates.map(tpl => (
                <option key={tpl.id} value={tpl.id}>{tpl.name}</option>
              ))}
            </select>
            {engineSelect(config.general.shortcut2TranscribeModel, id => update({ shortcut2TranscribeModel: id }))}
            <input {...shortcutInputProps(recording2, setRecording2, config.general.shortcut2, keys2)} />
          </div>
        </SettingRow>
        <SettingRow label={t('general.pttMode')} description={t('general.pttModeDesc')}>
          <Toggle
            checked={!!config.general.pttMode}
            onChange={checked => update({ pttMode: checked })}
          />
        </SettingRow>
        <SettingRow
          label={t('general.overwriteClipboard')}
          description={t('general.overwriteClipboardDesc')}
        >
          <Toggle
            checked={config.general.overwriteClipboard !== false}
            onChange={checked => update({ overwriteClipboard: checked })}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t('general.group.extract')}>
        <SettingRow label={
          <EditableLabel
            value={labelOf.extractShortcut}
            defaultValue={defaultLabels.extractShortcut}
            onChange={next => update({ extractShortcutLabel: next })}
          />
        }>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <span style={{ fontSize: 12, color: 'var(--text-secondary)', whiteSpace: 'nowrap' }}>{t('general.outputStyle')}</span>
            <select
              className="select"
              value={config.general.extractShortcutTemplate}
              onChange={e => update({ extractShortcutTemplate: e.target.value })}
              style={{ minWidth: 100 }}
            >
              {config.extract.templates.map(tpl => (
                <option key={tpl.id} value={tpl.id}>{tpl.name}</option>
              ))}
            </select>
            <input {...shortcutInputProps(recordingExtract, setRecordingExtract, config.general.extractShortcut, extractKeys1)} />
          </div>
        </SettingRow>
        <SettingRow label={
          <EditableLabel
            value={labelOf.extractShortcut2}
            defaultValue={defaultLabels.extractShortcut2}
            onChange={next => update({ extractShortcut2Label: next })}
          />
        }>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <span style={{ fontSize: 12, color: 'var(--text-secondary)', whiteSpace: 'nowrap' }}>{t('general.outputStyle')}</span>
            <select
              className="select"
              value={config.general.extractShortcut2Template}
              onChange={e => update({ extractShortcut2Template: e.target.value })}
              style={{ minWidth: 100 }}
            >
              {config.extract.templates.map(tpl => (
                <option key={tpl.id} value={tpl.id}>{tpl.name}</option>
              ))}
            </select>
            <input {...shortcutInputProps(recordingExtract2, setRecordingExtract2, config.general.extractShortcut2, extractKeys2)} />
          </div>
        </SettingRow>
      </SettingGroup>

      {conflictMsg && (
        <div style={{ color: '#ff3b30', fontSize: 12, padding: '4px 16px' }}>
          {conflictMsg}
        </div>
      )}
      {usesModifierShortcut && (
        <div style={{ color: 'var(--text-secondary)', fontSize: 12, padding: '4px 16px' }}>
          {t('general.modKeyHint')}
        </div>
      )}

      <SettingGroup title={t('general.group.other')}>
        <SettingRow label={t('general.maxRecording')} description={t('general.maxRecordingDesc')}>
          <input
            type="number"
            className="input"
            value={config.general.maxRecordingSeconds}
            min={10}
            max={600}
            step={10}
            onChange={e => {
              const v = parseInt(e.target.value, 10)
              if (!isNaN(v) && v >= 10 && v <= 600) update({ maxRecordingSeconds: v })
            }}
            style={{ width: 80, textAlign: 'center' }}
          />
        </SettingRow>
        <SettingRow label={t('general.launchAtLogin')} description={t('general.launchAtLoginDesc')}>
          <Toggle
            checked={config.general.launchAtLogin}
            onChange={async checked => {
              try {
                await setLaunchAtLogin(checked)
                update({ launchAtLogin: checked })
              } catch (e) {
                console.error('Failed to set launch at login:', e)
              }
            }}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t('general.group.microphone')}>
        <SettingRow label={t('general.inputDevice')} description={t('general.inputDeviceDesc')}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <select
              className="select"
              value={config.general.microphone}
              onChange={e => update({ microphone: e.target.value })}
              style={{ maxWidth: 200 }}
            >
              {devices.map(d => (
                <option key={d.name} value={d.name}>
                  {d.name === 'system-default'
                    ? t('general.systemDefault')
                    : d.isDefault ? t('general.deviceWithDefault', { name: d.name }) : d.name}
                </option>
              ))}
            </select>
            <button
              className="file-picker-btn"
              onClick={refreshDevices}
              title={t('general.refreshDevicesTitle')}
            >
              {t('general.refresh')}
            </button>
          </div>
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t('general.group.network')}>
        <SettingRow
          label={t('general.localApi')}
          description={t('general.localApiDesc')}
        >
          <Toggle
            checked={config.localApi.enabled}
            onChange={enabled => onSave({ ...config, localApi: { ...config.localApi, enabled } })}
          />
        </SettingRow>
        <SettingRow label={t('general.localApiPort')} description={t('general.localApiPortDesc')}>
          <input
            className="input"
            type="number"
            value={config.localApi.port}
            min={1024}
            max={65535}
            onChange={event => {
              const port = Number(event.target.value)
              if (Number.isInteger(port) && port >= 1024 && port <= 65535) {
                onSave({ ...config, localApi: { ...config.localApi, port } })
              }
            }}
            style={{ width: 100 }}
          />
        </SettingRow>
        <SettingRow label={t('general.apiStatus')}>
          <span style={{
            color: localApiStatus.error
              ? '#ff3b30'
              : localApiStatus.running
                ? '#34c759'
                : 'var(--text-secondary)',
            fontSize: 12,
            maxWidth: 360,
            display: 'inline-block',
            textAlign: 'right',
          }}>
            {localApiStatus.error
              ? localApiStatus.error
              : localApiStatus.running
                ? t('general.apiRunning', { port: localApiStatus.port ?? '' })
                : t('general.apiOff')}
          </span>
        </SettingRow>
        <SettingRow label={t('general.curlExample')} description={t('general.curlExampleDesc')}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, maxWidth: 440 }}>
            <code style={{
              fontSize: 11,
              color: 'var(--text-secondary)',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
              maxWidth: 340,
            }} title={curlCommand}>{curlCommand}</code>
            <button
              className="file-picker-btn"
              onClick={async () => {
                await navigator.clipboard.writeText(curlCommand)
                setCurlCopied(true)
                setTimeout(() => setCurlCopied(false), 1500)
              }}
            >
              {curlCopied ? t('general.copied') : t('common.copy')}
            </button>
          </div>
        </SettingRow>
        <SettingRow label={t('general.transcribeTimeout')} description={t('general.seconds')}>
          <input
            className="input"
            type="number"
            value={config.advanced.transcribeTimeout}
            onChange={e => {
              const v = Number(e.target.value)
              if (e.target.value === '' || !Number.isFinite(v) || v < 1) return
              updateAdvanced({ transcribeTimeout: v })
            }}
            min={1}
            style={{ width: 100 }}
          />
        </SettingRow>
        <SettingRow label={t('general.optimizeTimeout')} description={t('general.seconds')}>
          <input
            className="input"
            type="number"
            value={config.advanced.optimizeTimeout}
            onChange={e => {
              const v = Number(e.target.value)
              if (e.target.value === '' || !Number.isFinite(v) || v < 1) return
              updateAdvanced({ optimizeTimeout: v })
            }}
            min={1}
            style={{ width: 100 }}
          />
        </SettingRow>
        <SettingRow label={t('general.maxRetries')}>
          <input
            className="input"
            type="number"
            value={config.advanced.maxRetries}
            onChange={e => {
              const v = Number(e.target.value)
              if (e.target.value === '' || !Number.isFinite(v) || v < 0) return
              updateAdvanced({ maxRetries: v })
            }}
            min={0}
            style={{ width: 100 }}
          />
        </SettingRow>
        <SettingRow label={t('general.maxParallel')}>
          <input
            className="input"
            type="number"
            value={config.advanced.maxParallel}
            onChange={e => {
              const v = Number(e.target.value)
              if (e.target.value === '' || !Number.isFinite(v) || v < 1) return
              updateAdvanced({ maxParallel: v })
            }}
            min={1}
            style={{ width: 100 }}
          />
        </SettingRow>
      </SettingGroup>
    </div>
  )
}
