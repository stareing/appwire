"""appwire-cli 启动器：解析二进制、缺失时的报错，以及按真实 wheel 安装后的命令行为。"""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import venv
from pathlib import Path

import pytest

from appwire_cli.launcher import HostBinaryMissing, host_binary_path, host_executable_name

PACKAGING_DIR = Path(__file__).resolve().parents[2]
SRC_DIR = PACKAGING_DIR / "pypi" / "src"
sys.path.insert(0, str(PACKAGING_DIR / "scripts"))
import build_wheels  # noqa: E402

FAKE_EXIT_CODE = 7
POSIX_ONLY = pytest.mark.skipif(sys.platform == "win32", reason="假二进制是 shell 脚本")


def test_executable_name() -> None:
    assert host_executable_name("win32") == "app-mcp-host.exe"
    assert host_executable_name("linux") == "app-mcp-host"


def test_resolves_binary_in_bin_dir(tmp_path: Path) -> None:
    (tmp_path / "app-mcp-host").write_text("")
    assert host_binary_path(tmp_path, "linux") == tmp_path / "app-mcp-host"


def test_missing_binary_is_classified(tmp_path: Path) -> None:
    with pytest.raises(HostBinaryMissing, match="cargo build"):
        host_binary_path(tmp_path, "linux")


def test_missing_binary_exits_1_with_message() -> None:
    # 源码目录里没有 bin/（只有构建 wheel 时才放入）。
    env = {**os.environ, "PYTHONPATH": str(SRC_DIR)}
    result = subprocess.run([sys.executable, "-m", "appwire_cli", "status"], env=env, capture_output=True, text=True)
    assert result.returncode == 1
    assert "app-mcp-host" in result.stderr


def test_build_rejects_missing_binaries(tmp_path: Path) -> None:
    with pytest.raises(build_wheels.BuildError, match="缺少构建产物"):
        build_wheels.build_wheels("1.2.3", tmp_path, tmp_path / "out", ["win32-x64"])


def test_build_rejects_unknown_platform(tmp_path: Path) -> None:
    with pytest.raises(build_wheels.BuildError, match="未知平台"):
        build_wheels.build_wheels("1.2.3", tmp_path, tmp_path / "out", ["plan9-x64"])


def write_fake_bins(bins: Path, body: str) -> None:
    for platform in build_wheels.load_platforms():
        (bins / platform.id).mkdir(parents=True, exist_ok=True)
        for binary in platform.binaries:
            path = bins / platform.id / f"{binary}{platform.exe_suffix}"
            path.write_text(f"#!/bin/sh\n{body}\n")
            path.chmod(0o755)


def current_platform_id() -> str | None:
    machine = os.uname().machine if hasattr(os, "uname") else ""
    arch = {"x86_64": "x64", "amd64": "x64", "aarch64": "arm64", "arm64": "arm64"}.get(machine.lower())
    system = {"linux": "linux", "darwin": "darwin"}.get(sys.platform)
    return f"{system}-{arch}" if system and arch else None


@pytest.fixture(scope="module")
def installed_appwire(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """构建当前平台的 wheel（假二进制）并装进新虚拟环境，返回其中的 appwire 命令。"""
    platform_id = current_platform_id()
    if platform_id is None:
        pytest.skip("当前平台不在 platforms.json 中")
    work = tmp_path_factory.mktemp("wheel")
    script = "\n".join(
        [
            'if [ "$1" = "--signal" ]; then kill -TERM $$; fi',
            'for a in "$@"; do printf "<%s>" "$a"; done',
            f"exit {FAKE_EXIT_CODE}",
        ]
    )
    write_fake_bins(work / "bins", script)
    (wheel,) = build_wheels.build_wheels("1.2.3", work / "bins", work / "out", [platform_id])
    tag = next(p.wheel_platform_tag for p in build_wheels.load_platforms() if p.id == platform_id)
    name, version, python_tag, abi_tag, platform_tags = wheel.name.removesuffix(".whl").split("-")
    assert (name, version, python_tag, abi_tag) == ("appwire_cli", "1.2.3", "py3", "none")
    # wheel tags 会对复合标签排序。
    assert set(platform_tags.split(".")) == set(tag.split("."))
    env_dir = work / "venv"
    venv.EnvBuilder(with_pip=True).create(env_dir)
    python = env_dir / "bin" / "python"
    subprocess.run([str(python), "-m", "pip", "install", "--no-index", "--quiet", str(wheel)], check=True)
    return env_dir / "bin"


@POSIX_ONLY
def test_installed_command_passes_args_and_exit_code(installed_appwire: Path) -> None:
    for command in ("appwire", "appwire-cli"):
        result = subprocess.run(
            [str(installed_appwire / command), "serve", "--listen", "127.0.0.1:0", "a b", ""],
            capture_output=True,
            text=True,
        )
        assert result.stdout == "<serve><--listen><127.0.0.1:0><a b><>"
        assert result.returncode == FAKE_EXIT_CODE


@POSIX_ONLY
def test_installed_command_propagates_signal(installed_appwire: Path) -> None:
    result = subprocess.run([str(installed_appwire / "appwire"), "--signal"], capture_output=True)
    assert result.returncode == -signal.SIGTERM


@POSIX_ONLY
def test_installed_binary_keeps_executable_bit(installed_appwire: Path) -> None:
    python = installed_appwire / "python"
    code = "import json, os; from appwire_cli import host_binary_path as p; print(json.dumps(os.access(p(), os.X_OK)))"
    out = subprocess.run([str(python), "-c", code], capture_output=True, text=True, check=True).stdout
    assert json.loads(out) is True
