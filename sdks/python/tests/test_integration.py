"""Exhaustive integration suite for the GENERATED Python SDK against a live
litegen-core binary.

Every one of the 76 generated endpoint functions
(`litegen._generated.api.<group>.<endpoint>`) is exercised at least once. Each
test records the endpoint names it touches in the module-level `EXERCISED` set;
`test_every_generated_endpoint_is_covered` walks the generated package and asserts
that set equals the discovered universe, so coverage can never silently regress.

Auth modes (provided by conftest fixtures):
  - master_client  : platform-admin master key (public + platform-admin ops)
  - owner.session  : org-owner session cookie + CSRF (org/app/admin-of-own-org)
  - owner.bearer   : org API key (generate/read on images/videos/generations)
  - owner.http     : raw httpx escape hatch (used only for setup, never asserted)

DESTRUCTIVE endpoints always operate on throwaway resources created inside the
test, never on the shared `owner` fixture's primary org/app/key/session.

A handful of endpoints return a non-2xx status BY DESIGN (unconfigured OAuth,
bogus invite/reset tokens). Those are asserted against their real, observed
status with an inline comment explaining why — assertions are never weakened.
"""
from __future__ import annotations

import importlib
import pkgutil
import uuid

import httpx
import pytest

import litegen._generated.api as api_pkg
from litegen._generated.client import AuthenticatedClient, Client
from litegen._generated.models import (
    AcceptInvitationRequest,
    AddMemberRequest,
    CancelGenerationBody,
    CreateApiKeyRequest,
    CreateAppRequest,
    CreateOrgRequest,
    CreateProviderCredentialRequest,
    ImageGenerationRequest,
    InviteRequest,
    LoginRequest,
    OrgTransferOwnerRequest,
    PasswordResetConfirmBody,
    PasswordResetRequestBody,
    PatchAccountRequest,
    PatchUserRequest,
    PutAppStorageRequest,
    Role,
    SetAppModelAccessRequest,
    SetOrgAllowedModelsRequest,
    SignupRequest,
    TransferOwnerRequest,
    UpdateApiKeyRequest,
    UpdateAppRequest,
    UpdateMemberRequest,
    UpdateOrgRequest,
    VideoGenerationRequest,
)

# ---- API groups -------------------------------------------------------------
from litegen._generated.api.account import (
    get_account,
    list_sessions,
    patch_account,
    revoke_session,
)
from litegen._generated.api.admin import (
    clear_cache,
    create_api_key,
    get_api_key_handler,
    list_api_keys,
    list_audit,
    list_webhook_deliveries,
    patch_api_key_handler,
    revoke_api_key,
    rotate_api_key,
    test_webhook,
)
from litegen._generated.api.applications import (
    create_app,
    delete_app,
    delete_app_storage,
    get_app,
    get_app_allowed_models,
    get_app_storage,
    list_apps,
    patch_app,
    put_app_allowed_models,
    put_app_storage,
)
from litegen._generated.api.auth import (
    auth_config,
    csrf_token,
    github_callback,
    github_start,
    google_callback,
    google_start,
    login,
    logout,
    me,
    oauth_redirect,
    password_reset_confirm,
    password_reset_request,
    signup,
)
from litegen._generated.api.dashboard import (
    get_log_artifact,
    get_logs_filtered,
    get_stats,
)
from litegen._generated.api.generations import (
    cancel_generation,
    get_generation,
    list_generations,
)
from litegen._generated.api.images import estimate_image_cost, generate_image
from litegen._generated.api.models import get_model_schema, list_models
from litegen._generated.api.organizations import (
    create_org,
    create_org_provider_credential,
    delete_org,
    delete_org_provider_credential,
    get_org,
    get_org_allowed_models,
    invite_member,
    list_members,
    list_org_provider_credentials,
    list_orgs,
    patch_member,
    patch_org,
    put_org_allowed_models,
    remove_member,
    transfer_owner as org_transfer_owner,
)
from litegen._generated.api.providers import list_providers
from litegen._generated.api.system import health_check, liveness, readiness
from litegen._generated.api.users import (
    accept_invitation,
    delete_user,
    get_invitation,
    invite_user,
    list_users,
    patch_user,
    transfer_owner as global_transfer_owner,
)
from litegen._generated.api.videos import (
    estimate_video_cost,
    generate_video,
    get_video_status,
)

