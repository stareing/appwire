using System.Text.Json;
using AppMcp.Internal;
using AppMcp.Native;

namespace AppMcp;

/// <summary>
/// 作用域：<see cref="Dispose"/> 注销其下全部工具、资源与子作用域（例如页面关闭时）。
/// </summary>
public sealed class ToolScope : IDisposable
{
    private readonly ScopeSafeHandle _handle;
    private readonly AppMcpClient _client;
    private readonly bool _isRoot;

    internal ToolScope(ScopeSafeHandle handle, AppMcpClient client, bool isRoot)
    {
        _handle = handle;
        _client = client;
        _isRoot = isRoot;
    }

    internal ScopeSafeHandle Handle => _handle;

    public unsafe ToolScope CreateScope(string name)
    {
        ArgumentNullException.ThrowIfNull(name);
        using var strings = new Utf8Strings();
        NativeMethods.Check(NativeMethods.am_scope_create(_handle, strings.AddPtr(name), out var child));
        return new ToolScope(new ScopeSafeHandle(child), _client, isRoot: false);
    }

    /// <summary>
    /// 注册工具（参数为原始 JSON）。handler 返回的对象用客户端的 <see cref="JsonSerializerOptions"/> 序列化；
    /// 返回 null 表示 JSON null。
    /// </summary>
    public ToolRegistration RegisterTool(
        string name,
        string description,
        Func<JsonElement, ToolContext, Task<object?>> handler,
        ToolOptions? options = null)
    {
        ArgumentNullException.ThrowIfNull(handler);
        var json = _client.SerializerOptions;
        RawToolHandler raw = async (args, ctx) =>
        {
            JsonElement input;
            try
            {
                using var doc = JsonDocument.Parse(args);
                input = doc.RootElement.Clone();
            }
            catch (JsonException e)
            {
                throw new ToolCallException(ToolErrorKind.InvalidInput, "参数不是合法的 JSON：" + e.Message, e);
            }
            var result = await handler(input, ctx).ConfigureAwait(false);
            return result is null ? null : JsonSerializer.Serialize(result, result.GetType(), json);
        };
        return Register(name, description, options?.InputSchemaJson, raw, options);
    }

    /// <summary>
    /// 注册类型化工具：参数用 System.Text.Json 反序列化为 <typeparamref name="TInput"/>，结果序列化为 JSON。
    /// <see cref="ToolOptions.InputSchemaJson"/> 为 null 时由 <typeparamref name="TInput"/> 生成 inputSchema。
    /// </summary>
    public ToolRegistration RegisterTool<TInput, TOutput>(
        string name,
        string description,
        Func<TInput, ToolContext, Task<TOutput>> handler,
        ToolOptions? options = null)
    {
        ArgumentNullException.ThrowIfNull(handler);
        var json = _client.SerializerOptions;
        var schema = options?.InputSchemaJson ?? ToolSchema.For<TInput>(json);
        RawToolHandler raw = async (args, ctx) =>
        {
            TInput input;
            try
            {
                input = JsonSerializer.Deserialize<TInput>(args, json)!;
            }
            catch (JsonException e)
            {
                throw new ToolCallException(ToolErrorKind.InvalidInput, "参数无法解析：" + e.Message, e);
            }
            var result = await handler(input, ctx).ConfigureAwait(false);
            return JsonSerializer.Serialize(result, json);
        };
        return Register(name, description, schema, raw, options);
    }

    private unsafe ToolRegistration Register(
        string name,
        string description,
        string? schema,
        RawToolHandler raw,
        ToolOptions? options)
    {
        ArgumentNullException.ThrowIfNull(name);
        ArgumentNullException.ThrowIfNull(description);
        options ??= new ToolOptions();
        var invoker = new ToolInvoker(raw, _client.Dispatcher, _client.SerializerOptions);
        using var strings = new Utf8Strings();
        var spec = BuildSpec(strings, name, description, schema, options);
        // user_data 的所有权交给库：注册失败时库也会调用 FreeGCHandle。
        var status = NativeMethods.am_tool_register(
            _handle, &spec, Callbacks.ToolPtr, Callbacks.Alloc(invoker), Callbacks.FreeGCHandlePtr, out var tool);
        NativeMethods.Check(status);
        return new ToolRegistration(new ToolSafeHandle(tool), name);
    }

    internal static AmToolSpec BuildSpec(Utf8Strings strings, string? name, string description, string? schema, ToolOptions options) => new()
    {
        Name = strings.Add(name),
        Description = strings.Add(description),
        InputSchemaJson = strings.Add(schema),
        Risk = (int)options.Risk,
        Activation = options.Activation is { } a ? (int)a : -1,
        Title = strings.Add(options.Title),
        Enabled = options.Enabled ? (byte)1 : (byte)0,
    };

