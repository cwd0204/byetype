import { useState, useCallback } from 'react'
import { SettingGroup } from '../components/SettingGroup'
import { SettingRow } from '../components/SettingRow'
import type { AppConfig, BackupEntry } from '../../../core/types'
import {
  testS3Connection,
  backupToS3,
  listS3Backups,
  restoreFromS3,
  backupToLocal,
  restoreFromLocal,
} from '../../../lib/tauri-api'
import { getLang, t, useLang } from '../../../i18n'

interface Props {
  config: AppConfig
  onSave: (config: AppConfig) => void
}

export function BackupTab({ config, onSave }: Props) {
  useLang()
  const [testing, setTesting] = useState(false)
  const [testResult, setTestResult] = useState<{ ok: boolean; msg: string } | null>(null)
  const [s3Backing, setS3Backing] = useState(false)
  const [s3Restoring, setS3Restoring] = useState(false)
  const [localBacking, setLocalBacking] = useState(false)
  const [localRestoring, setLocalRestoring] = useState(false)
  const [backups, setBackups] = useState<BackupEntry[]>([])
  const [loadingBackups, setLoadingBackups] = useState(false)
  const [message, setMessage] = useState<{ type: 'success' | 'error'; text: string } | null>(null)

  const s3 = config.backup.s3

  const updateS3 = (changes: Partial<typeof s3>) => {
    onSave({ ...config, backup: { ...config.backup, s3: { ...s3, ...changes } } })
  }

  const showMessage = (type: 'success' | 'error', text: string) => {
    setMessage({ type, text })
    setTimeout(() => setMessage(null), 5000)
  }

  const handleTestConnection = useCallback(async () => {
    setTesting(true)
    setTestResult(null)
    try {
      await testS3Connection()
      setTestResult({ ok: true, msg: t('backup.connected') })
    } catch (e: any) {
      setTestResult({ ok: false, msg: String(e) })
    } finally {
      setTesting(false)
    }
  }, [])

  const refreshBackups = useCallback(async () => {
    setLoadingBackups(true)
    try {
      const list = await listS3Backups()
      setBackups(list)
    } catch (e: any) {
      setBackups([])
    } finally {
      setLoadingBackups(false)
    }
  }, [])

  const handleBackupToS3 = useCallback(async () => {
    setS3Backing(true)
    try {
      const key = await backupToS3()
      showMessage('success', t('backup.backupOk', { key }))
      await refreshBackups()
    } catch (e: any) {
      showMessage('error', t('backup.backupFailed', { error: String(e) }))
    } finally {
      setS3Backing(false)
    }
  }, [refreshBackups])

  const handleRestoreFromS3 = useCallback(async (key: string) => {
    if (!confirm(t('backup.restoreS3Confirm', { key }))) return
    setS3Restoring(true)
    try {
      await restoreFromS3(key)
      showMessage('success', t('backup.restoreOk'))
    } catch (e: any) {
      showMessage('error', t('backup.restoreFailed', { error: String(e) }))
    } finally {
      setS3Restoring(false)
    }
  }, [])

  const handleBackupToLocal = useCallback(async () => {
    setLocalBacking(true)
    try {
      const path = await backupToLocal()
      showMessage('success', t('backup.savedTo', { path }))
    } catch (e: any) {
      // 后端取消选择时返回的中文错误文案，用于判定「用户取消」而非展示
      if (String(e).includes('未选择')) return
      showMessage('error', t('backup.backupFailed', { error: String(e) }))
    } finally {
      setLocalBacking(false)
    }
  }, [])

  const handleRestoreFromLocal = useCallback(async () => {
    if (!confirm(t('backup.restoreLocalConfirm'))) return
    setLocalRestoring(true)
    try {
      await restoreFromLocal()
      showMessage('success', t('backup.restoreOk'))
    } catch (e: any) {
      // 同上：匹配后端「未选择备份文件」的取消错误
      if (String(e).includes('未选择')) return
      showMessage('error', t('backup.restoreFailed', { error: String(e) }))
    } finally {
      setLocalRestoring(false)
    }
  }, [])

  const formatSize = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  }

  const formatDate = (s: string) => {
    const d = new Date(s)
    if (isNaN(d.getTime())) return s
    return d.toLocaleString(getLang() === 'en' ? 'en-US' : 'zh-CN')
  }

  return (
    <div>
      <h2 className="content-title">{t('backup.title')}</h2>

      {message && (
        <div className="update-collapsed" style={{
          background: message.type === 'success' ? 'rgba(48, 209, 88, 0.1)' : 'rgba(255, 69, 58, 0.1)',
          marginBottom: 20,
        }}>
          <span className="update-collapsed-text" style={{
            color: message.type === 'success' ? 'var(--success-color)' : '#ff453a',
          }}>
            {message.text}
          </span>
        </div>
      )}

      <SettingGroup title={t('backup.group.s3Config')}>
        <SettingRow label="Endpoint" description={t('backup.endpointDesc')}>
          <input
            type="text"
            className="input"
            value={s3.endpoint}
            onChange={e => updateS3({ endpoint: e.target.value })}
            placeholder="https://s3.amazonaws.com"
            style={{ width: 240 }}
          />
        </SettingRow>
        <SettingRow label="Region">
          <input
            type="text"
            className="input"
            value={s3.region}
            onChange={e => updateS3({ region: e.target.value })}
            placeholder="us-east-1"
            style={{ width: 240 }}
          />
        </SettingRow>
        <SettingRow label="Bucket">
          <input
            type="text"
            className="input"
            value={s3.bucket}
            onChange={e => updateS3({ bucket: e.target.value })}
            placeholder="my-backup-bucket"
            style={{ width: 240 }}
          />
        </SettingRow>
        <SettingRow label="Access Key">
          <input
            type="text"
            className="input"
            value={s3.accessKey}
            onChange={e => updateS3({ accessKey: e.target.value })}
            placeholder="AKIAXXXXX"
            style={{ width: 240 }}
          />
        </SettingRow>
        <SettingRow label="Secret Key">
          <input
            type="password"
            className="input"
            value={s3.secretKey}
            onChange={e => updateS3({ secretKey: e.target.value })}
            placeholder="******"
            style={{ width: 240 }}
          />
        </SettingRow>
        <SettingRow label={t('backup.prefix')} description={t('backup.prefixDesc')}>
          <input
            type="text"
            className="input"
            value={s3.prefix}
            onChange={e => updateS3({ prefix: e.target.value })}
            placeholder="byetype/backups"
            style={{ width: 240 }}
          />
        </SettingRow>
        <SettingRow label={t('backup.connectionTest')}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <button className="model-test-btn" onClick={handleTestConnection} disabled={testing}>
              {testing ? t('backup.testing') : t('backup.testConnection')}
            </button>
            {testResult && (
              <span className="model-test-result" style={{
                color: testResult.ok ? 'var(--success-color)' : '#ff453a',
              }}>
                {testResult.ok ? '✓' : '✗'} {testResult.msg}
              </span>
            )}
          </div>
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t('backup.group.s3')}>
        <SettingRow label={t('backup.backupToS3Now')} description={t('backup.backupToS3Desc')}>
          <button className="model-test-btn" onClick={handleBackupToS3} disabled={s3Backing || !s3.bucket}>
            {s3Backing ? t('backup.backingUp') : t('backup.backupToS3')}
          </button>
        </SettingRow>
        <SettingRow label={t('backup.restoreFromS3')} description={t('backup.restoreFromS3Desc')}>
          <button className="model-test-btn" onClick={refreshBackups} disabled={loadingBackups || !s3.bucket}>
            {loadingBackups ? t('backup.loading') : t('backup.refreshList')}
          </button>
        </SettingRow>
        {backups.length > 0 && (
          <div style={{ margin: '8px 16px', maxHeight: 260, overflow: 'auto' }}>
            <table style={{ width: '100%', fontSize: 12, borderCollapse: 'collapse' }}>
              <thead>
                <tr style={{ borderBottom: '1px solid var(--border-color-light)' }}>
                  <th style={{ textAlign: 'left', padding: '4px 8px', color: 'var(--text-secondary)' }}>{t('backup.col.file')}</th>
                  <th style={{ textAlign: 'left', padding: '4px 8px', color: 'var(--text-secondary)' }}>{t('backup.col.size')}</th>
                  <th style={{ textAlign: 'left', padding: '4px 8px', color: 'var(--text-secondary)' }}>{t('backup.col.time')}</th>
                  <th style={{ padding: '4px 8px' }}></th>
                </tr>
              </thead>
              <tbody>
                {backups.map(b => (
                  <tr key={b.key} style={{ borderBottom: '1px solid var(--border-color-light)' }}>
                    <td style={{ padding: '4px 8px', color: 'var(--text-primary)' }}>{b.key.split('/').pop()}</td>
                    <td style={{ padding: '4px 8px', color: 'var(--text-secondary)' }}>{formatSize(b.size)}</td>
                    <td style={{ padding: '4px 8px', color: 'var(--text-secondary)' }}>{formatDate(b.lastModified)}</td>
                    <td style={{ padding: '4px 8px' }}>
                      <button
                        className="model-test-btn"
                        onClick={() => handleRestoreFromS3(b.key)}
                        disabled={s3Restoring}
                      >
                        {t('backup.restore')}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </SettingGroup>

      <SettingGroup title={t('backup.group.local')}>
        <SettingRow label={t('backup.backupToLocal')} description={t('backup.backupToLocalDesc')}>
          <button className="model-test-btn" onClick={handleBackupToLocal} disabled={localBacking}>
            {localBacking ? t('backup.backingUp') : t('backup.backupToLocal')}
          </button>
        </SettingRow>
        <SettingRow label={t('backup.restoreFromLocal')} description={t('backup.restoreFromLocalDesc')}>
          <button className="model-test-btn" onClick={handleRestoreFromLocal} disabled={localRestoring}>
            {localRestoring ? t('backup.restoring') : t('backup.restoreFromLocal')}
          </button>
        </SettingRow>
      </SettingGroup>
    </div>
  )
}
