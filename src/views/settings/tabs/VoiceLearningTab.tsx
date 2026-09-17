import { useMemo } from 'react'
import type { AppConfig, ThinkingConfig } from '../../../core/types'
import { getTextModels } from '../../../core/models'
import {
  getVoiceLearningDocument,
  getVoiceLearningPromptPath,
  saveVoiceLearningDocument,
} from '../../../lib/tauri-api'
import { t, useLang } from '../../../i18n'
import { PromptEditor, type PromptFileEntry } from '../components/PromptEditor'
import { SettingGroup } from '../components/SettingGroup'
import { SettingRow } from '../components/SettingRow'
import { Toggle } from '../components/Toggle'

interface Props {
  config: AppConfig
  onSave: (config: AppConfig) => void
}

export function VoiceLearningTab({ config, onSave }: Props) {
  const lang = useLang()
  const textModels = getTextModels(config)
  const builtinModels = textModels.filter(model => model.builtin)
  const customModels = textModels.filter(model => !model.builtin)

  // label 随语言变化，其余字段不变；按 lang 记忆化以保持 promptFiles 引用稳定
  const learningFiles = useMemo<PromptFileEntry[]>(() => [
    {
      key: 'voice-learning-prompt',
      label: t('learningTab.file.prompt'),
      resolvePath: getVoiceLearningPromptPath,
    },
    {
      key: 'voice-learning-result',
      label: t('learningTab.file.result'),
      loadContent: getVoiceLearningDocument,
      saveContent: saveVoiceLearningDocument,
      refreshEvent: 'voice-learning-updated',
    },
  ], [lang])

  const updateModel = (modelId: string) => {
    onSave({
      ...config,
      voiceLearning: { ...config.voiceLearning, modelId },
    })
  }

  const updateThinking = (changes: Partial<ThinkingConfig>) => {
    onSave({
      ...config,
      voiceLearning: {
        ...config.voiceLearning,
        thinking: { ...config.voiceLearning.thinking, ...changes },
      },
    })
  }

  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0, overflow: 'auto' }}>
      <h2 className="content-title" style={{ flexShrink: 0 }}>{t('learningTab.title')}</h2>

      <SettingGroup title={t('learningTab.group.model')}>
        <SettingRow label={t('learningTab.model')} description={t('learningTab.modelDesc')}>
          <select
            className="select"
            value={config.voiceLearning.modelId}
            onChange={event => updateModel(event.target.value)}
            style={{ width: 260 }}
          >
            <optgroup label={t('common.builtinModels')}>
              {builtinModels.map(model => (
                <option key={model.id} value={model.id}>{model.provider} - {model.model}</option>
              ))}
            </optgroup>
            {customModels.length > 0 && (
              <optgroup label={t('common.customModels')}>
                {customModels.map(model => (
                  <option key={model.id} value={model.id}>{model.provider} - {model.model}</option>
                ))}
              </optgroup>
            )}
          </select>
        </SettingRow>
        <SettingRow label={t('common.thinking.enable')} description={t('learningTab.thinkingDesc')}>
          <Toggle
            checked={config.voiceLearning.thinking.enabled}
            onChange={enabled => updateThinking({ enabled })}
          />
        </SettingRow>
        {config.voiceLearning.thinking.enabled && (
          <SettingRow label={t('common.thinking.level')} description={t('common.thinking.levelDesc')}>
            <select
              className="select"
              value={config.voiceLearning.thinking.level}
              onChange={event => updateThinking({ level: event.target.value as ThinkingConfig['level'] })}
              style={{ width: 120 }}
            >
              <option value="LOW">LOW</option>
              <option value="MEDIUM">MEDIUM</option>
              <option value="HIGH">HIGH</option>
            </select>
          </SettingRow>
        )}
      </SettingGroup>

      <h3 className="section-title">{t('learningTab.docsSection')}</h3>
      <PromptEditor config={config} onSave={onSave} promptFiles={learningFiles} editorHeight={320} />
    </div>
  )
}
