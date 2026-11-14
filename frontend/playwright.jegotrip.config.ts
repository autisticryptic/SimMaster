import { defineConfig, devices } from '@playwright/test'

// Actions-only component UI checks. APIs are mocked and even an unmatched
// request is proxied to an unused loopback port, never a modem/backend/provider.
export default defineConfig({
  testDir: './e2e',
  testMatch: ['jegotrip-device-identity.spec.ts', 'jegotrip-local-receiver.spec.ts', 'jegotrip-startup-diagnostics.spec.ts', 'jegotrip-alive-status.spec.ts', 'jegotrip-communication.spec.ts'],
  timeout: 45_000,
  expect: { timeout: 10_000 },
  workers: 1,
  retries: 0,
  outputDir: 'test-results/jegotrip-device-ui',
  reporter: [['list'], ['json', { outputFile: 'test-results/jegotrip-ui-results.json' }]],
  use: { baseURL: 'http://127.0.0.1:5189', ...devices['Desktop Chrome'], screenshot: 'only-on-failure' },
  webServer: {
    command: 'node node_modules/vite/bin/vite.js --host 127.0.0.1 --port 5189 --strictPort',
    url: 'http://127.0.0.1:5189/e2e/fixtures/jegotrip-device.html',
    reuseExistingServer: false,
    timeout: 120_000,
    env: { VITE_API_PROXY_TARGET: 'http://127.0.0.1:1' },
  },
})
