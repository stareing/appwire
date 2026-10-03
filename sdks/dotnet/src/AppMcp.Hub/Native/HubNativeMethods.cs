using System.Reflection;
using System.Runtime.InteropServices;

namespace AppMcp.Hub.Native;

// 与 bindings/hub-c/include/app_mcp_hub.h 逐项对应。枚举一律按 C int（4 字节）传递。

internal static unsafe partial class HubNativeMethods
{
    internal const string Lib = "app_mcp_hub";

    /// <summary>本封装对应的 C 接口版本（app_mcp_hub.h 中的 AM_HUB_API_VERSION）。</summary>
    internal const int ApiVersion = 3;

    [LibraryImport(Lib)] internal static partial nint am_hub_version();
    [LibraryImport(Lib)] internal static partial nint am_hub_last_error_message();
    [LibraryImport(Lib)] internal static partial void am_hub_string_free(nint s);

    [LibraryImport(Lib)] internal static partial HubStatus am_hub_start(byte* configJson, out nint hub);
    [LibraryImport(Lib)] internal static partial void am_hub_shutdown(HubSafeHandle hub);
    [LibraryImport(Lib)] internal static partial void am_hub_free(nint hub);
    [LibraryImport(Lib)] internal static partial nint am_hub_listen_addr(HubSafeHandle hub);
    [LibraryImport(Lib)] internal static partial nint am_hub_ipc_endpoint(HubSafeHandle hub);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_serve_http(HubSafeHandle hub, byte* addr, [MarshalAs(UnmanagedType.U1)] bool allowRemote, out nint outAddr);

    [LibraryImport(Lib)] internal static partial HubStatus am_hub_apps_json(HubSafeHandle hub, out nint json);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_tools_json(HubSafeHandle hub, byte* filterJson, out nint json);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_resources_json(HubSafeHandle hub, out nint json);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_overview_json(HubSafeHandle hub, byte* appId, out nint json);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_status_json(HubSafeHandle hub, out nint json);

    [LibraryImport(Lib)] internal static partial HubStatus am_hub_call(HubSafeHandle hub, byte* requestJson, nint cb, nint userData, out nint callId);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_call_with_progress(HubSafeHandle hub, byte* requestJson, nint cb, nint onProgress, nint userData, out nint callId);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_cancel_call(HubSafeHandle hub, byte* callId);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_read_resource(HubSafeHandle hub, byte* uri, nint cb, nint userData);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_subscribe(HubSafeHandle hub, byte* uri);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_unsubscribe(HubSafeHandle hub, byte* uri);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_select_instance(HubSafeHandle hub, byte* appId, byte* instanceId);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_reset_session(HubSafeHandle hub, byte* session);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_set_policy(HubSafeHandle hub, byte* policyJson);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_set_agents(HubSafeHandle hub, byte* agentsJson);

    [LibraryImport(Lib)] internal static partial HubStatus am_hub_export_tools(HubSafeHandle hub, int format, byte* filterJson, out nint json);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_dispatch(HubSafeHandle hub, int format, byte* toolCallJson, byte* session, nint cb, nint userData);

    [LibraryImport(Lib)] internal static partial HubStatus am_hub_set_event_cb(HubSafeHandle hub, nint cb, nint userData, nint freeUserData);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_set_approval_cb(HubSafeHandle hub, nint cb, nint userData, nint freeUserData);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_approval_complete(nint approval, [MarshalAs(UnmanagedType.U1)] bool approved);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_set_pairing_cb(HubSafeHandle hub, nint cb, nint userData, nint freeUserData);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_pairing_complete(nint pairing, [MarshalAs(UnmanagedType.U1)] bool approved);
    // v2：自定义唤醒
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_set_waker_cb(HubSafeHandle hub, nint cb, nint userData, nint freeUserData);
    [LibraryImport(Lib)] internal static partial HubStatus am_hub_waker_complete(nint wake, [MarshalAs(UnmanagedType.U1)] bool ok, byte* errorKind, byte* message);

    // -----------------------------------------------------------------------
    // 辅助
    // -----------------------------------------------------------------------

    internal static string? PtrToString(nint p) => p == 0 ? null : Marshal.PtrToStringUTF8(p);

    /// <summary>取走库返回的 char*（用 am_hub_string_free 释放）。</summary>
    internal static string? TakeString(nint p)
    {
        if (p == 0) return null;
        try { return Marshal.PtrToStringUTF8(p); }
        finally { am_hub_string_free(p); }
    }

    internal static string LastError() => PtrToString(am_hub_last_error_message()) ?? string.Empty;

    /// <summary>非 AM_HUB_OK 时抛出 <see cref="HubException"/>（必须紧跟在失败的调用之后、同一线程上调用）。</summary>
    internal static void Check(HubStatus status)
    {
        if (status != HubStatus.Ok) throw new HubException(status, LastError());
    }

    // -----------------------------------------------------------------------
    // 原生库加载
    // -----------------------------------------------------------------------

    internal const string NativePathEnv = "APP_MCP_HUB_NATIVE_PATH";

    static HubNativeMethods()
    {
        try
        {
            NativeLibrary.SetDllImportResolver(typeof(HubNativeMethods).Assembly, Resolve);
        }
        catch (InvalidOperationException)
        {
            // 宿主已设置过解析器，沿用默认行为。
        }
    }

    internal static string PlatformFileName =>
        OperatingSystem.IsWindows() ? "app_mcp_hub.dll"
        : OperatingSystem.IsMacOS() || OperatingSystem.IsIOS() ? "libapp_mcp_hub.dylib"
        : "libapp_mcp_hub.so";

    /// <summary>按 APP_MCP_HUB_NATIVE_PATH（文件或目录）→ 程序目录 → runtimes/&lt;rid&gt;/native → 系统默认 的顺序查找。</summary>
    private static nint Resolve(string libraryName, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (libraryName != Lib) return 0;
        foreach (var candidate in Candidates())
        {
            if (File.Exists(candidate) && NativeLibrary.TryLoad(candidate, out var handle)) return handle;
        }
        return 0;
    }

    private static IEnumerable<string> Candidates()
    {
        var env = Environment.GetEnvironmentVariable(NativePathEnv);
        if (!string.IsNullOrEmpty(env))
        {
            yield return Directory.Exists(env) ? Path.Combine(env, PlatformFileName) : env;
        }
        var baseDir = AppContext.BaseDirectory;
        yield return Path.Combine(baseDir, PlatformFileName);
        yield return Path.Combine(baseDir, "runtimes", RuntimeInformation.RuntimeIdentifier, "native", PlatformFileName);
    }
}

internal sealed class HubSafeHandle : SafeHandle
{
    public HubSafeHandle() : base(0, ownsHandle: true) { }
    public HubSafeHandle(nint handle) : this() => SetHandle(handle);
    public override bool IsInvalid => handle == 0;

    /// <summary>停止并释放 Hub（等待分发线程处理完已排队的回调）。</summary>
    protected override bool ReleaseHandle()
    {
        HubNativeMethods.am_hub_free(handle);
        return true;
    }
}

/// <summary>调用期间有效的 UTF-8 字符串（非托管内存），Dispose 时释放。</summary>
internal sealed class Utf8Strings : IDisposable
{
    private readonly List<nint> _allocations = new();

    public unsafe byte* Add(string? s)
    {
        if (s is null) return null;
        var p = Marshal.StringToCoTaskMemUTF8(s);
        _allocations.Add(p);
        return (byte*)p;
    }

    public void Dispose()
    {
        foreach (var p in _allocations) Marshal.FreeCoTaskMem(p);
        _allocations.Clear();
    }
}
