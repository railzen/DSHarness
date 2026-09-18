# DSCode

基于 Tauri 2 的 Windows x64 桌面封装，上游为 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)。

## 安装与离线使用

从 [Releases](https://github.com/railzen/deepseek-harness-win/releases) 下载完整 `.exe` 安装包，可复制到不连接外网的电脑安装。安装器需要管理员权限，默认安装到 `Program Files\DSCode`。

- 安装包内置固定版本的 DSH、Node.js、MinGit 和 PowerShell 7；无需预装 Node、npm、Git 或 PowerShell 7
- 桌面界面使用系统 WebView2，以缩小安装包；Windows 11 通常已自带，缺失时需要先联网或由管理员离线部署 WebView2
- DSH 和运行时仅通过新版完整桌面安装包升级或修复，不提供独立核心下载、npm 安装或版本切换
- 离线升级：在可联网的电脑下载新版安装包，复制到目标电脑后运行；在线电脑也可使用应用的桌面更新入口
- 程序及运行时在安装目录中；会话和档案在 `%USERPROFILE%\.dsh`，设置和日志在用户 AppData 中，普通用户运行无需管理员权限
- 默认服务地址为 `http://127.0.0.1:3080`；开发版使用 `3081` 和 `.dsh.dev`，与正式版数据隔离

离线可启动界面、管理本地文件和会话。模型推理需要配置可访问的内网或本地模型 API；安装包不含模型权重。联网搜索、外部 MCP、下载软件包及项目自身的编译工具仍取决于相应服务或项目依赖。

桌面端不再使用旧版全局 npm 安装的 DSH，也不会被自动卸载。用户数据保留；旧档案引用的第三方插件不属于内置发行版。

## 构建

仅支持 Windows x64 安装包。构建机需要 Node.js、pnpm 和 Rust/MSVC 工具链，并联网准备依赖。

```powershell
pnpm install --frozen-lockfile
pnpm prepare:runtime
pnpm typecheck
pnpm test:offline
cd src-tauri
cargo check
cargo test
cd ..
pnpm tauri build --bundles nsis
```

资源版本与 SHA-256 固定在 `scripts/runtime/assets.json`，DSH 依赖树固定在 `scripts/runtime/package-lock.json`。更新运行时版本后必须重新构建完整安装包。`test:offline` 使用全新数据目录、隔离 PATH，并阻止 Node 的非本机连接；发布前还应在断网、未预装开发工具/WebView2 的干净 Windows 虚拟机验证安装、启动、升级和卸载。

## 相关项目与 License

- [deepseek-harness-desktop](https://github.com/dsh-tauri-desk/deepseek-harness-desktop) — 上游桌面封装
- [MIT](./LICENSE)，额外遵循 [非商用条款](./LICENSE.details)
- 内置运行时保留各自发行包中的许可证
