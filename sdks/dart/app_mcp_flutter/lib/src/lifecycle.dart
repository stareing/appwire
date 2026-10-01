import 'package:app_mcp/app_mcp.dart' as mcp;
import 'package:flutter/widgets.dart';

/// 把 Flutter 生命周期映射为 app-mcp 可见性。
///
/// - `resumed`：可见且有焦点。
/// - `inactive`：可见但无焦点（如被系统对话框遮挡、桌面窗口失焦）。
/// - `hidden`：不可见（最小化、切到后台过程中）。
/// - `paused`：移动端（[isMobile]）视为冻结——系统随时可能挂起进程，handler 无法可靠执行；
///   桌面端视为不可见。
/// - `detached`：冻结。
({mcp.AppVisibility visibility, bool focused}) visibilityForLifecycle(
  AppLifecycleState state, {
  required bool isMobile,
}) =>
    switch (state) {
      AppLifecycleState.resumed => (visibility: mcp.AppVisibility.visible, focused: true),
      AppLifecycleState.inactive => (visibility: mcp.AppVisibility.visible, focused: false),
      AppLifecycleState.hidden => (visibility: mcp.AppVisibility.hidden, focused: false),
      AppLifecycleState.paused => (
          visibility: isMobile ? mcp.AppVisibility.frozen : mcp.AppVisibility.hidden,
          focused: false
        ),
      AppLifecycleState.detached => (visibility: mcp.AppVisibility.frozen, focused: false),
    };

/// 回到前台时是否主动回连：`idle` / `onDemand` 模式下为 true（原因 `visible`；时机见 [becameVisible]）。
/// 客户端未休眠时原生层的 `wake` 无效果，因此可以无条件调用。
bool wakeOnResume(mcp.LifecyclePolicy policy) => policy.mode != mcp.LifecycleMode.persistent;

/// 可见性是否从隐藏 / 冻结（或尚未上报，[previous] 为 null）变为可见——回到前台的回连时机（spec/lifecycle.md 第 3 节）。
///
/// @why 只看可见性变化，不看焦点：`inactive ↔ resumed`（下拉通知栏、系统对话框、桌面窗口失焦）不应让已休眠的客户端回连；
/// 首次上报即可见时回连，使 `onDemand`（移动端默认）在启动后第一次进入前台时连接。
bool becameVisible(mcp.AppVisibility? previous, mcp.AppVisibility current) =>
    current == mcp.AppVisibility.visible && previous != mcp.AppVisibility.visible;
