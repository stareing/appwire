using System.Reflection;
using System.Runtime.InteropServices;

namespace AppMcp.Native;

// 与 bindings/c/include/app_mcp.h 逐项对应。枚举一律按 C int（4 字节）传递。

internal enum AmStatus
{
    Ok = 0,
    InvalidArgument = 1,
    InvalidName = 2,
    InvalidSchema = 3,
    DuplicateName = 4,
    InvalidJson = 5,
    InvalidConfig = 6,
    AlreadyCompleted = 7,
    Disposed = 8,
    Stopped = 9,
    Internal = 10,
    Panic = 11,
}

[StructLayout(LayoutKind.Sequential)]
internal struct AmClientConfig
{
    public nint AppId;
    public nint AppName;
    public nint InstanceId;
    public nint HostUrl;
    public nint AppVersion;
    public nint InstanceTitle;
    public nint Token;
    public nint LaunchToken;
    public int ClientKind;
    public uint MaxConcurrentCalls;
    public nint OverviewSummary;
    public nint OverviewBody;
    public nint OverviewLocale;
}

[StructLayout(LayoutKind.Sequential)]
internal struct AmClientCallbacks
{
    // 回调中的字符串归回调方所有，需 am_string_free（AM_API_VERSION 2）。
    public nint OnState;   // void (*)(void*, AmStateStatus, uint64_t, char* reason)
    public nint OnPaired;  // void (*)(void*, char* token)
    public nint OnLog;     // void (*)(void*, AmLogLevel, char* message)
    public nint UserData;
    public nint FreeUserData;
}

/// <summary>v3：生命周期策略（与 C 的 AmLifecycle 布局一致）。</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct AmLifecycle
{
    public int Mode;
    public ulong IdleTimeoutMs;
    public ulong HiddenIdleTimeoutMs;
    public ulong GraceMs;
    public int Residency;
    public int WakeKind;          // -1 = AM_WAKE_UNSET
    public nint WakeTarget;
    public byte WakeBackground;   // C bool
}

/// <summary>v3：am_client_new_ex 的扩展选项。StructSize = sizeof(AmClientOptions)。</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct AmClientOptions
{
    public uint StructSize;
    public nint Lifecycle;        // const AmLifecycle*
    public uint ConnectTimeoutMs;
    public nint OnIdleExit;       // void (*)(void*)
    // v7（4e 功耗，spec/lifecycle.md 第 11 节）
    public int Heartbeat;         // AmHeartbeatMode
    public int HostAbsentRetries; // 0 = 默认 3，负数 = 一直重连
    public byte LegacyTimers;     // C bool
    // v8（4e 第二部分，spec/lifecycle.md 第 13 节）
    public long MergeWindowMs;    // 0 = 默认 2000，负数 = 不留窗口
    public byte SleepOnBackground; // C bool
    // v13（调用去重，spec/protocol.md 3.3）
    public long CallDedupTtlMs;    // 0 = 默认 300000，负数 = 关闭
    public int CallDedupMaxEntries; // 0 = 默认 64，负数 = 关闭
}

/// <summary>v8：am_resource_register_ex 的资源选项。StructSize = sizeof(AmResourceOptions)。</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct AmResourceOptions
{
    public uint StructSize;
    public byte Realtime; // C bool
    public nint AnnotationsJson; // v13：const char*，可为 0
}

[StructLayout(LayoutKind.Sequential)]
internal struct AmToolSpec
{
    public nint Name;
    public nint Description;
    public nint InputSchemaJson;
    public int Risk;
    public int Activation;
    public nint Title;
    public byte Enabled; // C bool
}

/// <summary>v9：am_tool_register_ex / am_tool_update_ex 的工具选项。StructSize = sizeof(AmToolOptions)。</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct AmToolOptions
{
    public uint StructSize;
    public nint AnnotationsJson;  // MCP 工具注解 JSON；0 = 未声明
    public nint OutputSchemaJson; // MCP outputSchema；0 = 未声明
    public nint Page;             // v14：所在页面名；0 = 未声明
    public int Surface;           // v14：AmToolSurface（0 = APP，1 = VIEW）
}

/// <summary>v9：am_call_complete_ex 的调用结果。StructSize = sizeof(AmCallResult)。</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct AmCallResult
{
    public uint StructSize;
    public nint DataJson;         // 0 = null
    public nint StateHints;       // const char* const*
    public nuint StateHintsLen;
    public int Status;            // AmResultStatus
    public nint StateResource;
    public nint Summary;
    public nint AnnotationsJson;  // MCP 内容注解 JSON；0 = 无
}

[StructLayout(LayoutKind.Sequential)]
internal struct AmResourceSpec
{
    public nint Name;
    public nint Description;
    public nint MimeType;
}

internal static unsafe partial class NativeMethods
{
    internal const string Lib = "app_mcp";

    /// <summary>本封装对应的 C 接口版本（app_mcp.h 中的 AM_API_VERSION）。</summary>
    internal const int ApiVersion = 3;

