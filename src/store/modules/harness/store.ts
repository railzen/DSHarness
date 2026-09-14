/* eslint-disable no-control-regex */
import type { UnlistenFn } from '@tauri-apps/api/event'
import type {
  InstallerState,
  InstallProgress,
  SetupStatus,
  SidebarBusyAction,
} from './types'
import type { ReadinessProbeResult } from '@/utils/readiness'
import { emitter } from '@hairy/react-lib'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import i18next from 'i18next'
import { defineStore } from 'valtio-define'
import { queryClient } from '@/config/client'
import { containsInotifyLimitError, pickErrorLines } from '@/utils/log'
import { pollReadiness } from '@/utils/readiness'

const MAX_RETRIES = 8
const IFRAME_LOAD_TIMEOUT = 20000
/** 启动失败时从服务日志尾部挑选的原始行上限（ANSI 清洗后按行截断） */
const LOG_TAIL_MAX_BYTES = 16 * 1024

/** 启动失败错误：附带从 dsh 服务日志中读取的真实错误行与可选的冲突提示 */
interface StartupError extends Error {
  logs?: string[]
  /** Linux inotify 文件监视上限（ENOSPC）导致服务启动即崩溃时的针对性提示 */
  inotifyLimitHint?: string
  /** 初始就绪窗口已耗尽，但后端进程仍由桌面端持有，可继续后台探测 */
  readinessTimedOut?: boolean
}

const initialInstaller: InstallerState = {
  title: '',
  detail: '',
  percentage: 0,
  logs: [],
}

/** 启动流程令牌：boot 并发/重复调用时只采纳最后一次的结果 */
let bootToken = 0
/** 首次自动启动去重（React StrictMode 会重复挂载 effect） */
let bootStarted = false
/** 本轮 boot 是否已成功走通第一下 IPC。首个 invoke 挂死时保持 false，看门狗再开一轮。 */
let bootIpcReady = false
/** 冷启动看门狗：后端 auto_start 往往已经把 dsh 拉起来，前端却卡在第一下 IPC。 */
let coldStartWatchdog: ReturnType<typeof setInterval> | null = null
/** 首次开机启动失败时只自动重启一次，避免故障时无限循环。 */
let startupRecoveryAttempted = false
/** 窗口连续激活时只允许一轮服务恢复探测，避免托盘双击并发启动。 */
let activationRecoveryInProgress = false

function stopColdStartWatchdog() {
  if (!coldStartWatchdog)
    return
  clearInterval(coldStartWatchdog)
  coldStartWatchdog = null
}

/** 构建带时间戳的 iframe URL，避免 WebView2 缓存旧页面 */
function generateTimestampedUrl(baseUrl: string): string {
  const timestamp = Date.now()
  const separator = baseUrl.includes('?') ? '&' : '?'
  return `${baseUrl}${separator}t=${timestamp}`
}

/** 通过 Rust 代理探测服务健康状态（超时 8s，网络抖动时重试） */
async function checkHealthViaProxy(): Promise<ReadinessProbeResult> {
  try {
    const timeoutPromise = new Promise<never>((_, reject) => {
      setTimeout(() => reject(new Error('health check timeout')), 8000)
    })
    const resultPromise = invoke<string>('proxy_health_check')
    const result = await Promise.race([resultPromise, timeoutPromise])

    const lower = result.toLowerCase()
    if (
      lower.includes('healthy')
      || lower.includes('ready')
      || result.includes('200')
      || result.includes('201')
      || lower.includes('ok')
    ) {
      console.warn('[Harness] health check passed:', result.split(' - <!doctype html>')[0])
      return { healthy: true, notOwned: false }
    }
    console.warn('[Harness] health check returned:', result)
    return { healthy: false, notOwned: false }
  }
  catch (err) {
    const message = String(err)
    if (message.includes('HARNESS_NOT_OWNED')) {
      // dsh 进程已退出（典型如插件冲突导致启动即崩溃），继续等只会白白耗完
      // 8 轮超时，让调用方立刻结束重试并展示日志里的真实错误。
      console.warn('[Harness] dsh process exited during startup, failing fast')
      return { healthy: false, notOwned: true }
    }
    if (message.includes('502') || message.includes('Bad Gateway')) {
      console.warn('[Harness] transient 502 during health check, retrying')
    }
    else {
      console.error('[Harness] health check failed:', err)
    }
    return { healthy: false, notOwned: false }
  }
}

