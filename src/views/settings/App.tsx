import { useState, useEffect, useCallback, useRef } from 'react'
import { GeneralTab } from './tabs/GeneralTab'
import { TranscribeTab } from './tabs/TranscribeTab'
import { ModelsTab } from './tabs/ModelsTab'
import { HistoryTab } from './tabs/HistoryTab'
import { UsageTab } from './tabs/UsageTab'
import { AboutTab } from './tabs/AboutTab'
import { ExtractTab } from './tabs/ExtractTab'
import { VoicePromptsTab } from './tabs/VoicePromptsTab'
import { ExtractPromptsTab } from './tabs/ExtractPromptsTab'
import { BackupTab } from './tabs/BackupTab'
import { VoiceLearningTab } from './tabs/VoiceLearningTab'
import { MeetingTab } from './tabs/MeetingTab'
import type { AppConfig, UpdateState, UpdateInfo } from '../../core/types'
import { getVersion } from '@tauri-apps/api/app'
import { getConfig, saveConfig, onEvent, checkUpdate } from '../../lib/tauri-api'
import { applyLanguage, t, useLang } from '../../i18n'
import { broadcastLanguage } from '../../i18n/tauri'
import './theme.css'

// label 存的是文案 key，渲染时经 t() 取当前语言
type TabItem =
  | { type: 'tab'; id: string; labelKey: string }
  | { type: 'group'; labelKey: string }
  | { type: 'divider' }

const TABS: TabItem[] = [
  { type: 'tab', id: 'general', labelKey: 'settingsApp.tab.general' },
  { type: 'tab', id: 'models', labelKey: 'settingsApp.tab.models' },
  { type: 'group', labelKey: 'settingsApp.group.voice' },
  { type: 'tab', id: 'transcribe', labelKey: 'settingsApp.tab.transcribe' },
  { type: 'tab', id: 'voice-prompts', labelKey: 'settingsApp.tab.voicePrompts' },
  { type: 'group', labelKey: 'settingsApp.group.extract' },
  { type: 'tab', id: 'extract', labelKey: 'settingsApp.tab.extract' },
  { type: 'tab', id: 'extract-prompts', labelKey: 'settingsApp.tab.extractPrompts' },
  { type: 'group', labelKey: 'settingsApp.group.learning' },
  { type: 'tab', id: 'voice-learning', labelKey: 'settingsApp.tab.voiceLearning' },
  { type: 'group', labelKey: 'settingsApp.group.meeting' },
  { type: 'tab', id: 'meeting', labelKey: 'settingsApp.tab.meeting' },
  { type: 'divider' },
  { type: 'tab', id: 'history', labelKey: 'settingsApp.tab.history' },
  { type: 'tab', id: 'usage', labelKey: 'settingsApp.tab.usage' },
  { type: 'tab', id: 'backup', labelKey: 'settingsApp.tab.backup' },
  { type: 'tab', id: 'about', labelKey: 'settingsApp.tab.about' },
]

const INITIAL_UPDATE_STATE: UpdateState = {
  phase: 'idle',
  info: null,
  progress: 0,
  error: null,
  dismissed: false,
  checkedOnce: false,
}

