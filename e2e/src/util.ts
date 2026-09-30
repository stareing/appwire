import { createServer } from 'node:net'

export const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

/** 轮询直到 `check` 返回真值（可为 async），返回该值；超时抛出带说明的错误。 */
export async function waitFor<T>(
  check: () => T | undefined | null | false | Promise<T | undefined | null | false>,
  what: string,
  timeoutMs = 20_000,
  intervalMs = 100,
): Promise<T> {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown
  for (;;) {
    try {
      const value = await check()
      if (value) return value
    } catch (error) {
      lastError = error
    }
    if (Date.now() > deadline) {
      const detail = lastError instanceof Error ? `（最后一次错误：${lastError.message}）` : ''
      throw new Error(`等待超时：${what}${detail}`)
    }
    await sleep(intervalMs)
  }
}

/** 取一个空闲的本地 TCP 端口。 */
export function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer()
    server.unref()
    server.on('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const address = server.address()
      server.close(() => (typeof address === 'object' && address ? resolve(address.port) : reject(new Error('无法取得端口'))))
    })
  })
}

/** 按行切分流数据。 */
export function lineSplitter(onLine: (line: string) => void): (chunk: Buffer | string) => void {
  let buffer = ''
  return (chunk) => {
    buffer += chunk.toString()
    let i: number
    while ((i = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, i).replace(/\r$/, '')
      buffer = buffer.slice(i + 1)
      onLine(line)
    }
  }
}
