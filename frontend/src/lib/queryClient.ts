import { QueryClient } from '@tanstack/react-query'

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // 默认缓存时间 5 分钟
      staleTime: 5 * 60 * 1000,
      // 失败时自动重试 1 次
      retry: 1,
      // 窗口重新聚焦时不自动刷新（因为我们有自己的刷新机制）
      refetchOnWindowFocus: false,
    },
  },
})
