// 工具演进（第 16 项 O4，spec/hub-api.md 3.21）：弃用声明与 schema 变化记录。
namespace AppMcp.Hub;

/// <summary>App 工具的弃用声明（原样，spec/protocol.md 3.7）：Message 面向模型说明原因与做法；Replacement 为同一 App 中的替代工具
/// 局部名；Until 为计划移除日期（YYYY-MM-DD，只作提示）。弃用工具照常列出与调用。</summary>
public sealed record ToolDeprecationInfo(string Message, string? Replacement = null, string? Until = null);

/// <summary>一条变化：Level 为 "breaking"（旧调用会出错）或 "warning"（可能破坏）；Path 为相对工具定义的 JSON Pointer 风格位置。</summary>
public sealed record SchemaChangeInfo(string Level, string Path, string Message);

/// <summary>一个工具的一次不兼容变化（HubStatus.schemaChanges）：Level 为 Changes 中最高的级别，At 为 Hub 收到新声明的时刻（Unix 毫秒）。</summary>
public sealed record SchemaChangeRecordInfo(string AppId, string Tool, string Level, IReadOnlyList<SchemaChangeInfo> Changes, ulong At);
