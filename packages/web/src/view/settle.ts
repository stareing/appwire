/**
 * 等待界面稳定：导航回调完成后，框架提交渲染、执行 effect（新页面的工具注册）通常在接下来的一两帧内完成。
 *
 * @output 两帧后的下一个任务，或 `maxMs` 到期（页面隐藏时 `requestAnimationFrame` 不触发），取先到者。
 */
export function settleView(win: Window | undefined, maxMs: number): Promise<void> {
  return new Promise((resolve) => {
    let done = false
    const finish = (): void => {
      if (done) return
      done = true
      clearTimeout(timer)
      resolve()
    }
    const timer = setTimeout(finish, Math.max(0, maxMs))
    const raf = win?.requestAnimationFrame?.bind(win)
    if (!raf) {
      setTimeout(finish, 0)
      return
    }
    raf(() => raf(() => setTimeout(finish, 0)))
  })
}