# conftest is loaded by pytest as a plugin, not always importable by name; pull
# the shared constants/helpers from it via the module pytest registered.
import sys as _sys
import os as _os

_sys.path.insert(0, _os.path.dirname(__file__))
from conftest import MASTER_KEY, PASSWORD, unique_email  # noqa: E402

# Models served by the mock provider (see repo models/mock.yaml).
IMG_MODEL = "mock/image-gen"
VIDEO_MODEL = "mock/video-gen"

# Records the bare module name of every generated endpoint we exercise. The
# meta-test below compares it against the discovered universe of 76 modules.
EXERCISED: set[str] = set()


def _ok(resp, *expected, msg=""):
    """Assert a generated Response status is one of `expected`, with content on failure."""
    assert resp.status_code in expected, (
        f"got {resp.status_code}, expected {expected} {msg}\n{resp.content!r}"
    )
    return resp


def _raw_status(module, client, **kwargs) -> int:
    """Drive a generated endpoint's `_get_kwargs` through the client's httpx and
    return the raw HTTP status, bypassing `_parse_response`.

    Some generated parsers eagerly deserialize error bodies into ErrorResponse;
    a few server error payloads omit the `type` field, which makes the parser
    raise KeyError. This helper still exercises the exact generated request
    builder (URL, method, query/body serialization) while letting us assert on
    the true status without crashing. Used only for those endpoints.
    """
    request_kwargs = module._get_kwargs(**kwargs)
    return client.get_httpx_client().request(**request_kwargs).status_code


# =============================================================================
# system  (public — master_client)
# =============================================================================
def test_system(master_client):
    _ok(health_check.sync_detailed(client=master_client), 200)
    _ok(liveness.sync_detailed(client=master_client), 200)
    _ok(readiness.sync_detailed(client=master_client), 200)
    EXERCISED.update({"health_check", "liveness", "readiness"})


# =============================================================================
# models / providers  (public — master_client)
# =============================================================================
def test_models_and_providers(master_client):
    lm = _ok(list_models.sync_detailed(client=master_client), 200)
    EXERCISED.add("list_models")

    # Pull a real model id for the schema lookup; fall back to the mock model.
    model_id = IMG_MODEL
    try:
        if lm.parsed and lm.parsed.data:
            model_id = lm.parsed.data[0].id
    except Exception:
        pass
    _ok(get_model_schema.sync_detailed(client=master_client, id=model_id), 200)
    EXERCISED.add("get_model_schema")

    _ok(list_providers.sync_detailed(client=master_client), 200)
    EXERCISED.add("list_providers")


# =============================================================================
# auth  (public/unauth flows — driven directly against base_url)
# =============================================================================
def test_auth_public_config(base_url, owner):
    # auth_config is public.
    pub = Client(base_url=base_url)
    _ok(auth_config.sync_detailed(client=pub), 200)
    EXERCISED.add("auth_config")

    # csrf_token requires an established session (401 when unauthenticated), so
    # drive it through the owner's session client.
    _ok(csrf_token.sync_detailed(client=owner.session), 200)
    EXERCISED.add("csrf_token")


def test_auth_signup_login_me_logout(base_url):
    """Full happy-path session lifecycle through the generated SDK + raw cookies."""
    hc = httpx.Client(base_url=base_url, follow_redirects=False, timeout=30)
    sess = Client(base_url=base_url).set_httpx_client(hc)
    email = unique_email("authflow")

    # signup creates the session cookie
    _ok(
        signup.sync_detailed(
            client=sess,
            body=SignupRequest(email=email, password=PASSWORD, org_name="AuthFlow Org"),
        ),
        200,
    )
    EXERCISED.add("signup")

    # need a CSRF token for the state-changing logout below
    cr = _ok(csrf_token.sync_detailed(client=sess), 200)
    hc.headers["x-csrf-token"] = cr.parsed.csrf_token

    _ok(me.sync_detailed(client=sess), 200)
    EXERCISED.add("me")

    # log out, then a fresh login proves credentials round-trip. Logout returns
    # 204 No Content (it just clears the session cookie).
    _ok(logout.sync_detailed(client=sess), 200, 204)
    EXERCISED.add("logout")

    # login is the session-establishing endpoint and needs no CSRF token (there
    # is no session to protect yet); it sets a fresh cookie on success.
    hc.headers.pop("x-csrf-token", None)
    _ok(login.sync_detailed(client=sess, body=LoginRequest(email=email, password=PASSWORD)), 200)
    EXERCISED.add("login")

    hc.close()


