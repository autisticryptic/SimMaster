/** Copy without placing sensitive text into a hidden input, title or tooltip.
 * The copy-event fallback also supports device UIs served over plain HTTP. */
export async function copyPrivateText(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    try { await navigator.clipboard.writeText(text); return } catch { /* Try user-initiated copy on HTTP/permission-restricted origins. */ }
  }
  let copied = false
  const onCopy = (event: ClipboardEvent) => {
    if (!event.clipboardData) return
    event.preventDefault()
    event.clipboardData.setData('text/plain', text)
    copied = true
  }
  document.addEventListener('copy', onCopy)
  try {
    document.execCommand('copy')
    if (!copied) throw new Error('复制失败，请允许浏览器访问剪贴板后重试')
  } finally {
    document.removeEventListener('copy', onCopy)
  }
}
