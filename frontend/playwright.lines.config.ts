import { defineConfig, devices } from '@playwright/test'

// Dedicated, mocked-only suite. No device credentials or live backend needed.
export default defineConfig({
  testDir: './e2e',
  testMatch: 'blocked-lines.spec.ts',
  timeout: 30_000,
  expect: { timeout: 10_000 },
  workers: 1,
  use: { baseURL: 'http://127.0.0.1:5187', ...devices['Desktop Chrome'] },
  webServer: {
    command: 'node node_modules/vite/bin/vite.js --host 127.0.0.1 --port 5187 --strictPort',
    url: 'http://127.0.0.1:5187',
    reuseExistingServer: false,
    timeout: 120_000,
    // Even an accidentally unmocked API request cannot hit a device proxy.
    env: { VITE_API_PROXY_TARGET: 'http://127.0.0.1:1' },
  },
})
