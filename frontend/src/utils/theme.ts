// Theme utilities with proper typing
import { alpha as muiAlpha } from '@mui/material'

/**
 * Type-safe alpha function wrapper
 */
export function alpha(color: string, opacity: number): string {
  return muiAlpha(color, opacity)
}

