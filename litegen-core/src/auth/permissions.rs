use crate::types::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    UserReadSelf,
    UserReadAny,
    UserWriteAny,
    UserDeleteAny,
    KeyReadOwn,
    KeyReadAny,
    KeyWriteOwn,
    KeyWriteAny,
    KeyDeleteOwn,
    KeyDeleteAny,
    KeyTestWebhookOwn,
    KeyTestWebhookAny,
    GenerationCreate,
    GenerationReadOwn,
    GenerationReadAny,
    GenerationCancelOwn,
    GenerationCancelAny,
    GenerationDeleteOwn,
    GenerationDeleteAny,
    AuditRead,
    CacheClear,
    SystemConfig,
    SystemTransferOwner,
    InvitationSend,
    InvitationRevoke,
    SessionRevokeOwn,
    SessionRevokeAny,
    // ─── Per-organization (membership-scoped) ───────────────────────────
    OrgRead,
    OrgWrite,
    OrgDelete,
    OrgTransferOwner,
    AppRead,
    AppWrite,
    AppDelete,
    MemberRead,
    MemberInvite,
    MemberWrite,
    MemberRemove,
    ProviderCredRead,
    ProviderCredWrite,
    ProviderCredDelete,
    StorageCredRead,
    StorageCredWrite,
    StorageCredDelete,
}

impl Permission {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UserReadSelf => "user:read:self",
            Self::UserReadAny => "user:read:any",
            Self::UserWriteAny => "user:write:any",
            Self::UserDeleteAny => "user:delete:any",
            Self::KeyReadOwn => "key:read:own",
            Self::KeyReadAny => "key:read:any",
            Self::KeyWriteOwn => "key:write:own",
            Self::KeyWriteAny => "key:write:any",
            Self::KeyDeleteOwn => "key:delete:own",
            Self::KeyDeleteAny => "key:delete:any",
            Self::KeyTestWebhookOwn => "key:test_webhook:own",
            Self::KeyTestWebhookAny => "key:test_webhook:any",
            Self::GenerationCreate => "generation:create",
            Self::GenerationReadOwn => "generation:read:own",
            Self::GenerationReadAny => "generation:read:any",
            Self::GenerationCancelOwn => "generation:cancel:own",
            Self::GenerationCancelAny => "generation:cancel:any",
            Self::GenerationDeleteOwn => "generation:delete:own",
            Self::GenerationDeleteAny => "generation:delete:any",
            Self::AuditRead => "audit:read",
            Self::CacheClear => "cache:clear",
            Self::SystemConfig => "system:config",
            Self::SystemTransferOwner => "system:transfer_owner",
            Self::InvitationSend => "invitation:send",
            Self::InvitationRevoke => "invitation:revoke",
            Self::SessionRevokeOwn => "session:revoke:own",
            Self::SessionRevokeAny => "session:revoke:any",
            Self::OrgRead => "org:read",
            Self::OrgWrite => "org:write",
            Self::OrgDelete => "org:delete",
            Self::OrgTransferOwner => "org:transfer_owner",
            Self::AppRead => "app:read",
            Self::AppWrite => "app:write",
            Self::AppDelete => "app:delete",
            Self::MemberRead => "member:read",
            Self::MemberInvite => "member:invite",
            Self::MemberWrite => "member:write",
            Self::MemberRemove => "member:remove",
            Self::ProviderCredRead => "provider_cred:read",
            Self::ProviderCredWrite => "provider_cred:write",
            Self::ProviderCredDelete => "provider_cred:delete",
            Self::StorageCredRead => "storage_cred:read",
            Self::StorageCredWrite => "storage_cred:write",
            Self::StorageCredDelete => "storage_cred:delete",
        }
    }
}

