// 仅在构建机联网准备；用户电脑只接收已解包、版本锁定的运行时。
import { createHash } from 'node:crypto'
import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync, rmSync, cpSync, readdirSync } from 'node:fs'
import { join, resolve } from 'node:path'
import process from 'node:process'

const root = resolve(import.meta.dirname, '..')
const out = join(root, 'src-tauri/resources/bundle')
const cache = join(root, '.tmp/runtime-downloads')
const spec = join(root, 'scripts/runtime')
if (process.platform !== 'win32' || process.arch !== 'x64')
  throw new Error('The offline installer currently targets Windows x64 only')
mkdirSync(cache, { recursive: true })

interface Asset { url: string, sha256: string, file: string }
const assets: Record<string, Asset> = JSON.parse(readFileSync(join(spec, 'assets.json'), 'utf8'))
const nodeVersion = readFileSync(join(root, 'src-tauri/src/config/constants.rs'), 'utf8').match(/NODE_VERSION: &str = "([^"]+)"/)?.[1]
if (!nodeVersion || assets.node.file !== `node-${nodeVersion}-win-x64.zip`)
  throw new Error('Bundled Node version must match config/constants.rs')

function run(exe: string, args: string[]) {
  const result = spawnSync(exe, args, { stdio: 'inherit', windowsHide: true })
  if (result.error || result.status !== 0)
    throw new Error(`${exe} failed: ${result.error || result.status}`)
}

function hash(bytes: Uint8Array) { return createHash('sha256').update(bytes).digest('hex') }

async function download(asset: Asset) {
  const path = join(cache, asset.file)
  if (existsSync(path) && hash(readFileSync(path)) === asset.sha256)
    return path
  console.log(`Downloading ${asset.file}`)
  const response = await fetch(asset.url, { signal: AbortSignal.timeout(600_000) })
  if (!response.ok) throw new Error(`HTTP ${response.status}: ${asset.url}`)
  const bytes = new Uint8Array(await response.arrayBuffer())
  if (hash(bytes) !== asset.sha256) throw new Error(`Checksum mismatch: ${asset.file}`)
  writeFileSync(path, bytes)
  return path
}

function unzip(archive: string, destination: string) {
  mkdirSync(destination, { recursive: true })
  run('tar.exe', ['-mxf', archive, '-C', destination])
}

function pruneDshRuntime(directory: string) {
  let removed = 0
  const documentation = /^(?:readme|changelog|changes|history|contributing|code_of_conduct)(?:\..*)?$/i

  function visit(current: string) {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, entry.name)
      if (entry.isDirectory()) {
        visit(path)
        continue
      }
      if (entry.name.endsWith('.map') || entry.name.endsWith('.d.ts')
        || entry.name.endsWith('.d.mts') || entry.name.endsWith('.d.cts')
        || documentation.test(entry.name)) {
        rmSync(path, { force: true })
        removed++
      }
    }
  }

  visit(directory)
  console.log(`Pruned ${removed} development-only DSH runtime files`)
  return removed
}

const pruneVersion = 'types-maps-docs-v1'
const stamp = hash(Buffer.concat([
  ...['assets.json', 'package.json', 'package-lock.json'].map(name => readFileSync(join(spec, name))),
  Buffer.from(pruneVersion),
]))
const marker = join(out, 'bundle.json')
const required = ['node/node.exe', 'dsh/node_modules/@deepseek-ai/dsh/lib/bin.js', 'git/cmd/git.exe', 'powershell/pwsh.exe', 'bin/dsh.cmd']
const webview = join(root, 'src-tauri/resources/webview2')
if (existsSync(marker) && JSON.parse(readFileSync(marker, 'utf8')).stamp === stamp
  && required.every(name => existsSync(join(out, name))) && !existsSync(webview)) {
  console.log('Offline runtime already prepared')
  process.exit(0)
}

// 输出目录完全由构建脚本拥有，不接触已安装的程序或用户数据。
rmSync(out, { recursive: true, force: true })
mkdirSync(out, { recursive: true })
for (const name of ['node', 'git', 'powershell']) {
  const archive = await download(assets[name])
  const destination = join(out, name)
  mkdirSync(destination, { recursive: true })
  unzip(archive, destination)
  if (name === 'node') {
    const nested = readdirSync(destination, { withFileTypes: true }).find(entry => entry.isDirectory())
    if (!nested) throw new Error(`Missing extracted ${name} directory`)
    const staged = `${destination}-flat`
    cpSync(join(destination, nested.name), staged, { recursive: true })
    rmSync(destination, { recursive: true })
    cpSync(staged, destination, { recursive: true })
    rmSync(staged, { recursive: true })
  }
}
// 旧构建可能留下固定 WebView2；系统运行时模式不能继续把它打入安装包。
rmSync(webview, { recursive: true, force: true })

const dsh = join(out, 'dsh')
mkdirSync(dsh, { recursive: true })
for (const name of ['package.json', 'package-lock.json']) cpSync(join(spec, name), join(dsh, name))
const node = join(out, 'node/node.exe')
run(node, [join(out, 'node/node_modules/npm/bin/npm-cli.js'), 'ci', '--prefix', dsh, '--omit=dev', '--no-audit', '--no-fund'])
const prunedFiles = pruneDshRuntime(join(dsh, 'node_modules'))

// 保留 Node 发行版许可证；包管理器不作为用户运行时安装入口交付。
rmSync(join(out, 'node/node_modules'), { recursive: true, force: true })
for (const name of ['npm', 'npm.cmd', 'npm.ps1', 'npx', 'npx.cmd', 'npx.ps1', 'corepack', 'corepack.cmd'])
  rmSync(join(out, 'node', name), { force: true })
mkdirSync(join(out, 'bin'), { recursive: true })
writeFileSync(join(out, 'bin/dsh.cmd'), '@echo off\r\nsetlocal\r\nset "PATH=%~dp0..\\node;%~dp0..\\powershell;%~dp0..\\git\\cmd;%PATH%"\r\n"%~dp0..\\node\\node.exe" "%~dp0..\\dsh\\node_modules\\@deepseek-ai\\dsh\\lib\\bin.js" %*\r\nexit /b %errorlevel%\r\n')
writeFileSync(marker, JSON.stringify({ stamp, assets, dsh: JSON.parse(readFileSync(join(dsh, 'package.json'), 'utf8')).dependencies['@deepseek-ai/dsh'], pruneVersion, prunedFiles }, null, 2))
console.log('Offline runtime prepared')

