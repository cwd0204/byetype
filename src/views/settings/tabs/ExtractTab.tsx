import type { AppConfig } from '../../../core/types'
import { getVisionModels } from '../../../core/models'
import { t, useLang } from '../../../i18n'
import { SettingGroup } from '../components/SettingGroup'
import { SettingRow } from '../components/SettingRow'

interface Props {
  config: AppConfig
  onSave: (config: AppConfig) => void
}

export function ExtractTab({ config, onSave }: Props) {
  useLang()
  const { extract } = config

  const visionModels = getVisionModels(config)
  const builtinVision = visionModels.filter(m => m.builtin)
  const customVision = visionModels.filter(m => !m.builtin)

  const updateExtract = (changes: Partial<AppConfig['extract']>) => {
    onSave({ ...config, extract: { ...extract, ...changes } })
  }

  return (
    <div>
      <h2 className="content-title">{t('extract.title')}</h2>

      <SettingGroup title={t('extract.group.model')}>
        <SettingRow label={t('extract.model')}>
          <select
            className="select"
            value={extract.modelId || ''}
            onChange={e => updateExtract({ modelId: e.target.value || undefined })}
            style={{ width: 260 }}
          >
            <optgroup label={t('common.builtinModels')}>
              {builtinVision.map(m => <option key={m.id} value={m.id}>{m.provider} - {m.model}</option>)}
            </optgroup>
            {customVision.length > 0 && (
              <optgroup label={t('common.customModels')}>
                {customVision.map(m => <option key={m.id} value={m.id}>{m.provider} - {m.model}</option>)}
              </optgroup>
            )}
          </select>
        </SettingRow>
      </SettingGroup>
    </div>
  )
}
