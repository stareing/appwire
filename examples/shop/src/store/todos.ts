import { createStore } from './create-store'

export interface Todo {
  id: string
  title: string
  done: boolean
}

let nextId = 3

export const todoStore = createStore<Todo[]>([
  { id: 't1', title: '买牛奶', done: false },
  { id: 't2', title: '写周报', done: true },
])

export function findTodo(id: string): Todo | undefined {
  return todoStore.get().find((t) => t.id === id)
}

/** 新增待办，返回新建的条目。 */
export function addTodo(title: string): Todo {
  const todo: Todo = { id: `t${nextId++}`, title: title.trim(), done: false }
  todoStore.set([...todoStore.get(), todo])
  return todo
}

/** 切换完成状态，返回更新后的条目；不存在时返回 undefined。 */
export function toggleTodo(id: string): Todo | undefined {
  let updated: Todo | undefined
  todoStore.set(
    todoStore.get().map((t) => {
      if (t.id !== id) return t
      updated = { ...t, done: !t.done }
      return updated
    }),
  )
  return updated
}

/** 删除待办，返回被删除的条目；不存在时返回 undefined。 */
export function removeTodo(id: string): Todo | undefined {
  const todo = findTodo(id)
  if (todo) todoStore.set(todoStore.get().filter((t) => t.id !== id))
  return todo
}

export function todoSummary(todos: readonly Todo[] = todoStore.get()) {
  const done = todos.filter((t) => t.done).length
  return { total: todos.length, done, pending: todos.length - done }
}
