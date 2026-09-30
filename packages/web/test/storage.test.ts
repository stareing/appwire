import { beforeEach, describe, expect, it } from 'vitest'
import { instanceIdKey, loadInstanceId, loadToken, randomId, saveToken, tokenKey } from '../src/storage'

beforeEach(() => {
  sessionStorage.clear()
  localStorage.clear()
})

describe('storage', () => {
  it('instanceId 按 appId 保存在 sessionStorage', () => {
    const a = loadInstanceId('shop')
    expect(sessionStorage.getItem(instanceIdKey('shop'))).toBe(a)
    expect(loadInstanceId('shop')).toBe(a)
    expect(loadInstanceId('todo')).not.toBe(a)
  })

  it('token 保存在 localStorage', () => {
    expect(loadToken('shop')).toBeUndefined()
    expect(saveToken('shop', 't')).toBe(true)
    expect(localStorage.getItem(tokenKey('shop'))).toBe('t')
    expect(loadToken('shop')).toBe('t')
  })

  it('randomId 唯一', () => {
    expect(randomId()).not.toBe(randomId())
  })
})