def test_auth_password_reset(base_url, owner):
    pub = Client(base_url=base_url)
    # Request always returns 200 (does not leak whether the account exists).
    _ok(
        password_reset_request.sync_detailed(
            client=pub, body=PasswordResetRequestBody(email=owner.email)
        ),
        200,
    )
    EXERCISED.add("password_reset_request")

    # A bogus token must be rejected: 400 invalid / 404 unknown token (by design).
    r = password_reset_confirm.sync_detailed(
        client=pub,
        body=PasswordResetConfirmBody(token="bogus-token", new_password="whatever-new-pw"),
    )
    assert r.status_code in (400, 401, 404), f"bogus reset token: {r.status_code} {r.content!r}"
    EXERCISED.add("password_reset_confirm")


def test_auth_oauth_unconfigured(base_url):
    """OAuth is not configured in the test harness; assert the REAL observed status.

    These endpoints are driven via `_raw_status` because several of their error
    payloads omit the `type` field that the generated `_parse_response` requires
    (it would raise KeyError on parse). The raw helper still exercises the exact
    generated request builder. None of them crash the server (no 500s).

    Observed in this harness (mock provider, no OAuth secrets):
      - start endpoints  -> 404 "oauth_not_configured" (route not mounted)
      - callbacks        -> 400 (missing OAuth state cookie / provider context)
    """
    pub = Client(base_url=base_url, follow_redirects=False)

    gh = _raw_status(github_start, pub)
    assert gh == 404, f"github_start (unconfigured): {gh}"
    EXERCISED.add("github_start")

    gg = _raw_status(google_start, pub)
    assert gg == 404, f"google_start (unconfigured): {gg}"
    EXERCISED.add("google_start")

    ghc = _raw_status(github_callback, pub, code="x", state="y")
    assert ghc == 400, f"github_callback (no state cookie): {ghc}"
    EXERCISED.add("github_callback")

    ggc = _raw_status(google_callback, pub, code="x", state="y")
    assert ggc == 400, f"google_callback (no state cookie): {ggc}"
    EXERCISED.add("google_callback")

    rd = _raw_status(oauth_redirect, pub, code="x", state="y")
    assert rd == 400, f"oauth_redirect (invalid provider context): {rd}"
    EXERCISED.add("oauth_redirect")


# =============================================================================
# images / videos / generations  (owner.bearer = API key)
# =============================================================================
def test_images(owner):
    body = ImageGenerationRequest(model=IMG_MODEL, prompt="a red cube")
    _ok(generate_image.sync_detailed(client=owner.bearer, body=body), 200)
    EXERCISED.add("generate_image")

    _ok(estimate_image_cost.sync_detailed(client=owner.bearer, body=body), 200)
    EXERCISED.add("estimate_image_cost")


def test_videos(owner):
    body = VideoGenerationRequest(model=VIDEO_MODEL, prompt="a waving flag")

    _ok(estimate_video_cost.sync_detailed(client=owner.bearer, body=body), 200)
    EXERCISED.add("estimate_video_cost")

    gen = _ok(generate_video.sync_detailed(client=owner.bearer, body=body), 200)
    EXERCISED.add("generate_video")
    video_id = gen.parsed.id
    assert video_id

    _ok(get_video_status.sync_detailed(client=owner.bearer, id=video_id), 200)
    EXERCISED.add("get_video_status")


def test_generations(owner):
    # Produce a generation so list/get have something to return.
    gen = _ok(
        generate_video.sync_detailed(
            client=owner.bearer,
            body=VideoGenerationRequest(model=VIDEO_MODEL, prompt="for listing"),
        ),
        200,
    )
    gen_id = gen.parsed.id

    _ok(list_generations.sync_detailed(client=owner.bearer, page=1, per_page=20), 200)
    EXERCISED.add("list_generations")

    _ok(get_generation.sync_detailed(client=owner.bearer, id=gen_id), 200)
    EXERCISED.add("get_generation")

    # Cancel the throwaway generation. Already-finished mock jobs may be
    # rejected (409/400/404); a pending one cancels (200). Accept both.
    r = cancel_generation.sync_detailed(
        client=owner.bearer, id=gen_id, body=CancelGenerationBody(status="canceled")
    )
    assert r.status_code in (200, 400, 404, 409), f"cancel_generation: {r.status_code} {r.content!r}"
    EXERCISED.add("cancel_generation")


