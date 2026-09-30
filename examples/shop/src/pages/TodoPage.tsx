import { useResource, useTool } from '@app-mcp/react'
import { ToolCallError } from '@app-mcp/web'
import { useState, type FormEvent } from 'react'
import { z } from 'zod'
import { useStore } from '../store/create-store'
import { addTodo, findTodo, removeTodo, todoStore, todoSummary, toggleTodo } from '../store/todos'

const HINTS = ['todos.list']

function requireTodo(id: string) {
  const todo = findTodo(id)
  if (!todo) throw new ToolCallError('INVALID_INPUT', `待办不存在：${id}。请先读取 todos.list 获取 ID`)
  return todo
}

export function TodoPage() {
  const todos = useStore(todoStore)
  const [draft, setDraft] = useState('')

  useResource('todos.list', {
    description: '全部待办（id、标题、是否完成）与统计',
    read: () => ({ todos, summary: todoSummary(todos) }),
    deps: [todos],
  })

  // 工具 handler 与按钮调用同一组状态更新函数。
  useTool('todos.add', {
    title: '新增待办',
    description: '新增一条待办，返回新建的待办及统计',
    input: z.object({ title: z.string().trim().min(1).max(100).describe('待办标题') }),
    risk: 'write',
    handler: ({ title }) => ({ data: { todo: addTodo(title), summary: todoSummary() }, stateHints: HINTS }),
  })

  useTool('todos.toggle', {
    title: '勾选/取消勾选待办',
    description: '切换待办的完成状态，返回更新后的待办',
    input: z.object({ id: z.string().describe('待办 ID，来自 todos.list') }),
    risk: 'write',
    handler: ({ id }) => {
      requireTodo(id)
      return { data: { todo: toggleTodo(id), summary: todoSummary() }, stateHints: HINTS }
    },
  })

  useTool('todos.remove', {
    title: '删除待办',
    description: '永久删除一条待办',
    input: z.object({ id: z.string().describe('待办 ID，来自 todos.list') }),
    risk: 'destructive',
    handler: ({ id }) => {
      requireTodo(id)
      return { data: { removed: removeTodo(id), summary: todoSummary() }, stateHints: HINTS }
    },
  })

  const onSubmit = (e: FormEvent) => {
    e.preventDefault()
    if (!draft.trim()) return
    addTodo(draft)
    setDraft('')
  }

  const summary = todoSummary(todos)
  return (
    <section className="card">
      <form className="row" onSubmit={onSubmit}>
        <input value={draft} onChange={(e) => setDraft(e.target.value)} placeholder="要做什么？" aria-label="待办标题" />
        <button type="submit" className="primary">
          添加
        </button>
      </form>
      {todos.length === 0 ? (
        <p className="empty">暂无待办</p>
      ) : (
        <ul className="list">
          {todos.map((todo) => (
            <li key={todo.id} className={todo.done ? 'done' : ''}>
              <label>
                <input type="checkbox" checked={todo.done} onChange={() => toggleTodo(todo.id)} />
                <span>{todo.title}</span>
              </label>
              <button className="link danger" onClick={() => removeTodo(todo.id)} aria-label={`删除 ${todo.title}`}>
                删除
              </button>
            </li>
          ))}
        </ul>
      )}
      <p className="muted">
        共 {summary.total} 项，已完成 {summary.done} 项
      </p>
    </section>
  )
}
