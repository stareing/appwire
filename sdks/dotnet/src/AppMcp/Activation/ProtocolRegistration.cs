using System.Runtime.Versioning;
using System.Text.RegularExpressions;

namespace AppMcp.Activation;

/// <summary>注册表写入的抽象（路径相对 HKEY_CURRENT_USER），便于测试时替换。</summary>
public interface IRegistryWriter
{
    /// <summary>写入字符串值；<paramref name="name"/> 为 null 表示默认值。按需创建子键。</summary>
    void SetValue(string subKeyPath, string? name, string value);

    /// <summary>读取字符串值；子键或值不存在时返回 null。</summary>
    string? GetValue(string subKeyPath, string? name);

    /// <summary>递归删除子键（不存在时无效果）。</summary>
    void DeleteTree(string subKeyPath);
}

/// <summary>写入真实的 HKEY_CURRENT_USER。</summary>
[SupportedOSPlatform("windows")]
public sealed class WindowsRegistryWriter : IRegistryWriter
{
    public void SetValue(string subKeyPath, string? name, string value)
    {
        using var key = Microsoft.Win32.Registry.CurrentUser.CreateSubKey(subKeyPath, writable: true);
        key.SetValue(name ?? string.Empty, value);
    }

    public string? GetValue(string subKeyPath, string? name)
    {
        using var key = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(subKeyPath);
        return key?.GetValue(name ?? string.Empty) as string;
    }

    public void DeleteTree(string subKeyPath) =>
        Microsoft.Win32.Registry.CurrentUser.DeleteSubKeyTree(subKeyPath, throwOnMissingSubKey: false);
}

/// <summary>
/// 未打包 Windows 应用的自定义 URL scheme 注册（<c>HKCU\Software\Classes\&lt;scheme&gt;</c>，无需管理员权限）。
/// Host 唤醒时打开 <c>&lt;scheme&gt;:app-mcp/wake?token=…</c>，系统以 <c>"exe" "%1"</c> 启动 App；
/// App 已在运行时由 <see cref="SingleInstance"/> 把参数转交给首实例。
/// </summary>
public static partial class ProtocolRegistration
{
    [GeneratedRegex("^[a-zA-Z][a-zA-Z0-9+.-]*$")]
    private static partial Regex SchemePattern();

    /// <summary>键路径：<c>Software\Classes\&lt;scheme&gt;</c>。</summary>
    public static string KeyPath(string scheme) => $@"Software\Classes\{ValidateScheme(scheme)}";

    /// <summary>shell\open\command 的值：<c>"exe" "%1"</c>。</summary>
    public static string CommandLine(string executablePath) => $"\"{executablePath}\" \"%1\"";

    /// <summary>注册（覆盖已有值）。<paramref name="registry"/> 为 null 时写入真实注册表（仅 Windows）。</summary>
    public static void Register(string scheme, string executablePath, string? friendlyName = null, IRegistryWriter? registry = null)
    {
        ArgumentException.ThrowIfNullOrEmpty(executablePath);
        var reg = registry ?? DefaultWriter();
        var key = KeyPath(scheme);
        reg.SetValue(key, null, $"URL:{friendlyName ?? scheme}");
        reg.SetValue(key, "URL Protocol", string.Empty);
        reg.SetValue($@"{key}\shell\open\command", null, CommandLine(executablePath));
    }

    /// <summary>当前注册是否指向 <paramref name="executablePath"/>。</summary>
    public static bool IsRegistered(string scheme, string executablePath, IRegistryWriter? registry = null)
    {
        var reg = registry ?? DefaultWriter();
        var key = KeyPath(scheme);
        return reg.GetValue(key, "URL Protocol") is not null
            && string.Equals(reg.GetValue($@"{key}\shell\open\command", null), CommandLine(executablePath), StringComparison.OrdinalIgnoreCase);
    }

    /// <summary>删除注册。</summary>
    public static void Unregister(string scheme, IRegistryWriter? registry = null) => (registry ?? DefaultWriter()).DeleteTree(KeyPath(scheme));

    private static string ValidateScheme(string scheme)
    {
        ArgumentException.ThrowIfNullOrEmpty(scheme);
        if (!SchemePattern().IsMatch(scheme)) throw new ArgumentException($"非法的 URL scheme：{scheme}", nameof(scheme));
        return scheme;
    }

    private static IRegistryWriter DefaultWriter()
    {
        if (!OperatingSystem.IsWindows()) throw new PlatformNotSupportedException("协议注册只支持 Windows；其他平台请传入 IRegistryWriter");
        return new WindowsRegistryWriter();
    }
}
