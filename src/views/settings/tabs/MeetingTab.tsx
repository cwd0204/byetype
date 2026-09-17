import { useEffect, useMemo, useState } from 'react'
import type { AppConfig, MeetingMeta, MeetingStatus, MeetingSupport, ThinkingConfig } from '../../../core/types'
import { getAudioModels, getTextModels } from '../../../core/models'
import { t, useLang } from '../../../i18n'
import { SettingGroup } from '../components/SettingGroup'
import { SettingRow } from '../components/SettingRow'
import { Toggle } from '../components/Toggle'
import { PromptEditor, type PromptFileEntry } from '../components/PromptEditor'
import {
  checkMeetingSupport,
  deleteMeeting,
  discardMeeting,
  getMeetingStatus,
  getNotesFolder,
  listMeetings,
  onEvent,
  openMeetingWindow,
  pickNotesFolder,
  regenerateMeetingSummary,
  requestMeetingPermissions,
  startMeeting,
  stopMeeting,
} from '../../../lib/tauri-api'

interface Props {
  config: AppConfig
  onSave: (config: AppConfig) => void
}

function fmtHms(secs: number): string {
  const h = Math.floor(secs / 3600)
  const m = Math.floor((secs % 3600) / 60)
  const s = Math.floor(secs % 60)
  const pad = (n: number) => String(n).padStart(2, '0')
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`
}

function fmtDate(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

function statusLabel(status: MeetingMeta['status']): string {
  switch (status) {
    case 'recording': return t('meetingTab.status.recording')
    case 'finalizing': return t('meetingTab.status.finalizing')
    case 'done': return t('meetingTab.status.done')
    case 'summary_failed': return t('meetingTab.status.summaryFailed')
    case 'failed': return t('meetingTab.status.failed')
    case 'interrupted': return t('meetingTab.status.interrupted')
    default: return status
  }
}

/** 录制中的状态描述：来源 · 已转写 N 段[，排队 M 段][ · 警告…] */
function recordingDescription(status: MeetingStatus): string {
  const source = t(status.source === 'auto' ? 'meetingTab.live.sourceAuto' : 'meetingTab.live.sourceManual')
  const pending = status.chunksPending > 0 ? t('meetingTab.live.pending', { n: status.chunksPending }) : ''
  const warnings = status.warnings.length ? ` · ${status.warnings.join(t('meetingTab.live.warnSep'))}` : ''
  return t('meetingTab.live.recordingDesc', { source, done: status.chunksDone, pending, warnings })
}

export function MeetingTab({ config, onSave }: Props) {
  const lang = useLang()
  const meeting = config.meeting
  const [status, setStatus] = useState<MeetingStatus | null>(null)
  const [support, setSupport] = useState<MeetingSupport | null>(null)
  const [permissionMsg, setPermissionMsg] = useState('')
  const [recent, setRecent] = useState<MeetingMeta[]>([])
  const [defaultNotesFolder, setDefaultNotesFolder] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const [actionMsg, setActionMsg] = useState('')

  const audioModels = getAudioModels(config)
  const textModels = getTextModels(config)

  // label 随语言变化；按 lang 记忆化以保持 promptFiles 引用稳定
  const meetingPromptFiles = useMemo<PromptFileEntry[]>(() => [
    { key: 'meeting-summary', label: t('meetingTab.promptFile'), configPath: 'meeting.prompts.summary', builtinFilename: 'meeting-summary.md' },
  ], [lang])

  const update = (changes: Partial<AppConfig['meeting']>) => {
    onSave({ ...config, meeting: { ...meeting, ...changes } })
  }
  const updateThinking = (changes: Partial<ThinkingConfig>) => {
    update({ summaryThinking: { ...meeting.summaryThinking, ...changes } })
  }

  const refresh = () => {
    getMeetingStatus().then(setStatus).catch(() => {})
    listMeetings().then(list => setRecent(list.slice(0, 8))).catch(() => {})
  }

  useEffect(() => {
    refresh()
    checkMeetingSupport().then(setSupport).catch(() => {})
    getNotesFolder().then(setDefaultNotesFolder).catch(() => {})
    let cancelled = false
    const unsubs: Array<() => void> = []
    onEvent<MeetingStatus>('meeting-status', s => { if (!cancelled) setStatus(s) }).then(u => { if (cancelled) u(); else unsubs.push(u) })
    onEvent<Record<string, never>>('meeting-list-updated', () => { if (!cancelled) listMeetings().then(list => setRecent(list.slice(0, 8))).catch(() => {}) }).then(u => { if (cancelled) u(); else unsubs.push(u) })
    return () => { cancelled = true; unsubs.forEach(u => u()) }
  }, [])

  // 笔记目录变化后刷新默认值显示
  useEffect(() => {
    getNotesFolder().then(setDefaultNotesFolder).catch(() => {})
  }, [meeting.notesFolder])

  const run = async (label: string, action: () => Promise<unknown>) => {
    setBusy(label)
    setActionMsg('')
    try {
      await action()
    } catch (e) {
      setActionMsg(typeof e === 'string' ? e : (e as Error)?.message ?? t('meetingTab.actionFailed'))
    } finally {
      setBusy(null)
      refresh()
    }
  }

  const requestPermission = async () => {
    setPermissionMsg(t('meetingTab.permission.requesting'))
    try {
      await requestMeetingPermissions()
      setPermissionMsg(t('meetingTab.permission.granted'))
    } catch (e) {
      setPermissionMsg(typeof e === 'string' ? e : (e as Error)?.message ?? t('meetingTab.permission.failed'))
    }
  }

  const numberInput = (value: number, onChange: (v: number) => void, width = 90) => (
    <input
      className="input"
      type="number"
      value={value}
      onChange={e => { const v = parseInt(e.target.value, 10); if (!Number.isNaN(v)) onChange(v) }}
      style={{ width }}
    />
  )

  const systemAudioUnsupported = support ? !support.systemAudioSupported : false

  return (
    <div>
      <h2 className="content-title">{t('meetingTab.title')}</h2>

      <SettingGroup title={t('meetingTab.group.status')}>
        <SettingRow
          label={status?.phase === 'recording' ? t('meetingTab.live.recording', { elapsed: fmtHms(status.elapsedSecs) }) : status?.phase === 'finalizing' ? t('meetingTab.live.finalizing') : t('meetingTab.live.idle')}
          description={
            status?.phase === 'recording'
              ? recordingDescription(status)
              : status?.lastError ?? t('meetingTab.live.idleDesc')
          }
        >
          <div style={{ display: 'flex', gap: 8 }}>
            {status?.phase === 'recording' ? (
              <>
                <button className="model-action-btn" disabled={busy !== null} onClick={() => run('stop', stopMeeting)}>{t('meetingTab.btn.stop')}</button>
                <button className="model-action-btn danger" disabled={busy !== null} onClick={() => run('discard', discardMeeting)}>{t('meetingTab.btn.discard')}</button>
              </>
            ) : (
              <button className="model-action-btn" disabled={busy !== null || status?.phase === 'finalizing'} onClick={() => run('start', startMeeting)}>{t('meetingTab.btn.start')}</button>
            )}
            <button className="model-action-btn" onClick={() => openMeetingWindow(status?.meetingId ?? undefined).catch(() => {})}>{t('meetingTab.btn.openWindow')}</button>
          </div>
        </SettingRow>
        {actionMsg && <div style={{ color: '#ff453a', fontSize: 12, padding: '4px 0' }}>{actionMsg}</div>}
      </SettingGroup>

      <SettingGroup title={t('meetingTab.group.recording')}>
        <SettingRow label={t('meetingTab.enabled')} description={t('meetingTab.enabledDesc')}>
          <Toggle checked={meeting.enabled} onChange={enabled => update({ enabled })} />
        </SettingRow>
        <SettingRow label={t('meetingTab.autoDetect')} description={t('meetingTab.autoDetectDesc')}>
          <Toggle checked={meeting.autoDetect} disabled={!meeting.enabled} onChange={autoDetect => update({ autoDetect })} />
        </SettingRow>
        <SettingRow
          label={t('meetingTab.systemAudio')}
          description={systemAudioUnsupported ? t('meetingTab.systemAudioUnsupported', { reason: support?.reason ?? '' }) : t('meetingTab.systemAudioDesc')}
        >
          <Toggle checked={meeting.captureSystemAudio && !systemAudioUnsupported} disabled={systemAudioUnsupported} onChange={captureSystemAudio => update({ captureSystemAudio })} />
        </SettingRow>
        {!systemAudioUnsupported && (
          <SettingRow label={t('meetingTab.systemAudioPermission')} description={permissionMsg || t('meetingTab.systemAudioPermissionDesc')}>
            <button className="model-action-btn" onClick={requestPermission}>{t('meetingTab.requestPermission')}</button>
          </SettingRow>
        )}
        <SettingRow label={t('meetingTab.microphone')} description={t('meetingTab.microphoneDesc')}>
          <Toggle checked={meeting.captureMicrophone} onChange={captureMicrophone => update({ captureMicrophone })} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t('meetingTab.group.model')}>
        <SettingRow label={t('meetingTab.transcribeModel')} description={t('meetingTab.transcribeModelDesc')}>
          <select className="select" value={meeting.transcribeModelId} onChange={e => update({ transcribeModelId: e.target.value })} style={{ width: 260 }}>
            <option value="">{t('meetingTab.followTranscribe')}</option>
            {audioModels.map(m => <option key={m.id} value={m.id}>{m.provider} - {m.model}</option>)}
          </select>
        </SettingRow>
        <SettingRow label={t('meetingTab.summaryModel')} description={t('meetingTab.summaryModelDesc')}>
          <select className="select" value={meeting.summaryModelId} onChange={e => update({ summaryModelId: e.target.value })} style={{ width: 260 }}>
            {textModels.map(m => <option key={m.id} value={m.id}>{m.provider} - {m.model}</option>)}
          </select>
        </SettingRow>
        <SettingRow label={t('meetingTab.summaryThinking')} description={t('meetingTab.summaryThinkingDesc')}>
          <Toggle checked={meeting.summaryThinking.enabled} onChange={enabled => updateThinking({ enabled })} />
        </SettingRow>
        {meeting.summaryThinking.enabled && (
          <SettingRow label={t('common.thinking.level')} description={t('common.thinking.levelDesc')}>
            <select className="select" value={meeting.summaryThinking.level} onChange={e => updateThinking({ level: e.target.value as ThinkingConfig['level'] })} style={{ width: 120 }}>
              <option value="LOW">LOW</option>
              <option value="MEDIUM">MEDIUM</option>
              <option value="HIGH">HIGH</option>
            </select>
          </SettingRow>
        )}
      </SettingGroup>

      <SettingGroup title={t('meetingTab.group.chunking')}>
        <SettingRow label={t('meetingTab.chunkSeconds')} description={t('meetingTab.chunkSecondsDesc')}>
          {numberInput(meeting.chunkSeconds, v => update({ chunkSeconds: v }))}
        </SettingRow>
        <SettingRow label={t('meetingTab.chunkTimeout')} description={t('meetingTab.chunkTimeoutDesc')}>
          {numberInput(meeting.chunkTimeoutSecs, v => update({ chunkTimeoutSecs: v }))}
        </SettingRow>
        <SettingRow label={t('meetingTab.summaryTimeout')} description={t('meetingTab.summaryTimeoutDesc')}>
          {numberInput(meeting.summaryTimeoutSecs, v => update({ summaryTimeoutSecs: v }))}
        </SettingRow>
        <SettingRow label={t('meetingTab.maxMinutes')} description={t('meetingTab.maxMinutesDesc')}>
          {numberInput(meeting.maxMeetingMinutes, v => update({ maxMeetingMinutes: v }))}
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t('meetingTab.group.save')}>
        <SettingRow label={t('meetingTab.notesFolder')} description={t('meetingTab.notesFolderDesc', { path: meeting.notesFolder || defaultNotesFolder || t('meetingTab.notesFolderDefault') })}>
          <div style={{ display: 'flex', gap: 8 }}>
            <button className="model-action-btn" onClick={() => pickNotesFolder().then(folder => { if (folder) update({ notesFolder: folder }) }).catch(() => {})}>{t('meetingTab.chooseFolder')}</button>
            {meeting.notesFolder && <button className="model-action-btn" onClick={() => update({ notesFolder: '' })}>{t('meetingTab.restoreDefault')}</button>}
          </div>
        </SettingRow>
        <SettingRow label={t('meetingTab.keepAudio')} description={t('meetingTab.keepAudioDesc')}>
          <Toggle checked={meeting.keepAudio} onChange={keepAudio => update({ keepAudio })} />
        </SettingRow>
        <SettingRow label={t('meetingTab.showWindowOnStart')} description={t('meetingTab.showWindowOnStartDesc')}>
          <Toggle checked={meeting.showWindowOnStart} onChange={showWindowOnStart => update({ showWindowOnStart })} />
        </SettingRow>
        <SettingRow label={t('meetingTab.openSummaryWhenDone')} description={t('meetingTab.openSummaryWhenDoneDesc')}>
          <Toggle checked={meeting.openSummaryWhenDone} onChange={openSummaryWhenDone => update({ openSummaryWhenDone })} />
        </SettingRow>
      </SettingGroup>

      <div style={{ color: 'var(--text-tertiary)', fontSize: 11.5, margin: '4px 0 16px' }}>
        {t('meetingTab.disclaimer')}
      </div>

      <h3 className="section-title">{t('meetingTab.promptsSection')}</h3>
      <div style={{ height: 320, display: 'flex', flexDirection: 'column', marginBottom: 16 }}>
        <PromptEditor config={config} onSave={onSave} promptFiles={meetingPromptFiles} editorHeight={220} />
      </div>

      <h3 className="section-title">{t('meetingTab.recentSection')}</h3>
      <SettingGroup>
        {recent.length === 0 && <div style={{ color: 'var(--text-tertiary)', fontSize: 12, padding: '8px 0' }}>{t('meetingTab.noMeetings')}</div>}
        {recent.map(m => (
          <SettingRow key={m.id} label={m.title || t('meetingTab.untitled')} description={`${fmtDate(m.startedAt)}${m.durationSecs > 0 ? ` · ${fmtHms(m.durationSecs)}` : ''} · ${statusLabel(m.status)}${m.error ? ` · ${m.error}` : ''}`}>
            <div style={{ display: 'flex', gap: 6 }}>
              <button className="model-action-btn" onClick={() => openMeetingWindow(m.id).catch(() => {})}>{t('common.open')}</button>
              {(m.status === 'summary_failed' || m.status === 'interrupted') && (
                <button className="model-action-btn" disabled={busy !== null} onClick={() => run('regen', () => regenerateMeetingSummary(m.id))}>{t('meetingTab.btn.regen')}</button>
              )}
              {m.status !== 'recording' && m.status !== 'finalizing' && (
                <button className="model-action-btn danger" disabled={busy !== null} onClick={() => { if (confirm(t('meetingTab.deleteConfirm'))) run('delete', () => deleteMeeting(m.id)) }}>{t('common.delete')}</button>
              )}
            </div>
          </SettingRow>
        ))}
        <div style={{ padding: '6px 0' }}>
          <button className="add-model-btn" onClick={() => openMeetingWindow().catch(() => {})} style={{ fontSize: 12 }}>{t('meetingTab.viewAll')}</button>
        </div>
      </SettingGroup>
    </div>
  )
}
