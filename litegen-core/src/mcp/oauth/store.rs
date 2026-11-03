//! What the OAuth authorization server keeps: registered clients, pending
//! authorization requests (and the codes they become), grants and tokens.
//! Tokens and codes are stored as SHA-256 hashes only.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::mcp::Surface;

/// How a client registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registration {
    /// RFC 7591 dynamic registration: LiteGen minted the `client_id`.
    Dcr,
    /// A Client ID Metadata Document: the `client_id` is the document's URL.
    Cimd,
}

impl Registration {
    pub fn as_str(self) -> &'static str {
        match self {
            Registration::Dcr => "dcr",
            Registration::Cimd => "cimd",
        }
    }

    pub fn parse(s: &str) -> Option<Registration> {
        match s {
            "dcr" => Some(Registration::Dcr),
            "cimd" => Some(Registration::Cimd),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthClient {
    pub client_id: String,
    pub registration: Registration,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub logo_uri: Option<String>,
    pub client_uri: Option<String>,
    /// The metadata as the client sent it (DCR) or published it (CIMD).
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    /// When a CIMD document was last fetched; for DCR, when it registered.
    pub refreshed_at: DateTime<Utc>,
}

/// An authorization request: pending until a person decides it, then (if
/// approved) holding the hash of the code it was answered with.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthRequest {
    pub id: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub state: Option<String>,
    /// base64url(SHA-256(code_verifier)): PKCE S256.
    pub code_challenge: String,
    /// The scopes the client asked for.
    pub scopes: Vec<String>,
    pub surface: Surface,
    pub created_at: DateTime<Utc>,
    /// The request's deadline while pending; once approved, the code's.
    pub expires_at: DateTime<Utc>,
    pub user_id: Option<String>,
    pub org_id: Option<String>,
    pub app_id: Option<String>,
    pub granted_scopes: Option<Vec<String>>,
    pub code_hash: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub consumed_at: Option<DateTime<Utc>>,
}

/// A person's decision on a pending request.
#[derive(Debug, Clone)]
pub enum Decision {
    Approve {
        user_id: String,
        org_id: String,
        app_id: Option<String>,
        scopes: Vec<String>,
        code_hash: String,
        /// When the code stops being redeemable.
        code_expires_at: DateTime<Utc>,
        at: DateTime<Utc>,
    },
    Deny {
        at: DateTime<Utc>,
    },
}

/// What one person allowed one client to do on one endpoint, in one org.
#[derive(Debug, Clone, PartialEq)]
pub struct Grant {
    pub id: String,
    pub client_id: String,
    pub user_id: String,
    pub org_id: String,
    pub app_id: Option<String>,
    pub surface: Surface,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Access,
    Refresh,
}

impl TokenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Access => "access",
            TokenKind::Refresh => "refresh",
        }
    }

    pub fn parse(s: &str) -> Option<TokenKind> {
        match s {
            "access" => Some(TokenKind::Access),
            "refresh" => Some(TokenKind::Refresh),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenRow {
    /// SHA-256 hex of the token.
    pub token_hash: String,
    pub grant_id: String,
    pub kind: TokenKind,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Refresh tokens only: when it was exchanged for a new pair.
    pub rotated_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Redeeming an authorization code.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsumeOutcome {
    /// First use of a live code.
    Consumed(AuthRequest),
    /// The code was redeemed before: a replay.
    AlreadyUsed(AuthRequest),
    /// No such code, or it expired unused.
    Missing,
}

/// Exchanging a refresh token for a new pair.
#[derive(Debug, Clone, PartialEq)]
pub enum RotateOutcome {
    Rotated,
    /// The token was exchanged before: a replay, so its grant is suspect.
    Reused { grant_id: String },
    /// No such token, or it is expired or revoked.
    Missing,
}

#[async_trait]
pub trait McpOAuthStore: Send + Sync {
    async fn upsert_client(&self, client: &OAuthClient) -> Result<(), sqlx::Error>;
    async fn get_client(&self, client_id: &str) -> Result<Option<OAuthClient>, sqlx::Error>;

    async fn insert_request(&self, request: &AuthRequest) -> Result<(), sqlx::Error>;
    async fn get_request(&self, id: &str) -> Result<Option<AuthRequest>, sqlx::Error>;
    /// Record a decision; true only when the request was pending and unexpired.
    async fn decide_request(&self, id: &str, decision: Decision) -> Result<bool, sqlx::Error>;
    /// Redeem a code once.
    async fn consume_code(&self, code_hash: &str, now: DateTime<Utc>) -> Result<ConsumeOutcome, sqlx::Error>;

    async fn insert_grant(&self, grant: &Grant) -> Result<(), sqlx::Error>;
    async fn get_grant(&self, id: &str) -> Result<Option<Grant>, sqlx::Error>;
    /// The unrevoked grant for this client, person, tenant and endpoint.
    async fn find_active_grant(
        &self,
        client_id: &str,
        user_id: &str,
        org_id: &str,
        app_id: Option<&str>,
        surface: Surface,
    ) -> Result<Option<Grant>, sqlx::Error>;
    async fn update_grant_scopes(&self, id: &str, scopes: &[String]) -> Result<(), sqlx::Error>;
    /// A person's unrevoked grants, newest first.
    async fn list_grants_for_user(&self, user_id: &str) -> Result<Vec<Grant>, sqlx::Error>;
    /// Revoke a grant and every token it issued; true when it was active.
    async fn revoke_grant(&self, id: &str, at: DateTime<Utc>) -> Result<bool, sqlx::Error>;
    async fn touch_grant(&self, id: &str, at: DateTime<Utc>) -> Result<(), sqlx::Error>;

    async fn insert_token(&self, token: &TokenRow) -> Result<(), sqlx::Error>;
    async fn get_token(&self, token_hash: &str) -> Result<Option<TokenRow>, sqlx::Error>;
    /// Exchange a live refresh token for `new_access` and `new_refresh`, atomically.
    async fn rotate_refresh(
        &self,
        old_hash: &str,
        new_access: &TokenRow,
        new_refresh: &TokenRow,
        at: DateTime<Utc>,
    ) -> Result<RotateOutcome, sqlx::Error>;
    async fn revoke_token(&self, token_hash: &str, at: DateTime<Utc>) -> Result<(), sqlx::Error>;

    /// Delete requests that expired before `before`, tokens that expired or
    /// were revoked before it, and grants revoked before it with no tokens
    /// left; returns how many rows went.
    async fn purge_expired(&self, before: DateTime<Utc>) -> Result<u64, sqlx::Error>;
}
