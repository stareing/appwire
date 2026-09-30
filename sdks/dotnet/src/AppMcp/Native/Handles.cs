using System.Runtime.InteropServices;

namespace AppMcp.Native;

// 句柄的 SafeHandle 包装。释放句柄（*_free）不会注销工具或资源；注销用 *_dispose。

internal sealed class ClientSafeHandle : SafeHandle
{
    public ClientSafeHandle() : base(0, ownsHandle: true) { }
    public ClientSafeHandle(nint handle) : this() => SetHandle(handle);
    public override bool IsInvalid => handle == 0;

    /// <summary>停止并释放客户端（阻塞到后台线程结束）。</summary>
    protected override bool ReleaseHandle()
    {
        NativeMethods.am_client_free(handle);
        return true;
    }
}

internal sealed class ScopeSafeHandle : SafeHandle
{
    public ScopeSafeHandle() : base(0, ownsHandle: true) { }
    public ScopeSafeHandle(nint handle) : this() => SetHandle(handle);
    public override bool IsInvalid => handle == 0;

    protected override bool ReleaseHandle()
    {
        NativeMethods.am_scope_free(handle);
        return true;
    }
}

internal sealed class ToolSafeHandle : SafeHandle
{
    public ToolSafeHandle() : base(0, ownsHandle: true) { }
    public ToolSafeHandle(nint handle) : this() => SetHandle(handle);
    public override bool IsInvalid => handle == 0;

    protected override bool ReleaseHandle()
    {
        NativeMethods.am_tool_free(handle);
        return true;
    }
}

internal sealed class ResourceSafeHandle : SafeHandle
{
    public ResourceSafeHandle() : base(0, ownsHandle: true) { }
    public ResourceSafeHandle(nint handle) : this() => SetHandle(handle);
    public override bool IsInvalid => handle == 0;

    protected override bool ReleaseHandle()
    {
        NativeMethods.am_resource_free(handle);
        return true;
    }
}

/// <summary>v3：阻止自动休眠的持有。释放时调用 am_hold_release（释放持有并释放句柄）。</summary>
internal sealed class HoldSafeHandle : SafeHandle
{
    public HoldSafeHandle() : base(0, ownsHandle: true) { }
    public HoldSafeHandle(nint handle) : this() => SetHandle(handle);
    public override bool IsInvalid => handle == 0;

    protected override bool ReleaseHandle()
    {
        NativeMethods.am_hold_release(handle);
        return true;
    }
}

/// <summary>调用期间有效的 UTF-8 字符串（非托管内存），Dispose 时释放。</summary>
internal sealed class Utf8Strings : IDisposable
{
    private readonly List<nint> _allocations = new();

    public nint Add(string? s)
    {
        if (s is null) return 0;
        var p = Marshal.StringToCoTaskMemUTF8(s);
        _allocations.Add(p);
        return p;
    }

    public unsafe byte* AddPtr(string? s) => (byte*)Add(s);

    public void Dispose()
    {
        foreach (var p in _allocations) Marshal.FreeCoTaskMem(p);
        _allocations.Clear();
    }
}