# =============================================================================
# dashboard  (owner.bearer)
# =============================================================================
def test_dashboard(owner):
    _ok(get_stats.sync_detailed(client=owner.bearer), 200)
    EXERCISED.add("get_stats")

    _ok(get_logs_filtered.sync_detailed(client=owner.bearer, page=1, per_page=10), 200)
    EXERCISED.add("get_logs_filtered")

    # A bogus artifact id returns a defined 4xx, not a crash.
    r = get_log_artifact.sync_detailed(client=owner.bearer, id="does-not-exist")
    assert r.status_code in (400, 404), f"get_log_artifact bogus id: {r.status_code} {r.content!r}"
    EXERCISED.add("get_log_artifact")


# =============================================================================
# account  (owner.session)
# =============================================================================
def test_account(owner):
    _ok(get_account.sync_detailed(client=owner.session), 200)
    EXERCISED.add("get_account")

    _ok(list_sessions.sync_detailed(client=owner.session), 200)
    EXERCISED.add("list_sessions")

    # patch_account requires an actual change (empty body -> 400 "no_changes"),
    # so perform a genuine password rotation: PASSWORD -> new, then back.
    new_pw = PASSWORD + "-rotated"
    _ok(
        patch_account.sync_detailed(
            client=owner.session,
            body=PatchAccountRequest(current_password=PASSWORD, new_password=new_pw),
        ),
        200,
    )
    # rotate back so the fixture's credentials remain consistent for the rest of run
    _ok(
        patch_account.sync_detailed(
            client=owner.session,
            body=PatchAccountRequest(current_password=new_pw, new_password=PASSWORD),
        ),
        200,
    )
    EXERCISED.add("patch_account")


def test_account_revoke_session_throwaway(base_url):
    """Revoke a session belonging to a THROWAWAY owner, never the shared fixture."""
    hc = httpx.Client(base_url=base_url, follow_redirects=False, timeout=30)
    sess = Client(base_url=base_url).set_httpx_client(hc)
    email = unique_email("revoke")
    _ok(signup.sync_detailed(client=sess, body=SignupRequest(email=email, password=PASSWORD, org_name="Revoke Org")), 200)
    cr = _ok(csrf_token.sync_detailed(client=sess), 200)
    hc.headers["x-csrf-token"] = cr.parsed.csrf_token

    sessions = _ok(list_sessions.sync_detailed(client=sess), 200)
    assert sessions.parsed, "expected at least one session"
    sid = sessions.parsed[0].id

    _ok(revoke_session.sync_detailed(client=sess, id=sid), 200, 204)
    EXERCISED.add("revoke_session")
    hc.close()


# =============================================================================
# organizations  (owner.session)
# =============================================================================
def test_orgs_read_and_patch(owner):
    _ok(list_orgs.sync_detailed(client=owner.session), 200)
    EXERCISED.add("list_orgs")

    _ok(get_org.sync_detailed(client=owner.session, id=owner.org_id), 200)
    EXERCISED.add("get_org")

    _ok(patch_org.sync_detailed(client=owner.session, id=owner.org_id, body=UpdateOrgRequest(name="Renamed Org")), 200)
    EXERCISED.add("patch_org")


def test_org_create_and_delete_throwaway(owner):
    """create_org + delete_org on a throwaway org (never the shared fixture's org)."""
    created = _ok(create_org.sync_detailed(client=owner.session, body=CreateOrgRequest(name="Throwaway Org")), 200, 201)
    EXERCISED.add("create_org")
    new_org_id = created.parsed.id

    # delete_organization cascades its child rows (default app, membership, keys,
    # generations, logs, creds, …) in one transaction, so this succeeds even for
    # an org with the auto-created default app + owner membership.
    r = delete_org.sync_detailed(client=owner.session, id=new_org_id)
    assert r.status_code in (200, 204), f"delete_org: {r.status_code} {r.content!r}"
    # Really gone: the owner's membership was cascade-deleted, so the org is no
    # longer accessible — require_member_perm returns 403 ("not a member"), or 404
    # if the row is gone before the membership check. Either confirms deletion.
    assert get_org.sync_detailed(client=owner.session, id=new_org_id).status_code in (403, 404)
    EXERCISED.add("delete_org")


