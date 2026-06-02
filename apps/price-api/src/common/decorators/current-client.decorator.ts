import { createParamDecorator, ExecutionContext } from '@nestjs/common';
import { AuthPrincipal } from '../auth-principal';

/** Injects the authenticated {@link AuthPrincipal} into a controller handler. */
export const CurrentClient = createParamDecorator(
  (_data: unknown, ctx: ExecutionContext): AuthPrincipal | undefined => {
    const request = ctx.switchToHttp().getRequest();
    return request.user;
  },
);
