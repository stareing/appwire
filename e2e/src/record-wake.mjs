// 测试用唤醒程序（Host 配置 `lifecycle.waker = {"exec": [node, 本文件, 日志文件]}`）：
// Host 把 WakeRequest 以一行 JSON 写到 stdin；本程序原样追加到日志文件，由测试决定如何交给页面。
import { appendFileSync } from 'node:fs'

const log = process.argv[2]
if (!log) {
  process.stderr.write('用法：record-wake.mjs <日志文件>\n')
  process.exit(2)
}
const chunks = []
for await (const chunk of process.stdin) chunks.push(chunk)
const line = Buffer.concat(chunks).toString('utf8').trim()
JSON.parse(line) // 格式错误时以非 0 退出，Host 返回 LAUNCH_FAILED
appendFileSync(log, `${line}\n`)
