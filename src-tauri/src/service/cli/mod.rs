//! 命令行集成仅注册安装目录的 PATH，不创建或删除安装文件。
mod core;
mod path;
mod shim;
pub use core::{ensure, get_status, remove, CliLinkStatus};