    [LibraryImport(Lib)] internal static partial nint am_version();
    [LibraryImport(Lib)] internal static partial nint am_last_error_message();
    [LibraryImport(Lib)] internal static partial void am_string_free(nint s);

    [LibraryImport(Lib)] internal static partial AmStatus am_client_new(AmClientConfig* config, AmClientCallbacks* callbacks, out nint client);
    [LibraryImport(Lib)] internal static partial void am_client_free(nint client);
    [LibraryImport(Lib)] internal static partial AmStatus am_client_start(ClientSafeHandle client);
    [LibraryImport(Lib)] internal static partial AmStatus am_client_stop(ClientSafeHandle client);
    [LibraryImport(Lib)] internal static partial AmStatus am_client_set_visibility(ClientSafeHandle client, int visibility, [MarshalAs(UnmanagedType.U1)] bool focused);
    [LibraryImport(Lib)] internal static partial AmStatus am_client_state(ClientSafeHandle client, out int status, out ulong retryInMs, out nint reason);
    [LibraryImport(Lib)] internal static partial nint am_client_instance_id(ClientSafeHandle client);
    [LibraryImport(Lib)] internal static partial nint am_client_token(ClientSafeHandle client);
    // v6：诊断（spec/protocol.md 第 10 节）；输出字符串需 am_string_free。
    [LibraryImport(Lib)] internal static partial AmStatus am_client_state_code(ClientSafeHandle client, out nint code);
    [LibraryImport(Lib)] internal static partial AmStatus am_client_connection_id(ClientSafeHandle client, out nint id);
    [LibraryImport(Lib)] internal static partial AmStatus am_client_root_scope(ClientSafeHandle client, out nint scope);

    // v3：生命周期
    [LibraryImport(Lib)] internal static partial AmStatus am_client_new_ex(AmClientConfig* config, AmClientCallbacks* callbacks, AmClientOptions* options, out nint client);
    [LibraryImport(Lib)] [return: MarshalAs(UnmanagedType.U1)] internal static partial bool am_client_handle_wake(ClientSafeHandle client, byte* args);
    [LibraryImport(Lib)] internal static partial AmStatus am_client_wake(ClientSafeHandle client, byte* started);
    [LibraryImport(Lib)] internal static partial AmStatus am_client_wake_with_reason(ClientSafeHandle client, int reason, byte* started);
    [LibraryImport(Lib)] internal static partial AmStatus am_client_connect_now(ClientSafeHandle client, byte* started);
    [LibraryImport(Lib)] internal static partial AmStatus am_client_sleep(ClientSafeHandle client, byte* changed);
    [LibraryImport(Lib)] internal static partial AmStatus am_client_sleep_with_reason(ClientSafeHandle client, int reason, byte* changed);
    [LibraryImport(Lib)] internal static partial AmStatus am_client_hold(ClientSafeHandle client, out nint hold);
    [LibraryImport(Lib)] internal static partial void am_hold_release(nint hold);
    [LibraryImport(Lib)] internal static partial nint am_client_tools_hash(ClientSafeHandle client);
    [LibraryImport(Lib)] internal static partial nint am_parse_wake_token(byte* args);

    [LibraryImport(Lib)] internal static partial AmStatus am_scope_create(ScopeSafeHandle parent, byte* name, out nint scope);
    [LibraryImport(Lib)] internal static partial AmStatus am_scope_dispose(ScopeSafeHandle scope);
    [LibraryImport(Lib)] internal static partial void am_scope_free(nint scope);

    [LibraryImport(Lib)] internal static partial AmStatus am_tool_register(ScopeSafeHandle scope, AmToolSpec* spec, nint handler, nint userData, nint freeUserData, out nint tool);
    [LibraryImport(Lib)] internal static partial AmStatus am_tool_update(ToolSafeHandle tool, AmToolSpec* spec);
    // v9
    [LibraryImport(Lib)] internal static partial AmStatus am_tool_register_ex(ScopeSafeHandle scope, AmToolSpec* spec, AmToolOptions* options, nint handler, nint userData, nint freeUserData, out nint tool);
    [LibraryImport(Lib)] internal static partial AmStatus am_tool_update_ex(ToolSafeHandle tool, AmToolSpec* spec, AmToolOptions* options);
    [LibraryImport(Lib)] internal static partial AmStatus am_tool_set_enabled(ToolSafeHandle tool, [MarshalAs(UnmanagedType.U1)] bool enabled);
    [LibraryImport(Lib)] internal static partial AmStatus am_tool_dispose(ToolSafeHandle tool);
    [LibraryImport(Lib)] internal static partial void am_tool_free(nint tool);

    [LibraryImport(Lib)] internal static partial AmStatus am_resource_register_ex(ScopeSafeHandle scope, AmResourceSpec* spec, AmResourceOptions* options, nint reader, nint userData, nint freeUserData, out nint resource);
    [LibraryImport(Lib)] internal static partial AmStatus am_resource_notify_changed(ResourceSafeHandle resource);
    [LibraryImport(Lib)] internal static partial AmStatus am_resource_dispose(ResourceSafeHandle resource);
    [LibraryImport(Lib)] internal static partial void am_resource_free(nint resource);

