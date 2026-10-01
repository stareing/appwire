"""定位并执行 wheel 内的 app-mcp-host。

@invariant 二进制位于本包 ``bin/`` 目录（由 packaging/scripts/build_wheels.py 放入）；运行时不联网下载。
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from typing import NoReturn, Sequence

# crates/host/Cargo.toml 中的 [[bin]] 名。
HOST_BINARY = "app-mcp-host"

_BIN_DIR = Path(__file__).resolve().parent / "bin"


class HostBinaryMissing(RuntimeError):
    """wheel 内没有当前平台的 app-mcp-host（例如从源码目录直接运行，或安装了其他平台的 wheel）。"""


def host_executable_name(platform: str = sys.platform) -> str:
    return f"{HOST_BINARY}.exe" if platform == "win32" else HOST_BINARY


def host_binary_path(bin_dir: Path | None = None, platform: str = sys.platform) -> Path:
    """@error HostBinaryMissing：文件不存在。"""
    path = (_BIN_DIR if bin_dir is None else bin_dir) / host_executable_name(platform)
    if not path.is_file():
        raise HostBinaryMissing(
            f"找不到 {path}。appwire-cli 只以平台 wheel 发布（含预编译 {HOST_BINARY}）；"
            "请用 pip / uv 在本机重新安装，或从源码构建：cargo build --release -p app-mcp-host"
            "（https://github.com/stareing/appwire）。"
        )
    return path


def exec_host(binary: Path, args: Sequence[str]) -> NoReturn:
    """@side-effect POSIX 上以 execv 替换当前进程（信号、退出码天然一致）；
    Windows 无真正的 exec（os.execv 会另起进程并立即返回），改为子进程等待后以其退出码退出。"""
    argv = [str(binary), *args]
    if sys.platform == "win32":
        process = subprocess.Popen(argv)
        while True:
            try:
                sys.exit(process.wait())
            except KeyboardInterrupt:
                # 控制台的 Ctrl+C 同时送达子进程：不杀它，等它自行收尾后取其退出码。
                continue
    os.execv(argv[0], argv)


def main(argv: Sequence[str] | None = None) -> NoReturn:
    args = list(sys.argv[1:] if argv is None else argv)
    try:
        binary = host_binary_path()
    except HostBinaryMissing as error:
        print(f"[appwire] {error}", file=sys.stderr)
        sys.exit(1)
    exec_host(binary, args)