pub fn permissions_for(role: Role) -> &'static [Permission] {
    use Permission::*;
    match role {
        Role::Owner => &[
            UserReadSelf,
            UserReadAny,
            UserWriteAny,
            UserDeleteAny,
            KeyReadOwn,
            KeyReadAny,
            KeyWriteOwn,
            KeyWriteAny,
            KeyDeleteOwn,
            KeyDeleteAny,
            KeyTestWebhookOwn,
            KeyTestWebhookAny,
            GenerationCreate,
            GenerationReadOwn,
            GenerationReadAny,
            GenerationCancelOwn,
            GenerationCancelAny,
            GenerationDeleteOwn,
            GenerationDeleteAny,
            AuditRead,
            CacheClear,
            SystemConfig,
            SystemTransferOwner,
            InvitationSend,
            InvitationRevoke,
            SessionRevokeOwn,
            SessionRevokeAny,
            OrgRead,
            OrgWrite,
            OrgDelete,
            OrgTransferOwner,
            AppRead,
            AppWrite,
            AppDelete,
            MemberRead,
            MemberInvite,
            MemberWrite,
            MemberRemove,
            ProviderCredRead,
            ProviderCredWrite,
            ProviderCredDelete,
            StorageCredRead,
            StorageCredWrite,
            StorageCredDelete,
        ],
        Role::Admin => &[
            UserReadSelf,
            UserReadAny,
            UserWriteAny,
            UserDeleteAny,
            KeyReadOwn,
            KeyReadAny,
            KeyWriteOwn,
            KeyWriteAny,
            KeyDeleteOwn,
            KeyDeleteAny,
            KeyTestWebhookOwn,
            KeyTestWebhookAny,
            GenerationCreate,
            GenerationReadOwn,
            GenerationReadAny,
            GenerationCancelOwn,
            GenerationCancelAny,
            GenerationDeleteOwn,
            GenerationDeleteAny,
            AuditRead,
            CacheClear,
            SystemConfig,
            InvitationSend,
            InvitationRevoke,
            SessionRevokeOwn,
            SessionRevokeAny,
            // Admin gets every per-org permission EXCEPT org delete + transfer-owner.
            OrgRead,
            OrgWrite,
            AppRead,
            AppWrite,
            AppDelete,
            MemberRead,
            MemberInvite,
            MemberWrite,
            MemberRemove,
            ProviderCredRead,
            ProviderCredWrite,
            ProviderCredDelete,
            StorageCredRead,
            StorageCredWrite,
            StorageCredDelete,
        ],
        // README's roles table is the contract (review 2026-11-11): a Member
        // manages their own keys, creates generations and views their own
        // logs. An app's budgets, limits, model access and settings, its
        // storage and access settings, and the org's vendor credentials are an
        // Owner's or Admin's (this reverses the 2026-06-04 BYO design's grants).
        Role::Member => &[
            UserReadSelf,
            KeyReadOwn,
            KeyWriteOwn,
            KeyDeleteOwn,
            KeyTestWebhookOwn,
            GenerationCreate,
            GenerationReadOwn,
            GenerationCancelOwn,
            GenerationDeleteOwn,
            SessionRevokeOwn,
            OrgRead,
            AppRead,
            MemberRead,
            ProviderCredRead,
            StorageCredRead,
        ],
        // Read-only: "cannot create or mutate" (README).
        Role::Viewer => &[
            UserReadSelf,
            KeyReadOwn,
            GenerationReadOwn,
            SessionRevokeOwn,
            OrgRead,
            AppRead,
            MemberRead,
            ProviderCredRead,
            StorageCredRead,
        ],
    }
}

