use serde::Serialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};

use super::constants::*;
use super::format::get_dsh_service_url;

/// 获取 App Data 基础目录
pub fn get_base_dir<R: Runtime>(app_handle: &AppHandle<R>) -> PathBuf {
    app_handle
        .path()
        .app_data_dir()
        .expect("Failed to resolve app data directory")
}

/// 为任意 GitHub Release 资产 URL 生成 ghfast.top 镜像兜底地址
/// （透传原 URL，下载内容一致，仍可做 SHA-256 完整性校验）。
pub fn mirror_download_url(asset_url: &str) -> String {
    format!("{DSH_MIRROR_PREFIX}{asset_url}")
}

/// 运行 `node --version` 并捕获输出
///
/// Windows 打包版是 GUI 进程（没有控制台），必须以 CREATE_NO_WINDOW 启动
/// node.exe，否则每次版本检查都会闪现一个黑色 cmd 窗口。
fn node_version_output(node: &Path) -> Option<std::process::Output> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(node)
            .arg("--version")
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output()
            .ok()
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new(node)
            .arg("--version")
            .output()
            .ok()
    }
}

/// 固定使用内置 Node，不回退到用户 PATH 或 AppData 中的旧运行时。
pub fn get_node_binary_path(app: &tauri::AppHandle) -> PathBuf {
    get_node_install_path(app).join("node.exe")
}

pub fn get_node_install_path(app_handle: &tauri::AppHandle) -> PathBuf {
    get_bundle_dir(app_handle).join("node")
}

/// Harness 发行版安装目录
pub fn get_dsh_install_path<R: Runtime>(app_handle: &AppHandle<R>) -> PathBuf {
    get_bundle_dir(app_handle).join(DSH_CORE_DIR)
}

/// dsh CLI 入口
pub fn get_dsh_binary_path<R: Runtime>(app_handle: &AppHandle<R>) -> PathBuf {
    get_dsh_install_path(app_handle).join(DSH_ENTRY_RELATIVE)
}

/// Harness 发行版清单路径
pub fn get_dsh_package_json_path<R: Runtime>(app_handle: &AppHandle<R>) -> PathBuf {
    get_dsh_install_path(app_handle).join("node_modules/@deepseek-ai/dsh/package.json")
}

/// 用户主目录（Windows 取 `%USERPROFILE%`，Unix 取 `$HOME`）。
///
/// 不使用 dirs crate（未引入该依赖），与官方 dsh 的 `$HOME/.dsh` 语义保持一致。
fn user_home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";
    std::env::var_os(key).map(PathBuf::from)
}

