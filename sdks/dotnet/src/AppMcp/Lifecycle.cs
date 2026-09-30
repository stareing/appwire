namespace AppMcp;

/// <summary>生命周期模式（spec/lifecycle.md 第 3 节）。</summary>
public enum LifecycleMode
{
    /// <summary>不休眠（默认，兼容现有行为）。</summary>
    Persistent = 0,
    /// <summary>启动即连接；空闲后休眠；唤醒后回连。</summary>
    Idle = 1,
    /// <summary>启动时不连接；被唤醒或 <see cref="AppMcpClient.ConnectNow"/> 时连接，任务完成后经过 <see cref="LifecycleOptions.Grace"/> 休眠。</summary>
    OnDemand = 2,
}

/// <summary>休眠后的进程驻留策略。</summary>
public enum Residency
{
    /// <summary>只断开连接，进程照常运行。</summary>
    Keep = 0,
    /// <summary>仅当本进程由唤醒冷启动时，休眠后触发 <see cref="AppMcpClient.IdleExit"/>。</summary>
    ExitWhenIdle = 1,
    /// <summary>每次休眠后都触发 <see cref="AppMcpClient.IdleExit"/>（无界面辅助进程）。</summary>
    ExitAlways = 2,
}

/// <summary>唤醒方式。</summary>
public enum WakeKind
{
    None = 0,
    Uri = 1,
    Aumid = 2,
    AppleEvent = 3,
    Dbus = 4,
    AndroidIntent = 5,
    WebUrl = 6,
}

/// <summary>回连原因。</summary>
public enum WakeReason { OsActivation = 0, App = 1, Visible = 2, ColdStart = 3 }

/// <summary>休眠原因。</summary>
public enum SleepReason { Idle = 0, Grace = 1, Background = 2, App = 3 }

/// <summary>本实例的唤醒描述（spec/lifecycle.md 第 5 节），随 <c>app/sleep</c> 上报。</summary>
/// <param name="Kind">唤醒方式。</param>
/// <param name="Target">定位信息：URI scheme、AUMID 等。</param>
/// <param name="Background">能否不把窗口带到前台就唤醒（Windows 激活会前置窗口，一般为 false；托盘 / 无窗口进程为 true）。</param>
public sealed record WakeDescriptor(WakeKind Kind, string? Target = null, bool Background = false);

/// <summary>生命周期策略。</summary>
public sealed class LifecycleOptions
{
    public LifecycleMode Mode { get; init; } = LifecycleMode.Persistent;
    /// <summary>空闲多久进入休眠（<see cref="LifecycleMode.Idle"/>）。默认 60 秒。</summary>
    public TimeSpan IdleTimeout { get; init; } = TimeSpan.FromSeconds(60);
    /// <summary>可见性为 hidden / frozen 时的空闲时间。默认 15 秒。</summary>
    public TimeSpan HiddenIdleTimeout { get; init; } = TimeSpan.FromSeconds(15);
    /// <summary><see cref="LifecycleMode.OnDemand"/> 下任务完成后保留连接的时间。默认 10 秒。</summary>
    public TimeSpan Grace { get; init; } = TimeSpan.FromSeconds(10);
    public Residency Residency { get; init; } = Residency.Keep;
    /// <summary>唤醒描述；为 null 时不上报（Host 回退到清单 launch）。Windows 上可用 <see cref="Activation.WakeDescriptorFactory"/> 生成。</summary>
    public WakeDescriptor? Wake { get; init; }
}
