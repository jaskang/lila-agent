import { useMutation, useQuery } from '@tanstack/react-query'
import { type FormEvent, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { type Connection, useServerEvents } from '@/hooks/use-server-events'
import { api, unwrap } from '@/lib/api'
import { cn } from '@/lib/utils'

const CONNECTION: Record<Connection, { label: string; dot: string }> = {
  connecting: { label: '正在连接', dot: 'bg-muted-foreground' },
  online: { label: '已连接', dot: 'bg-emerald-500' },
  offline: { label: '连不上 lilad', dot: 'bg-destructive' },
  mismatch: { label: 'lilad 版本不符，请重启它', dot: 'bg-amber-500' },
}

export default function App() {
  const connection = useServerEvents()
  const health = useQuery({ queryKey: ['health'], queryFn: () => unwrap(api.health.$get()) })
  const projects = useQuery({ queryKey: ['projects'], queryFn: () => unwrap(api.projects.$get()) })

  const [path, setPath] = useState('')
  const add = useMutation({
    mutationFn: (path: string) => unwrap(api.projects.$post({ json: { path } })),
    onSuccess: () => setPath(''),
  })
  const remove = useMutation({
    mutationFn: (id: string) => unwrap(api.projects[':id'].$delete({ param: { id } })),
  })

  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (path.trim()) add.mutate(path.trim())
  }

  const status = CONNECTION[connection]

  return (
    <div className="flex h-dvh bg-background text-foreground">
      <aside className="flex w-64 shrink-0 flex-col border-r border-border">
        <h1 className="px-4 pt-4 pb-2 text-sm font-semibold">Lila</h1>
        <nav className="flex-1 overflow-y-auto px-2">
          <p className="px-2 py-1 text-xs text-muted-foreground">工作区</p>
          {projects.data?.length === 0 && (
            <p className="px-2 py-1 text-sm text-muted-foreground">还没有工作区</p>
          )}
          <ul>
            {projects.data?.map((project) => (
              <li
                key={project.id}
                className="group flex items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-accent"
              >
                <span className="min-w-0 flex-1 truncate" title={project.path}>
                  {project.name}
                </span>
                <button
                  type="button"
                  className="text-xs text-muted-foreground opacity-0 group-hover:opacity-100 hover:text-foreground focus-visible:opacity-100"
                  onClick={() => remove.mutate(project.id)}
                >
                  移除
                </button>
              </li>
            ))}
          </ul>
        </nav>
        <footer className="flex items-center gap-2 border-t border-border px-4 py-2 text-xs text-muted-foreground">
          <span className={cn('size-2 rounded-full', status.dot)} />
          <span className="min-w-0 flex-1 truncate">{status.label}</span>
          {health.data && <span className="tabular-nums">v{health.data.version}</span>}
        </footer>
      </aside>

      <main className="flex min-w-0 flex-1 items-center justify-center p-6">
        <form onSubmit={submit} className="w-full max-w-md space-y-3">
          <h2 className="text-lg font-medium">添加工作区</h2>
          <p className="text-sm text-muted-foreground">填一个文件夹的绝对路径。会话还没有做，这里只是骨架。</p>
          <div className="flex gap-2">
            <Input
              value={path}
              onChange={(event) => setPath(event.target.value)}
              placeholder="/Users/you/projects/shop"
              aria-label="文件夹路径"
            />
            <Button type="submit" disabled={add.isPending || !path.trim()}>
              添加
            </Button>
          </div>
          {add.error && <p className="text-sm text-destructive">{add.error.message}</p>}
        </form>
      </main>
    </div>
  )
}
