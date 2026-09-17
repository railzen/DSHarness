//! Windows 专用：以隐藏控制台的方式启动子进程。
//!
//! 打包版应用是 GUI 进程（没有控制台）。如果直接以 `CREATE_NO_WINDOW` 启动
//! node.exe，node 自身没有控制台，dsh 在 JS 里通过 `child_process` 派生的
//! 每个子进程（cmd / node / git 等）都会各自创建一个新的可见控制台窗口，
//! 表现为使用过程中频繁闪烁黑色 cmd 窗口。
//!
//! 这里改用 `CREATE_NEW_CONSOLE` + `STARTF_USESHOWWINDOW`/`SW_HIDE` 给 node
//! 分配一个隐藏的控制台：node 及其所有后代进程共享这个隐藏控制台，不再弹窗。
//! 模块声明处已按 `#[cfg(windows)]` 门控，仅在 Windows 构建中编译。

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{
    CloseHandle, SetHandleInformation, GENERIC_READ, GENERIC_WRITE, HANDLE, HANDLE_FLAG_INHERIT,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetProcessId, ResumeThread, TerminateProcess, CREATE_NEW_CONSOLE,
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTF_USESHOWWINDOW,
    STARTF_USESTDHANDLES, STARTUPINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

/// 以隐藏控制台方式启动 `program`，返回其 stdout / stderr 管道读取端。
///
/// `envs` 中的键值会覆盖当前进程环境变量后传给子进程。
#[cfg_attr(not(test), allow(dead_code))]
pub fn spawn_with_hidden_console(
    program: &Path,
    args: &[OsString],
    current_dir: Option<&Path>,
    envs: &HashMap<String, String>,
) -> io::Result<(File, File)> {
    let (stdout, stderr, handle) =
        spawn_with_hidden_console_tracked(program, args, current_dir, envs)?;
    unsafe {
        CloseHandle(handle);
    }
    Ok((stdout, stderr))
}

/// 启动由桌面端持有的 Harness，返回进程和不可继承的 Job 句柄。
///
/// PID 用于只结束本应用创建的进程树；句柄由调用方等待并关闭，避免进程退出后
/// PID 被系统复用时误伤其他程序。
pub fn spawn_with_hidden_console_owned(
    program: &Path,
    args: &[OsString],
    current_dir: Option<&Path>,
    envs: &HashMap<String, String>,
) -> io::Result<(File, File, u32, OwnedHandle, OwnedHandle)> {
    let job = unsafe {
        let raw = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = OwnedHandle::from_raw_handle(raw);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            raw,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of_val(&limits) as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        job
    };
    let (stdout, stderr, handle) =
        spawn_with_hidden_console_inner(program, args, current_dir, envs, Some(&job))?;
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let pid = unsafe { GetProcessId(handle.as_raw_handle()) };
    if pid == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((stdout, stderr, pid, handle, job))
}

/// 同 [`spawn_with_hidden_console`]，但额外返回进程句柄，供调用方等待进程
/// 结束并读取退出码（预装插件安装等需要判断成败的场景）。
pub fn spawn_with_hidden_console_tracked(
    program: &Path,
    args: &[OsString],
    current_dir: Option<&Path>,
    envs: &HashMap<String, String>,
) -> io::Result<(File, File, HANDLE)> {
    spawn_with_hidden_console_inner(program, args, current_dir, envs, None)
}

/// 受管进程先暂停，加入 Job 后再运行，避免后代在归属登记前逃逸。
fn spawn_with_hidden_console_inner(
    program: &Path,
    args: &[OsString],
    current_dir: Option<&Path>,
    envs: &HashMap<String, String>,
    job: Option<&OwnedHandle>,
) -> io::Result<(File, File, HANDLE)> {
    unsafe {
        // 1. 创建 stdout / stderr 匿名管道（写端可继承，交给子进程）
        let pipe_attrs = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };

        let mut stdout_read: HANDLE = std::ptr::null_mut();
        let mut stdout_write: HANDLE = std::ptr::null_mut();
        if CreatePipe(&mut stdout_read, &mut stdout_write, &pipe_attrs, 0) == 0 {
            return Err(io::Error::last_os_error());
        }

        let mut stderr_read: HANDLE = std::ptr::null_mut();
        let mut stderr_write: HANDLE = std::ptr::null_mut();
        if CreatePipe(&mut stderr_read, &mut stderr_write, &pipe_attrs, 0) == 0 {
            CloseHandle(stdout_read);
            CloseHandle(stdout_write);
            return Err(io::Error::last_os_error());
        }

        // 读取端留在父进程，禁止被子进程继承
        SetHandleInformation(stdout_read, HANDLE_FLAG_INHERIT, 0);
        SetHandleInformation(stderr_read, HANDLE_FLAG_INHERIT, 0);

        // 2. stdin 指向 NUL，避免 dsh 尝试 setRawMode 时报错
        let mut nul = "NUL".encode_utf16().collect::<Vec<u16>>();
        nul.push(0);
        let stdin_handle = CreateFileW(
            nul.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if stdin_handle == INVALID_HANDLE_VALUE {
            CloseHandle(stdout_read);
            CloseHandle(stdout_write);
            CloseHandle(stderr_read);
            CloseHandle(stderr_write);
            return Err(io::Error::last_os_error());
        }
        SetHandleInformation(stdin_handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);

        // 3. 组装命令行、环境块与启动参数
        let mut command_line = build_command_line(program, args);
        let env_block = build_env_block(envs);

        let mut application_name = program.as_os_str().encode_wide().collect::<Vec<u16>>();
        application_name.push(0);

        let mut current_dir_wide: Option<Vec<u16>> = None;
        if let Some(dir) = current_dir {
            let mut wide = dir.as_os_str().encode_wide().collect::<Vec<u16>>();
            wide.push(0);
            current_dir_wide = Some(wide);
        }

        let startup_info = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESHOWWINDOW | STARTF_USESTDHANDLES,
            wShowWindow: SW_HIDE as u16,
            hStdInput: stdin_handle,
            hStdOutput: stdout_write,
            hStdError: stderr_write,
            ..Default::default()
        };

        let mut process_info = PROCESS_INFORMATION::default();

        let created = CreateProcessW(
            application_name.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // bInheritHandles
            CREATE_NEW_CONSOLE
                | CREATE_UNICODE_ENVIRONMENT
                | if job.is_some() { CREATE_SUSPENDED } else { 0 },
            env_block.as_ptr() as *const core::ffi::c_void,
            current_dir_wide
                .as_ref()
                .map(|wide| wide.as_ptr())
                .unwrap_or(std::ptr::null()),
            &startup_info,
            &mut process_info,
        );

        let create_error = if created == 0 {
            Some(io::Error::last_os_error())
        } else {
            None
        };
        // 无论成功与否，父进程都要关闭自己持有的写端和临时句柄
        CloseHandle(stdout_write);
        CloseHandle(stderr_write);
        CloseHandle(stdin_handle);

        if let Some(error) = create_error {
            CloseHandle(stdout_read);
            CloseHandle(stderr_read);
            return Err(error);
        }

        if let Some(job) = job {
            if AssignProcessToJobObject(job.as_raw_handle(), process_info.hProcess) == 0
                || ResumeThread(process_info.hThread) == u32::MAX
            {
                let error = io::Error::last_os_error();
                // 登记失败时进程尚未运行，直接终止，禁止留下无管理的后台实例。
                TerminateProcess(process_info.hProcess, 1);
                CloseHandle(process_info.hThread);
                CloseHandle(process_info.hProcess);
                CloseHandle(stdout_read);
                CloseHandle(stderr_read);
                return Err(error);
            }
        }

        // 进程句柄不再需要时由调用方负责关闭（tracked 调用方等待退出后关闭；
        // 非 tracked 由包装函数立即关闭）。关闭线程句柄避免泄漏
        CloseHandle(process_info.hThread);

        let stdout = File::from_raw_handle(stdout_read as RawHandle);
        let stderr = File::from_raw_handle(stderr_read as RawHandle);
        Ok((stdout, stderr, process_info.hProcess))
    }
}

