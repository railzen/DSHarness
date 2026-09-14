//! 内置核心只读信息。
use crate::service::core;
use tauri::AppHandle;
#[tauri::command]
pub async fn get_cores(app_handle: AppHandle) -> Vec<core::HarnessCore> {
    core::list(&app_handle).await
}