/// Harness 用户数据目录（$DSH_HOME）。
///
/// 与官方 dsh（`${DSH_HOME:-$HOME/.dsh}`）保持一致：
/// - 环境变量 `DSH_HOME` 非空时优先使用（用户显式指定优先于构建差异）；
/// - 否则 release 构建默认 `~/.dsh`（Windows `%USERPROFILE%\.dsh`，Unix
///   `$HOME/.dsh`，与官方 node 安装共用同一份数据）；
/// - debug 构建默认 `~/.dsh.dev`：开发版与生产版同时运行时，会话、档案、
///   插件与主题等数据互不干扰，也不会互相污染对方的会话（核心目录
///   `dependencies/` 仍共用同一份安装）。
pub fn get_dsh_data_path<R: Runtime>(_app_handle: &AppHandle<R>) -> PathBuf {
    if let Some(home) = std::env::var_os("DSH_HOME") {
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    let dir_name = if cfg!(debug_assertions) {
        DSH_HOME_DEV_DIR_NAME
    } else {
        DSH_HOME_DIR_NAME
    };
    user_home_dir()
        .map(|home| home.join(dir_name))
        .unwrap_or_else(|| PathBuf::from(dir_name))
}

/// dsh 服务日志文件路径
///
/// debug 构建写入独立的 `dsh-web.dev.log`：开发版每次启动都会轮转日志，若与
/// 生产共用同一个文件，会把正在运行的生产版日志记录轮转覆盖掉。
pub fn get_service_log_path<R: Runtime>(app_handle: &AppHandle<R>) -> PathBuf {
    let name = if cfg!(debug_assertions) {
        "dsh-web.dev.log"
    } else {
        "dsh-web.log"
    };
    get_base_dir(app_handle).join("logs").join(name)
}

/// 捆绑的 Node.js 版本号
pub fn get_bundled_node_version() -> String {
    NODE_VERSION.trim_start_matches('v').to_string()
}

/// 当前内置 Node.js 版本号
pub fn get_active_node_version() -> String {
    get_bundled_node_version()
}

fn parse_node_version(output: &str) -> Option<(u64, u64, u64)> {
    let version = output.trim().trim_start_matches('v');
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// 兼容性规则：v22.15.0+ 或 v23.8.0+（v24+ 也满足）
fn is_supported_node_version(version: &str) -> bool {
    let Some((major, minor, _patch)) = parse_node_version(version) else {
        return false;
    };
    match major {
        22 => minor >= 15,
        23 => minor >= 8,
        major if major >= 24 => true,
        _ => false,
    }
}

/// 运行 `node --version` 并判断运行时是否兼容
pub fn is_runtime_compatible(app_handle: &tauri::AppHandle) -> bool {
    let node = get_node_binary_path(app_handle);
    if !node.exists() {
        return false;
    }
    let output = match node_version_output(&node) {
        Some(out) => out,
        None => return false,
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    is_supported_node_version(stdout.trim())
}

/// 从打包的 Harness 清单读取 dsh 版本（界面展示用）
pub fn get_dsh_version<R: Runtime>(app_handle: &AppHandle<R>) -> Option<String> {
    let manifest_path = get_dsh_package_json_path(app_handle);
    let content = fs::read_to_string(&manifest_path).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&content).ok()?;
    manifest
        .get("version")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
}

/// 侧边栏展示的运行时/版本/诊断信息
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeInfo {
    pub app_version: String,
    pub dsh_version: Option<String>,
    pub node_version: String,
    pub service_url: String,
    pub data_dir: String,
    pub log_path: String,
    pub platform: String,
    pub arch: String,
}

pub fn runtime_info<R: Runtime>(app: &AppHandle<R>, port: u16) -> RuntimeInfo {
    RuntimeInfo {
        app_version: app.package_info().version.to_string(),
        dsh_version: get_dsh_version(app),
        node_version: get_active_node_version(),
        service_url: get_dsh_service_url(port),
        // 用户数据所在目录 = $DSH_HOME（release 为官方 ~/.dsh，debug 为独立
        // ~/.dsh.dev，见 get_dsh_data_path），不再是 AppData
        data_dir: get_dsh_data_path(app).to_string_lossy().into_owned(),
        log_path: get_service_log_path(app).to_string_lossy().into_owned(),
        platform: env::consts::OS.to_string(),
        arch: env::consts::ARCH.to_string(),
    }
}

/// 程序资源由安装器写入 Program Files；用户数据仍使用独立可写目录。
pub fn get_bundle_dir<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    let root = if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/bundle")
    } else {
        app.path()
            .resource_dir()
            .expect("BUNDLE_PATH_UNAVAILABLE")
            .join("resources/bundle")
    };
    // Tauri 在 Windows 打包版中可能返回 `\\?\C:\...`。Node 24 把该
    // verbatim 路径用作入口脚本时会错误地解析为 `C:`，因此交给 Node、PATH
    // 和子进程前统一去掉该前缀；路径本身仍是同一个绝对目录。
    dunce::simplified(&root).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn simplified_bundle_path_removes_windows_verbatim_prefix() {
        let path = PathBuf::from(r"\\?\C:\Program Files\DSCode\resources\bundle");
        assert_eq!(
            dunce::simplified(&path),
            Path::new(r"C:\Program Files\DSCode\resources\bundle")
        );
    }
}
