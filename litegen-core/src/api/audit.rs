//! Where an audit entry is filed, and who reads it. An org's Owners and
//! Admins (`audit:read`) and its `admin`-scope keys read the entries filed
//! under that org (`GET /v1/audit`). In hosted mode the master key acts in no
//! org: its changes to the gateway and to accounts are filed under no org,
//! and it reads those. In either mode it reads any one org's with `?org_id=`.

use crate::api::middleware::{is_platform_admin, KeyContext, DEFAULT_ORG_ID};
use crate::config::Mode;
use crate::db::{AuditLogOrg, DatabaseStore};

/// The org an audit entry is filed under, given `org`: the one whose object
/// the action changed, else the one the caller acts in. Without one,
/// single-tenant mode files it under the default org, the deployment's own,
/// whose Owners and Admins are the operator's people. Hosted mode files it
/// under no org: there the master key acts in none, and the default org's
/// members must not read the operator's changes to the gateway and to
/// accounts (item 11, 2026-11-15).
pub(crate) fn audit_org(mode: Mode, org: Option<String>) -> Option<String> {
    match mode {
        Mode::SingleTenant => org.or_else(|| Some(DEFAULT_ORG_ID.to_string())),
        Mode::Hosted => org,
    }
}

/// Whose entries `ctx` reads at `GET /v1/audit`, given its `org_id` query
/// parameter: the platform admin (the master key) reads any org it names, as
/// the spend endpoints have it; anyone else only the org they act in, which
/// `org_id` may name. Without `org_id`, the org `ctx` acts in; in hosted mode
/// the master key acts in none and reads the entries filed under no org.
pub(crate) fn readable_audit_org<'a>(mode: Mode, ctx: &'a KeyContext, org_id: Option<&'a str>) -> Result<AuditLogOrg<'a>, AuditRefusal> {
    match (org_id.filter(|o| !o.is_empty()), ctx.org_id.as_deref()) {
        (Some(named), _) if is_platform_admin(ctx) => Ok(AuditLogOrg::Org(named)),
        (Some(named), Some(own)) if named == own => Ok(AuditLogOrg::Org(own)),
        (Some(_), _) => Err(AuditRefusal::OtherOrg),
        (None, Some(own)) => Ok(AuditLogOrg::Org(own)),
        (None, None) if mode == Mode::Hosted && is_platform_admin(ctx) => Ok(AuditLogOrg::NoOrg),
        (None, None) => Err(AuditRefusal::NoOrg),
    }
}

/// Why [`readable_audit_org`] refused (both 403).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuditRefusal {
    /// The caller acts in no org.
    NoOrg,
    /// `org_id` names an org other than the caller's own.
    OtherOrg,
}

/// The actions versions before 2026-11-15 filed under the default org when
/// the hosted master key took them, although they concern no org: its
/// changes to the gateway (`/v1/admin/*`) and to accounts (`/v1/users`). In
/// hosted mode no one else takes them.
const MASTER_KEY_NO_ORG_ACTIONS: [&str; 14] = [
    "model.create",
    "model.update",
    "model.disable",
    "model.enable",
    "model.delete",
    "provider.configure",
    "provider.reset",
    "routes.update",
    "routes.reset",
    "links.revoke",
    "settings.update",
    "user.update",
    "user.delete",
    "system.transfer_owner",
];

/// Hosted mode, at every start (`main.rs`): versions before 2026-11-15 filed
/// the master key's changes to the gateway and to accounts under the default
/// org, whose Owners and Admins read them. They move to no org, where only
/// the master key reads them. Its entries about an org's object (a budget, a
/// key, an end user, an invitation) were filed under that org and stay.
/// Single-tenant mode moves nothing: there the default org is the
/// deployment's. Returns how many moved; once none are left, a start moves
/// none.
pub async fn move_operator_entries_out_of_the_default_org(db: &dyn DatabaseStore, mode: Mode) -> Result<u64, sqlx::Error> {
    match mode {
        Mode::Hosted => db.move_master_key_audit_entries_to_no_org(DEFAULT_ORG_ID, &MASTER_KEY_NO_ORG_ACTIONS).await,
        Mode::SingleTenant => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_without_an_org_is_filed_under_the_default_org_only_in_single_tenant_mode() {
        assert_eq!(audit_org(Mode::SingleTenant, None).as_deref(), Some(DEFAULT_ORG_ID));
        assert_eq!(audit_org(Mode::Hosted, None), None, "hosted: filed under no org");
        for mode in [Mode::SingleTenant, Mode::Hosted] {
            assert_eq!(audit_org(mode, Some("org-b".into())).as_deref(), Some("org-b"), "{mode:?}: the org it names");
        }
    }

    /// The start-up step: in hosted mode the master key's entries about the
    /// gateway and accounts leave the default org, and its entries about an
    /// org's object stay there; single-tenant mode moves nothing.
    #[tokio::test]
    async fn at_start_hosted_mode_moves_the_master_keys_gateway_and_account_entries_out_of_the_default_org() {
        use crate::db::sqlite::SqliteDatabase;
        use crate::types::{AuditLogEntry, AuditLogFilter};
        let entry = |action: &str, target_type: &str| AuditLogEntry {
            id: format!("audit-{action}"),
            actor_key_id: None,
            actor_label: "master-key".to_string(),
            action: action.to_string(),
            target_type: target_type.to_string(),
            target_id: "t".to_string(),
            before_json: None,
            after_json: None,
            created_at: chrono::Utc::now(),
            org_id: Some(DEFAULT_ORG_ID.to_string()),
        };
        let gateway_and_accounts = [
            ("model.create", "model"),
            ("provider.configure", "provider"),
            ("routes.update", "routes"),
            ("settings.update", "settings"),
            ("links.revoke", "settings"),
            ("user.update", "user"),
            ("system.transfer_owner", "user"),
        ];
        let an_orgs_objects = [
            ("limits.update", "org_limits"),
            ("key.block", "api_key"),
            ("end_user.create", "end_user"),
            ("invitation.create", "invitation"),
            ("org.member.remove", "membership"),
        ];
        for (mode, moved) in [(Mode::SingleTenant, 0), (Mode::Hosted, gateway_and_accounts.len() as u64)] {
            let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
            for (action, target_type) in gateway_and_accounts.iter().chain(&an_orgs_objects) {
                db.insert_audit_log(&entry(action, target_type)).await.unwrap();
            }
            assert_eq!(move_operator_entries_out_of_the_default_org(&db, mode).await.unwrap(), moved, "{mode:?}");
            let (entries, _) = db.list_audit_log(&AuditLogFilter::default(), 1, 50).await.unwrap();
            assert_eq!(entries.len(), gateway_and_accounts.len() + an_orgs_objects.len());
            for e in entries {
                let leaves = mode == Mode::Hosted && gateway_and_accounts.iter().any(|(action, _)| *action == e.action);
                assert_eq!(e.org_id.is_none(), leaves, "{mode:?} {}", e.action);
            }
        }
    }
}
