import fs from 'fs';
import type { Reporter, TestCase, TestResult, TestStep } from '@playwright/test/reporter';

// Writes every test's steps with their wall-clock times, beside the path of
// its video, to the file LITEGEN_E2E_TIMELINE names: what an annotated cut of
// the run's videos is built from (which feature, which assertion, when).

type Step = { title: string; category: string; start: number; duration: number; depth: number; error?: string };

export default class TimelineReporter implements Reporter {
  private tests: Array<Record<string, unknown>> = [];

  onTestEnd(test: TestCase, result: TestResult): void {
    const steps: Step[] = [];
    const walk = (list: TestStep[], depth: number) => {
      for (const s of list) {
        steps.push({ title: s.title, category: s.category, start: s.startTime.getTime(), duration: s.duration, depth, error: s.error?.message });
        walk(s.steps, depth + 1);
      }
    };
    walk(result.steps, 0);
    this.tests.push({
      file: test.location.file.split('/e2e/').pop(),
      line: test.location.line,
      title: test.titlePath().slice(2).join(' › ') || test.title,
      status: result.status,
      start: result.startTime.getTime(),
      duration: result.duration,
      video: result.attachments.find(a => a.name === 'video')?.path ?? null,
      steps,
    });
  }

  onEnd(): void {
    const out = process.env.LITEGEN_E2E_TIMELINE;
    if (out) fs.writeFileSync(out, JSON.stringify(this.tests, null, 2));
  }
}
