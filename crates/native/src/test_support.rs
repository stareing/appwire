//! 测试支持（feature `test-support`，只供本仓库的测试使用）：构建并定位 `examples/fake_host`。
//!
//! @why `cargo test` 只以测试模式（libtest 入口、带哈希的文件名）构建示例，不刷新
//! `target/<profile>/examples/fake_host`；直接用该路径会拿到旧版本（协议更新后测试莫名失败）。
//! 因此每个测试进程先用调用它的 cargo 构建一次（已是最新时 cargo 只做检查），再返回路径。

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// 直接指定 fake_host 可执行文件（跳过构建）的环境变量，与各语言 SDK 测试一致。
pub const FAKE_HOST_ENV: &str = "APP_MCP_FAKE_HOST";

/// 构建（每个进程一次）并返回 fake_host 的路径。
///
/// @input 测试可执行文件位于 `<target-dir>/<profile>/deps/`（cargo 的布局）；输出到同一 target 目录与 profile。
/// @output 设置了 [`FAKE_HOST_ENV`] 时原样返回该路径；否则为新构建的 `<target-dir>/<profile>/examples/fake_host[.exe]`。
/// @error 无法确定 target 目录、无法运行 cargo 或构建失败时返回说明（含 cargo 的 stderr）。
pub fn fake_host_path() -> Result<PathBuf, String> {
    static PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    PATH.get_or_init(build_fake_host).clone()
}

fn build_fake_host() -> Result<PathBuf, String> {
    if let Some(explicit) = std::env::var_os(FAKE_HOST_ENV) {
        return Ok(PathBuf::from(explicit));
    }
    let exe = std::env::current_exe().map_err(|e| format!("无法确定测试可执行文件路径：{e}"))?;
    let profile_dir = exe
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| format!("测试可执行文件不在 <target>/<profile>/deps 下：{}", exe.display()))?;
    let profile = profile_dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("无法确定 profile：{}", profile_dir.display()))?;
    let target_dir = profile_dir
        .parent()
        .ok_or_else(|| format!("无法确定 target 目录：{}", profile_dir.display()))?;
    // cargo 运行测试时设置 CARGO（当前 cargo 的路径）。
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut cmd = Command::new(cargo);
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["build", "-q", "-p", "app-mcp-native", "--example", "fake_host", "--target-dir"])
        .arg(target_dir);
    match profile {
        "debug" => {}
        "release" => {
            cmd.arg("--release");
        }
        other => {
            cmd.args(["--profile", other]);
        }
    }
    let out = cmd.output().map_err(|e| format!("无法运行 cargo 构建 fake_host：{e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo 构建 fake_host 失败（{}）：{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let path = profile_dir
        .join("examples")
        .join(format!("fake_host{}", std::env::consts::EXE_SUFFIX));
    if path.exists() {
        Ok(path)
    } else {
        Err(format!("cargo 构建成功，但找不到 {}", path.display()))
    }
}
