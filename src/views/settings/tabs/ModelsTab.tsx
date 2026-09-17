import { useState } from 'react'
import type { AppConfig, AwsConfig, CustomModelEntry } from '../../../core/types'
import { BUILTIN_MODELS, getAllModels } from '../../../core/models'
import { testModelConnectivity, type ConnectivityResult } from '../../../lib/tauri-api'
import { t, useLang } from '../../../i18n'

interface Props {
  config: AppConfig
  onSave: (config: AppConfig) => void
}

interface TestResults {
  [modelId: string]: { loading: boolean; result?: ConnectivityResult }
}

type CustomForm = Omit<CustomModelEntry, 'id'>

const EMPTY_FORM: CustomForm = {
  provider: 'Amazon Bedrock',
  model: '',
  protocol: 'bedrock',
  supportsText: true,
  supportsVision: true,
}

export function ModelsTab({ config, onSave }: Props) {
  useLang()
  const [testResults, setTestResults] = useState<TestResults>({})
  const [showForm, setShowForm] = useState(false)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [form, setForm] = useState<CustomForm>(EMPTY_FORM)

  const aws = config.models.aws
  const updateAws = (changes: Partial<AwsConfig>) => {
    onSave({ ...config, models: { ...config.models, aws: { ...aws, ...changes } } })
  }

  const testModel = async (modelId: string) => {
    setTestResults(prev => ({ ...prev, [modelId]: { loading: true } }))
    try {
      const result = await testModelConnectivity(modelId)
      setTestResults(prev => ({ ...prev, [modelId]: { loading: false, result } }))
    } catch (e) {
      setTestResults(prev => ({ ...prev, [modelId]: { loading: false, result: { success: false, latencyMs: 0, error: String(e) } } }))
    }
  }

  const testAll = async () => {
    for (const m of getAllModels(config)) { testModel(m.id) }
  }

  const saveCustomModel = () => {
    const id = editingId || crypto.randomUUID()
    const entry: CustomModelEntry = { ...form, id, model: form.model.trim(), provider: form.provider.trim() || 'Amazon Bedrock', protocol: 'bedrock' }
    const custom = editingId
      ? config.models.custom.map(m => (m.id === editingId ? entry : m))
      : [...config.models.custom, entry]
    onSave({ ...config, models: { ...config.models, custom } })
    cancelForm()
  }

  const deleteCustomModel = (id: string) => {
    onSave({ ...config, models: { ...config.models, custom: config.models.custom.filter(m => m.id !== id) } })
  }

  const startEdit = (entry: CustomModelEntry) => {
    setEditingId(entry.id)
    setForm({ provider: entry.provider, model: entry.model, protocol: 'bedrock', supportsText: entry.supportsText ?? true, supportsVision: entry.supportsVision ?? true })
    setShowForm(true)
  }

  const cancelForm = () => { setShowForm(false); setEditingId(null); setForm(EMPTY_FORM) }

  const renderTestResult = (modelId: string) => {
    const res = testResults[modelId]
    if (!res) return null
    if (res.loading) return <span className="model-test-result" style={{ color: 'var(--text-tertiary)' }}>...</span>
    if (res.result?.success) return <span className="model-test-result success">{res.result.latencyMs}ms</span>
    return <span className="model-test-result error" title={res.result?.error || ''}>{res.result?.error?.slice(0, 30) || t('common.failed')}</span>
  }

  const renderCaps = (m: { supportsAudio: boolean; supportsVision: boolean; supportsText: boolean }) => (
    <span className="model-caps">
      {m.supportsAudio && <span className="cap-tag cap-audio">{t('models.cap.audio')}</span>}
      {m.supportsVision && <span className="cap-tag cap-vision">{t('models.cap.vision')}</span>}
      {m.supportsText && <span className="cap-tag cap-text">{t('models.cap.text')}</span>}
    </span>
  )

  const bedrockModels = BUILTIN_MODELS.filter(m => m.protocol === 'bedrock')
  const transcribeModels = BUILTIN_MODELS.filter(m => m.protocol === 'aws-transcribe')

  const profileField = (label: string, value: string, onChange: (v: string) => void, placeholder: string) => (
    <div className="model-card-row">
      <label>{label}</label>
      <input className="input" value={value} onChange={e => onChange(e.target.value)} placeholder={placeholder} style={{ maxWidth: 240 }} spellCheck={false} />
    </div>
  )

  return (
    <div>
      <div className="models-header">
        <h2 className="content-title" style={{ margin: 0 }}>{t('models.title')}</h2>
        <button className="test-all-btn" onClick={testAll}>{t('models.testAll')}</button>
      </div>

      <div className="models-section-title">Amazon Bedrock</div>
      <div className="model-card">
        <div className="model-card-header">
          <span className="model-card-title">Claude on Bedrock</span>
        </div>
        {profileField('AWS Profile', aws.bedrockProfile, v => updateAws({ bedrockProfile: v }), 'default')}
        {profileField('Region', aws.bedrockRegion, v => updateAws({ bedrockRegion: v }), 'us-east-1')}
        <div className="model-card-subtitle">
          {t('models.bedrockDesc')}
        </div>
        <div className="provider-model-list">
          {bedrockModels.map(m => (
            <div key={m.id} className="provider-model-item">
              <span className="provider-model-name">{m.model}</span>
              {renderCaps(m)}
              <div className="model-card-actions">
                {renderTestResult(m.id)}
                <button className="model-test-btn" onClick={() => testModel(m.id)}>{t('models.test')}</button>
              </div>
            </div>
          ))}
        </div>
      </div>

      <div className="models-section-title">Amazon Transcribe</div>
      <div className="model-card">
        <div className="model-card-header">
          <span className="model-card-title">{t('models.transcribeTitle')}</span>
        </div>
        {profileField('AWS Profile', aws.transcribeProfile, v => updateAws({ transcribeProfile: v }), 'default')}
        {profileField('Region', aws.transcribeRegion, v => updateAws({ transcribeRegion: v }), 'us-east-1')}
        <div className="model-card-row">
          <label>{t('models.transcribeLanguage')}</label>
          <select className="input" value={aws.transcribeLanguage} onChange={e => updateAws({ transcribeLanguage: e.target.value })} style={{ maxWidth: 240 }}>
            <option value="auto">{t('models.lang.auto')}</option>
            <option value="zh-CN">{t('models.lang.zh')}</option>
            <option value="en-US">{t('models.lang.en')}</option>
            <option value="ja-JP">{t('models.lang.ja')}</option>
          </select>
        </div>
        <div className="model-card-subtitle">
          {t('models.transcribeDesc')}
        </div>
        <div className="provider-model-list">
          {transcribeModels.map(m => (
            <div key={m.id} className="provider-model-item">
              <span className="provider-model-name">{m.model}</span>
              {renderCaps(m)}
              <div className="model-card-actions">
                {renderTestResult(m.id)}
                <button className="model-test-btn" onClick={() => testModel(m.id)}>{t('models.test')}</button>
              </div>
            </div>
          ))}
        </div>
      </div>

      <div className="models-section-title">{t('models.customTitle')}</div>
      {config.models.custom.map(entry => (
        <div key={entry.id} className="model-card">
          <div className="model-card-header">
            <span className="model-card-title">{entry.provider} - {entry.model}</span>
            <div className="model-card-actions">
              {renderTestResult(entry.id)}
              <button className="model-test-btn" onClick={() => testModel(entry.id)}>{t('models.test')}</button>
              <button className="model-action-btn" onClick={() => startEdit(entry)}>{t('common.edit')}</button>
              <button className="model-action-btn danger" onClick={() => deleteCustomModel(entry.id)}>{t('common.delete')}</button>
            </div>
          </div>
          <div className="model-card-subtitle">
            {t('models.customDesc')}
            {renderCaps({ supportsAudio: false, supportsVision: entry.supportsVision ?? true, supportsText: entry.supportsText ?? true })}
          </div>
        </div>
      ))}

      {showForm ? (
        <div className="model-form">
          <div style={{ fontWeight: 600, fontSize: 13, marginBottom: 12, color: 'var(--text-primary)' }}>{editingId ? t('models.editModel') : t('models.newModel')}</div>
          <div className="model-form-row"><label>{t('models.displayName')}</label><input className="input" value={form.provider} onChange={e => setForm(f => ({ ...f, provider: e.target.value }))} placeholder="Amazon Bedrock" style={{ flex: 1, maxWidth: 300 }} /></div>
          <div className="model-form-row"><label>Model ID</label><input className="input" value={form.model} onChange={e => setForm(f => ({ ...f, model: e.target.value }))} placeholder="global.amazon.nova-2-lite-v1:0" style={{ flex: 1, maxWidth: 400 }} spellCheck={false} /></div>
          <div className="model-form-row">
            <label>{t('models.capabilities')}</label>
            <div style={{ display: 'flex', gap: 12 }}>
              <label style={{ display: 'flex', alignItems: 'center', gap: 4, fontSize: 12, cursor: 'pointer', color: 'var(--text-primary)' }}>
                <input type="checkbox" checked={form.supportsText} onChange={e => setForm(f => ({ ...f, supportsText: e.target.checked }))} /> {t('models.capText')}
              </label>
              <label style={{ display: 'flex', alignItems: 'center', gap: 4, fontSize: 12, cursor: 'pointer', color: 'var(--text-primary)' }}>
                <input type="checkbox" checked={form.supportsVision} onChange={e => setForm(f => ({ ...f, supportsVision: e.target.checked }))} /> {t('models.capVision')}
              </label>
            </div>
          </div>
          <div className="model-card-subtitle" style={{ marginBottom: 8 }}>
            {t('models.formHint')}
          </div>
          <div className="model-form-actions">
            <button className="model-form-btn" onClick={cancelForm}>{t('common.cancel')}</button>
            <button className="model-form-btn primary" onClick={saveCustomModel} disabled={!form.model.trim() || (!form.supportsText && !form.supportsVision)}>{t('common.save')}</button>
          </div>
        </div>
      ) : (
        <button className="add-model-btn" onClick={() => { setEditingId(null); setForm(EMPTY_FORM); setShowForm(true) }}>{t('models.addModel')}</button>
      )}
    </div>
  )
}
