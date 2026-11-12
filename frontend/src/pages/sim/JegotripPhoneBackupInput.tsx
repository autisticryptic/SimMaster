import { useLayoutEffect, useRef, useState } from 'react'
import { Alert, Button, Stack, Typography } from '@mui/material'
import { PHONE_BACKUP_MAX_BYTES, phoneBackupError, phoneUuidFromBackupText } from '../../policies/phoneBackupIdentity'

interface PhoneBackupInputProps {
  open: boolean; disabled: boolean; contextKey: string; label: string; onUuid: (value: string) => void
}

// 文件仅在浏览器内读取。只把 UUID 交给现有表单，保存/CAS/登录仍须另行确认。
function BackupFileReader({ open, disabled, label, onUuid }: Omit<PhoneBackupInputProps, 'contextKey'>) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const generation = useRef({ active: false })
  const fileGeneration = useRef(0)
  useLayoutEffect(() => {
    const scope = { active: open && !disabled }
    generation.current = scope
    fileGeneration.current += 1
    return () => { scope.active = false }
  }, [open, disabled])

  return <Stack spacing={1}>
    <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1}>
      <Button component="label" size="small" variant="outlined" disabled={disabled || !open || busy}>{label}
        <input hidden type="file" aria-label={label} accept=".xml,.dat,.json" onChange={(event) => {
          const file = event.target.files?.[0]
          event.target.value = ''
          const scope = generation.current
          const selected = ++fileGeneration.current
          if (!file || !scope.active) return
          setError(null)
          if (file.name.toLowerCase().endsWith('.zip')) {
            setError(phoneBackupError(new Error('jegotrip_backup_archive_requires_local_reader'))); return
          }
          if (file.size > PHONE_BACKUP_MAX_BYTES) {
            setError(phoneBackupError(new Error('jegotrip_backup_identity_file_too_large'))); return
          }
          setBusy(true)
          void file.text().then((value) => {
            if (!scope.active || selected !== fileGeneration.current) return
            try { onUuid(phoneUuidFromBackupText(value, file.name)) }
            catch (failure) { setError(phoneBackupError(failure)) }
          }).catch(() => {
            if (scope.active && selected === fileGeneration.current) setError(phoneBackupError(new Error('jegotrip_backup_read_failed')))
          }).finally(() => {
            if (scope.active && selected === fileGeneration.current) setBusy(false)
          })
        }} />
      </Button>
      {busy && <Button size="small" disabled={disabled} onClick={() => {
        fileGeneration.current += 1; setBusy(false); setError(null)
      }}>取消本地读取</Button>}
    </Stack>
    <Typography variant="caption" color="text.secondary">读取原始 setting_power.xml、可读的 device.dat 或离线脚本生成的 UUID-only JSON（≤256 KiB）。ZIP 先在本地解析；不会上传整个备份、复制密码/token、导入“已授信”缓存或自动保存/登录。</Typography>
    {error && <Alert severity="warning">{error}</Alert>}
  </Stack>
}

// 目标、手工值、开关或占用变化时重新挂载：旧读取 scope 在清理中失效，
// 新组件从空状态开始，不在 effect 中同步 setState，也不放宽读取守卫。
export default function JegotripPhoneBackupInput({ contextKey, ...props }: PhoneBackupInputProps) {
  return <BackupFileReader key={JSON.stringify([props.open, props.disabled, contextKey])} {...props} />
}
