import { humanizeCostPolicyError } from '../policies/imsRegistration.ts'

/** Task success is not proof of a connected call. Keep the backend's fixed
 * no-answer/busy/timeout explanation visible instead of hiding it in a tooltip. */
export function automationOutcomeLabel(status: string, detail: string): string {
  if (status !== 'success') return `上次失败: ${humanizeCostPolicyError(detail)}`
  if (!detail || detail === '执行成功') return '上次成功'
  return `上次成功: ${detail}`
}