/** 读取服务日志尾部（去掉 ANSI 转义与空行），启动失败时展示真实错误 */
async function readServiceLogTail(): Promise<string[]> {
  try {
    const raw = await invoke<string>('read_service_logs', { maxBytes: LOG_TAIL_MAX_BYTES })
    return raw
      .split(/\r?\n/)
      .map(line => line.replace(/\x1B\[[0-9;]*m/g, '').trim())
      .filter(Boolean)
  }
  catch (err) {
    console.error('[Harness] failed to read service logs:', err)
    return []
  }
}

/** 失败时把服务日志的真实错误行与冲突提示挂到错误对象上 */
async function attachStartupDiagnostics(err: unknown): Promise<StartupError> {
  // Tauri `invoke` 对 `Result<_, String>` 命令的 rejection 是裸字符串，
  // 必须先归一化为 Error 对象，否则在其上赋属性（ESM 严格模式）会抛
  // `TypeError: Cannot create property ... on string`，反而遮蔽真实错误。
  const startupError: StartupError = err instanceof Error ? err : new Error(String(err))
  if (!startupError.logs) {
    const lines = await readServiceLogTail()
    startupError.logs = pickErrorLines(lines)
    // 识别 Linux inotify 文件监视上限（ENOSPC）：harness 服务启动即崩溃且用户无法直接解决，
    // 需要系统级调高 fs.inotify.max_user_watches（见 errors.inotify_limit 文案）
    if (containsInotifyLimitError(lines)) {
      startupError.inotifyLimitHint = i18next.t('errors.inotify_limit')
    }
  }
  return startupError as StartupError
}

/**
 * 桌面外壳核心业务模块：安装/启动流程、服务生命周期（启动/健康检查/重启/停止）、
 * iframe 加载状态与挂起兜底。
 *
 * 拆分说明（参考 damn-reports 的 store 组织方式）：
 * 版本更新与下载完成提示分别收敛到 updater / download 模块，
 * 本模块专注服务生命周期与页面加载状态。
 */
export const harness = defineStore({
  state: () => ({
    status: 'ready' as SetupStatus,
    installer: initialInstaller,
    errorMsg: '',
    /** 启动失败时从 dsh 服务日志中读取的真实错误行（Loadable 错误态日志面板） */
    errorLogs: [] as string[],
    /** 识别到 Linux inotify 文件监视上限（ENOSPC）时的针对性提示（Loadable children 展示） */
    inotifyLimitHint: '',
    serviceUrl: 'http://127.0.0.1:3080',
    /** 带时间戳的 iframe 地址（boot 时生成一次，避免缓存） */
    iframeSrc: '',
    iframeLoaded: false,
    iframeError: false,
    iframeKey: 0,
    serviceHealthy: false,
    serviceRunning: false,
    busyAction: null as SidebarBusyAction,
  }),
  actions: {
    /** 首次挂载时自动启动（StrictMode 重复挂载下保证只执行一次） */
    startup() {
      if (bootStarted)
        return
      bootStarted = true
      bootIpcReady = false
      void this.boot()
      // 托盘退出后再开：点「启动」能起来，是因为又开了一轮 boot。
      // 首个 invoke 若挂死，这里按同样方式自动再试，并尝试附着已在跑的 dsh。
      let ticks = 0
      let ticking = false
      stopColdStartWatchdog()
      const harness = this
      async function tickColdStartWatchdog() {
        if (ticking || harness.serviceHealthy) {
          if (harness.serviceHealthy)
            stopColdStartWatchdog()
          return
        }
        ticking = true
        try {
          ticks++
          await harness.tryAttachRunning()
          if (harness.serviceHealthy) {
            stopColdStartWatchdog()
            return
          }
          if (!bootIpcReady && ticks >= 2)
            void harness.boot()
          if (ticks >= 8)
            stopColdStartWatchdog()
        }
        finally {
          ticking = false
        }
      }
      void tickColdStartWatchdog()
      coldStartWatchdog = setInterval(() => {
        void tickColdStartWatchdog()
      }, 1500)
    },

    /**
     * 后端 auto_start 已把 dsh 拉起来、前端 boot 还卡在 IPC 时，直接探测并挂上 iframe。
     * 等价于用户看到「已停止」后点启动，但不必等那一下。
     */
    async tryAttachRunning() {
      if (this.serviceHealthy || this.status === 'installing')
        return
      try {
        const result = await checkHealthViaProxy()
        if (!result.healthy)
          return
        await this.completeReadiness()
      }
      catch (err) {
        console.warn('[Harness] attach running service failed:', err)
      }
    },

    /** 窗口从托盘/单实例唤醒时核对服务；健康时不刷新页面，停止时重新启动。 */
    async resumeOnActivation() {
      if (activationRecoveryInProgress || this.busyAction)
        return
      activationRecoveryInProgress = true
      try {
        // 点过「停止」后再关窗口：进程还在托盘里，React 不会重新挂载，
        // startup() 也不会再跑。服务已停就直接拉起，不必先做 8s 健康检查。
        if (!this.serviceRunning) {
          await this.start()
          return
        }
        const result = await checkHealthViaProxy()
        if (result.healthy) {
          this.serviceRunning = true
          if (!this.serviceHealthy || this.iframeError)
            await this.completeReadiness()
          return
        }
        await this.start()
      }
      finally {
        activationRecoveryInProgress = false
      }
    },

    /** 刷新 iframe：清除加载态并延迟重新挂载 */
    refreshIframe() {
      this.iframeLoaded = false
      this.iframeError = false
      setTimeout(() => {
        this.iframeKey++
      }, 800)
    },

    /** iframe 加载成功/失败时由视图回调更新状态 */
    markIframeLoaded() {
      this.iframeLoaded = true
      this.iframeError = false
    },

    markIframeError() {
      this.iframeError = true
      this.iframeLoaded = false
    },

    /** 安装进度流：只前进不后退，供首次安装/手动更新共用 */
    async listenInstallProgress(): Promise<UnlistenFn> {
      return listen<InstallProgress>('install-progress', (e) => {
        const payload = e.payload
        if (payload.percentage < this.installer.percentage) {
          return
        }
        const logs = payload.log
          ? [...this.installer.logs, payload.log].slice(-5)
          : this.installer.logs
        this.installer = {
          title: payload.title || this.installer.title,
          detail: payload.detail || this.installer.detail,
          percentage: payload.percentage,
          logs,
        }
      })
    },

    /** 服务探测通过后的统一收尾；token 用于阻止旧启动流程覆盖新状态 */
    async completeReadiness(token?: number): Promise<boolean> {
      const readyInfo = await invoke<{ service_url: string }>('get_runtime_info')
      if (token !== undefined && token !== bootToken)
        return false

      this.serviceUrl = readyInfo.service_url
      this.iframeSrc = generateTimestampedUrl(readyInfo.service_url)
      this.serviceHealthy = true
      this.serviceRunning = true
      this.status = 'ready'
      stopColdStartWatchdog()
      this.errorMsg = ''
      this.errorLogs = []
      this.inotifyLimitHint = ''
      // 服务（重）启动成功后，dsh 版本/端口/CLI 链接状态等运行时信息可能已变化
      // （典型：Harness 更新后旧版本缓存仍在，调试侧边栏需刷新页面才显示新版本）。
      // 使侧边栏相关查询缓存失效，重新打开/已挂载时自动拉取最新值。
      void queryClient.invalidateQueries({ queryKey: ['info'] })
      void queryClient.invalidateQueries({ queryKey: ['config'] })
      void queryClient.invalidateQueries({ queryKey: ['cli_status'] })
      // 档案/核心切换后重启，刷新对应状态。
      void queryClient.invalidateQueries({ queryKey: ['profiles'] })
      void queryClient.invalidateQueries({ queryKey: ['cores'] })
      return true
    },

    /**
     * 初始就绪窗口超时后继续探测同一已持有进程。错误界面仍会及时出现，但只要后端
     * 稍后完成启动，就自动恢复并挂载 iframe；新一轮 boot 会用 token 终止旧探测。
     */
    async recoverReadiness(token: number) {
      const result = await pollReadiness({
        probe: checkHealthViaProxy,
        intervalMs: 2000,
        shouldContinue: () => token === bootToken && this.serviceRunning && !this.serviceHealthy,
      })
      if (token !== bootToken)
        return
      if (result.notOwned) {
        this.serviceRunning = false
        return
      }
      if (!result.healthy)
        return

      try {
        await this.completeReadiness(token)
      }
      catch (err) {
        console.error('[Harness] failed to complete delayed readiness recovery:', err)
      }
    },

    /** 拉起服务并等待健康检查通过，通过后才允许挂载 iframe */
    async launchAndWait(token?: number) {
      this.status = 'ready'
      this.installer = initialInstaller
      this.errorMsg = ''
      this.errorLogs = []
      this.inotifyLimitHint = ''
      this.serviceHealthy = false
      this.iframeLoaded = false
      this.iframeError = false
      try {
        await invoke('launch_harness')
        this.serviceRunning = true
        // 后端遇到端口占用时会自动递增并持久化端口，启动后重新读取真实地址。
        const runtimeInfo = await invoke<{ service_url: string }>('get_runtime_info')
        this.serviceUrl = runtimeInfo.service_url
        this.iframeSrc = generateTimestampedUrl(runtimeInfo.service_url)

        const result = await pollReadiness({
          probe: checkHealthViaProxy,
          intervalMs: 2000,
          maxAttempts: MAX_RETRIES,
          shouldContinue: () => token === undefined || token === bootToken,
        })
        if (!result.healthy) {
          const error: StartupError = new Error(
            i18next.t('errors.service_start_timeout', { port: new URL(this.serviceUrl).port || '3080' }),
          )
          error.readinessTimedOut = !result.notOwned
          throw error
        }
        // 服务已就绪后再取一次真实地址：`launch_harness` 可能因后端已在并发拉起
        // （auto_start）而提前返回，此刻端口若尚未落库，上面读到的 service_url 会是
        // 旧端口；健康检查通过意味着服务已在最终端口就绪，此时读取必然准确。
        // 避免 iframe 挂载到一个无人监听的地址（表现为首次加载失败、刷新后恢复）。
        await this.completeReadiness(token)
      }
      catch (err) {
        // 失败时附上服务日志里的真实错误行，供错误界面展示而不是只显示超时文案
        throw await attachStartupDiagnostics(err)
      }
    },

    /** 启动流程：检测环境/安装依赖 → 拉起服务 → 已安装时后台检查更新 */
    async boot() {
      const token = ++bootToken
      // 回到加载态：已安装时不再显示检测/启动界面，直接进入页面加载状态
      this.serviceHealthy = false
      this.iframeLoaded = false
      this.iframeError = false
      // 重新启动/进入启动流程时先退出上一轮的错误态。
      this.errorMsg = ''
      this.errorLogs = []
      this.inotifyLimitHint = ''
      this.status = 'ready'
      let unlistenInstall: UnlistenFn | null = null

      try {
        console.info('[Harness] boot: start', token)
        const runtimeInfo = await invoke<{ service_url: string }>('get_runtime_info')
        if (token !== bootToken)
          return
        if (this.serviceHealthy)
          return
        bootIpcReady = true
        console.info('[Harness] boot: ipc ready')
        this.serviceUrl = runtimeInfo.service_url
        this.iframeSrc = generateTimestampedUrl(runtimeInfo.service_url)

        // 已安装过则跳过安装界面，避免每次启动都闪现"正在安装依赖..."
        const config = await invoke<{
          installed: boolean
        }>('get_app_config')

        // 每次启动都做纯本地运行时检查：旧版本升级后 installed 仍为 true，但新版
        // 可能新增依赖（如 Windows 空白环境需要的 MinGit），必须进入幂等自愈。
        // 已全部就绪时不调用安装命令，因此不会联网，也不会闪现安装界面。
        const ready = await invoke<boolean>('runtime_ready')
        if (!ready || !config.installed) {
          // 只在真正安装时才订阅进度。冷启动若先 await listen，IPC 未就绪会
          // 把 boot 卡住，表现为托盘退出后再开必须手动点「启动」。
          try {
            unlistenInstall = await this.listenInstallProgress()
          }
          catch (err) {
            console.error('[Harness] failed to listen install-progress:', err)
          }
          if (!ready) {
            this.status = 'installing'
            this.installer = { ...initialInstaller, title: i18next.t('status.installing') }
          }
          await invoke('install_dependencies')
        }

        await this.launchAndWait(token)

        if (token !== bootToken)
          return
      }
      catch (err) {
        if (token !== bootToken)
          return
        console.error('[Harness] startup failed:', err)
        const startupError = await attachStartupDiagnostics(err)
        if (!startupRecoveryAttempted) {
          startupRecoveryAttempted = true
          // 开机时偶发 dsh 尚未就绪或进程刚退出，延迟一次完整重启再交给用户处理。
          setTimeout(() => {
            void this.restart()
          }, 1000)
        }
        const keepServiceRunning = startupError.readinessTimedOut === true
        this.fail(
          String(startupError),
          startupError.logs,
          startupError.inotifyLimitHint,
          keepServiceRunning,
        )
        if (keepServiceRunning) {
          void this.recoverReadiness(token)
        }
      }
      finally {
        unlistenInstall?.()
      }
    },

    /** 进入安装态（手动更新前复用，标题区分"安装/更新"） */
    prepareInstall(title: string) {
      this.status = 'installing'
      this.installer = { ...initialInstaller, title }
    },

    /** 进入错误态（供本模块与 updater 模块共用） */
    fail(message: string, logs?: string[], inotifyLimitHint?: string, keepServiceRunning = false) {
      this.errorMsg = message
      this.errorLogs = logs ?? []
      this.inotifyLimitHint = inotifyLimitHint ?? ''
      this.status = 'error'
      this.serviceRunning = keepServiceRunning
    },

    /** 重启服务：先强杀再拉起，最终回到就绪/错误态 */
    async restart() {
      if (this.busyAction)
        return
      this.busyAction = 'restart'
      try {
        emitter.emit('config:dialog:hidden')
        await invoke('shutdown_harness')
      }
      catch (err) {
        console.error('[Harness] shutdown during restart failed:', err)
      }
      this.serviceRunning = false
      this.iframeLoaded = false
      try {
        await this.boot()
      }
      finally {
        this.busyAction = null
      }
    },

    /** 停止服务并回到停止态界面 */
    async shutdown() {
      if (this.busyAction)
        return
      this.busyAction = 'shutdown'
      // 停止服务后应用回到「已停止」态，配置弹窗已无意义，与 restart 一致地关闭它
      emitter.emit('config:dialog:hidden')
      try {
        await invoke('shutdown_harness')
      }
      catch (err) {
        console.error('[Harness] shutdown failed:', err)
      }
      finally {
        this.busyAction = null
      }
      this.serviceRunning = false
      this.status = 'error'
      this.errorMsg = i18next.t('ui.stopped')
      this.errorLogs = []
      this.inotifyLimitHint = ''
    },

    /** 服务未运行时点击"重试"：重新拉起服务并等待健康检查 */
    async start() {
      if (this.busyAction)
        return
      this.busyAction = 'start'
      try {
        await this.boot()
      }
      finally {
        this.busyAction = null
      }
    },

    /** 在系统浏览器中打开服务地址 */
    async openBrowser() {
      if (this.busyAction)
        return
      this.busyAction = 'openBrowser'
      try {
        await invoke('open_in_browser')
      }
      catch (err) {
        console.error('[Harness] open in browser failed:', err)
      }
      finally {
        this.busyAction = null
      }
    },

  },
})

// 进入 ready 后 iframe 长时间未加载（dsh 未就绪/挂起）→ 转为错误界面，
// 避免一直停在黑色加载遮罩
let iframeLoadTimer: ReturnType<typeof setTimeout> | null = null
harness.$subscribe(() => {
  const { status, serviceHealthy, iframeLoaded, iframeError } = harness.$state
  if (status === 'ready' && serviceHealthy && !iframeLoaded && !iframeError) {
    if (!iframeLoadTimer) {
      iframeLoadTimer = setTimeout(() => {
        iframeLoadTimer = null
        harness.iframeLoaded = false
        harness.iframeError = true
      }, IFRAME_LOAD_TIMEOUT)
    }
  }
  else {
    if (iframeLoadTimer) {
      clearTimeout(iframeLoadTimer)
      iframeLoadTimer = null
    }
  }
})
