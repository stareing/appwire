/**
 * 一致性检查：
 * - 类型层面：@app-mcp/node 的实例可以当作 @app-mcp/web 的 AppMcp 使用，
 *   反之页面代码写给 web 的工具定义可以直接注册到 Node 实例上；
 * - 协议常量与 @app-mcp/web 的桥接实现一致。
 */
import { expect, expectTypeOf, it } from 'vitest'
import type {
  AppMcp as NodeAppMcp,
  LazyToolDefinition as NodeLazyToolDefinition,
  ToolDefinition as NodeToolDefinition,
} from '@app-mcp/node'
import {
  BRIDGE_VERSION as WEB_BRIDGE_VERSION,
  DEFAULT_BRIDGE_KEY as WEB_BRIDGE_KEY,
  type AppMcp as WebAppMcp,
  type LazyToolDefinition as WebLazyToolDefinition,
  type ToolDefinition as WebToolDefinition,
} from '@app-mcp/web'
import { BRIDGE_VERSION, DEFAULT_BRIDGE_KEY } from './protocol.js'

it('@app-mcp/node 与 @app-mcp/web 的 AppMcp 同形', () => {
  expectTypeOf<NodeAppMcp>().toExtend<WebAppMcp>()
  expectTypeOf<WebToolDefinition<{ a: number }, string>>().toExtend<NodeToolDefinition<{ a: number }, string>>()
  expectTypeOf<WebLazyToolDefinition<{ a: number }, string>>().toExtend<NodeLazyToolDefinition<{ a: number }, string>>()
})

it('桥接协议版本与默认名称与 @app-mcp/web 一致', () => {
  expect(BRIDGE_VERSION).toBe(WEB_BRIDGE_VERSION)
  expect(DEFAULT_BRIDGE_KEY).toBe(WEB_BRIDGE_KEY)
})
