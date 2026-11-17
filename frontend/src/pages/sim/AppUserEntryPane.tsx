import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Stack, TextField, Typography } from '@mui/material'
import { api } from '../../api/current'
import { userEntryView, userEntryRecipient, userEntryUid, userEntryLabel, USER_ENTRY_SCOPE, type UserEntryView } from '../../policies/userEntry'
export default function AppUserEntryPane({ lineId, revision, hosted, installed, localOnly = false }: { lineId: string; revision: number; hosted: boolean; installed: boolean; localOnly?: boolean }) {
  const context = JSON.stringify([lineId, revision, hosted, installed, localOnly])
  const [input, setInput] = useState({ context, value: '' })
  const [observed, setObserved] = useState<{ context: string; value: UserEntryView } | null>(null)
  const [failed, setFailed] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const current = useRef({ context, generation: 0, live: false })
  const reading = useRef(false)
  useEffect(() => {
    const generation = current.current.generation + 1; current.current = { context, generation, live: true }
    return () => { if (current.current.generation === generation) current.current.live = false }
  }, [context])
  const value = input.context === context ? input.value : ''
  const lookup = async () => {
    if (reading.current || !installed || !hosted || !Number.isSafeInteger(revision) || revision <= 0 || !userEntryRecipient(value) || localOnly && !userEntryUid(value) || !current.current.live || current.current.context !== context) return
    if (!window.confirm('只读向原 UserEntry 查询本次收件人？不标记已读、不发送消息、不清空未知写入、不重新登录。')) return
    const original = { ...current.current }; reading.current = true; setBusy(true)
    try {
      const result = await api.lookupAppUserEntry(lineId, revision, value)
      const parsed = userEntryView(result.data)
      if (!parsed) throw new Error('Original query observation unavailable')
      if (!current.current.live || current.current.generation !== original.generation || current.current.context !== original.context) return
      setObserved({ context, value: parsed }); setFailed(null)
    } catch { if (current.current.live && current.current.generation === original.generation && current.current.context === original.context) setFailed(context) }
    finally { reading.current = false; if (current.current.live) setBusy(false) }
  }
  if (!installed) return null
  return <Stack spacing={1} sx={{ mt: 1, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">原收件人只读解析</Typography>
    <Typography variant="caption" color="text.secondary">{USER_ENTRY_SCOPE}</Typography>
    {localOnly && <Typography variant="caption">当前原引用仅提供 UID 本地分支；别名解析需其原拥有者路由，不能猜测默认服务。</Typography>}
    <TextField size="small" label="收件人原 URI" placeholder="[phone:+号码@原账号域]" value={value} autoComplete="off" disabled={busy || !hosted} slotProps={{ htmlInput: { maxLength: 512 } }} onChange={(event) => setInput({ context, value: event.target.value })} />
    <Button size="small" disabled={busy || !hosted || !Number.isSafeInteger(revision) || revision <= 0 || !userEntryRecipient(value) || localOnly && !userEntryUid(value)} onClick={() => { void lookup() }}>只读解析收件人</Button>
    {failed === context && <Alert severity="warning">本次原解析未完成；不自动重查、重发或重新登录。</Alert>}
    {observed?.context === context && <Typography variant="body2">{userEntryLabel(observed.value)}</Typography>}
  </Stack>
}