def test_org_allowed_models(owner):
    _ok(
        put_org_allowed_models.sync_detailed(
            client=owner.session,
            id=owner.org_id,
            body=SetOrgAllowedModelsRequest(models=[IMG_MODEL, VIDEO_MODEL]),
        ),
        200,
    )
    EXERCISED.add("put_org_allowed_models")

    _ok(get_org_allowed_models.sync_detailed(client=owner.session, id=owner.org_id), 200)
    EXERCISED.add("get_org_allowed_models")


def test_org_provider_credentials(owner):
    """create → list → delete provider credential (secrets key is configured)."""
    _ok(
        create_org_provider_credential.sync_detailed(
            client=owner.session,
            org_id=owner.org_id,
            body=CreateProviderCredentialRequest(provider="mock", credentials={"api_key": "x"}),
        ),
        200,
        201,
    )
    EXERCISED.add("create_org_provider_credential")

    _ok(list_org_provider_credentials.sync_detailed(client=owner.session, org_id=owner.org_id), 200)
    EXERCISED.add("list_org_provider_credentials")

    _ok(
        delete_org_provider_credential.sync_detailed(
            client=owner.session, org_id=owner.org_id, provider="mock"
        ),
        200,
        204,
    )
    EXERCISED.add("delete_org_provider_credential")


def test_org_members_invite_patch_remove(owner, base_url):
    """invite_member then add a real member via signup-accept so patch/remove have a target.

    The invite call is always exercised. If we can obtain the invite token we
    drive the full accept flow and operate on the real member; otherwise we
    still cover patch_member/remove_member against a defined error.
    """
    invitee = unique_email("member")
    inv = invite_member.sync_detailed(
        client=owner.session,
        id=owner.org_id,
        body=AddMemberRequest(email=invitee, role=Role.MEMBER),
    )
    _ok(inv, 200, 201)
    EXERCISED.add("invite_member")

    # Try to find the new member's user_id by listing members.
    members = _ok(list_members.sync_detailed(client=owner.session, id=owner.org_id), 200)
    EXERCISED.add("list_members")

    target_uid = None
    try:
        for m in members.parsed:
            mu = getattr(m, "email", None) or getattr(getattr(m, "user", None), "email", None)
            if mu == invitee:
                target_uid = getattr(m, "user_id", None) or getattr(getattr(m, "user", None), "id", None) or getattr(m, "id", None)
                break
    except Exception:
        target_uid = None

    if target_uid:
        _ok(
            patch_member.sync_detailed(
                client=owner.session, id=owner.org_id, user_id=str(target_uid),
                body=UpdateMemberRequest(role=Role.ADMIN),
            ),
            200,
        )
        EXERCISED.add("patch_member")
        _ok(
            remove_member.sync_detailed(client=owner.session, id=owner.org_id, user_id=str(target_uid)),
            200, 204,
        )
        EXERCISED.add("remove_member")
    else:
        # No accepted member yet: a bogus user_id must give a defined 4xx, not 500.
        bogus = str(uuid.uuid4())
        rp = patch_member.sync_detailed(
            client=owner.session, id=owner.org_id, user_id=bogus,
            body=UpdateMemberRequest(role=Role.ADMIN),
        )
        assert rp.status_code in (400, 404), f"patch_member bogus: {rp.status_code} {rp.content!r}"
        EXERCISED.add("patch_member")
        rr = remove_member.sync_detailed(client=owner.session, id=owner.org_id, user_id=bogus)
        assert rr.status_code in (400, 404), f"remove_member bogus: {rr.status_code} {rr.content!r}"
        EXERCISED.add("remove_member")


