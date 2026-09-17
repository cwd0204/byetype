import { SettingGroup } from '../components/SettingGroup'
import { SettingRow } from '../components/SettingRow'
import type { UpdateState } from '../../../core/types'
import {
  checkUpdate,
  downloadUpdate,
  installAndRestart,
} from '../../../lib/tauri-api'
import { t, useLang } from '../../../i18n'

interface Props {
  updateState: UpdateState
  onUpdateState: (state: Partial<UpdateState>) => void
  appVersion: string
}

export function AboutTab({ updateState, onUpdateState, appVersion }: Props) {
  useLang()
  const { phase, info, progress, error, dismissed } = updateState

  const handleCheck = async () => {
    onUpdateState({ phase: 'checking', error: null })
    try {
      const result = await checkUpdate()
      if (result) {
        onUpdateState({ phase: 'available', info: result, dismissed: false, checkedOnce: true })
      } else {
        onUpdateState({ phase: 'idle', info: null, checkedOnce: true })
      }
    } catch (e) {
      onUpdateState({ phase: 'error', error: String(e) })
    }
  }

  const handleDownload = async () => {
    onUpdateState({ phase: 'downloading', progress: 0, dismissed: true })
    try {
      await downloadUpdate()
    } catch (e) {
      onUpdateState({ phase: 'error', error: String(e) })
    }
  }

  const handleInstall = async () => {
    try {
      await installAndRestart()
    } catch (e) {
      onUpdateState({ phase: 'error', error: String(e) })
    }
  }

  const handleDismiss = () => {
    onUpdateState({ dismissed: true })
  }

  return (
    <div>
      <h2 className="content-title">{t('about.title')}</h2>

      <div className="about-header">
        <div className="about-app-icon">
          <svg width="36" height="38" viewBox="0 0 92 94">
            <rect x="1" y="30" width="16" height="34" rx="8" fill="white"/>
            <rect x="19.5" y="12" width="16" height="70" rx="8" fill="white"/>
            <rect x="38" y="2" width="16" height="90" rx="8" fill="white"/>
            <rect x="56.5" y="12" width="16" height="70" rx="8" fill="white"/>
            <rect x="75" y="30" width="16" height="34" rx="8" fill="white"/>
          </svg>
        </div>
        <div className="about-app-name">ByeType</div>
        <div className="about-app-version">{t('about.version', { version: appVersion })}</div>
      </div>

      {(phase === 'idle' || phase === 'checking') && (
        <div className="about-check-area">
          <button
            className="update-btn update-btn-primary"
            onClick={handleCheck}
            disabled={phase === 'checking'}
          >
            {phase === 'checking' ? t('about.checking') : t('about.checkUpdate')}
          </button>
        </div>
      )}

      {phase === 'idle' && info === null && updateState.checkedOnce && (
        <div className="about-status success">{t('about.upToDate')}</div>
      )}

      {phase === 'available' && info && dismissed && (
        <div className="update-collapsed">
          <span className="update-collapsed-text">{t('about.versionAvailable', { version: info.version })}</span>
          <button className="update-btn update-btn-primary" onClick={handleDownload}>
            {t('about.updateNow')}
          </button>
        </div>
      )}

      {phase === 'available' && info && !dismissed && (
        <div className="update-card">
          <div className="update-card-header">
            <span className="update-card-version">v{info.version}</span>
            <span className="update-badge">{t('about.newVersion')}</span>
          </div>
          {info.body && (
            <ul className="update-changelog">
              {info.body.split('\n').filter(line => line.trim()).map((line, i) => (
                <li key={i}>{line.replace(/^[-*]\s*/, '')}</li>
              ))}
            </ul>
          )}
          <div className="update-actions">
            <button className="update-btn update-btn-primary" onClick={handleDownload}>
              {t('about.updateNow')}
            </button>
            <button className="update-btn update-btn-secondary" onClick={handleDismiss}>
              {t('about.later')}
            </button>
          </div>
        </div>
      )}

      {phase === 'downloading' && (
        <div className="update-card">
          <div className="update-card-header">
            <span className="update-card-version">{t('about.downloading', { version: info?.version ?? '' })}</span>
          </div>
          <div className="update-progress-area">
            <div className="update-progress-bar">
              <div className="update-progress-fill" style={{ width: `${progress}%` }} />
            </div>
            <div className="update-progress-text">{Math.round(progress)}%</div>
          </div>
        </div>
      )}

      {phase === 'downloaded' && (
        <div className="update-card">
          <div className="update-card-header">
            <span className="update-card-version">{t('about.ready', { version: info?.version ?? '' })}</span>
          </div>
          <div className="update-actions">
            <button className="update-btn update-btn-primary" onClick={handleInstall}>
              {t('about.installRestart')}
            </button>
          </div>
        </div>
      )}

      {phase === 'error' && (
        <div>
          <div className="update-error">{error}</div>
          <div className="about-check-area">
            <button className="update-btn update-btn-primary" onClick={handleCheck}>
              {t('common.retry')}
            </button>
          </div>
        </div>
      )}

      <SettingGroup>
        <SettingRow label={t('about.currentVersion')}>
          <span style={{ fontSize: 13, color: 'var(--text-secondary)' }}>{appVersion}</span>
        </SettingRow>
      </SettingGroup>
    </div>
  )
}
