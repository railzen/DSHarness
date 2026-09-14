// 用全新数据目录和隔离 PATH 验证安装包资源，禁止 Node 向非本机地址连接。
import { spawn, spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, existsSync, readFileSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { setTimeout } from 'node:timers/promises'

const root = resolve(import.meta.dirname, '..')
const bundle = join(root, 'src-tauri/resources/bundle')
mkdirSync(join(process.env.TEMP || 'C:/tmp', 'dsh-offline-tests'), { recursive: true })
const home = mkdtempSync(join(process.env.TEMP || 'C:/tmp', 'dsh-offline-tests/offline-smoke-'))
const guard = join(home, 'offline.cjs')
writeFileSync(guard, `
const net = require('node:net');
const original = net.Socket.prototype.connect;
net.Socket.prototype.connect = function (...args) {
  const options = net._normalizeArgs(args)[0];
  const host = options.host || 'localhost';
  if (!options.path && !['127.0.0.1', 'localhost', '::1'].includes(host)) {
    require('node:fs').appendFileSync(${JSON.stringify(join(home, 'blocked.log'))}, host + '\\n');
    throw new Error('OFFLINE_NETWORK_BLOCKED: ' + host);
  }
  return original.apply(this, args);
};
`)
const system = process.env.SystemRoot || 'C:\\Windows'
const env = {
  SystemRoot: system, WINDIR: system, COMSPEC: join(system, 'System32/cmd.exe'),
  TEMP: home, TMP: home, USERPROFILE: home, HOME: home,
  APPDATA: join(home, 'AppData/Roaming'), LOCALAPPDATA: join(home, 'AppData/Local'),
  PATH: [join(bundle, 'node'), join(bundle, 'powershell'), join(bundle, 'git/cmd'), join(system, 'System32')].join(';'),
  DSH_HOME: home, DSH_TELEMETRY_DISABLED: '1', NO_COLOR: '1',
  NODE_OPTIONS: `--require ${JSON.stringify(guard)}`,
}
const node = join(bundle, 'node/node.exe')
for (const [exe, args] of [
  [node, ['--version']],
  [join(bundle, 'git/cmd/git.exe'), ['--version']],
  [join(bundle, 'powershell/pwsh.exe'), ['-NoLogo', '-NoProfile', '-Command', 'Write-Output offline-pwsh']],
] as [string, string[]][]) {
  const result = spawnSync(exe, args, { env, windowsHide: true, encoding: 'utf8', timeout: 30_000 })
  if (result.status !== 0) throw new Error(`${exe}: ${result.error || result.stderr}`)
  console.log(result.stdout.trim())
}
// 由系统分配空闲端口，避免与已安装桌面端或其他测试争用。
const { createServer } = await import('node:net')
const probe = createServer()
await new Promise<void>(resolve => probe.listen(0, '127.0.0.1', resolve))
const port = (probe.address() as import('node:net').AddressInfo).port
await new Promise<void>(resolve => probe.close(() => resolve()))
const child = spawn(node, [join(bundle, 'dsh/node_modules/@deepseek-ai/dsh/lib/bin.js'), '--profile', 'web', '--host', '127.0.0.1', '--port', String(port), '--no-open'], { env, cwd: home, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] })
let logs = ''
child.stdout.on('data', bytes => { logs += bytes })
child.stderr.on('data', bytes => { logs += bytes })
try {
  let ready = false
  for (let attempt = 0; attempt < 90; attempt++) {
    if (child.exitCode !== null) throw new Error(`Harness exited ${child.exitCode}`)
    try {
      const authenticated = logs.match(new RegExp('http://127[.]0[.]0[.]1:[0-9]+/[?]token=[A-Za-z0-9_-]+'))?.[0]
      if (!authenticated) { await setTimeout(1000); continue }
      const auth = await fetch(authenticated, { redirect: 'manual', signal: AbortSignal.timeout(1000) })
      const cookie = auth.headers.getSetCookie().map(value => value.split(';')[0]).join('; ')
      const response = await fetch(`http://127.0.0.1:${port}/`, { headers: { cookie }, signal: AbortSignal.timeout(1000) })
      const html = await response.text()
      if (response.ok && html.includes('<html')) {
        for (const match of html.matchAll(/(?:src|href)="([^"#]+\.(?:js|css))"/g)) {
          const asset = new URL(match[1], `http://127.0.0.1:${port}/`)
          if (asset.hostname !== '127.0.0.1') throw new Error(`External UI asset: ${asset}`)
          const result = await fetch(asset, { headers: { cookie } })
          if (!result.ok) throw new Error(`Missing UI asset: ${asset}`)
        }
        ready = true; break
      }
    }
    catch {}
    await setTimeout(1000)
  }
  if (!ready) throw new Error('Offline startup timed out')
  await setTimeout(3000)
  if (child.exitCode !== null) throw new Error('Harness exited after startup')
  if (existsSync(join(home, 'blocked.log'))) throw new Error(`Unexpected outbound connection: ${readFileSync(join(home, 'blocked.log'), 'utf8')}`)
  console.log(`PASS: fresh offline web profile, isolated PATH, ${home}`)
}
finally {
  writeFileSync(join(home, 'service.log'), logs)
  if (child.pid && child.exitCode === null)
    spawnSync(join(system, 'System32/taskkill.exe'), ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true })
  console.log(logs.slice(-4000).replace(/token=[A-Za-z0-9_-]+/g, 'token=[redacted]'))
}