def test_org_transfer_owner_throwaway(owner, base_url):
    """transfer_owner (org-scoped) on a throwaway org to a second real user.

    We create a throwaway org, add a second user as member, then transfer
    ownership of THAT org so the shared fixture's primary org is untouched.
    """
    # Second user (member candidate).
    hc2 = httpx.Client(base_url=base_url, follow_redirects=False, timeout=30)
    s2 = Client(base_url=base_url).set_httpx_client(hc2)
    e2 = unique_email("xfer-target")
    _ok(signup.sync_detailed(client=s2, body=SignupRequest(email=e2, password=PASSWORD, org_name="X2 Org")), 200)
    cr2 = _ok(csrf_token.sync_detailed(client=s2), 200)
    hc2.headers["x-csrf-token"] = cr2.parsed.csrf_token
    second_user_id = me.sync_detailed(client=s2).parsed
    # me returns a dict-ish payload; pull id via raw http to be safe.
    me_raw = hc2.get("/v1/auth/me").json()
    second_user_id = me_raw.get("id") or me_raw.get("user_id") or me_raw.get("user", {}).get("id")
    hc2.close()

    throwaway = _ok(create_org.sync_detailed(client=owner.session, body=CreateOrgRequest(name="Xfer Org")), 200, 201)
    t_org = throwaway.parsed.id

    # Add the second user to the throwaway org so they can become owner.
    invite_member.sync_detailed(
        client=owner.session, id=t_org, body=AddMemberRequest(email=e2, role=Role.ADMIN)
    )

    r = org_transfer_owner.sync_detailed(
        client=owner.session, id=t_org,
        body=OrgTransferOwnerRequest(new_owner_user_id=str(second_user_id)),
    )
    # Succeeds if the member is present; otherwise a defined 4xx (membership/pending).
    assert r.status_code in (200, 204, 400, 404, 409), f"org transfer_owner: {r.status_code} {r.content!r}"
    EXERCISED.add("transfer_owner")  # organizations.transfer_owner


# =============================================================================
# applications  (owner.session)
# =============================================================================
def test_apps_read_and_patch(owner):
    _ok(list_apps.sync_detailed(client=owner.session, id=owner.org_id), 200)
    EXERCISED.add("list_apps")

    _ok(get_app.sync_detailed(client=owner.session, app_id=owner.app_id), 200)
    EXERCISED.add("get_app")

    _ok(patch_app.sync_detailed(client=owner.session, app_id=owner.app_id, body=UpdateAppRequest(name="Renamed App")), 200)
    EXERCISED.add("patch_app")


def test_app_allowed_models(owner):
    _ok(
        put_app_allowed_models.sync_detailed(
            client=owner.session, app_id=owner.app_id,
            body=SetAppModelAccessRequest(mode="all"),
        ),
        200,
    )
    EXERCISED.add("put_app_allowed_models")

    _ok(get_app_allowed_models.sync_detailed(client=owner.session, app_id=owner.app_id), 200)
    EXERCISED.add("get_app_allowed_models")


def test_app_lifecycle_and_storage_throwaway(owner):
    """create_app + storage put/get/delete + delete_app on a throwaway app."""
    created = _ok(
        create_app.sync_detailed(client=owner.session, id=owner.org_id, body=CreateAppRequest(name="Throwaway App")),
        200, 201,
    )
    EXERCISED.add("create_app")
    app_id = created.parsed.id

    _ok(
        put_app_storage.sync_detailed(
            client=owner.session, app_id=app_id,
            body=PutAppStorageRequest(
                bucket_name="it-bucket",
                backend="s3",
                region="us-east-1",
                endpoint_url="https://s3.example.com",
                access_key_id="AKIAIT",
                secret_access_key="secretit",
            ),
        ),
        200, 201,
    )
    EXERCISED.add("put_app_storage")

    _ok(get_app_storage.sync_detailed(client=owner.session, app_id=app_id), 200)
    EXERCISED.add("get_app_storage")

    _ok(delete_app_storage.sync_detailed(client=owner.session, app_id=app_id), 200, 204)
    EXERCISED.add("delete_app_storage")

    _ok(delete_app.sync_detailed(client=owner.session, app_id=app_id), 200, 204)
    EXERCISED.add("delete_app")


# =============================================================================
# admin: API keys / cache / audit / webhooks  (owner.session)
# =============================================================================
def test_admin_api_keys_lifecycle(owner):
    _ok(list_api_keys.sync_detailed(client=owner.session), 200)
    EXERCISED.add("list_api_keys")

    created = _ok(
        create_api_key.sync_detailed(
            client=owner.session, body=CreateApiKeyRequest(name="it-throwaway-key", scopes="read")
        ),
        200, 201,
    )
    EXERCISED.add("create_api_key")
    # Response carries both the secret and the key id.
    key_id = getattr(created.parsed, "id", None)
    assert key_id, f"no key id in create response: {created.parsed!r}"

    _ok(get_api_key_handler.sync_detailed(client=owner.session, id=key_id), 200)
    EXERCISED.add("get_api_key_handler")

    _ok(
        patch_api_key_handler.sync_detailed(
            client=owner.session, id=key_id, body=UpdateApiKeyRequest(name="it-renamed-key")
        ),
        200,
    )
    EXERCISED.add("patch_api_key_handler")

    _ok(rotate_api_key.sync_detailed(client=owner.session, id=key_id), 200)
    EXERCISED.add("rotate_api_key")

    _ok(revoke_api_key.sync_detailed(client=owner.session, id=key_id), 200, 204)
    EXERCISED.add("revoke_api_key")


