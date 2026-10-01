import { describe, expect, it } from 'vitest'
import { hostTransport, isLoopbackHost } from '../src/host-transport'

describe('hostTransport', () => {
  it('回环地址为 loopback，其他为 remote', () => {
    expect(hostTransport(['ws://127.0.0.1:7717/app', 'ws://127.0.0.1:7737/app'])).toBe('loopback')
    expect(hostTransport(['ws://localhost:7717/app'])).toBe('loopback')
    expect(hostTransport(['ws://[::1]:7717/app'])).toBe('loopback')
    expect(hostTransport(['wss://example.com/app'])).toBe('remote')
    expect(hostTransport(['ws://127.0.0.1:7717/app', 'ws://10.0.0.2:7717/app'])).toBe('remote')
    expect(hostTransport(['not a url'])).toBe('remote')
    expect(hostTransport([])).toBe('remote')
  })

  it('isLoopbackHost', () => {
    expect(isLoopbackHost('127.255.0.1')).toBe(true)
    expect(isLoopbackHost('LOCALHOST')).toBe(true)
    expect(isLoopbackHost('127.0.0.1.example.com')).toBe(false)
    expect(isLoopbackHost('127.0.0.300')).toBe(false)
  })
})
