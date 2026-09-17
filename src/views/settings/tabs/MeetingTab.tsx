import { useEffect, useState } from 'react'
import type { AppConfig, MeetingMeta, MeetingStatus, MeetingSupport, ThinkingConfig } from '../../../core/types'
import { getAudioModels, getTextModels } from '../../../core/models'
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

const MEETING_PROMPT_FILES: PromptFileEntry[] = [
  { key: 'meeting-transcribe', label: '会议转写提示词', configPath: 'meeting.prompts.transcribe', builtinFilename: 'meeting-transcribe.md' },
  { key: 'meeting-summary', label: '会议纪要提示词', configPath: 'meeting.prompts.summary', builtinFilename: 'meeting-summary.md' },
]

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
    case 'recording': return '录制中'
    case 'finalizing': return '生成纪要中'
    case 'done': return '已完成'
    case 'summary_failed': return '纪要失败'
    case 'failed': return '失败'
    case 'interrupted': return '已中断'
    default: return status
  }
}

export function MeetingTab({ config, onSave }: Props) {
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
      setActionMsg(typeof e === 'string' ? e : (e as Error)?.message ?? '操作失败')
    } finally {
      setBusy(null)
      refresh()
    }
  }

  const requestPermission = async () => {
    setPermissionMsg('正在请求系统音频录制权限…')
    try {
      await requestMeetingPermissions()
      setPermissionMsg('系统音频录制可用。')
    } catch (e) {
      setPermissionMsg(typeof e === 'string' ? e : (e as Error)?.message ?? '权限请求失败')
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
      <h2 className="content-title">会议记录</h2>

      <SettingGroup title="当前状态">
        <SettingRow
          label={status?.phase === 'recording' ? `录制中 ${fmtHms(status.elapsedSecs)}` : status?.phase === 'finalizing' ? '正在生成会议纪要…' : '空闲'}
          description={
            status?.phase === 'recording'
              ? `${status.source === 'auto' ? 'Zoom 自动开始' : '手动开始'} · 已转写 ${status.chunksDone} 段${status.chunksPending > 0 ? `，排队 ${status.chunksPending} 段` : ''}${status.warnings.length ? ` · ${status.warnings.join('；')}` : ''}`
              : status?.lastError ?? '开 Zoom 会议会自动开始（需先启用），也可以手动开始'
          }
        >
          <div style={{ display: 'flex', gap: 8 }}>
            {status?.phase === 'recording' ? (
              <>
                <button className="model-action-btn" disabled={busy !== null} onClick={() => run('stop', stopMeeting)}>停止并生成纪要</button>
                <button className="model-action-btn danger" disabled={busy !== null} onClick={() => run('discard', discardMeeting)}>丢弃</button>
              </>
            ) : (
              <button className="model-action-btn" disabled={busy !== null || status?.phase === 'finalizing'} onClick={() => run('start', startMeeting)}>开始录制</button>
            )}
            <button className="model-action-btn" onClick={() => openMeetingWindow(status?.meetingId ?? undefined).catch(() => {})}>打开会议窗口</button>
          </div>
        </SettingRow>
        {actionMsg && <div style={{ color: '#ff453a', fontSize: 12, padding: '4px 0' }}>{actionMsg}</div>}
      </SettingGroup>

      <SettingGroup title="录制">
        <SettingRow label="启用会议记录" description="总开关；关闭时不会探测 Zoom，托盘里仍可手动开始">
          <Toggle checked={meeting.enabled} onChange={enabled => update({ enabled })} />
        </SettingRow>
        <SettingRow label="自动检测 Zoom 会议" description="检测到 Zoom 开会自动开始录制，会议结束自动生成纪要">
          <Toggle checked={meeting.autoDetect} disabled={!meeting.enabled} onChange={autoDetect => update({ autoDetect })} />
        </SettingRow>
        <SettingRow
          label="录制系统音频"
          description={systemAudioUnsupported ? `当前系统不支持：${support?.reason ?? ''}` : '录下其他参会者的声音（macOS 14.2+，需要「仅系统音频录制」权限）'}
        >
          <Toggle checked={meeting.captureSystemAudio && !systemAudioUnsupported} disabled={systemAudioUnsupported} onChange={captureSystemAudio => update({ captureSystemAudio })} />
        </SettingRow>
        {!systemAudioUnsupported && (
          <SettingRow label="系统音频权限" description={permissionMsg || '首次使用前先请求权限，避免开会时才弹窗；被拒后可在「系统设置 → 隐私与安全性 → 屏幕与系统音频录制」里打开'}>
            <button className="model-action-btn" onClick={requestPermission}>请求权限</button>
          </SettingRow>
        )}
        <SettingRow label="录制麦克风" description="录下自己的声音；使用「通用设置」里选的麦克风">
          <Toggle checked={meeting.captureMicrophone} onChange={captureMicrophone => update({ captureMicrophone })} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="模型">
        <SettingRow label="转写模型" description="Amazon Transcribe 会自动区分说话人；多模态模型按提示词标注「我 / 对方」">
          <select className="select" value={meeting.transcribeModelId} onChange={e => update({ transcribeModelId: e.target.value })} style={{ width: 260 }}>
            <option value="">跟随转写设置</option>
            {audioModels.map(m => <option key={m.id} value={m.id}>{m.provider} - {m.model}</option>)}
          </select>
        </SettingRow>
        <SettingRow label="纪要模型" description="会议结束后用它生成结构化纪要">
          <select className="select" value={meeting.summaryModelId} onChange={e => update({ summaryModelId: e.target.value })} style={{ width: 260 }}>
            {textModels.map(m => <option key={m.id} value={m.id}>{m.provider} - {m.model}</option>)}
          </select>
        </SettingRow>
        <SettingRow label="纪要启用思考" description="长会议建议开启，纪要更完整">
          <Toggle checked={meeting.summaryThinking.enabled} onChange={enabled => updateThinking({ enabled })} />
        </SettingRow>
        {meeting.summaryThinking.enabled && (
          <SettingRow label="Thinking Level" description="思考深度级别">
            <select className="select" value={meeting.summaryThinking.level === 'MINIMAL' ? 'LOW' : meeting.summaryThinking.level} onChange={e => updateThinking({ level: e.target.value as ThinkingConfig['level'] })} style={{ width: 120 }}>
              <option value="LOW">LOW</option>
              <option value="MEDIUM">MEDIUM</option>
              <option value="HIGH">HIGH</option>
            </select>
          </SettingRow>
        )}
      </SettingGroup>

      <SettingGroup title="分段与超时">
        <SettingRow label="分段时长" description="每段音频的目标时长（60–600 秒），到点后在静音处切开送去转写">
          {numberInput(meeting.chunkSeconds, v => update({ chunkSeconds: v }))}
        </SettingRow>
        <SettingRow label="单段转写超时" description="秒，含自动重试；Amazon Transcribe 建议 ≥ 120">
          {numberInput(meeting.chunkTimeoutSecs, v => update({ chunkTimeoutSecs: v }))}
        </SettingRow>
        <SettingRow label="纪要生成超时" description="秒，长会议开思考时可以放宽">
          {numberInput(meeting.summaryTimeoutSecs, v => update({ summaryTimeoutSecs: v }))}
        </SettingRow>
        <SettingRow label="会议最长时长" description="分钟，到点自动停止并生成纪要（10–600）">
          {numberInput(meeting.maxMeetingMinutes, v => update({ maxMeetingMinutes: v }))}
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="保存">
        <SettingRow label="笔记文件夹" description={`纪要 + 转写导出为 Markdown，可指到 Obsidian vault。当前：${meeting.notesFolder || defaultNotesFolder || '应用数据目录/meetings-notes'}`}>
          <div style={{ display: 'flex', gap: 8 }}>
            <button className="model-action-btn" onClick={() => pickNotesFolder().then(folder => { if (folder) update({ notesFolder: folder }) }).catch(() => {})}>选择目录</button>
            {meeting.notesFolder && <button className="model-action-btn" onClick={() => update({ notesFolder: '' })}>恢复默认</button>}
          </div>
        </SettingRow>
        <SettingRow label="保留音频分段" description="把每段 FLAC 留在会议目录里（占空间，便于排查）">
          <Toggle checked={meeting.keepAudio} onChange={keepAudio => update({ keepAudio })} />
        </SettingRow>
        <SettingRow label="开始录制时打开会议窗口" description="实时查看转写">
          <Toggle checked={meeting.showWindowOnStart} onChange={showWindowOnStart => update({ showWindowOnStart })} />
        </SettingRow>
        <SettingRow label="纪要生成后自动打开" description="会议结束、纪要写好后弹出会议窗口">
          <Toggle checked={meeting.openSummaryWhenDone} onChange={openSummaryWhenDone => update({ openSummaryWhenDone })} />
        </SettingRow>
      </SettingGroup>

      <div style={{ color: 'var(--text-tertiary)', fontSize: 11.5, margin: '4px 0 16px' }}>
        录制会议会保存其他参会者的语音内容，请按所在组织的要求提前告知参会者。
      </div>

      <h3 className="section-title">提示词</h3>
      <div style={{ height: 320, display: 'flex', flexDirection: 'column', marginBottom: 16 }}>
        <PromptEditor config={config} onSave={onSave} promptFiles={MEETING_PROMPT_FILES} editorHeight={220} />
      </div>

      <h3 className="section-title">最近会议</h3>
      <SettingGroup>
        {recent.length === 0 && <div style={{ color: 'var(--text-tertiary)', fontSize: 12, padding: '8px 0' }}>还没有会议记录</div>}
        {recent.map(m => (
          <SettingRow key={m.id} label={m.title || '会议记录'} description={`${fmtDate(m.startedAt)}${m.durationSecs > 0 ? ` · ${fmtHms(m.durationSecs)}` : ''} · ${statusLabel(m.status)}${m.error ? ` · ${m.error}` : ''}`}>
            <div style={{ display: 'flex', gap: 6 }}>
              <button className="model-action-btn" onClick={() => openMeetingWindow(m.id).catch(() => {})}>打开</button>
              {(m.status === 'summary_failed' || m.status === 'interrupted') && (
                <button className="model-action-btn" disabled={busy !== null} onClick={() => run('regen', () => regenerateMeetingSummary(m.id))}>生成纪要</button>
              )}
              {m.status !== 'recording' && m.status !== 'finalizing' && (
                <button className="model-action-btn danger" disabled={busy !== null} onClick={() => { if (confirm('删除这场会议的全部记录？')) run('delete', () => deleteMeeting(m.id)) }}>删除</button>
              )}
            </div>
          </SettingRow>
        ))}
        <div style={{ padding: '6px 0' }}>
          <button className="add-model-btn" onClick={() => openMeetingWindow().catch(() => {})} style={{ fontSize: 12 }}>在会议窗口查看全部</button>
        </div>
      </SettingGroup>
    </div>
  )
}