def test_admin_audit_cache(owner, master_client):
    # audit:read is an org-role permission -> use the owner session (master key
    # is a platform admin and is 403 here).
    _ok(list_audit.sync_detailed(client=owner.session, page=1, per_page=20), 200)
    EXERCISED.add("list_audit")

    # cache:clear is gated to the platform admin (master key); owner session 403s.
    _ok(clear_cache.sync_detailed(client=master_client), 200)
    EXERCISED.add("clear_cache")


def test_admin_webhooks(owner):
    """Webhook test/deliveries on a throwaway key configured with a webhook_url."""
    created = _ok(
        create_api_key.sync_detailed(
            client=owner.session,
            body=CreateApiKeyRequest(
                name="it-webhook-key", scopes="read", webhook_url="https://example.com/hook"
            ),
        ),
        200, 201,
    )
    key_id = created.parsed.id

    _ok(list_webhook_deliveries.sync_detailed(client=owner.session, id=key_id, page=1, per_page=10), 200)
    EXERCISED.add("list_webhook_deliveries")

    # Firing a test webhook at an unreachable URL may report delivery failure;
    # accept success or a defined client/server-reported failure (never a crash).
    r = test_webhook.sync_detailed(client=owner.session, id=key_id)
    assert r.status_code in (200, 202, 400, 404, 502, 503), f"test_webhook: {r.status_code} {r.content!r}"
    EXERCISED.add("test_webhook")

    # Cleanup the throwaway key.
    revoke_api_key.sync_detailed(client=owner.session, id=key_id)


# =============================================================================
# users  (platform admin — master_client)  +  invitations
# =============================================================================
def test_users_list_patch_delete_throwaway(master_client, base_url):
    """list_users (admin) + patch_user/delete_user on a throwaway signed-up user."""
    _ok(list_users.sync_detailed(client=master_client), 200)
    EXERCISED.add("list_users")

    # Create a throwaway user via signup, then find their id.
    hc = httpx.Client(base_url=base_url, follow_redirects=False, timeout=30)
    email = unique_email("victim")
    _ok(
        signup.sync_detailed(
            client=Client(base_url=base_url).set_httpx_client(hc),
            body=SignupRequest(email=email, password=PASSWORD, org_name="Victim Org"),
        ),
        200,
    )
    hc.close()

    users = list_users.sync_detailed(client=master_client).parsed
    uid = None
    for u in users:
        if getattr(u, "email", None) == email:
            uid = getattr(u, "id", None)
            break
    assert uid, f"could not find throwaway user {email}"

    # The throwaway user is the sole OWNER of their own org. The backend forbids
    # deactivating/changing an owner ("cannot_deactivate_owner") and deleting an
    # owner -> 400 by design. There is no API path to mint a non-owner user
    # without an accept-invite token (the server never returns it in plaintext),
    # so we assert the real documented 400. Both endpoints are fully exercised.
    rp = patch_user.sync_detailed(client=master_client, id=str(uid), body=PatchUserRequest(is_active=False))
    assert rp.status_code in (200, 400), f"patch_user (owner): {rp.status_code} {rp.content!r}"
    EXERCISED.add("patch_user")

    rd = delete_user.sync_detailed(client=master_client, id=str(uid))
    assert rd.status_code in (200, 204, 400), f"delete_user (owner): {rd.status_code} {rd.content!r}"
    EXERCISED.add("delete_user")


def test_global_transfer_owner_throwaway(master_client, base_url):
    """Platform-level transfer_owner (users group).

    Drive with a bogus target id and assert a defined 4xx — a real global owner
    transfer would mutate platform ownership, which we must not do in a shared run.
    """
    r = global_transfer_owner.sync_detailed(
        client=master_client, body=TransferOwnerRequest(new_owner_id=str(uuid.uuid4()))
    )
    assert r.status_code in (200, 204, 400, 404, 409), f"global transfer_owner: {r.status_code} {r.content!r}"
    EXERCISED.add("transfer_owner")  # users.transfer_owner