/// 构建完整的 Unicode 环境块（每个条目以 `\0` 结尾，整个块以额外 `\0` 结尾）
fn build_env_block(extra: &HashMap<String, String>) -> Vec<u16> {
    let mut vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    for (key, value) in extra {
        let key_os = OsString::from(key);
        // Windows 环境变量大小写不敏感：用户/系统环境里的键通常是 `Path` 而非
        // `PATH`，若按大小写敏感匹配会追加重复键，子进程（CreateProcessW 环境块）
        // 取到旧值。必须大小写不敏感匹配再替换。
        if let Some(entry) = vars
            .iter_mut()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(&key_os))
        {
            entry.1 = OsString::from(value);
        } else {
            vars.push((key_os, OsString::from(value)));
        }
    }

    let mut block = Vec::new();
    for (key, value) in vars {
        block.extend(key.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

/// 构建 CreateProcessW 命令行（遵循 MSVC 的引号转义规则）
fn build_command_line(program: &Path, args: &[OsString]) -> Vec<u16> {
    let mut command = quote_arg(&program.as_os_str().to_string_lossy());
    for arg in args {
        command.push(' ');
        command.push_str(&quote_arg(&arg.to_string_lossy()));
    }
    let mut wide = command.encode_utf16().collect::<Vec<u16>>();
    wide.push(0);
    wide
}

/// 按 MSVC 规则为命令行参数加引号并转义反斜杠/双引号
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| matches!(c, ' ' | '\t' | '"')) {
        return arg.to_string();
    }

    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                for _ in 0..backslashes * 2 {
                    out.push('\\');
                }
                backslashes = 0;
                out.push('\\');
                out.push('"');
            }
            _ => {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                backslashes = 0;
                out.push(c);
            }
        }
    }
    for _ in 0..backslashes * 2 {
        out.push('\\');
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 使用真实 Node 后代验证 Job 回收，避免只检查标志位却遗漏进程继承。
    fn check_owned_tree_cleanup(root_exits: bool, terminate_explicitly: bool) {
        use std::io::BufRead;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };

        let Some(node) = find_node_on_path() else {
            eprintln!("跳过进程树测试：未找到 Node");
            return;
        };
        let script = format!(
            "const c=require('child_process').spawn(process.execPath,['-e',\"console.log(process.pid);setTimeout(()=>process.exit(0),30000)\"],{{stdio:['ignore','inherit','inherit'],detached:true,windowsHide:true}});c.unref();{}",
            if root_exits { "" } else { "setTimeout(()=>process.exit(0),30000);" }
        );
        let args = vec![OsString::from("-e"), OsString::from(script)];
        let (stdout, _stderr, _pid, process, job) =
            spawn_with_hidden_console_owned(&node, &args, None, &HashMap::new()).unwrap();
        let mut line = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut line)
            .unwrap();
        let child_pid: u32 = line.trim().parse().expect("后代应成功启动并输出 PID");
        let child = unsafe {
            let raw = OpenProcess(PROCESS_SYNCHRONIZE, 0, child_pid);
            assert!(
                !raw.is_null(),
                "无法打开后代句柄: {}",
                io::Error::last_os_error()
            );
            OwnedHandle::from_raw_handle(raw)
        };
        if root_exits {
            assert_eq!(
                unsafe { WaitForSingleObject(process.as_raw_handle(), 5_000) },
                0
            );
        }
        assert_eq!(
            unsafe { WaitForSingleObject(child.as_raw_handle(), 0) },
            258
        );
        if terminate_explicitly {
            assert_ne!(unsafe { TerminateJobObject(job.as_raw_handle(), 1) }, 0);
        } else {
            drop(job);
        }
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle(), 5_000) },
            0
        );
        assert_eq!(
            unsafe { WaitForSingleObject(child.as_raw_handle(), 5_000) },
            0
        );
    }

    #[test]
    fn closing_owned_job_terminates_entire_tree() {
        check_owned_tree_cleanup(false, false);
    }

    #[test]
    fn closing_owned_job_after_root_exit_terminates_descendants() {
        check_owned_tree_cleanup(true, false);
    }

    #[test]
    fn terminating_owned_job_terminates_entire_tree() {
        check_owned_tree_cleanup(false, true);
    }

    #[test]
    fn owned_job_supports_bundled_powershell_pty() {
        use std::io::Read;
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/bundle");
        let node = bundle.join("node/node.exe");
        let pwsh = bundle.join("powershell/pwsh.exe");
        let pty = bundle.join("dsh/node_modules/node-pty");
        if !node.is_file() || !pwsh.is_file() || !pty.is_dir() {
            eprintln!("跳过 PTY 测试：未准备离线运行时");
            return;
        }
        let script = format!(
            "const pty=require({});const timer=setTimeout(()=>process.exit(2),20000);const t=pty.spawn({},['-NoLogo','-NoProfile','-Command',\"Write-Output JOB_PTY_OK\"],{{cols:80,rows:24,env:process.env}});let text='';t.onData(d=>text+=d);t.onExit(e=>{{clearTimeout(timer);console.log(text);process.exit(e.exitCode===0&&text.includes('JOB_PTY_OK')?0:3);}});",
            serde_json::to_string(&pty.to_string_lossy()).unwrap(),
            serde_json::to_string(&pwsh.to_string_lossy()).unwrap(),
        );
        let (mut stdout, mut stderr, _, process, job) = spawn_with_hidden_console_owned(
            &node,
            &[OsString::from("-e"), OsString::from(script)],
            None,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle(), 25_000) },
            0
        );
        let mut code = 0;
        assert_ne!(
            unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) },
            0
        );
        // 先回收 ConPTY 可能持有的管道写端，再读取全部输出。
        drop(job);
        let mut output = String::new();
        stdout.read_to_string(&mut output).unwrap();
        stderr.read_to_string(&mut output).unwrap();
        assert_eq!(code, 0, "PTY 在 Job 内运行失败: {output}");
    }

    #[test]
    fn env_key_match_is_case_insensitive() {
        // Windows 用户环境里的键通常是 `Path`：用 `PATH` 覆盖必须替换而不是追加
        let mut vars: Vec<(OsString, OsString)> = vec![
            (OsString::from("Path"), OsString::from("OLD")),
            (OsString::from("DSH_HOME"), OsString::from("x")),
        ];
        let key = OsString::from("PATH");
        let found = vars
            .iter_mut()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(&key));
        assert!(found.is_some());
        if let Some(entry) = found {
            entry.1 = OsString::from("NEW");
        }
        // 替换后仍是单个 Path 条目且取新值
        let path_entries: Vec<String> = vars
            .iter()
            .filter(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("path"))
            .map(|(_, v)| v.to_string_lossy().into_owned())
            .collect();
        assert_eq!(path_entries.len(), 1);
        assert_eq!(path_entries[0], "NEW");
    }

    #[test]
    fn spawn_captures_stdout_with_env_and_workdir() {
        let mut envs = HashMap::new();
        envs.insert("DSH_WIN_SPAWN_TEST".to_string(), "hello world".to_string());

        let workdir =
            std::env::temp_dir().join(format!("dsh_win_spawn_test_{}", std::process::id()));
        std::fs::create_dir_all(&workdir).unwrap();

        let args = vec![
            OsString::from("/d"),
            OsString::from("/c"),
            OsString::from("echo %DSH_WIN_SPAWN_TEST% && cd"),
        ];
        let (stdout, _stderr) = spawn_with_hidden_console(
            Path::new("C:\\Windows\\System32\\cmd.exe"),
            &args,
            Some(&workdir),
            &envs,
        )
        .unwrap();

        let mut output_bytes = Vec::new();
        use std::io::Read;
        let mut reader = stdout;
        reader.read_to_end(&mut output_bytes).unwrap();
        let output = String::from_utf8_lossy(&output_bytes);

        std::fs::remove_dir_all(&workdir).ok();

        assert!(output.contains("hello world"), "stdout: {output:?}");
        assert!(
            output.contains(&workdir.to_string_lossy().into_owned()),
            "stdout: {output:?}"
        );
    }

    #[test]
    fn spawn_preserves_node_script_path_with_spaces() {
        let node = find_node_on_path().expect("node.exe not found for the test");
        let root = std::env::temp_dir().join(format!(
            "dsh win spawn path with spaces {}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("print argv.js");
        std::fs::write(&script, "process.stdout.write(process.argv[2]);").unwrap();

        let args = vec![script.as_os_str().to_os_string(), OsString::from("probe")];
        let (stdout, _stderr) =
            spawn_with_hidden_console(&node, &args, Some(&root), &HashMap::new()).unwrap();

        let mut output = String::new();
        use std::io::Read;
        let mut reader = stdout;
        reader.read_to_string(&mut output).unwrap();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(output, "probe");
    }

    #[test]
    fn spawned_process_gets_hidden_console() {
        let script = "$code='using System;using System.Runtime.InteropServices;public class C{[DllImport(\"kernel32.dll\")]public static extern IntPtr GetConsoleWindow();[DllImport(\"user32.dll\")]public static extern bool IsWindowVisible(IntPtr h);}';Add-Type -TypeDefinition $code;$h=[C]::GetConsoleWindow();if($h -eq [IntPtr]::Zero){'NO_CONSOLE'}else{'HAS_CONSOLE_VISIBLE='+[C]::IsWindowVisible($h)}";

        let args = vec![
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-Command"),
            OsString::from(script),
        ];
        let (stdout, _stderr) = spawn_with_hidden_console(
            Path::new("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
            &args,
            None,
            &HashMap::new(),
        )
        .unwrap();

        let mut output = String::new();
        use std::io::Read;
        let mut reader = stdout;
        reader.read_to_string(&mut output).unwrap();

        assert!(
            output.contains("HAS_CONSOLE_VISIBLE=False"),
            "expected a hidden console, got: {output:?}"
        );
    }

    #[test]
    fn grandchildren_inherit_hidden_console() {
        let node = find_node_on_path().expect("node.exe not found for the test");
        let ps1 =
            std::env::temp_dir().join(format!("dsh_console_check_{}.ps1", std::process::id()));
        std::fs::write(
            &ps1,
            "$code='using System;using System.Runtime.InteropServices;public class C{[DllImport(\"kernel32.dll\")]public static extern IntPtr GetConsoleWindow();[DllImport(\"user32.dll\")]public static extern bool IsWindowVisible(IntPtr h);}';Add-Type -TypeDefinition $code;$h=[C]::GetConsoleWindow();if($h -eq [IntPtr]::Zero){'NO_CONSOLE'}else{'HAS_CONSOLE_VISIBLE='+[C]::IsWindowVisible($h)}",
        )
        .unwrap();

        // node 作为“dsh 代理”：用 child_process 派生一个 powershell 孙进程，
        // 旧实现（node 无控制台）下孙进程会新建可见控制台窗口。
        let node_js = format!(
            "const{{spawnSync}}=require('child_process');const r=spawnSync('C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe',['-NoProfile','-NonInteractive','-File','{}'],{{encoding:'utf8'}});process.stdout.write((r.stdout??'')+(r.stderr??''));",
            ps1.to_string_lossy().replace('\\', "/")
        );

        let args = vec![OsString::from("-e"), OsString::from(node_js)];
        let (stdout, stderr) =
            spawn_with_hidden_console(&node, &args, None, &HashMap::new()).unwrap();

        let mut output = String::new();
        let mut error = String::new();
        use std::io::Read;
        let mut reader = stdout;
        reader.read_to_string(&mut output).unwrap();
        let mut reader = stderr;
        reader.read_to_string(&mut error).unwrap();
        std::fs::remove_file(&ps1).ok();

        assert!(
            output.contains("HAS_CONSOLE_VISIBLE=False"),
            "expected grandchildren to inherit a hidden console, got: {output:?} stderr: {error:?}"
        );
    }

    fn find_node_on_path() -> Option<std::path::PathBuf> {
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("node.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }
}
