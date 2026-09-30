using System.Runtime.InteropServices;
using System.Runtime.Versioning;

namespace AppMcp.Activation;

/// <summary>当前进程的包身份（MSIX 等）。</summary>
public interface IPackageIdentity
{
    /// <summary>有包身份时返回 AUMID（Application User Model ID），否则 null。</summary>
    string? GetAppUserModelId();
}

/// <summary>通过 kernel32 的 <c>GetCurrentPackageFullName</c> / <c>GetCurrentApplicationUserModelId</c> 检测包身份；非 Windows 返回 null。</summary>
public sealed partial class WindowsPackageIdentity : IPackageIdentity
{
    private const int ErrorInsufficientBuffer = 122;

    public string? GetAppUserModelId()
    {
        if (!OperatingSystem.IsWindows()) return null;
        try
        {
            return QueryPackageFullName() is null ? null : QueryAumid();
        }
        catch (Exception e) when (e is DllNotFoundException or EntryPointNotFoundException)
        {
            return null; // Windows 8 之前没有这些 API
        }
    }

    // 先取长度再取值；没有包身份（APPMODEL_ERROR_NO_PACKAGE 等）时返回 null。
    [SupportedOSPlatform("windows")]
    private static unsafe string? QueryPackageFullName()
    {
        uint len = 0;
        if (GetCurrentPackageFullName(ref len, null) != ErrorInsufficientBuffer || len == 0) return null;
        var buf = new char[len];
        fixed (char* p = buf)
        {
            return GetCurrentPackageFullName(ref len, p) == 0 ? new string(p, 0, (int)Math.Max(0, len - 1)) : null;
        }
    }

    [SupportedOSPlatform("windows")]
    private static unsafe string? QueryAumid()
    {
        uint len = 0;
        if (GetCurrentApplicationUserModelId(ref len, null) != ErrorInsufficientBuffer || len == 0) return null;
        var buf = new char[len];
        fixed (char* p = buf)
        {
            return GetCurrentApplicationUserModelId(ref len, p) == 0 ? new string(p, 0, (int)Math.Max(0, len - 1)) : null;
        }
    }

    [SupportedOSPlatform("windows")]
    [LibraryImport("kernel32.dll")]
    private static unsafe partial int GetCurrentPackageFullName(ref uint packageFullNameLength, char* packageFullName);

    [SupportedOSPlatform("windows")]
    [LibraryImport("kernel32.dll")]
    private static unsafe partial int GetCurrentApplicationUserModelId(ref uint applicationUserModelIdLength, char* applicationUserModelId);
}

/// <summary>按平台生成 <see cref="WakeDescriptor"/>（spec/lifecycle.md 第 5 节）。</summary>
public static class WakeDescriptorFactory
{
    /// <summary>
    /// Windows：有包身份（MSIX）时用 <c>aumid</c>（Host 以 <c>ActivateApplication(aumid, "app-mcp-wake:&lt;token&gt;")</c> 激活）；
    /// 否则用 <c>uri</c>，target 为 <paramref name="uriScheme"/>（需先 <see cref="ProtocolRegistration.Register"/>）。
    /// </summary>
    /// <param name="uriScheme">未打包时使用的 URL scheme；为 null 且无包身份时返回 kind = none。</param>
    /// <param name="background">能否不前置窗口就唤醒：普通窗口应用为 false，托盘 / 无窗口进程为 true。</param>
    /// <param name="identity">包身份检测，默认 <see cref="WindowsPackageIdentity"/>。</param>
    public static WakeDescriptor ForWindows(string? uriScheme, bool background = false, IPackageIdentity? identity = null)
    {
        var aumid = (identity ?? new WindowsPackageIdentity()).GetAppUserModelId();
        if (!string.IsNullOrEmpty(aumid)) return new WakeDescriptor(WakeKind.Aumid, aumid, background);
        if (!string.IsNullOrEmpty(uriScheme)) return new WakeDescriptor(WakeKind.Uri, uriScheme, background);
        return new WakeDescriptor(WakeKind.None);
    }
}
