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

/// 回到前台（`resumed`）时是否主动回连：`idle` / `onDemand` 模式下为 true（原因 `visible`）。
/// 客户端未休眠时原生层的 `wake` 无效果，因此可以无条件调用。
bool wakeOnResume(mcp.LifecyclePolicy policy) => policy.mode != mcp.LifecycleMode.persistent;
