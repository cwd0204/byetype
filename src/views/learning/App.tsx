import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { t, useLang } from '../../i18n'
import { bootstrapLanguage } from '../../i18n/tauri'

interface LearningDraft {
  original: string
  corrected: string
  generated: string
  notice: string
  generatedOnce: boolean
}

interface LearningApplyResult {
  added: number
}

type BusyAction = 'regenerate' | 'apply' | null

/** 底部提示：前端文案存 key（切语言后能重译），后端 draft.notice 原文直接存字符串 */
type Notice = string | { key: string; vars?: Record<string, string | number> }

function noticeText(notice: Notice): string {
  return typeof notice === 'string' ? notice : t(notice.key, notice.vars)
}

export default function App() {
  const lang = useLang()
  const [original, setOriginal] = useState('')
  const [corrected, setCorrected] = useState('')
  const [generated, setGenerated] = useState('')
  const [notice, setNotice] = useState<Notice>({ key: 'learningWindow.notice.loading' })
  const [error, setError] = useState('')
  const [busy, setBusy] = useState<BusyAction>(null)
  const [saved, setSaved] = useState(false)
  const [generatedOnce, setGeneratedOnce] = useState(false)

  // 语言跟随设置；窗口标题随语言切换
  useEffect(() => bootstrapLanguage(), [])
  useEffect(() => { document.title = t('learningWindow.title') }, [lang])

  const loadDraft = (draft: LearningDraft) => {
    setOriginal(draft.original)
    setCorrected(draft.corrected)
    setGenerated(draft.generated)
    setNotice(draft.notice)
    setGeneratedOnce(draft.generatedOnce)
    setSaved(false)
    setError('')
  }

  useEffect(() => {
    let disposed = false
    const unlistenPromise = listen<LearningDraft>('voice-learning-draft', event => {
      if (!disposed) loadDraft(event.payload)
    })
    invoke<LearningDraft | null>('get_voice_learning_draft')
      .then(draft => {
        if (disposed) return
        if (draft) loadDraft(draft)
        else setNotice({ key: 'learningWindow.notice.noDraft' })
      })
      .catch(reason => {
        if (!disposed) setError(String(reason))
      })
    return () => {
      disposed = true
      unlistenPromise.then(unlisten => unlisten()).catch(() => {})
    }
  }, [])

  const regenerate = async () => {
    setBusy('regenerate')
    setError('')
    setSaved(false)
    try {
      const draft = await invoke<LearningDraft>('regenerate_voice_learning', {
        original,
        corrected,
      })
      loadDraft(draft)
    } catch (reason) {
      setError(String(reason))
    } finally {
      setBusy(null)
    }
  }

  const apply = async () => {
    setBusy('apply')
    setError('')
    try {
      const result = await invoke<LearningApplyResult>('apply_voice_learning_generated', {
        generated,
      })
      setSaved(true)
      setNotice(result.added > 0
        ? { key: 'learningWindow.notice.added', vars: { count: result.added } }
        : { key: 'learningWindow.notice.nothingAdded' })
    } catch (reason) {
      setError(String(reason))
    } finally {
      setBusy(null)
    }
  }

  const clearDraft = () => {
    setOriginal('')
    setCorrected('')
    setGenerated('')
    setGeneratedOnce(false)
    setSaved(false)
    setError('')
    setNotice({ key: 'learningWindow.notice.cleared' })
  }

  const close = () => invoke('close_voice_learning_window')

  return (
    <main className="learning-shell">
      <header className="learning-header">
        <div>
          <h1>{t('learningWindow.heading')}</h1>
          <p>{t('learningWindow.subtitle')}</p>
        </div>
        <button className="button button-quiet" onClick={close} disabled={busy !== null}>{t('common.cancel')}</button>
      </header>

      <section className="editor-grid" aria-busy={busy !== null}>
        <EditorPanel
          label={t('learningWindow.panel.original.label')}
          helper={t('learningWindow.panel.original.helper')}
          value={original}
          onChange={value => { setOriginal(value); setSaved(false) }}
        />
        <EditorPanel
          label={t('learningWindow.panel.corrected.label')}
          helper={t('learningWindow.panel.corrected.helper')}
          placeholder={t('learningWindow.panel.corrected.placeholder')}
          value={corrected}
          onChange={value => { setCorrected(value); setSaved(false) }}
        />
        <EditorPanel
          label={t('learningWindow.panel.generated.label')}
          helper={t('learningWindow.panel.generated.helper')}
          value={generated}
          onChange={value => { setGenerated(value); setSaved(false) }}
          result
        />
        {busy === 'regenerate' && (
          <div className="generating-layer" role="status">
            <strong className="generating-title">{t('learningWindow.generating.title')}</strong>
            <div className="generating-bar" />
            <span>{t('learningWindow.generating.desc')}</span>
          </div>
        )}
      </section>

      <footer className="learning-footer">
        <div className={error ? 'status status-error' : saved ? 'status status-success' : 'status'}>
          {error || noticeText(notice)}
        </div>
        <div className="actions">
          <button
            className="button button-clear"
            onClick={clearDraft}
            disabled={busy !== null || (original === '' && corrected === '' && generated === '')}
          >
            {t('learningWindow.clear')}
          </button>
          <button
            className="button button-secondary"
            onClick={regenerate}
            disabled={busy !== null || corrected.trim() === ''}
          >
            {busy === 'regenerate' ? t('learningWindow.learning') : generatedOnce ? t('learningWindow.regenerate') : t('learningWindow.startLearning')}
          </button>
          <button
            className="button button-primary"
            onClick={apply}
            disabled={busy !== null || generated.trim() === '' || saved}
          >
            {busy === 'apply' ? t('learningWindow.applying') : saved ? t('learningWindow.applied') : t('learningWindow.apply')}
          </button>
        </div>
      </footer>
    </main>
  )
}

interface EditorPanelProps {
  label: string
  helper: string
  value: string
  onChange: (value: string) => void
  result?: boolean
  placeholder?: string
}

function EditorPanel({ label, helper, value, onChange, result = false, placeholder }: EditorPanelProps) {
  useLang()
  return (
    <label className={result ? 'editor-panel result-panel' : 'editor-panel'}>
      <span className="panel-heading">
        <strong>{label}</strong>
        {result && <span className="save-badge">{t('learningWindow.panel.willSave')}</span>}
      </span>
      <span className="panel-helper">{helper}</span>
      <textarea
        value={value}
        placeholder={placeholder}
        onChange={event => onChange(event.target.value)}
        spellCheck={false}
      />
    </label>
  )
}
