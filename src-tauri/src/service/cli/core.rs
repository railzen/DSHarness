//! 命令行集成对外接口：状态查询、启用/确保（幂等自愈）、禁用/清理。

use crate::config;
use serde::Serialize;
use tauri::AppHandle;

use super::path::{get_bin_dir, get_shim_path, path_registered, register_path, unregister_path};

/// 命令行集成状态（设置页展示）
#[derive(Debug, Clone, Serialize)]
pub struct CliLinkStatus {
    /// 用户开关（Setting.cli_link_enabled）
    pub enabled: bool,
    /// 主 shim 文件是否存在
    pub shim_exists: bool,
    /// bin 目录是否已在用户 PATH 中注册
    pub path_registered: bool,
    /// 检测到用户自行安装的同名 `dsh`（未被覆盖，已保留）
    pub user_dsh_preserved: bool,
    /// bin 目录绝对路径
    pub bin_dir: String,
    /// 主 shim 文件绝对路径
    pub shim_path: String,
}

/// 当前命令行集成状态
pub fn get_status(app_handle: &AppHandle) -> CliLinkStatus {
    let setting = config::get_store_dat_setting(app_handle);
    let shim_path = get_shim_path(app_handle);
    let bin_dir = get_bin_dir(app_handle);
    CliLinkStatus {
        enabled: setting.cli_link_enabled,
        shim_exists: shim_path.is_file(),
        path_registered: path_registered(app_handle),
        user_dsh_preserved: false,
        bin_dir: bin_dir.to_string_lossy().into_owned(),
        shim_path: shim_path.to_string_lossy().into_owned(),
    }
}

/// 启用命令行入口只注册 PATH；安装目录的文件由安装器统一维护。
pub fn ensure(app_handle: &AppHandle) -> Result<CliLinkStatus, String> {
    let bin_dir = get_bin_dir(app_handle);
    if !get_shim_path(app_handle).is_file() {
        return Err("BUNDLED_CLI_MISSING: reinstall the full desktop installer".into());
    }
    // 开发（debug）构建不注册用户 PATH：bin 目录与 PATH 是共享的用户级状态，
    // 由生产版维护；开发版只写 shim（debug 下仅 pnpm shim，见 write_shims），
    // 既不增删 PATH 条目、也不覆盖生产的 dsh shim，避免干扰生产命令行集成。
    if cfg!(debug_assertions) {
        log::info!(
            "dsh CLI shims ensured at {} (debug build: PATH registration skipped)",
            bin_dir.display()
        );
        return Ok(get_status(app_handle));
    }
    register_path(app_handle)?;

    log::info!("dsh CLI links ensured at {}", bin_dir.display());
    Ok(get_status(app_handle))
}

/// 禁用命令行入口只注销 PATH，不删除安装包内的脚本。
pub fn remove(app_handle: &AppHandle) -> Result<CliLinkStatus, String> {
    // 开发（debug）构建不删除 shim、不注销 PATH：这些是共享的用户级状态，
    // 由生产版维护——开发版执行清理会让正在运行的生产版命令行集成失效。
    if cfg!(debug_assertions) {
        log::info!("cli link removal skipped in debug build (shared user state kept)");
        return Ok(get_status(app_handle));
    }
    unregister_path(app_handle)?;

    log::info!("dsh CLI links removed");
    Ok(get_status(app_handle))
}
