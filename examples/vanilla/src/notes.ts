/**
 * 便签页面逻辑：普通 DOM 事件，不依赖任何框架，也不直接调用 app-mcp。
 */

interface Note {
  id: string
  title: string
  color: string
  pinned: boolean
  createdAt: number
}

const STORAGE_KEY = 'vanilla-notes'
const COLORS = new Set(['yellow', 'green', 'blue', 'pink'])

function load(): Note[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    const data: unknown = raw ? JSON.parse(raw) : []
    return Array.isArray(data) ? (data as Note[]) : []
  } catch {
    return []
  }
}

function save(): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(notes))
  } catch {
    // 隐私模式等情况下忽略
  }
}

let notes: Note[] = load()
let seq = notes.reduce((max, n) => Math.max(max, Number(n.id.slice(1)) || 0), 0)

const form = document.getElementById('new-note') as HTMLFormElement
const list = document.getElementById('notes') as HTMLUListElement
const clearAll = document.getElementById('clear-all') as HTMLButtonElement
const count = document.getElementById('count') as HTMLSpanElement
const empty = document.getElementById('empty') as HTMLParagraphElement

function sorted(): Note[] {
  return [...notes].sort((a, b) => Number(b.pinned) - Number(a.pinned) || a.createdAt - b.createdAt)
}

function render(): void {
  const items = sorted()
  list.replaceChildren(
    ...items.map((note) => {
      const li = document.createElement('li')
      li.className = `note ${note.color}`
      const title = document.createElement('span')
      title.className = 'title'
      title.textContent = note.pinned ? `📌 ${note.title}` : note.title
      const remove = document.createElement('button')
      remove.type = 'button'
      remove.textContent = '删除'
      remove.dataset.mcpTool = 'notes.remove'
      remove.dataset.mcpDesc = '删除一条便签'
      remove.dataset.mcpKey = note.id
      remove.dataset.mcpLabel = note.title
      remove.dataset.mcpHints = 'notes.list'
      remove.addEventListener('click', () => {
        notes = notes.filter((n) => n.id !== note.id)
        update()
      })
      li.append(title, remove)
      return li
    }),
  )
  list.dataset.mcpJson = JSON.stringify(items.map(({ id, title, color, pinned }) => ({ id, title, color, pinned })))
  clearAll.disabled = notes.length === 0
  count.textContent = `共 ${notes.length} 条`
  empty.hidden = notes.length > 0
}

function update(): void {
  save()
  render()
}

form.addEventListener('submit', (event) => {
  event.preventDefault()
  const data = new FormData(form)
  const title = String(data.get('title') ?? '').trim()
  if (!title) {
    form.dispatchEvent(new CustomEvent('mcp:error', { detail: { kind: 'INVALID_INPUT', message: '标题不能为空' } }))
    return
  }
  const color = String(data.get('color') ?? 'yellow')
  const note: Note = {
    id: `n${++seq}`,
    title,
    color: COLORS.has(color) ? color : 'yellow',
    pinned: data.get('pinned') === 'on',
    createdAt: Date.now(),
  }
  notes.push(note)
  update()
  form.reset()
  // 结构化结果：attachDom 等待 data-mcp-result 指定的事件，以 detail 作为工具返回值
  form.dispatchEvent(
    new CustomEvent('note-created', { detail: { id: note.id, title: note.title, color: note.color, pinned: note.pinned } }),
  )
})

clearAll.addEventListener('click', () => {
  notes = []
  update()
})

render()
