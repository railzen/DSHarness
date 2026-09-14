//! 识别旧版生成的脚本；新脚本由构建脚本生成并由安装器维护。
use std::path::Path;
#[cfg(windows)]
pub const SHIM_CMD_NAME: &str = "dsh.cmd";
#[cfg(unix)]
pub const SHIM_SH_NAME: &str = "dsh";

pub fn is_generated_shim(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|s| s.contains("DeepSeek Harness Desktop"))
        .unwrap_or(false)
}
