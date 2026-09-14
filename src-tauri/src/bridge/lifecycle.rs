//! 内置运行时校验与服务生命周期；文件损坏只能通过完整安装包修复。
use crate::config;
use crate::service::workflow;
use tauri::AppHandle;

#[tauri::command]
pub async fn install_dependencies(app_handle: AppHandle) -> Result<bool, String> {
    if !runtime_ready(app_handle.clone()) {
        return Err("BUNDLED_RUNTIME_MISSING: 内置运行时缺失或损坏，请重新运行完整安装包 / Bundled runtime missing or damaged; reinstall the full installer".into());
    }
    let mut setting = config::get_store_dat_setting(&app_handle);
    if !setting.installed {
        setting.installed = true;
        config::set_store_dat_setting(&app_handle, setting);
    }
    // 首次启动也要注册内置 CLI；setup 阶段的旧 installed 标记可能仍为 false。
    if config::get_store_dat_setting(&app_handle).cli_link_enabled {
        if let Err(error) = crate::service::cli::ensure(&app_handle) {
            log::warn!("Bundled CLI PATH registration failed: {error}");
        }
    }
    Ok(false)
}

#[tauri::command]
pub fn runtime_ready(app_handle: AppHandle) -> bool {
    config::is_runtime_compatible(&app_handle)
        && config::get_dsh_binary_path(&app_handle).is_file()
        && config::get_dsh_version(&app_handle).is_some()
}

/// 启动 Harness 服务
#[tauri::command]
pub async fn launch_harness(app_handle: AppHandle) -> Result<(), String> {
    workflow::launch(app_handle).await
}

/// 停止 Harness 服务
#[tauri::command]
pub async fn shutdown_harness(app_handle: AppHandle) -> Result<(), String> {
    workflow::stop(app_handle).await
}

/// 重启 Harness 服务
#[tauri::command]
pub async fn restart_harness(app_handle: AppHandle) -> Result<(), String> {
    workflow::restart(app_handle).await
}

/// 获取当前 Harness 服务状态
#[tauri::command]
pub fn get_dsh_status() -> workflow::status::Status {
    workflow::status::get_status()
}
