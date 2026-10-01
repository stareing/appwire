import { readdirSync, readFileSync, readlinkSync } from 'node:fs'

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

/**
 * 本机**其他进程**到 `127.0.0.1:<port>` 的已建立 TCP 连接数（客户端一侧，读 `/proc/net/tcp`）。
 * 排除本测试进程自己的套接字（MCP 客户端与 App 连接共用 Host 的同一端口）。
 * 非 Linux 返回 undefined（调用方跳过该断言）。
 */
export function establishedTo(port: number): number | undefined {
  if (process.platform !== 'linux') return undefined
  let text: string
  const own = new Set<string>()
  try {
    text = readFileSync('/proc/net/tcp', 'utf8')
    for (const fd of readdirSync('/proc/self/fd')) {
      try {
        const m = /^socket:\[(\d+)\]$/.exec(readlinkSync(`/proc/self/fd/${fd}`))
        if (m?.[1]) own.add(m[1])
      } catch {
        // 遍历期间关闭的描述符
      }
    }
  } catch {
    return undefined
  }
  const remote = `0100007F:${port.toString(16).toUpperCase().padStart(4, '0')}`
  return text
    .split('\n')
    .slice(1)
    .map((l) => l.trim().split(/\s+/))
    .filter((f) => f[2] === remote && f[3] === '01' && !own.has(f[9] ?? '')).length
}
