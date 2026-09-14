import { invoke } from '@tauri-apps/api/core'
import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { PanelHeader } from './panel-header'
import { PanelState } from './panel-state'

interface BundledCore { version: string, path: string }

export function ConfigCore() {
  const { t } = useTranslation()
  const { data, isLoading, error } = useQuery({
    queryKey: ['cores'],
    queryFn: () => invoke<BundledCore[]>('get_cores'),
  })
  return (
    <div className="space-y-3">
      <PanelHeader title={t('core.title')} description={t('core.bundled_hint')} />
      <PanelState loading={isLoading} error={error?.message ?? ''}>
        <p className="text-sm">{t('core.bundled_version', { version: data?.[0]?.version || '—' })}</p>
        <p className="break-all text-xs text-muted">{data?.[0]?.path}</p>
      </PanelState>
    </div>
  )
}
