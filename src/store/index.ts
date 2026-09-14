import { desktopUpdater } from './modules/desktop-updater'
import { download } from './modules/download'
import { harness } from './modules/harness'
import { setting } from './modules/setting'

/** 全局 store 聚合（参考 damn-reports 的组织方式：模块各自独立，聚合统一出口） */
export const store = {
  harness,
  download,
  setting,
  desktopUpdater,
}