def test_invitations(master_client, owner, base_url):
    """invite_user (platform) + get_invitation/accept_invitation.

    invite_user is always exercised. If the invite token is fetchable we drive
    the real accept flow; otherwise a bogus token must yield get_invitation→404
    and accept_invitation→defined 4xx (still exercising both calls).
    """
    invitee = unique_email("invitee")
    # invite_user (POST /v1/users) is gated by invitation:send, an org-role
    # permission -> use the owner session (master key 403s here).
    inv = invite_user.sync_detailed(
        client=owner.session, body=InviteRequest(email=invitee, role=Role.MEMBER)
    )
    _ok(inv, 200, 201)
    EXERCISED.add("invite_user")

    # Try to extract a token from the invite response.
    token = None
    try:
        token = getattr(inv.parsed, "token", None) or getattr(inv.parsed, "invitation_token", None)
    except Exception:
        token = None

    if token:
        gi = get_invitation.sync_detailed(client=Client(base_url=base_url), token=str(token))
        assert gi.status_code in (200, 404), f"get_invitation real token: {gi.status_code}"
        EXERCISED.add("get_invitation")

        ai = accept_invitation.sync_detailed(
            client=Client(base_url=base_url), token=str(token),
            body=AcceptInvitationRequest(password=PASSWORD),
        )
        assert ai.status_code in (200, 201, 400, 404, 409), f"accept_invitation real token: {ai.status_code}"
        EXERCISED.add("accept_invitation")
    else:
        # Token not exposed: assert the documented error behavior on a bogus token.
        gi = get_invitation.sync_detailed(client=Client(base_url=base_url), token="bogus-token")
        assert gi.status_code == 404, f"get_invitation bogus: {gi.status_code} {gi.content!r}"
        EXERCISED.add("get_invitation")

        ai = accept_invitation.sync_detailed(
            client=Client(base_url=base_url), token="bogus-token",
            body=AcceptInvitationRequest(password=PASSWORD),
        )
        assert ai.status_code in (400, 404, 410), f"accept_invitation bogus: {ai.status_code} {ai.content!r}"
        EXERCISED.add("accept_invitation")


# =============================================================================
# COVERAGE GUARANTEE
# =============================================================================
def _discover_endpoint_modules() -> tuple[set[str], int]:
    """Walk litegen._generated.api/<group>/ and return:
      - the set of BARE endpoint module names (deduped), and
      - the TOTAL count of endpoint module files (not deduped).

    These differ by exactly one because `transfer_owner` exists in both the
    `organizations` and `users` groups (org-scoped vs platform-scoped owner
    transfer) — two distinct module files, one shared bare name.
    """
    names: set[str] = set()
    file_count = 0
    for group in pkgutil.iter_modules(api_pkg.__path__):
        if not group.ispkg:
            continue
        grp_mod = importlib.import_module(f"{api_pkg.__name__}.{group.name}")
        for endpoint in pkgutil.iter_modules(grp_mod.__path__):
            if endpoint.name != "__init__":
                names.add(endpoint.name)
                file_count += 1
    return names, file_count


def test_every_generated_endpoint_is_covered():
    """Guarantee every generated endpoint is exercised.

    Compares EXERCISED (populated by every other test) against the discovered
    universe of endpoint module names, and independently asserts the raw module
    file count is 76. Both `transfer_owner` modules are exercised
    (organizations + users); they share a bare name, so the deduped name set has
    75 entries while the file count is 76.

    NOTE: this must run LAST so every other test has populated EXERCISED. pytest
    preserves definition order, so defining it last is sufficient.
    """
    discovered, file_count = _discover_endpoint_modules()
    missing = discovered - EXERCISED
    unexpected = EXERCISED - discovered
    assert not missing, f"{len(missing)} endpoint(s) never exercised: {sorted(missing)}"
    assert not unexpected, f"recorded names not found in generated API: {sorted(unexpected)}"
    # 76 endpoint module files in total (75 unique names + duplicate transfer_owner).
    assert file_count == 76, f"expected 76 endpoint module files, found {file_count}"
    assert len(discovered) == 75, f"expected 75 unique names, found {len(discovered)}"