export function App() {
  useLang()
  const [activeTab, setActiveTab] = useState('general')
  const [config, setConfig] = useState<AppConfig | null>(null)
  const [saved, setSaved] = useState(false)
  const [errorMsg, setErrorMsg] = useState('')
  const [updateState, setUpdateState] = useState<UpdateState>(INITIAL_UPDATE_STATE)
  const [appVersion, setAppVersion] = useState('')
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  // 上一次应用到界面的语言设置；null 表示配置还没加载过，首次应用不广播
  const appliedLanguageRef = useRef<string | null>(null)

  const handleUpdateState = useCallback((partial: Partial<UpdateState>) => {
    setUpdateState(prev => ({ ...prev, ...partial }))
  }, [])

  useEffect(() => {
    let cancelled = false
    getConfig().then((c) => { if (!cancelled) setConfig(c) })
    getVersion().then((v) => { if (!cancelled) setAppVersion(v) })

    let unsubUpdateAvailable: (() => void) | null = null
    onEvent<UpdateInfo>('update-available', (payload) => {
      if (cancelled) return
      setUpdateState(prev => ({
        ...prev,
        phase: 'available',
        info: payload,
        dismissed: false,
      }))
    }).then(unsub => { if (cancelled) unsub(); else unsubUpdateAvailable = unsub })

    let unsubProgress: (() => void) | null = null
    onEvent<{ percent: number }>('update-progress', (payload) => {
      if (cancelled) return
      setUpdateState(prev => ({ ...prev, progress: payload.percent }))
    }).then(unsub => { if (cancelled) unsub(); else unsubProgress = unsub })

    let unsubComplete: (() => void) | null = null
    onEvent<Record<string, never>>('update-complete', () => {
      if (cancelled) return
      setUpdateState(prev => ({ ...prev, phase: 'downloaded' }))
    }).then(unsub => { if (cancelled) unsub(); else unsubComplete = unsub })

    let unsubError: (() => void) | null = null
    onEvent<{ message: string }>('update-error', (payload) => {
      if (cancelled) return
      setUpdateState(prev => ({ ...prev, phase: 'error', error: payload.message }))
    }).then(unsub => { if (cancelled) unsub(); else unsubError = unsub })

    let unsubNavigate: (() => void) | null = null
    onEvent<{ tab: string }>('navigate-to-tab', (payload) => {
      if (cancelled) return
      setActiveTab(payload.tab)
      if (payload.tab === 'about') {
        setUpdateState(prev => {
          if (prev.phase === 'idle') {
            checkUpdate().then(result => {
              if (cancelled) return
              if (result) {
                setUpdateState(p => ({ ...p, phase: 'available', info: result, dismissed: false, checkedOnce: true }))
              } else {
                setUpdateState(p => ({ ...p, phase: 'idle', info: null, checkedOnce: true }))
              }
            }).catch(() => { if (cancelled) return; setUpdateState(p => ({ ...p, phase: 'idle', info: null, checkedOnce: true })) })
            return { ...prev, phase: 'checking' }
          }
          return prev
        })
      }
    }).then(unsub => { if (cancelled) unsub(); else unsubNavigate = unsub })

    // 托盘等后端入口改了配置（比如切换「自动检测 Zoom 会议」）时重新拉取
    let unsubConfigUpdated: (() => void) | null = null
    onEvent<Record<string, never>>('config-updated', () => {
      if (cancelled) return
      getConfig().then(c => { if (!cancelled) setConfig(c) }).catch(() => {})
    }).then(unsub => { if (cancelled) unsub(); else unsubConfigUpdated = unsub })

    return () => {
      cancelled = true
      unsubUpdateAvailable?.()
      unsubProgress?.()
      unsubComplete?.()
      unsubError?.()
      unsubNavigate?.()
      unsubConfigUpdated?.()
    }
  }, [])

  useEffect(() => {
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current)
    }
  }, [])

  useEffect(() => {
    const theme = config?.general.theme
    if (!theme) return

    if (theme === 'light' || theme === 'dark') {
      document.documentElement.dataset.theme = theme
      return
    }

    // theme === 'system': follow OS preference
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    document.documentElement.dataset.theme = mq.matches ? 'dark' : 'light'

    const handler = (e: MediaQueryListEvent) => {
      document.documentElement.dataset.theme = e.matches ? 'dark' : 'light'
    }
    mq.addEventListener('change', handler)
    return () => mq.removeEventListener('change', handler)
  }, [config?.general.theme])

  // 配置里的语言变化时应用到本窗口；非首次变化再广播给其他窗口
  useEffect(() => {
    if (!config) return
    const setting = config.general.language ?? 'system'
    applyLanguage(setting)
    const previous = appliedLanguageRef.current
    appliedLanguageRef.current = setting
    if (previous !== null && previous !== setting) broadcastLanguage(setting)
  }, [config?.general.language])

  const handleSave = useCallback((newConfig: AppConfig) => {
    setConfig(newConfig)
    if (debounceRef.current) clearTimeout(debounceRef.current)
    debounceRef.current = setTimeout(async () => {
      try {
        await saveConfig(newConfig)
        setSaved(true)
        setTimeout(() => setSaved(false), 1500)
      } catch (e: any) {
        const msg = typeof e === 'string' ? e : e?.message || t('settingsApp.saveFailed')
        setErrorMsg(msg)
        getConfig().then(setConfig).catch(() => {})
        setTimeout(() => setErrorMsg(''), 4000)
      }
    }, 300)
  }, [])

  const showDot = updateState.phase === 'available' && !updateState.dismissed

  if (!config) return <div style={{ padding: 20, color: 'var(--text-primary)' }}>{t('settingsApp.loading')}</div>

  return (
    <div style={{ fontFamily: '-apple-system, BlinkMacSystemFont, sans-serif', height: '100vh', display: 'flex' }}>
      <div className="sidebar">
        {TABS.map((item, i) => {
          if (item.type === 'group') {
            return <div key={`group-${i}`} className="sidebar-group">{t(item.labelKey)}</div>
          }
          if (item.type === 'divider') {
            return <div key={`div-${i}`} className="sidebar-divider" />
          }
          return (
            <button
              key={item.id}
              className={`sidebar-item${activeTab === item.id ? ' active' : ''}`}
              onClick={() => setActiveTab(item.id)}
              style={{ position: 'relative' }}
            >
              {t(item.labelKey)}
              {item.id === 'about' && showDot && <span className="sidebar-dot" />}
            </button>
          )
        })}
      </div>
      <div style={{ flex: 1, padding: 24, overflow: 'auto', position: 'relative', display: 'flex', flexDirection: 'column' }}>
        <span className={`saved-toast${saved ? ' visible' : ''}`}
          style={{ position: 'absolute', top: 24, right: 24 }}>
          {t('settingsApp.saved')}
        </span>
        {errorMsg && (
          <span className="saved-toast visible"
            style={{ position: 'absolute', top: 24, right: 24, background: '#ff3b30', color: '#fff' }}>
            ✗ {errorMsg}
          </span>
        )}
        {activeTab === 'history' && <HistoryTab />}
        {activeTab === 'usage' && <UsageTab />}
        {activeTab === 'general' && <GeneralTab config={config} onSave={handleSave} />}
        {activeTab === 'transcribe' && <TranscribeTab config={config} onSave={handleSave} />}
        {activeTab === 'models' && <ModelsTab config={config} onSave={handleSave} />}
        {activeTab === 'voice-learning' && <VoiceLearningTab config={config} onSave={handleSave} />}
        {activeTab === 'meeting' && <MeetingTab config={config} onSave={handleSave} />}
        {activeTab === 'extract' && <ExtractTab config={config} onSave={handleSave} />}
        {activeTab === 'voice-prompts' && <VoicePromptsTab config={config} onSave={handleSave} />}
        {activeTab === 'extract-prompts' && <ExtractPromptsTab config={config} onSave={handleSave} />}
        {activeTab === 'backup' && config && <BackupTab config={config} onSave={handleSave} />}
        {activeTab === 'about' && (
          <AboutTab
            updateState={updateState}
            onUpdateState={handleUpdateState}
            appVersion={appVersion}
          />
        )}
      </div>
    </div>
  )
}
