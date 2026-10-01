// tauri-plugin-app-mcp 注入到每个 WebView 主框架的初始化脚本（Rust 侧 include_str!）。
//
// 在页面上暴露 @app-mcp/web 的宿主 IPC 桥接对象 `window.appMcpBridge`（与 Electron preload 暴露的对象同形，
// 协议见 packages/web/src/electron-bridge.ts）。页面里直接用 `@app-mcp/web` 的 `createAppMcp` 即可：
// 检测到桥接后改走 Rust 侧的原生客户端，不加载 WASM、不连接 Host。
//
// - 页面 → Rust：`__TAURI_INTERNALS__.invoke('plugin:app-mcp|op', { op })`，返回 OpReply。
// - Rust → 页面：Rust 侧对本 WebView 执行 `window.__APP_MCP_TAURI_DISPATCH__(<事件 JSON>)`。
//
// 本脚本只透传消息，不解析字段：协议在版本 1 内新增的可选字段（如 hello 回复 / state 事件的 connectionId）无需改动这里。
// 版本号须与 @app-mcp/web 的 BRIDGE_VERSION 一致（packages/tauri 的测试会检查）。
;(function () {
  'use strict'
  var w = window
  if (w.appMcpBridge) return
  var COMMAND = 'plugin:app-mcp|op'
  var listeners = []

  function dispatch(event) {
    var current = listeners.slice()
    for (var i = 0; i < current.length; i++) {
      try {
        current[i](event)
      } catch (error) {
        console.error('[app-mcp] 桥接事件监听器抛出异常', error)
      }
    }
  }

  function request(op) {
    var internals = w.__TAURI_INTERNALS__
    if (!internals || typeof internals.invoke !== 'function') {
      return Promise.resolve({ ok: false, code: 'NO_TAURI', message: '当前页面没有 Tauri IPC（__TAURI_INTERNALS__）' })
    }
    return Promise.resolve(internals.invoke(COMMAND, { op: op })).then(
      function (reply) {
        return reply
      },
      function (error) {
        return {
          ok: false,
          code: 'IPC_ERROR',
          message:
            '调用 ' + COMMAND + ' 失败（capabilities 是否包含 "app-mcp:default"？）：' +
            (error && error.message ? error.message : String(error)),
        }
      },
    )
  }

  var bridge = Object.freeze({
    version: 1,
    request: request,
    onMessage: function (listener) {
      listeners.push(listener)
      return function () {
        var index = listeners.indexOf(listener)
        if (index >= 0) listeners.splice(index, 1)
      }
    },
  })

  Object.defineProperty(w, '__APP_MCP_TAURI_DISPATCH__', { value: dispatch })
  Object.defineProperty(w, 'appMcpBridge', { value: bridge, enumerable: true })
})()
