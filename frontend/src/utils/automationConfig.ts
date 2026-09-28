import type { ApiResponse, AutomationConfig } from '../api/contracts'

/** A task edit is not permission to enable the whole scheduler. Rejections must
 * reach the dialog so it retains the draft instead of announcing a false save. */
export async function persistAutomationConfig(
  config: AutomationConfig,
  write: (config: AutomationConfig) => Promise<ApiResponse<unknown>>,
): Promise<AutomationConfig> {
  const response = await write(config)
  if (response.status !== 'ok') {
    throw new Error(response.message || '自动化配置保存失败')
  }
  return config
}