    /// <summary>注册资源。reader 返回的对象序列化为资源内容。</summary>
    /// <remarks><c>realtime</c>：需实时推送（spec/lifecycle.md 第 13 节 B3），被订阅时保持连接、休眠中变化时回连推送。
    /// 默认 false：订阅不阻止休眠，变化在下次连接时补发。只用于"模型在等待变化"的资源。</remarks>
    public ResourceRegistration RegisterResource(
        string name,
        string description,
        Func<CancellationToken, Task<object?>> reader,
        string? mimeType = null,
        bool realtime = false)
    {
        ArgumentNullException.ThrowIfNull(reader);
        var json = _client.SerializerOptions;
        RawResourceReader raw = async ct =>
        {
            var result = await reader(ct).ConfigureAwait(false);
            return result is null ? "null" : JsonSerializer.Serialize(result, result.GetType(), json);
        };
        return RegisterResourceRaw(name, description, raw, mimeType, realtime);
    }

    /// <summary>注册类型化资源。</summary>
    /// <remarks><c>realtime</c> 见 <see cref="RegisterResource(string, string, Func{CancellationToken, Task{object?}}, string?, bool)"/>。</remarks>
    public ResourceRegistration RegisterResource<T>(
        string name,
        string description,
        Func<CancellationToken, Task<T>> reader,
        string? mimeType = null,
        bool realtime = false)
    {
        ArgumentNullException.ThrowIfNull(reader);
        var json = _client.SerializerOptions;
        RawResourceReader raw = async ct => JsonSerializer.Serialize(await reader(ct).ConfigureAwait(false), json);
        return RegisterResourceRaw(name, description, raw, mimeType, realtime);
    }

    private unsafe ResourceRegistration RegisterResourceRaw(string name, string description, RawResourceReader raw, string? mimeType, bool realtime)
    {
        ArgumentNullException.ThrowIfNull(name);
        ArgumentNullException.ThrowIfNull(description);
        var invoker = new ResourceInvoker(raw, _client.Dispatcher);
        using var strings = new Utf8Strings();
        var spec = new AmResourceSpec
        {
            Name = strings.Add(name),
            Description = strings.Add(description),
            MimeType = strings.Add(mimeType),
        };
        var resourceOptions = new AmResourceOptions
        {
            StructSize = (uint)sizeof(AmResourceOptions),
            Realtime = realtime ? (byte)1 : (byte)0,
        };
        var status = NativeMethods.am_resource_register_ex(
            _handle, &spec, &resourceOptions, Callbacks.ReadPtr, Callbacks.Alloc(invoker), Callbacks.FreeGCHandlePtr, out var resource);
        NativeMethods.Check(status);
        return new ResourceRegistration(new ResourceSafeHandle(resource), name);
    }

    /// <summary>注销该作用域下的全部工具、资源与子作用域（幂等）。</summary>
    public void Unregister() => NativeMethods.Check(NativeMethods.am_scope_dispose(_handle));

    /// <summary>注销并释放句柄。客户端已释放时静默忽略。根作用域只释放句柄。</summary>
    public void Dispose()
    {
        if (_handle.IsClosed) return;
        if (!_isRoot)
        {
            _ = NativeMethods.am_scope_dispose(_handle);
        }
        _handle.Dispose();
    }
}

/// <summary>已注册的工具。<see cref="Dispose"/> 注销工具；被 GC 回收（未 Dispose）时只释放句柄、不注销。</summary>
public sealed class ToolRegistration : IDisposable
{
    private readonly ToolSafeHandle _handle;

    internal ToolRegistration(ToolSafeHandle handle, string name)
    {
        _handle = handle;
        Name = name;
    }

    public string Name { get; }

    public void SetEnabled(bool enabled) => NativeMethods.Check(NativeMethods.am_tool_set_enabled(_handle, enabled));

    /// <summary>用新定义整体替换（名称不变）。</summary>
    public unsafe void Update(string description, ToolOptions? options = null)
    {
        options ??= new ToolOptions();
        using var strings = new Utf8Strings();
        var spec = ToolScope.BuildSpec(strings, null, description, options.InputSchemaJson, options);
        NativeMethods.Check(NativeMethods.am_tool_update(_handle, &spec));
    }

    public void Dispose()
    {
        if (_handle.IsClosed) return;
        _ = NativeMethods.am_tool_dispose(_handle);
        _handle.Dispose();
    }
}

/// <summary>已注册的资源。<see cref="Dispose"/> 注销资源。</summary>
public sealed class ResourceRegistration : IDisposable
{
    private readonly ResourceSafeHandle _handle;

    internal ResourceRegistration(ResourceSafeHandle handle, string name)
    {
        _handle = handle;
        Name = name;
    }

    public string Name { get; }

    /// <summary>通知 Host 资源内容已变化。</summary>
    public void NotifyChanged() => NativeMethods.Check(NativeMethods.am_resource_notify_changed(_handle));

    public void Dispose()
    {
        if (_handle.IsClosed) return;
        _ = NativeMethods.am_resource_dispose(_handle);
        _handle.Dispose();
    }
}
