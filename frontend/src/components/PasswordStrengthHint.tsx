import { Chip, LinearProgress, Stack, Typography } from '@mui/material'
import type { SecurityConfig } from '../api/types'
import {
  PASSWORD_MAX_LENGTH,
  analyzePassword,
  type PasswordStrength,
} from '../lib/passwordPolicy'

function strengthLabel(strength: PasswordStrength) {
  if (strength === 'strong') return '强'
  if (strength === 'medium') return '中'
  return '弱'
}

function strengthColor(strength: PasswordStrength): 'error' | 'warning' | 'success' {
  if (strength === 'strong') return 'success'
  if (strength === 'medium') return 'warning'
  return 'error'
}

export function PasswordStrengthHint({ password, settings }: { password: string; settings: SecurityConfig }) {
  if (!password) return null

  const analysis = analyzePassword(password, settings)
  const progress = analysis.strength === 'strong' ? 100 : analysis.strength === 'medium' ? 62 : 28

  const rules = [
    { ok: analysis.lengthOk, label: `${settings.password_min_length}-${PASSWORD_MAX_LENGTH} 个字符` },
    ...(settings.password_require_letters ? [{ ok: analysis.requiredLettersOk, label: '包含英文字母' }] : []),
    ...(settings.password_require_digits ? [{ ok: analysis.requiredDigitsOk, label: '包含数字' }] : []),
    ...(settings.password_require_symbols ? [{ ok: analysis.requiredSymbolsOk, label: '包含符号' }] : []),
  ]

  return (
    <Stack
      spacing={1}
      sx={{
        ml: { xs: 1.5, sm: 2 },
        mr: 1,
        width: { xs: 'calc(100% - 20px)', sm: 'calc(100% - 24px)' },
      }}
    >
      <Stack direction="row" spacing={1} alignItems="center">
        <Typography variant="caption" color="text.secondary">密码强度</Typography>
        <Chip
          size="small"
          color={strengthColor(analysis.strength)}
          label={strengthLabel(analysis.strength)}
          sx={{ height: 20, borderRadius: 1, fontSize: 12 }}
        />
      </Stack>
      <LinearProgress
        variant="determinate"
        value={progress}
        color={strengthColor(analysis.strength)}
        sx={{ height: 6, borderRadius: 999, bgcolor: 'action.hover' }}
      />
      <Stack spacing={0.4}>
        {rules.map((rule) => (
          <Typography
            key={rule.label}
            variant="caption"
            color={rule.ok ? 'success.main' : 'text.secondary'}
            sx={{ lineHeight: 1.45 }}
          >
            {rule.ok ? '✓' : '•'} {rule.label}
          </Typography>
        ))}
      </Stack>
    </Stack>
  )
}
