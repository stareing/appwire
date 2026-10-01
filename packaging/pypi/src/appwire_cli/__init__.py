"""AppWire 命令行入口：执行随 wheel 附带的预编译 app-mcp-host（参数透传、退出码回传）。"""

from appwire_cli.launcher import HostBinaryMissing, host_binary_path

__all__ = ["HostBinaryMissing", "host_binary_path"]