    [LibraryImport(Lib)] internal static partial nint am_call_id(nint call);
    [LibraryImport(Lib)] internal static partial nint am_call_tool_name(nint call);
    [LibraryImport(Lib)] internal static partial nint am_call_arguments_json(nint call);
    [LibraryImport(Lib)] [return: MarshalAs(UnmanagedType.U1)] internal static partial bool am_call_is_cancelled(nint call);
    [LibraryImport(Lib)] internal static partial AmStatus am_call_set_cancel_callback(nint call, nint onCancel, nint userData, nint freeUserData);
    [LibraryImport(Lib)] internal static partial AmStatus am_call_complete(nint call, byte* dataJson, byte** stateHints, nuint stateHintsLen);
    [LibraryImport(Lib)] internal static partial AmStatus am_call_complete_ex(nint call, AmCallResult* result);
    [LibraryImport(Lib)] internal static partial AmStatus am_call_fail(nint call, byte* kind, byte* message);
    [LibraryImport(Lib)] internal static partial AmStatus am_call_fail_with_details(nint call, byte* kind, byte* message, byte* detailsJson);
    [LibraryImport(Lib)] internal static partial AmStatus am_call_fail_user_action(nint call, byte* message, byte* reason, byte* uri);
    [LibraryImport(Lib)] internal static partial AmStatus am_call_hold(nint call, out nint hold);
    [LibraryImport(Lib)] internal static partial AmStatus am_call_progress(nint call, double progress, double total, byte* message);

    [LibraryImport(Lib)] internal static partial AmStatus am_client_set_navigation_handler(ClientSafeHandle client, nint handler, nint userData, nint freeUserData);
    [LibraryImport(Lib)] internal static partial nint am_navigate_page(nint navigate);
    [LibraryImport(Lib)] internal static partial nint am_navigate_params_json(nint navigate);
    [LibraryImport(Lib)] internal static partial AmStatus am_navigate_complete(nint navigate);
    [LibraryImport(Lib)] internal static partial AmStatus am_navigate_fail(nint navigate, byte* message);
    [LibraryImport(Lib)] internal static partial AmStatus am_navigate_deny(nint navigate, byte* message);

    [LibraryImport(Lib)] internal static partial nint am_read_resource_name(nint read);
    [LibraryImport(Lib)] internal static partial AmStatus am_read_complete(nint read, byte* contentsJson);
    [LibraryImport(Lib)] internal static partial AmStatus am_read_fail(nint read, byte* kind, byte* message);
    [LibraryImport(Lib)] internal static partial AmStatus am_read_fail_with_details(nint read, byte* kind, byte* message, byte* detailsJson);
    [LibraryImport(Lib)] internal static partial AmStatus am_read_fail_user_action(nint read, byte* message, byte* reason, byte* uri);

    // -----------------------------------------------------------------------
    // 辅助
    // -----------------------------------------------------------------------

    internal static string? PtrToString(nint p) => p == 0 ? null : Marshal.PtrToStringUTF8(p);

    /// <summary>取走库返回的 char*（用 am_string_free 释放）。</summary>
    internal static string? TakeString(nint p)
    {
        if (p == 0) return null;
        try { return Marshal.PtrToStringUTF8(p); }
        finally { am_string_free(p); }
    }

    internal static string LastError() => PtrToString(am_last_error_message()) ?? string.Empty;

    /// <summary>非 AM_OK 时抛出 <see cref="AppMcpException"/>（必须紧跟在失败的调用之后、同一线程上调用）。</summary>
    internal static void Check(AmStatus status)
    {
        if (status != AmStatus.Ok) throw new AppMcpException((AppMcpStatus)status, LastError());
    }

    // -----------------------------------------------------------------------
    // 原生库加载
    // -----------------------------------------------------------------------

    internal const string NativePathEnv = "APP_MCP_NATIVE_PATH";

    // 静态构造函数在首次调用本类任何 P/Invoke 之前运行，保证解析器先于加载注册。
    static NativeMethods()
    {
        try
        {
            NativeLibrary.SetDllImportResolver(typeof(NativeMethods).Assembly, Resolve);
        }
        catch (InvalidOperationException)
        {
            // 宿主已设置过解析器，沿用默认行为。
        }
    }

    internal static string PlatformFileName =>
        OperatingSystem.IsWindows() ? "app_mcp.dll"
        : OperatingSystem.IsMacOS() || OperatingSystem.IsIOS() ? "libapp_mcp.dylib"
        : "libapp_mcp.so";

    /// <summary>按 APP_MCP_NATIVE_PATH（文件或目录）→ 程序目录 → runtimes/&lt;rid&gt;/native → 系统默认 的顺序查找。</summary>
    private static nint Resolve(string libraryName, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (libraryName != Lib) return 0;

        foreach (var candidate in Candidates())
        {
            if (File.Exists(candidate) && NativeLibrary.TryLoad(candidate, out var handle)) return handle;
        }
        return 0; // 交给运行时的默认查找
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
