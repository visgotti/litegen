//! Which tools a caller sees. Each tool's gate mirrors the check its HTTP
//! route performs, so `tools/list` shows what the caller's credential can call
//! and nothing it never could. The handlers still run every check on each call
//! (the path's org must be the caller's, a member may edit only their own
//! key, and so on).

use super::auth::Caller;
use super::catalog::{Gate, Http, ToolDef};
use super::Surface;
use crate::api::handlers::management::require_gateway_admin;
use crate::api::handlers::users::may_manage_users;
use crate::api::middleware::{is_deployment_reader, is_platform_admin, AppState, KeyContext, Scope};
use crate::auth::permissions::Permission;
use crate::config::Mode;

pub fn visible(def: &ToolDef, caller: &Caller, state: &AppState) -> bool {
    if !def.surfaces.includes(caller.surface) {
        return false;
    }
    if let Some(toolsets) = &caller.toolsets {
        if !toolsets.iter().any(|t| t == def.toolset) {
            return false;
        }
    }
    let ctx = &caller.ctx;
    // The AI tools act inside a tenant. The hosted master key has none, so of
    // them it can only use the catalog (`forbidden_no_org` everywhere else).
    if caller.surface == Surface::Ai && ctx.org_id.is_none() && def.toolset != "catalog" {
        return false;
    }
    passes(def, ctx, state)
}

fn passes(def: &ToolDef, ctx: &KeyContext, state: &AppState) -> bool {
    let has = |scope: Scope| ctx.scopes.contains(&scope);
    // Where a role decides, `admin` covers reads too, as the API's role
    // checks (`require_org`, `check_grant_scope`) have it.
    let may = |scope: Scope| has(scope) || (scope == Scope::Read && has(Scope::Admin));
    // A signed-in user acts by role, narrowed by the scopes their credential
    // carries (a browser session carries all three; an OAuth grant, the
    // ones the person approved).
    let user_may = |perm, scope| ctx.user.is_some() && ctx.permissions.contains(&perm) && may(scope);
    let key = ctx.key_id.is_some();
    // What a call through a user's role needs from their scopes.
    let method_scope = if def.method == Http::Get { Scope::Read } else { Scope::Admin };
    match def.gate {
        // The routes that start or change generations also want a person to
        // hold `generation:create` (`check_generation_principal`): a Viewer's
        // grant never shows them, whatever its scopes.
        Gate::Scope(Scope::Generate) => has(Scope::Generate) && (ctx.user.is_none() || key || ctx.permissions.contains(&Permission::GenerationCreate)),
        Gate::Scope(scope) => has(scope),
        Gate::AnyScope(scopes) => scopes.iter().any(|s| has(*s)),
        Gate::OrgRead(perm) => is_platform_admin(ctx) || user_may(perm, Scope::Read) || (key && may(Scope::Read)),
        Gate::OrgWrite(perm) => is_platform_admin(ctx) || user_may(perm, Scope::Admin) || (key && has(Scope::Admin)),
        Gate::Member(perm) => is_platform_admin(ctx) || user_may(perm, method_scope),
        Gate::Perm(perm) => ctx.permissions.contains(&perm) && may(method_scope),
        Gate::Global(perm) => match state.mode {
            Mode::Hosted => is_platform_admin(ctx),
            Mode::SingleTenant => may_manage_users(state, ctx, perm) && may(method_scope),
        },
        Gate::GatewayAdmin => require_gateway_admin(state, ctx).is_ok(),
        Gate::Deployment(scope) => has(scope) && is_deployment_reader(ctx, state.mode),
        // A person's own things: reading needs no particular scope (it is
        // their account), changing needs `admin`.
        Gate::User => ctx.user.is_some() && (def.method == Http::Get || may(Scope::Admin)),
        Gate::Any => true,
    }
}
