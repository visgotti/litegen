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
    AuditRead,
    CacheClear,
    SystemConfig,
    SystemTransferOwner,
    InvitationSend,
    InvitationRevoke,
    SessionRevokeOwn,
    SessionRevokeAny,
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
            Self::AuditRead => "audit:read",
            Self::CacheClear => "cache:clear",
            Self::SystemConfig => "system:config",
            Self::SystemTransferOwner => "system:transfer_owner",
            Self::InvitationSend => "invitation:send",
            Self::InvitationRevoke => "invitation:revoke",
            Self::SessionRevokeOwn => "session:revoke:own",
            Self::SessionRevokeAny => "session:revoke:any",
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
            AuditRead,
            CacheClear,
            SystemConfig,
            SystemTransferOwner,
            InvitationSend,
            InvitationRevoke,
            SessionRevokeOwn,
            SessionRevokeAny,
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
            AuditRead,
            CacheClear,
            SystemConfig,
            InvitationSend,
            InvitationRevoke,
            SessionRevokeOwn,
            SessionRevokeAny,
        ],
        Role::Member => &[
            UserReadSelf,
            KeyReadOwn,
            KeyWriteOwn,
            KeyDeleteOwn,
            KeyTestWebhookOwn,
            GenerationCreate,
            GenerationReadOwn,
            GenerationCancelOwn,
            SessionRevokeOwn,
        ],
        Role::Viewer => &[
            UserReadSelf,
            KeyReadOwn,
            GenerationCreate,
            GenerationReadOwn,
            SessionRevokeOwn,
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
}
