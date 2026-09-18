import type { PropsWithOverlays } from '@overlastic/react'
import { ArrowUpRightFromSquare } from '@gravity-ui/icons'
import { Button, Description, Modal } from '@heroui/react'
import { useDisclosure } from '@overlastic/react'
import { invoke } from '@tauri-apps/api/core'
import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { useStore } from 'valtio-define'
import { store } from '@/store'
import { Info } from './info'

export interface DesktopAboutDialogProps extends PropsWithOverlays {}

/**
 * 「关于 Desktop」对话框：展示 Powered by、版本号、发布时间与版权信息。
 *
 * 使用 overlastic 命令式打开（`useOverlay(DesktopAboutDialog)`），
 * 打开时通过 store 的 `loadAbout` 拉取信息，关闭由 `disclosure.cancel` 完成。
 */
export function DesktopAboutDialog(props: DesktopAboutDialogProps) {
  const disclosure = useDisclosure({ props })
  const { t } = useTranslation()
  const { about } = useStore(store.desktopUpdater)

  useEffect(() => {
    if (disclosure.visible)
      void store.desktopUpdater.loadAbout()
  }, [disclosure.visible])

  return (
    <Modal isOpen={disclosure.visible} onOpenChange={disclosure.cancel}>
      <Modal.Backdrop>
        <Modal.Container size="xs">
          <Modal.Dialog>
            <Modal.CloseTrigger />
            <Modal.Body className="space-y-3">
              <div className="flex flex-col items-center gap-2 pt-1 text-center">
                <img src="/favicon.svg" alt={t('about.title')} className="w-12 h-12 rounded-md" />

                <div className="text-base font-semibold text-ink">
                  {about?.powered_by ?? 'DSCode'}
                </div>
                <Description className="text-xs">
                  {t('about.powered_by', { name: about?.powered_by ?? 'Hairy & DeepSeek' })}
                </Description>
              </div>
              <div className="space-y-1.5 border-t border-line/40 pt-3">
                <Info term={t('ui.current_version')}>{about?.version ?? '-'}</Info>
                <Info term={t('about.release_date')}>{about?.published_at ? formatDate(about.published_at) : '-'}</Info>
                <div className="flex items-center justify-between text-sm border-t border-line/40 pt-2 ">
                  <span className="text-muted">{t('about.source_code')}</span>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="rounded-md h-8"
                    onPress={() => {
                      if (about?.repo)
                        void invoke('open_external_url', { url: about.repo })
                    }}
                  >
                    {t('about.github')}
                    <ArrowUpRightFromSquare />
                  </Button>
                </div>
                <Description className="text-xs pt-2 flex justify-center text-center">
                  {about?.copyright ?? '-'}
                </Description>
              </div>

            </Modal.Body>

          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </Modal>
  )
}

function formatDate(iso: string): string {
  try {
    const d = new Date(iso)
    if (Number.isNaN(d.getTime()))
      return iso
    return d.toLocaleDateString()
  }
  catch {
    return iso
  }
}
