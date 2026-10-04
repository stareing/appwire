// 只读结果缓存（第 16 项 O3，spec/hub-api.md 3.20）：配置上限与状态统计。
using System.Text.Json.Nodes;

namespace AppMcp.Hub;

/// <summary>只读结果缓存上限（配置 JSON 的 resultCache）。为 null 的字段取默认值（1024 条 / 8 MiB / 64 KiB）；负数在生成配置时抛
/// <see cref="ArgumentOutOfRangeException"/>。超出条数 / 总字节数时淘汰最久未用的条目；只在内存，Hub 重启清空。</summary>
public sealed class HubResultCacheLimits
{
    /// <summary>条目数上限（默认 1024）；0 关闭缓存（不查、不存）。</summary>
    public int? MaxEntries { get; set; }
    /// <summary>全部条目的字节数上限（键 + 序列化后的结果，默认 8 MiB）。</summary>
    public int? MaxBytes { get; set; }
    /// <summary>单个条目的字节数上限（默认 64 KiB）；超出的结果不存。</summary>
    public int? MaxEntryBytes { get; set; }

    internal JsonObject ToJson()
    {
        var o = new JsonObject();
        HubOptions.AddCount(o, "maxEntries", MaxEntries, nameof(MaxEntries));
        HubOptions.AddCount(o, "maxBytes", MaxBytes, nameof(MaxBytes));
        HubOptions.AddCount(o, "maxEntryBytes", MaxEntryBytes, nameof(MaxEntryBytes));
        return o;
    }
}

/// <summary>只读结果缓存的统计（HubStatus.cache），计数自 Hub 启动起累计。Entries：当前条目数（含尚未被访问判定的过期条目）；
/// Bytes：当前条目的字节数合计；Misses 只计声明了 cache 的请求（绕过不计）；Evictions：因条数 / 字节上限淘汰的条目数；
/// Limits：生效上限（旧版 Hub 不报告时为 null）。</summary>
public sealed record CacheStatusInfo(long Entries, ulong Bytes, ulong Hits, ulong Misses, ulong Evictions, CacheLimitsInfo? Limits = null);

/// <summary>结果缓存的生效上限（HubStatus.cache.limits）。MaxEntries 为 0 表示缓存已关闭。</summary>
public sealed record CacheLimitsInfo(long MaxEntries, long MaxBytes, long MaxEntryBytes);
