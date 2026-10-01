#!/usr/bin/env python3
"""构建 appwire-cli 的平台 wheel：每个平台一个 py3-none-<平台标签> wheel，内含该平台的 app-mcp-host。

用法：python packaging/scripts/build_wheels.py --version <x.y.z> --bins <dir> --out <dir> [--platform <id> ...]

@input  <bins>/<平台 id>/<二进制>[.exe]（平台、二进制与 wheel 平台标签见 packaging/platforms.json）
@output <out>/appwire_cli-<版本>-py3-none-<平台标签>.whl
@error  缺少二进制、构建失败时以非零退出码结束
@why    先按纯 Python 包构建（launcher + bin/ 数据文件），再用 `wheel tags` 改平台标签：
        setuptools 的 --plat-name 会把复合标签中的 "." 替换为 "_"，而 Linux 静态二进制需要
        manylinux + musllinux 复合标签。二进制已由发布流水线构建（与 GitHub Release / npm 同一份），这里不再编译。
依赖：setuptools>=77、build、wheel（`--no-isolation` 使用当前环境中的版本）。
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from dataclasses import dataclass
from pathlib import Path

PACKAGING_DIR = Path(__file__).resolve().parent.parent
REPO_ROOT = PACKAGING_DIR.parent
PROJECT_DIR = PACKAGING_DIR / "pypi"
PACKAGE = "appwire_cli"
LICENSE_FILES = ("LICENSE-MIT", "LICENSE-APACHE")
VERSION_LINE = re.compile(r'^version = ".*"$', re.MULTILINE)
EXECUTABLE_MODE = 0o755


@dataclass(frozen=True)
class Platform:
    id: str
    binaries: tuple[str, ...]
    exe_suffix: str
    wheel_platform_tag: str


class BuildError(RuntimeError):
    pass


def load_platforms() -> list[Platform]:
    data = json.loads((PACKAGING_DIR / "platforms.json").read_text(encoding="utf-8"))
    return [
        Platform(p["id"], tuple(p["binaries"]), p["exeSuffix"], p["wheelPlatformTag"])
        for p in data["platforms"]
    ]


def stage_project(platform: Platform, version: str, bins_dir: Path, stage_dir: Path) -> None:
    shutil.copytree(
        PROJECT_DIR,
        stage_dir,
        ignore=shutil.ignore_patterns("tests", "build", "dist", "*.egg-info", "__pycache__", ".pytest_cache"),
    )
    pyproject = stage_dir / "pyproject.toml"
    text, count = VERSION_LINE.subn(f'version = "{version}"', pyproject.read_text(encoding="utf-8"), count=1)
    if count != 1:
        raise BuildError(f"{pyproject} 中没有 version 行")
    pyproject.write_text(text, encoding="utf-8")
    for name in LICENSE_FILES:
        shutil.copy2(REPO_ROOT / name, stage_dir / name)
    bin_dir = stage_dir / "src" / PACKAGE / "bin"
    bin_dir.mkdir(parents=True)
    for binary in platform.binaries:
        source = bins_dir / platform.id / f"{binary}{platform.exe_suffix}"
        if not source.is_file():
            raise BuildError(f"缺少构建产物：{source}")
        target = bin_dir / source.name
        shutil.copyfile(source, target)
        target.chmod(EXECUTABLE_MODE)


def run(argv: list[str]) -> str:
    completed = subprocess.run(argv, capture_output=True, text=True)
    if completed.returncode != 0:
        raise BuildError(f"命令失败（{completed.returncode}）：{' '.join(argv)}\n{completed.stdout}{completed.stderr}")
    return completed.stdout


def check_wheel(wheel: Path, platform: Platform) -> None:
    """@error BuildError：二进制缺失或丢失可执行位（安装器按 zip 中的模式位恢复可执行权限）。"""
    with zipfile.ZipFile(wheel) as archive:
        infos = {info.filename: info for info in archive.infolist()}
    for binary in platform.binaries:
        entry = f"{PACKAGE}/bin/{binary}{platform.exe_suffix}"
        info = infos.get(entry)
        if info is None:
            raise BuildError(f"{wheel.name} 中缺少 {entry}")
        if (info.external_attr >> 16) & 0o111 == 0:
            raise BuildError(f"{wheel.name} 中 {entry} 没有可执行位")


def build_wheel(platform: Platform, version: str, bins_dir: Path, out_dir: Path) -> Path:
    with tempfile.TemporaryDirectory(prefix=f"appwire-wheel-{platform.id}-") as tmp:
        stage_dir = Path(tmp) / "project"
        dist_dir = Path(tmp) / "dist"
        stage_project(platform, version, bins_dir, stage_dir)
        run([sys.executable, "-m", "build", "--wheel", "--no-isolation", "--outdir", str(dist_dir), str(stage_dir)])
        (pure,) = dist_dir.glob("*-py3-none-any.whl")
        output = run(
            [sys.executable, "-m", "wheel", "tags", "--platform-tag", platform.wheel_platform_tag, "--remove", str(pure)]
        )
        tagged = dist_dir / output.strip().splitlines()[-1]
        check_wheel(tagged, platform)
        out_dir.mkdir(parents=True, exist_ok=True)
        return Path(shutil.move(str(tagged), out_dir / tagged.name))


def build_wheels(version: str, bins_dir: Path, out_dir: Path, only: list[str] | None = None) -> list[Path]:
    platforms = load_platforms()
    unknown = sorted(set(only or []) - {p.id for p in platforms})
    if unknown:
        raise BuildError(f"未知平台：{', '.join(unknown)}")
    selected = [p for p in platforms if not only or p.id in only]
    return [build_wheel(p, version, bins_dir, out_dir) for p in selected]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--version", required=True)
    parser.add_argument("--bins", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--platform", action="append", help="只构建指定平台 id（可重复）；默认全部")
    args = parser.parse_args()
    try:
        for wheel in build_wheels(args.version, args.bins.resolve(), args.out.resolve(), args.platform):
            print(wheel)
    except BuildError as error:
        print(f"[build_wheels] {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
