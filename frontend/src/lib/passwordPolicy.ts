import type { SecurityConfig } from '../api/types'

export type PasswordStrength = 'weak' | 'medium' | 'strong'

export const PASSWORD_MAX_LENGTH = 64
export const DEFAULT_SECURITY_SETTINGS: SecurityConfig = {
  password_protection_enabled: true,
  password_min_length: 8,
  password_require_letters: true,
  password_require_digits: true,
  password_require_symbols: true,
  session_ttl_seconds: 7 * 24 * 60 * 60,
  idle_timeout_seconds: 60 * 60,
}

function isAsciiGraphic(char: string) {
  return /^[\x21-\x7E]$/.test(char)
}

function isLetter(char: string) {
  return /^[A-Za-z]$/.test(char)
}

function isDigit(char: string) {
  return /^\d$/.test(char)
}

function isSymbol(char: string) {
  return /^[!"#$%&'()*+,\-./:;<=>?@[\\\]^_`{|}~]$/.test(char)
}

function joinChineseList(items: string[]) {
  if (items.length <= 1) return items[0] ?? ''
  if (items.length === 2) return `${items[0]}和${items[1]}`
  return `${items.slice(0, -1).join('、')}和${items[items.length - 1]}`
}

export function enabledPasswordTypeLabels(settings: SecurityConfig) {
  return [
    ...(settings.password_require_letters ? ['英文字母'] : []),
    ...(settings.password_require_digits ? ['数字'] : []),
    ...(settings.password_require_symbols ? ['符号'] : []),
  ]
}

export function enabledPasswordTypesText(settings: SecurityConfig) {
  const labels = enabledPasswordTypeLabels(settings)
  return joinChineseList(labels.length ? labels : ['英文字母', '数字', '符号'])
}

export function passwordPolicyHelperText(settings: SecurityConfig) {
  return `长度 ${settings.password_min_length}-${PASSWORD_MAX_LENGTH}，仅限${enabledPasswordTypesText(settings)}`
}

export function isAllowedPasswordChar(char: string, settings?: SecurityConfig) {
  if (!isAsciiGraphic(char)) return false
  if (!settings) return true
  if (settings.password_require_letters && isLetter(char)) return true
  if (settings.password_require_digits && isDigit(char)) return true
  if (settings.password_require_symbols && isSymbol(char)) return true
  return false
}

export function normalizePasswordInput(value: string, settings?: SecurityConfig) {
  return Array.from(value)
    .filter((char) => isAllowedPasswordChar(char, settings))
    .join('')
    .slice(0, PASSWORD_MAX_LENGTH)
}

export function getPasswordCategories(password: string) {
  return {
    lower: /[a-z]/.test(password),
    upper: /[A-Z]/.test(password),
    digit: /\d/.test(password),
    symbol: /[!"#$%&'()*+,\-./:;<=>?@[\\\]^_`{|}~]/.test(password),
  }
}

export function analyzePassword(password: string, settings: SecurityConfig) {
  const categories = getPasswordCategories(password)
  const categoryCount = Object.values(categories).filter(Boolean).length
  const hasLetters = categories.lower || categories.upper
  const lengthOk = password.length >= settings.password_min_length && password.length <= PASSWORD_MAX_LENGTH
  const charsOk = password.length > 0 && password === normalizePasswordInput(password, settings)
  const requiredLettersOk = !settings.password_require_letters || hasLetters
  const requiredDigitsOk = !settings.password_require_digits || categories.digit
  const requiredSymbolsOk = !settings.password_require_symbols || categories.symbol
  const requiredTypesOk = requiredLettersOk && requiredDigitsOk && requiredSymbolsOk
  let score = 0
  if (lengthOk) score += 1
  if (requiredTypesOk) score += 1
  if (password.length >= 12) score += 1
  if (categoryCount >= 3) score += 1
  const strength: PasswordStrength = score >= 4 ? 'strong' : score >= 2 ? 'medium' : 'weak'

  return {
    categoryCount,
    lengthOk,
    charsOk,
    requiredLettersOk,
    requiredDigitsOk,
    requiredSymbolsOk,
    requiredTypesOk,
    valid: lengthOk && charsOk && requiredTypesOk,
    strength,
  }
}

export function validatePasswordAgainstSecurity(password: string, settings: SecurityConfig) {
  const analysis = analyzePassword(password, settings)
  if (!analysis.charsOk) {
    return `密码只能包含${enabledPasswordTypesText(settings)}，不能包含空格、中文或未启用的字符类型`
  }
  if (!analysis.lengthOk) {
    return `密码长度需为 ${settings.password_min_length}-${PASSWORD_MAX_LENGTH} 个字符`
  }
  if (!analysis.requiredLettersOk) {
    return '密码需包含英文字母'
  }
  if (!analysis.requiredDigitsOk) {
    return '密码需包含数字'
  }
  if (!analysis.requiredSymbolsOk) {
    return '密码需包含符号'
  }
  return null
}
