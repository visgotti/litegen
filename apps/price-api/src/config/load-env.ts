import * as fs from 'fs';
import * as path from 'path';

/**
 * Minimal `.env` loader for the standalone CLI entrypoints (TypeORM migrations,
 * seed, coverage check) which run outside Nest's `ConfigModule`. Lines already
 * present in `process.env` are never overwritten. Deliberately dependency-free.
 */
export function loadEnv(file = '.env'): void {
  const fullPath = path.resolve(process.cwd(), file);
  if (!fs.existsSync(fullPath)) {
    return;
  }
  const content = fs.readFileSync(fullPath, 'utf8');
  for (const rawLine of content.split('\n')) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#')) {
      continue;
    }
    const eq = line.indexOf('=');
    if (eq === -1) {
      continue;
    }
    const key = line.slice(0, eq).trim();
    let value = line.slice(eq + 1).trim();
    // Strip surrounding quotes.
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }
    if (process.env[key] === undefined) {
      process.env[key] = value;
    }
  }
}
