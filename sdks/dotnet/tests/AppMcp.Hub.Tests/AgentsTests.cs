using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace AppMcp.Hub.Tests;

/// <summary>Agent 登记（spec/hub-api.md 3.6「Agent 身份」）：HubOptions.Agents、AppMcpHub.SetAgents。</summary>
public class AgentsTests
{
    private const string Claude = "claude-0123456789abcdef0123456789abcdef";
    private const string Cursor = "cursor-0123456789abcdef0123456789abcdef";

    private static HubAgentCredential Cred(string name, string token) => new() { Name = name, Token = token };

    [Fact]
    public void SerializesAndRedacts()
    {
        var json = JsonNode.Parse(new HubOptions { Agents = [Cred("claude", Claude)] }.ToConfigJson())!.AsObject();
        Assert.Equal("claude", (string?)json["agents"]![0]!["name"]);
        Assert.Equal(Claude, (string?)json["agents"]![0]!["token"]);
        Assert.False(JsonNode.Parse(new HubOptions().ToConfigJson())!.AsObject().ContainsKey("agents"));
        Assert.DoesNotContain(Claude, Cred("claude", Claude).ToString());
    }

    [Fact]
    public void InvalidAgentsFailStart()
    {
        var e = Assert.Throws<HubException>(() => AppMcpHub.Start(new HubOptions
        {
            DisableIpc = true, DisableListen = true, Dispatcher = null, Agents = [Cred("a b", Claude)],
        }));
        Assert.Equal(HubStatus.InvalidConfig, e.Status);
        Assert.Contains("agents", e.Message);
        Assert.DoesNotContain(Claude, e.Message);
    }

    /// <summary>经 /mcp 出示 Agent 令牌的请求归该 Agent；SetAgents 替换，不合法时保留之前的登记。</summary>
    [Fact]
    public async Task TokenIdentifiesAgentAndSetAgentsReplaces()
    {
        await using var hub = AppMcpHub.Start(new HubOptions
        {
            DisableIpc = true, Listen = "127.0.0.1:0", McpHttp = true, Dispatcher = null, Agents = [Cred("claude", Claude)],
        });
        Assert.Equal(["claude"], hub.Status().Agents);
        var task = await BeginTask(hub, Claude);
        Assert.Equal("claude", hub.Status().Tasks!.Single(t => t.Id == task).Agent);

        var e = Assert.Throws<HubException>(() => hub.SetAgents([Cred("a", Claude), Cred("a", Cursor)]));
        Assert.Equal(HubStatus.InvalidConfig, e.Status);
        Assert.Equal(["claude"], hub.Status().Agents);

        hub.SetAgents([Cred("cursor", Cursor)]);
        Assert.Equal(["cursor"], hub.Status().Agents);
        task = await BeginTask(hub, Cursor);
        Assert.Equal("cursor", hub.Status().Tasks!.Single(t => t.Id == task).Agent);
        hub.SetAgents([]);
        Assert.Empty(hub.Status().Agents!);
    }

    /// <summary>无会话（2026-07-28）apps.task.begin → 任务 ID。</summary>
    private static async Task<string> BeginTask(AppMcpHub hub, string token)
    {
        using var http = new HttpClient();
        var body = """
            {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"apps.task.begin","arguments":{},"_meta":{
             "io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},
             "io.modelcontextprotocol/clientInfo":{"name":"t","version":"1"}}}}
            """;
        using var req = new HttpRequestMessage(HttpMethod.Post, $"http://{hub.ListenAddress}/mcp")
        {
            Content = new StringContent(body, Encoding.UTF8, "application/json"),
        };
        req.Headers.Authorization = new AuthenticationHeaderValue("Bearer", token);
        req.Headers.Accept.ParseAdd("application/json");
        req.Headers.Accept.ParseAdd("text/event-stream");
        req.Headers.Add("Mcp-Protocol-Version", "2026-07-28");
        req.Headers.Add("Mcp-Method", "tools/call");
        req.Headers.Add("Mcp-Name", "apps.task.begin");
        using var resp = await http.SendAsync(req);
        var text = await resp.Content.ReadAsStringAsync();
        var line = text.Split('\n').Select(l => l.StartsWith("data:") ? l[5..].Trim() : l.Trim()).First(l => l.StartsWith('{'));
        using var doc = JsonDocument.Parse(line);
        return doc.RootElement.GetProperty("result").GetProperty("structuredContent").GetProperty("taskId").GetString()!;
    }
}