pub fn role_has(role: Role, perm: Permission) -> bool {
    permissions_for(role).contains(&perm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_has_transfer_owner() {
        assert!(role_has(Role::Owner, Permission::SystemTransferOwner));
    }

    #[test]
    fn admin_does_not_have_transfer_owner() {
        assert!(!role_has(Role::Admin, Permission::SystemTransferOwner));
    }

    #[test]
    fn viewer_cannot_write_keys() {
        assert!(!role_has(Role::Viewer, Permission::KeyWriteOwn));
    }

    #[test]
    fn member_can_test_own_webhook_but_not_any() {
        assert!(role_has(Role::Member, Permission::KeyTestWebhookOwn));
        assert!(!role_has(Role::Member, Permission::KeyTestWebhookAny));
    }

    #[test]
    fn member_cannot_delete_org() {
        assert!(role_has(Role::Member, Permission::OrgRead));
        assert!(!role_has(Role::Member, Permission::OrgDelete));
        assert!(!role_has(Role::Member, Permission::OrgWrite));
    }

    #[test]
    fn viewer_cannot_write_app() {
        assert!(role_has(Role::Viewer, Permission::AppRead));
        assert!(!role_has(Role::Viewer, Permission::AppWrite));
        assert!(!role_has(Role::Viewer, Permission::ProviderCredWrite));
    }

    #[test]
    fn admin_cannot_transfer_owner() {
        assert!(role_has(Role::Admin, Permission::OrgWrite));
        assert!(!role_has(Role::Admin, Permission::OrgTransferOwner));
        assert!(!role_has(Role::Admin, Permission::OrgDelete));
    }

    /// Review 2026-11-11 SEC-STORAGECRED: an app's storage and access settings
    /// are an Owner's or Admin's (`docs/storage-access.md`, README roles).
    #[test]
    fn storage_settings_are_owner_and_admin_only() {
        use Permission::*;
        for r in [Role::Owner, Role::Admin] {
            assert!(role_has(r, StorageCredRead));
            assert!(role_has(r, StorageCredWrite));
            assert!(role_has(r, StorageCredDelete));
        }
        for r in [Role::Member, Role::Viewer] {
            assert!(role_has(r, StorageCredRead), "{r:?} reads them");
            assert!(!role_has(r, StorageCredWrite), "{r:?} may not change them");
            assert!(!role_has(r, StorageCredDelete), "{r:?} may not delete them");
        }
        assert_eq!(StorageCredRead.as_str(), "storage_cred:read");
        assert_eq!(StorageCredWrite.as_str(), "storage_cred:write");
        assert_eq!(StorageCredDelete.as_str(), "storage_cred:delete");
    }

    /// Review 2026-11-11 NEW 1–2, the user's decision to match the README: a
    /// Member manages their own keys, creates generations and views their own
    /// logs. An app's budgets, limits, model access and settings, and the
    /// org's vendor credentials, are an Owner's or Admin's.
    #[test]
    fn members_hold_no_app_or_vendor_credential_writes() {
        use Permission::*;
        for p in [AppWrite, ProviderCredWrite, ProviderCredDelete, StorageCredWrite] {
            assert!(!role_has(Role::Member, p), "a member holds {p:?}");
            assert!(role_has(Role::Admin, p) && role_has(Role::Owner, p), "{p:?} stays an Owner's and Admin's");
        }
        for p in [KeyReadOwn, KeyWriteOwn, KeyDeleteOwn, GenerationCreate, GenerationReadOwn, AppRead, ProviderCredRead] {
            assert!(role_has(Role::Member, p), "a member keeps {p:?}");
        }
    }

    /// Review 2026-11-11 NEW 3: a Viewer "cannot create or mutate" (README).
    #[test]
    fn viewers_cannot_create_generations() {
        assert!(!role_has(Role::Viewer, Permission::GenerationCreate));
        assert!(role_has(Role::Viewer, Permission::GenerationReadOwn), "a viewer still reads their own");
        for r in [Role::Owner, Role::Admin, Role::Member] {
            assert!(role_has(r, Permission::GenerationCreate), "{r:?} creates generations");
        }
    }

    #[test]
    fn owner_has_all_org_perms() {
        for p in [
            Permission::OrgRead,
            Permission::OrgWrite,
            Permission::OrgDelete,
            Permission::OrgTransferOwner,
            Permission::AppRead,
            Permission::AppWrite,
            Permission::AppDelete,
            Permission::MemberRead,
            Permission::MemberInvite,
            Permission::MemberWrite,
            Permission::MemberRemove,
            Permission::ProviderCredRead,
            Permission::ProviderCredWrite,
            Permission::ProviderCredDelete,
        ] {
            assert!(role_has(Role::Owner, p), "owner should have {:?}", p);
        }
    }
}
