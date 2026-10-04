// 标准意图（第 16 项 N4，spec/intents.md 第 4 节）的机主默认表状态。
namespace AppMcp.Hub;

/// <summary>意图默认表状态（IntentsStatus）：生效的默认表（意图 → 工具全名）。</summary>
public sealed record IntentsStatusInfo(IReadOnlyDictionary<string, string> Defaults)
{
    /// <summary>最近一次 <see cref="AppMcpHub.SetIntentDefaults"/> 失败的原因（之前的默认表继续生效）；之后成功时清除。</summary>
    public string? LastError { get; init; }
}
