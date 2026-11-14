export function callNotificationLabel(value: unknown): string {
  const labels: Record<string, string> = { pending: '等待发送（仅原来电仍有效时）', sending: '发送中', delivered: '渠道已确认送达', failed: '明确发送失败',
    cancelled: '原来电或目的地已变化，未继续发送', uncertain: '结果不确定，不自动重发', expired: '发送窗口已到期' }
  const label = typeof value === 'string' && Object.prototype.hasOwnProperty.call(labels, value) ? labels[value] : '通知状态未知'
  return `${label}；不代表来电已接听或媒体就绪`
}
