import { describe, expect, it, vi } from 'vitest'
import { z } from 'zod'
import { EMPTY_INPUT_SCHEMA, describeParseError, isZodLike, toJsonSchema } from '../src/schema'

describe('toJsonSchema', () => {
  it('缺省为无参数', () => {
    expect(toJsonSchema(undefined)).toEqual(EMPTY_INPUT_SCHEMA)
  })

  it('JSON Schema 原样返回', () => {
    const s = { type: 'object' as const, properties: { a: { type: 'string' } } }
    expect(toJsonSchema(s)).toBe(s)
  })

  it('调用 toJSONSchema() 方法', () => {
    expect(toJsonSchema({ toJSONSchema: () => ({ type: 'object', title: 'x' }) })).toEqual({ type: 'object', title: 'x' })
  })

  it('zod：同步转换（输入视角，default 字段非必填）', () => {
    const out = toJsonSchema(z.object({ a: z.string(), b: z.number().default(1) }))
    expect(out).not.toBeInstanceOf(Promise)
    expect(out).toMatchObject({ type: 'object', required: ['a'] })
  })

  it('zod：结果与 z.toJSONSchema(schema, { io: "input" }) 一致（与构建插件的清单一致）', () => {
    const schema = z.object({
      id: z.string().describe('商品 ID'),
      qty: z.number().int().min(1).default(1),
      note: z.string().optional(),
      tags: z.array(z.enum(['a', 'b'])).default([]),
    })
    expect(toJsonSchema(schema)).toEqual(z.toJSONSchema(schema, { io: 'input' }))
  })

  it('zod：无 Standard JSON Schema 时动态加载 z.toJSONSchema', async () => {
    const schema = z.object({ a: z.string() })
    const legacy = Object.create(schema) as object
    Object.defineProperty(legacy, '~standard', { value: undefined })
    Object.defineProperty(legacy, 'toJSONSchema', { value: undefined })
    const out = toJsonSchema(legacy as never)
    expect(out).toBeInstanceOf(Promise)
    expect(await out).toMatchObject({ type: 'object', properties: { a: { type: 'string' } }, required: ['a'] })
  })

  it('zod：无 Standard JSON Schema 但有 toJSONSchema 方法时同步转换（按输入形态）', () => {
    const toJSONSchema = vi.fn(() => ({ type: 'object', properties: { q: { type: 'string' } } }))
    const zodLike = { _zod: {}, parse: (v: unknown) => v, toJSONSchema }
    expect(toJsonSchema(zodLike as never)).toEqual({ type: 'object', properties: { q: { type: 'string' } } })
    expect(toJSONSchema).toHaveBeenCalledWith({ io: 'input' })
  })

  it('非对象抛错', () => {
    expect(() => toJsonSchema('x' as never)).toThrow()
    expect(() => toJsonSchema([] as never)).toThrow()
  })
})

describe('zod 辅助', () => {
  it('isZodLike', () => {
    expect(isZodLike(z.string())).toBe(true)
    expect(isZodLike({ type: 'object' })).toBe(false)
    expect(isZodLike(null)).toBe(false)
  })

  it('describeParseError', () => {
    const r = z.object({ a: z.object({ b: z.string() }) }).safeParse({ a: { b: 1 } })
    const d = describeParseError(r.error)
    expect(d.message).toMatch(/^参数校验失败：a\.b: /)
    expect(d.details).toEqual({ issues: [{ path: 'a.b', message: expect.any(String) }] })
    expect(describeParseError(new Error('x')).message).toBe('参数校验失败：x')
  })
})
