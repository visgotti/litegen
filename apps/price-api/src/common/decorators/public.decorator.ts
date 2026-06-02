import { SetMetadata } from '@nestjs/common';

export const IS_PUBLIC_KEY = 'isPublic';

/**
 * Marks a route as publicly accessible, bypassing the global JWT guard.
 * Used on read endpoints (`GET /v1/...`) which are open but still rate-limited.
 */
export const Public = () => SetMetadata(IS_PUBLIC_KEY, true);
