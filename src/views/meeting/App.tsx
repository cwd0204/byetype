import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import type { MeetingDetail, MeetingMeta, MeetingStatus, TranscriptSegment } from '../../core/types'
import {
  deleteMeeting,
  discardMeeting,
  getConfig,
  getMeeting,
  getMeetingStatus,
  listMeetings,
  onEvent,
  regenerateMeetingSummary,
  revealMeeting,
  saveMeetingSummary,
  startMeeting,
  stopMeeting,
} from '../../lib/tauri-api'

type Tab = 'summary' | 'transcript'

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
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
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

/** 极简 Markdown → React：只处理标题、列表、分隔线、段落和行内代码 / 粗体，够纪要用 */
function renderMarkdown(md: string) {
  const lines = md.split('\n')
  const nodes: React.ReactNode[] = []
  let list: string[] = []
  let para: string[] = []
  const flushList = () => {
    if (list.length) {
      nodes.push(<ul key={`ul-${nodes.length}`}>{list.map((item, i) => <li key={i}>{inline(item)}</li>)}</ul>)
      list = []
    }
  }
  const flushPara = () => {
    if (para.length) {
      nodes.push(<p key={`p-${nodes.length}`}>{inline(para.join(' '))}</p>)
      para = []
    }
  }
  const inline = (text: string): React.ReactNode[] => {
    const parts = text.split(/(`[^`]+`|\*\*[^*]+\*\*)/g)
    return parts.map((part, i) => {
      if (part.startsWith('`') && part.endsWith('`')) return <code key={i}>{part.slice(1, -1)}</code>
      if (part.startsWith('**') && part.endsWith('**')) return <strong key={i}>{part.slice(2, -2)}</strong>
      return <span key={i}>{part}</span>
    })
  }
  for (const raw of lines) {
    const line = raw.trimEnd()
    const heading = /^(#{1,6})\s+(.*)$/.exec(line)
    if (heading) {
      flushList(); flushPara()
      const level = heading[1].length
      const text = heading[2]
      const key = `h-${nodes.length}`
      if (level === 1) nodes.push(<h1 key={key}>{text}</h1>)
      else if (level === 2) nodes.push(<h2 key={key}>{text}</h2>)
      else nodes.push(<h3 key={key}>{text}</h3>)
      continue
    }
    if (/^\s*[-*]\s+/.test(line)) {
      flushPara()
      list.push(line.replace(/^\s*[-*]\s+/, ''))
      continue
    }
    if (/^---+$/.test(line.trim())) {
      flushList(); flushPara()
      nodes.push(<hr key={`hr-${nodes.length}`} />)
      continue
    }
    if (line.trim() === '') {
      flushList(); flushPara()
      continue
    }
    flushList()
    para.push(line.trim())
  }
  flushList(); flushPara()
  return nodes
}

export default function App() {
  const [meetings, setMeetings] = useState<MeetingMeta[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detail, setDetail] = useState<MeetingDetail | null>(null)
  const [status, setStatus] = useState<MeetingStatus | null>(null)
  const [tab, setTab] = useState<Tab>('summary')
  const [query, setQuery] = useState('')
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const [notice, setNotice] = useState('')
  const transcriptEndRef = useRef<HTMLDivElement | null>(null)
  const selectedIdRef = useRef<string | null>(null)
  selectedIdRef.current = selectedId

  // 主题跟随设置
  useEffect(() => {
    getConfig().then(config => {
      const theme = config.general.theme
      if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme
      else delete document.documentElement.dataset.theme
    }).catch(() => {})
  }, [])

  const refreshList = useCallback(async () => {
    try {
      const list = await listMeetings()
      setMeetings(list)
      if (!selectedIdRef.current && list.length > 0) setSelectedId(list[0].id)
    } catch (e) {
      console.error('listMeetings failed', e)
    }
  }, [])

  const refreshDetail = useCallback(async (id: string | null) => {
    if (!id) { setDetail(null); return }
    try {
      const d = await getMeeting(id)
      setDetail(d)
    } catch (e) {
      console.error('getMeeting failed', e)
      setDetail(null)
    }
  }, [])

  useEffect(() => {
    refreshList()
    getMeetingStatus().then(setStatus).catch(() => {})
    const unsubs: Array<() => void> = []
    let cancelled = false
    const listen = <T,>(event: string, handler: (payload: T) => void) => {
      onEvent<T>(event, handler).then(unsub => { if (cancelled) unsub(); else unsubs.push(unsub) })
    }
    listen<MeetingStatus>('meeting-status', s => setStatus(s))
    listen<Record<string, never>>('meeting-list-updated', () => { refreshList(); refreshDetail(selectedIdRef.current) })
    listen<{ meetingId: string; segment: TranscriptSegment }>('meeting-transcript-appended', ({ meetingId, segment }) => {
      if (meetingId !== selectedIdRef.current) return
      setDetail(prev => prev ? { ...prev, segments: [...prev.segments.filter(s => s.index !== segment.index), segment].sort((a, b) => a.index - b.index) } : prev)
    })
    listen<{ meetingId: string; ok: boolean; error?: string | null; notesPath?: string | null }>('meeting-finished', p => {
      setNotice(p.ok ? (p.notesPath ? `纪要已保存到 ${p.notesPath}` : '纪要已生成') : `纪要生成失败：${p.error ?? ''}`)
      refreshList(); refreshDetail(selectedIdRef.current)
    })
    listen<{ meetingId: string | null }>('meeting-open', ({ meetingId }) => {
      if (meetingId) { setSelectedId(meetingId); setTab('summary') }
      refreshList()
    })
    return () => { cancelled = true; unsubs.forEach(u => u()) }
  }, [refreshList, refreshDetail])

  useEffect(() => { refreshDetail(selectedId); setEditing(false) }, [selectedId, refreshDetail])

  // 录制中的会议：新段到达时滚到底部
  useEffect(() => {
    if (tab === 'transcript' && detail?.meta.status === 'recording') {
      transcriptEndRef.current?.scrollIntoView({ block: 'end' })
    }
  }, [detail?.segments.length, tab, detail?.meta.status])

  // 选中的是正在录制的会议时，默认看转写
  useEffect(() => {
    if (status?.phase === 'recording' && status.meetingId === selectedId) setTab('transcript')
  }, [status?.phase, status?.meetingId, selectedId])

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return meetings
    return meetings.filter(m => m.title.toLowerCase().includes(q) || m.id.includes(q))
  }, [meetings, query])

  const run = async (label: string, action: () => Promise<unknown>) => {
    setBusy(label)
    setNotice('')
    try {
      await action()
    } catch (e) {
      setNotice(typeof e === 'string' ? e : (e as Error)?.message ?? '操作失败')
    } finally {
      setBusy(null)
    }
  }

  const isLive = !!status && status.phase !== 'idle' && status.meetingId === selectedId
  const meta = detail?.meta

  return (
    <div className="meeting-app">
      <aside className="sidebar">
        <div className="sidebar-header">
          <h1>会议记录</h1>
          {status?.phase === 'recording' ? (
            <button className="btn small danger" disabled={busy !== null} onClick={() => run('stop', stopMeeting)}>停止 {fmtHms(status.elapsedSecs)}</button>
          ) : status?.phase === 'finalizing' ? (
            <span className="warning-text">生成纪要中…</span>
          ) : (
            <button className="btn small primary" disabled={busy !== null} onClick={() => run('start', startMeeting)}>开始录制</button>
          )}
        </div>
        <input className="sidebar-search" placeholder="搜索标题" value={query} onChange={e => setQuery(e.target.value)} />
        <div className="meeting-list">
          {filtered.length === 0 && <div className="empty" style={{ marginTop: 40 }}>还没有会议记录<br />开 Zoom 会议会自动开始，或点「开始录制」</div>}
          {filtered.map(m => (
            <button key={m.id} className={`meeting-item${m.id === selectedId ? ' active' : ''}`} onClick={() => setSelectedId(m.id)}>
              <span className="title">{m.title || '会议记录'}</span>
              <span className="sub">
                <span>{fmtDate(m.startedAt)}</span>
                {m.durationSecs > 0 && <span>{fmtHms(m.durationSecs)}</span>}
                <span className="status"><span className={`status-dot ${m.status}`} />{statusLabel(m.status)}</span>
              </span>
            </button>
          ))}
        </div>
        {status?.lastError && <div className="sidebar-footer error-text" title={status.lastError}>{status.lastError}</div>}
      </aside>

      <main className="main">
        {!meta ? (
          <div className="content"><div className="empty">选择左侧的一场会议查看纪要与转写</div></div>
        ) : (
          <>
            <div className="main-header">
              <div className="title-row">
                <h2>{meta.title || '会议记录'}</h2>
                <div className="toolbar">
                  {isLive && status?.phase === 'recording' && (
                    <>
                      <button className="btn danger" disabled={busy !== null} onClick={() => run('stop', stopMeeting)}>停止并生成纪要</button>
                      <button className="btn" disabled={busy !== null} onClick={() => { if (confirm('丢弃这次录制？转写内容会一并删除。')) run('discard', discardMeeting) }}>丢弃</button>
                    </>
                  )}
                  {!isLive && (
                    <>
                      {(meta.status === 'summary_failed' || meta.status === 'interrupted' || meta.status === 'done') && (
                        <button className="btn" disabled={busy !== null} onClick={() => run('regen', () => regenerateMeetingSummary(meta.id))}>{meta.status === 'done' ? '重新生成纪要' : '生成纪要'}</button>
                      )}
                      <button className="btn" onClick={() => revealMeeting(meta.id).catch(() => {})}>在 Finder 中显示</button>
                      <button className="btn danger" disabled={busy !== null} onClick={() => { if (confirm('删除这场会议的全部记录？')) run('delete', async () => { await deleteMeeting(meta.id); setSelectedId(null); await refreshList() }) }}>删除</button>
                    </>
                  )}
                </div>
              </div>
              <div className="meta-row">
                {isLive && status?.phase === 'recording' && <span className="live-badge"><span className="status-dot recording" />录制中 {fmtHms(status.elapsedSecs)}</span>}
                {isLive && status?.phase === 'finalizing' && <span className="warning-text">正在生成会议纪要…</span>}
                <span>{fmtDate(meta.startedAt)}</span>
                {meta.durationSecs > 0 && <span>时长 {fmtHms(meta.durationSecs)}</span>}
                <span>{meta.source === 'auto' ? 'Zoom 自动' : '手动'}</span>
                <span>{[meta.microphone && '麦克风', meta.systemAudio && '系统音频'].filter(Boolean).join(' + ') || '无音源'}</span>
                {isLive && status && status.chunksPending > 0 && <span>转写队列 {status.chunksPending}</span>}
                {status?.warnings?.length && isLive ? <span className="warning-text">{status.warnings.join('；')}</span> : null}
                {meta.error && !isLive && <span className="error-text">{meta.error}</span>}
              </div>
            </div>

            <div className="tabs">
              <button className={`tab${tab === 'summary' ? ' active' : ''}`} onClick={() => setTab('summary')}>纪要</button>
              <button className={`tab${tab === 'transcript' ? ' active' : ''}`} onClick={() => setTab('transcript')}>转写{detail && detail.segments.length > 0 ? `（${detail.segments.length} 段）` : ''}</button>
            </div>

            <div className="content">
              {tab === 'summary' && (
                <>
                  {editing ? (
                    <>
                      <div className="toolbar" style={{ marginBottom: 10 }}>
                        <button className="btn primary" disabled={busy !== null} onClick={() => run('save', async () => { await saveMeetingSummary(meta.id, draft); setEditing(false); await refreshDetail(meta.id) })}>保存</button>
                        <button className="btn" onClick={() => setEditing(false)}>取消</button>
                      </div>
                      <textarea className="summary-editor" value={draft} onChange={e => setDraft(e.target.value)} spellCheck={false} />
                    </>
                  ) : detail?.summary ? (
                    <>
                      <div className="toolbar" style={{ marginBottom: 10 }}>
                        <button className="btn small" onClick={() => { setDraft(detail.summary ?? ''); setEditing(true) }}>编辑</button>
                        <button className="btn small" onClick={() => { navigator.clipboard.writeText(detail.summary ?? '').then(() => setNotice('纪要已复制')).catch(() => {}) }}>复制</button>
                      </div>
                      <div className="summary">{renderMarkdown(detail.summary)}</div>
                    </>
                  ) : (
                    <div className="empty">
                      {meta.status === 'recording' ? '会议结束后会自动生成纪要' : meta.status === 'finalizing' ? '正在生成纪要…' : '还没有纪要'}
                    </div>
                  )}
                </>
              )}
              {tab === 'transcript' && (
                <>
                  {detail && detail.segments.length === 0 && (
                    <div className="empty">{meta.status === 'recording' ? '第一段音频还在录制中，稍后出现转写' : '没有转写内容'}</div>
                  )}
                  {detail?.segments.map(seg => (
                    <div key={seg.index} className={`segment${seg.failed ? ' failed' : ''}`}>
                      <div className="range">[{fmtHms(seg.startSecs)} – {fmtHms(seg.endSecs)}]</div>
                      <div className="text">{seg.text}</div>
                    </div>
                  ))}
                  {isLive && status?.phase === 'recording' && (
                    <div className="pending-note">{status.chunksPending > 0 ? `第 ${detail ? detail.segments.length + 1 : 1} 段转写中…` : '录制中…'}</div>
                  )}
                  <div ref={transcriptEndRef} />
                </>
              )}
            </div>
            <div className="footer-note" title={meta.notesPath ?? ''}>
              {notice || (meta.notesPath ? `笔记已导出到 ${meta.notesPath}` : `转写模型 ${meta.transcribeModel || '-'}${meta.summaryModel ? ` · 纪要模型 ${meta.summaryModel}` : ''}`)}
            </div>
          </>
        )}
      </main>
    </div>
  )
}

// 让 Esc 关闭（隐藏）窗口，和其他辅助窗口一致
window.addEventListener('keydown', e => {
  if (e.key === 'Escape') getCurrentWindow().hide().catch(() => {})
})
