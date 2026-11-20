import { useEffect, useRef, useState } from 'react'
import { Alert, Button, MenuItem, Stack, TextField, Typography } from '@mui/material'
import { api } from '../../api/current'
import { directoryBody, directorySnapshot, DIRECTORY_READ_SCOPE, type DirectoryInput, type DirectorySnapshot } from '../../policies/nativeDirectory'
export default function AppNativeDirectoryPane({ lineId, revision, hosted, installed }: { lineId: string; revision: number; hosted: boolean; installed: boolean }) {
  const context = JSON.stringify([lineId, revision, hosted, installed])
  const [input, setInput] = useState({ context, value: { operation: 'relations', groupId: '', since: '-1' } as DirectoryInput })
  const currentInput: DirectoryInput = input.context === context ? input.value : { operation: 'relations', groupId: '', since: '-1' }
  const [result, setResult] = useState<{ context: string; snapshot: DirectorySnapshot } | null>(null)
  const [error, setError] = useState<{ context: string } | null>(null)
  const [busy, setBusy] = useState(false)
  const active = useRef({ context, generation: 0, serial: 0, live: false })
  const reading = useRef(false)
  useEffect(() => {
    const generation = active.current.generation + 1
    active.current = { context, generation, serial: 0, live: true }
    return () => { if (active.current.generation === generation) active.current.live = false }
  }, [context])
  const read = async () => {
    if (reading.current || !active.current.live || active.current.context !== context || !installed || !hosted) return
    const original = { ...active.current, serial: active.current.serial + 1 }; const requested = { ...currentInput }
    const body = directoryBody(revision, requested)
    if (!body || !window.confirm('通过当前原 SDK 查询 Group/Buddy 目录？这是一次只读查询，不创建关系、群组或业务，不重试未知 WRITE，也不会重新登录。')) return
    active.current.serial = original.serial; reading.current = true; setBusy(true); setError(null); setResult(null)
    try {
      const response = await api.readAppNativeDirectory(lineId, body)
      const snapshot = directorySnapshot(response.data, requested)
      if (!snapshot) throw new Error('Invalid original directory projection')
      if (!active.current.live || active.current.context !== original.context || active.current.generation !== original.generation || active.current.serial !== original.serial) return
      setResult({ context: original.context, snapshot })
    } catch {
      if (active.current.live && active.current.context === original.context && active.current.generation === original.generation && active.current.serial === original.serial) setError({ context: original.context })
    } finally { reading.current = false; if (active.current.live) setBusy(false) }
  }
  if (!installed) return null
  const snapshot = result?.context === context ? result.snapshot : null
  const change = (value: DirectoryInput) => { setInput({ context, value }); setResult(null); setError(null) }
  return <Stack spacing={1} sx={{ mt: 1, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">原 Group/Buddy 目录（只读）</Typography>
    <Typography variant="caption" color="text.secondary">{DIRECTORY_READ_SCOPE}</Typography>
    {!hosted && <Typography variant="caption">当前原 SDK 未托管；不会自动启动、重新登录或建立其他连接。</Typography>}
    <TextField select size="small" label="目录读取操作" value={currentInput.operation} disabled={busy || !hosted} onChange={(event) => change({ ...currentInput, operation: event.target.value === 'properties' ? 'properties' : 'relations', since: '-1' })}>
      <MenuItem value="relations">关系目录观测</MenuItem><MenuItem value="properties">原群组属性观测</MenuItem>
    </TextField>
    <TextField size="small" label="原群组 ID（关系查询可留空）" value={currentInput.groupId} autoComplete="off" disabled={busy || !hosted} slotProps={{ htmlInput: { maxLength: 508 } }} onChange={(event) => change({ ...currentInput, groupId: event.target.value })} helperText="留空仅使用当前 SDK 的原 UID；不能输入电话号码、URI、Box 或网络端点。" />
    {currentInput.operation === 'relations' && <TextField size="small" label="原目录 since（signed64 字符串）" value={currentInput.since} autoComplete="off" disabled={busy || !hosted} slotProps={{ htmlInput: { maxLength: 20 } }} onChange={(event) => change({ ...currentInput, since: event.target.value })} />}
    <Button size="small" disabled={busy || !hosted || !directoryBody(revision, currentInput)} onClick={() => { void read() }}>明确读取原目录（只读）</Button>
    {error?.context === context && <Alert severity="warning">原目录读取结果未核实；不会自动重试、重新登录、确认消息位置或重放未知 WRITE。</Alert>}
    {snapshot && <Stack spacing={0.5}>
      <Typography variant="caption">本次原服务端结果：{snapshot.accepted ? '读取返回成功' : '读取返回拒绝'}；不是 SDK、消息 ACK 或联系人在线证明。</Typography>
      <Typography variant="caption">目标：{snapshot.targetKind === 'current_user' ? '当前原 SDK 用户' : '本次原群组'} · 操作：{snapshot.operation === 'relations' ? '关系目录' : '群组属性'}</Typography>
      {snapshot.operation === 'relations' ? <>
        <Typography variant="caption">原关系集合 A {snapshot.first} 项 · 集合 B {snapshot.second} 项 · 名称集合 {snapshot.names} 项</Typography>
        <Typography variant="caption">原 reported 字段：{snapshot.reported ? 'true' : 'false'} · 原始时间值：{snapshot.time}（不推断墙上时钟或在线状态）</Typography>
      </> : <Typography variant="caption">原属性集合 {snapshot.properties} 项；私有属性值、关系标识和当前 SDK 凭据不从本视图导出。</Typography>}
    </Stack>}
  </Stack>
}
