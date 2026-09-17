import type { AppConfig, ThinkingConfig } from '../../../core/types'
import { getAudioModels, getTextModels } from '../../../core/models'
import { t, useLang } from '../../../i18n'
import { SettingGroup } from '../components/SettingGroup'
import { SettingRow } from '../components/SettingRow'
import { Toggle } from '../components/Toggle'

interface Props {
  config: AppConfig
  onSave: (config: AppConfig) => void
}

export function TranscribeTab({ config, onSave }: Props) {
  useLang()
  const { transcribe, voiceTemplates } = config

  const audioModels = getAudioModels(config)
  const textModels = getTextModels(config)

  const updateTranscribe = (changes: Partial<AppConfig['transcribe']>) => {
    onSave({ ...config, transcribe: { ...transcribe, ...changes } })
  }

  const updateVoiceTemplates = (changes: Partial<AppConfig['voiceTemplates']>) => {
    onSave({ ...config, voiceTemplates: { ...voiceTemplates, ...changes } })
  }

  const updateVoiceTemplatesThinking = (changes: Partial<ThinkingConfig>) => {
    updateVoiceTemplates({ thinking: { ...voiceTemplates.thinking, ...changes } })
  }

  const builtinText = textModels.filter(m => m.builtin)
  const customText = textModels.filter(m => !m.builtin)

  return (
    <div>
      <h2 className="content-title">{t('transcribe.title')}</h2>

      {/* 区域一：转写模型 */}
      <SettingGroup title={t('transcribe.group.model')}>
        <SettingRow label={t('transcribe.model')} description={t('transcribe.modelDesc')}>
          <select
            className="select"
            value={transcribe.modelId}
            onChange={e => updateTranscribe({ modelId: e.target.value })}
            style={{ width: 260 }}
          >
            {audioModels.map(m => <option key={m.id} value={m.id}>{m.provider} - {m.model}</option>)}
          </select>
        </SettingRow>
      </SettingGroup>

      {/* 区域二：文本优化模型 */}
      <h3 className="section-title">{t('transcribe.optimizeSection')}</h3>

      <SettingGroup title={t('transcribe.group.model')}>
        <SettingRow label={t('transcribe.optimizeModel')} description={t('transcribe.optimizeModelDesc')}>
          <select
            className="select"
            value={voiceTemplates.modelId}
            onChange={e => updateVoiceTemplates({ modelId: e.target.value })}
            style={{ width: 260 }}
          >
            <optgroup label={t('common.builtinModels')}>
              {builtinText.map(m => <option key={m.id} value={m.id}>{m.provider} - {m.model}</option>)}
            </optgroup>
            {customText.length > 0 && (
              <optgroup label={t('common.customModels')}>
                {customText.map(m => <option key={m.id} value={m.id}>{m.provider} - {m.model}</option>)}
              </optgroup>
            )}
          </select>
        </SettingRow>
        <SettingRow label={t('common.thinking.enable')} description={t('transcribe.thinkingDesc')}>
          <Toggle
            checked={voiceTemplates.thinking.enabled}
            onChange={checked => updateVoiceTemplatesThinking({ enabled: checked })}
          />
        </SettingRow>
        {voiceTemplates.thinking.enabled && (
          <SettingRow label={t('common.thinking.level')} description={t('common.thinking.levelDesc')}>
            <select
              className="select"
              value={voiceTemplates.thinking.level}
              onChange={e => updateVoiceTemplatesThinking({ level: e.target.value as ThinkingConfig['level'] })}
              style={{ width: 120 }}
            >
              <option value="LOW">LOW</option>
              <option value="MEDIUM">MEDIUM</option>
              <option value="HIGH">HIGH</option>
            </select>
          </SettingRow>
        )}
      </SettingGroup>

      {/* 区域三：其他 */}
      <h3 className="section-title">{t('transcribe.otherSection')}</h3>

      <SettingGroup>
        <SettingRow
          label={t('transcribe.ruleBoost')}
          description={t('transcribe.ruleBoostDesc')}
        >
          <Toggle checked disabled onChange={() => {}} />
        </SettingRow>
      </SettingGroup>
    </div>
  )
}
