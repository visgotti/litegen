import { test, expect } from '@playwright/test';

// Verifies the live dashboard gates unauthenticated visitors to a clean /login
// page (no app shell / sidebar). Run:
//   LIVE_URL=https://app.litegen.ai npx playwright test --config playwright.live.config.ts authgate
test('unauthenticated visitor is redirected to a clean /login (no sidebar/app shell)', async ({ page }) => {
  await page.goto('/');
  // Client-side RequireAuth should redirect to /login
  await page.waitForURL(/\/login/, { timeout: 20_000 });

  // The clean auth page is shown…
  await expect(page.locator('[data-testid="auth-page"]')).toBeVisible({ timeout: 10_000 });
  // …and a Google sign-in option is present
  await expect(page.locator('[data-testid="oauth-google"]')).toBeVisible({ timeout: 10_000 });

  // The app shell / sidebar must NOT be present for an unauthenticated user
  await expect(page.locator('nav.sidebar')).toHaveCount(0);
  // The legacy master-API-key widget must NOT be present
  await expect(page.getByPlaceholder(/master API key/i)).toHaveCount(0);

  // Deep link to a protected route also bounces to /login (preserving next)
  await page.goto('/keys');
  await page.waitForURL(/\/login/, { timeout: 20_000 });
});
