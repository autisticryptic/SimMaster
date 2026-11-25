import { defineConfig, devices } from '@playwright/test'

// Actions-only component UI checks. APIs are mocked and even an unmatched
// request is proxied to an unused loopback port, never a modem/backend/provider.
export default defineConfig({
  testDir: './e2e',
  testMatch: ['jegotrip-device-identity.spec.ts', 'jegotrip-local-receiver.spec.ts', 'jegotrip-startup-diagnostics.spec.ts', 'jegotrip-alive-status.spec.ts', 'jegotrip-communication.spec.ts', 'jegotrip-call-notifications.spec.ts', 'jegotrip-invite-notifications.spec.ts', 'jegotrip-im-messages.spec.ts', 'jegotrip-im-info.spec.ts', 'jegotrip-im-receipts.spec.ts', 'jegotrip-im-user-read.spec.ts', 'jegotrip-im-user-read-state.spec.ts', 'jegotrip-im-reconciliation.spec.ts', 'jegotrip-user-entry.spec.ts', 'jegotrip-uid-local.spec.ts', 'jegotrip-im-recipient-read.spec.ts', 'jegotrip-im-read-status.spec.ts', 'jegotrip-im-read-status-recipient.spec.ts', 'jegotrip-im-send.spec.ts', 'jegotrip-native-directory.spec.ts'].concat(['jegotrip-tls-startup-detail.spec.ts']).concat(['jegotrip-connect-startup-detail.spec.ts']).concat(['jegotrip-relation-history.spec.ts']).concat(['jegotrip-relation-write.spec.ts']).concat(['jegotrip-relation-events.spec.ts']),
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
