import { listen } from '@tauri-apps/api/event'
import { useEffect } from 'react'
import { If } from 'react-if-lite'
import { useStore } from 'valtio-define'
import { useDshTheme } from '../hooks/use-dsh-theme'
import { store } from '../store'
import { DesktopUpdater } from './components/desktop-updater'
import { DownloadToast } from './components/download-toast-trigger'
import { Webview } from './components/webview'
import '../i18n'
/**
 * 应用根布局：只负责首次启动与整体壳层结构。
 * 业务状态与操作方法全部收敛到 valtio-define store，
 * 各子组件自行订阅 store，不再通过 props 透传回调与状态。
 * 弹出层（关于 / 检查更新 / 应用配置）统一由 overlastic 命令式打开，
 * 仅在需要时挂载，不常驻渲染。
 */
export function App() {
  useDshTheme()
  const { status } = useStore(store.harness)
  // 首次挂载自动启动 harness（store 内部对 StrictMode 重复挂载去重）
  useEffect(() => {
    store.harness.startup()
  }, [])

  // 关闭窗口只会隐藏到托盘；再次打开不会重新挂载 React，因此监听后端统一的
  // 激活事件，在现有页面中重新核对并按需启动 dsh。
  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined

    async function listenWindowActivation() {
      const stop = await listen('desktop-window-activated', () => {
        void store.harness.resumeOnActivation()
      })
      if (disposed)
        stop()
      else
        unlisten = stop
    }

    void listenWindowActivation()
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  return (
    <div className="flex h-screen w-screen">
      <Webview />
      <If cond={status === 'ready'}>
        <DownloadToast />
      </If>
      <DesktopUpdater />
    </div>
  )
}
