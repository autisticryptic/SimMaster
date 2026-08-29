import { defineConfig, devices } from '@playwright/test'

/**
 * Browser tests for the VoLTE Profile dialog.
 *
 * These run against a *running* SimAdmin, not a mock. The dialog's value is in
 * what it does to a real line — the candidate slots, the derived-source rule,
 * the runtime resolution panel — and none of that exists without a backend that
 * has modem lines.
 *
 * The frontend under test is the local working copy, served by Vite, whose dev
 * server already proxies `/api` to a device (`VITE_API_PROXY_TARGET`, default
 * `http://192.168.100.13:3000`). So a UI change is testable without
 * redeploying the frontend to the device.
 *
 *   E2E_PASSWORD=... pnpm test:e2e
 *
 * Set `E2E_BASE_URL` to skip the local dev server and drive an already-served
 * frontend instead — but note a deployed build will not carry test ids added
 * since it was built.
 *
 * Without `E2E_PASSWORD` every test skips, so `pnpm test:e2e` stays green on a
 * machine with no device and CI is unaffected.
 */
const explicitBaseURL = process.env.E2E_BASE_URL
const devServerURL = 'http://127.0.0.1:5173'
const baseURL = explicitBaseURL ?? devServerURL

export default defineConfig({
  testDir: './e2e',
  // A device over the LAN is slower than localhost, and the dialog waits on
  // real API calls.
  timeout: 60_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  // One worker: these tests write per-line configuration, so two of them
  // touching the same line would race.
  workers: 1,
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL,
    actionTimeout: 20_000,
    navigationTimeout: 30_000,
    // The device serves plain HTTP on the LAN.
    ignoreHTTPSErrors: true,
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  // Start Vite unless the caller pointed at an already-served frontend. Reusing
  // an existing server keeps a local `pnpm dev` usable while iterating.
  webServer: explicitBaseURL
    ? undefined
    : {
        command: 'pnpm dev',
        url: devServerURL,
        reuseExistingServer: true,
        timeout: 120_000,
      },
})
