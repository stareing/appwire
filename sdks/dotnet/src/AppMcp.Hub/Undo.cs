// 撤销（第 15 项 X2，spec/hub-api.md 3.23）：配置上限、调用结果的撤销登记与状态。
using System.Text.Json.Nodes;

namespace AppMcp.Hub;

/// <summary>撤销记录上限（配置 JSON 的 undo）。为 null 的字段取默认值（30 分钟 / 32 条）；负数在生成配置时抛
/// <see cref="ArgumentOutOfRangeException"/>。只在内存、无定时器：取用时惰性丢弃过期记录，超过条数时丢弃最早的一条。</summary>
public sealed class HubUndoLimits
{
    /// <summary>记录自登记起的有效期（默认 30 分钟）。</summary>
    public TimeSpan? Ttl { get; set; }
    /// <summary>每个 Agent 任务保留的记录数上限（默认 32）；0 关闭撤销（不登记、不列出 apps.undo）。</summary>
    public int? MaxPerTask { get; set; }

    internal JsonObject ToJson()
    {
        var o = new JsonObject();
        HubOptions.AddMs(o, "ttlMs", Ttl);
        HubOptions.AddCount(o, "maxPerTask", MaxPerTask, nameof(MaxPerTask));
        return o;
    }
}

/// <summary>本次调用已登记撤销（CallOutcome.undo）：可用内置工具 apps.undo 撤销。Label 为 App 给出的说明（可为 null）；
/// ExpiresInMs 为距记录过期的毫秒数。</summary>
public sealed record UndoOfferInfo(string? Label, ulong ExpiresInMs);

/// <summary>撤销记录统计（HubStatus.undo）。MaxPerTask 为 0 表示撤销已关闭；Records 为各任务记录数合计（含尚未惰性丢弃的过期记录）。</summary>
public sealed record UndoStatusInfo(ulong TtlMs, long MaxPerTask, long Records);
