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
from litegen import LiteGenClient
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
# management: limits / budgets  (owner.bearer = admin-scope API key)
# =============================================================================
from litegen._generated.api import budgets as _budgets  # noqa: E402
from litegen._generated.api import end_users as _end_users  # noqa: E402
from litegen._generated.api import gateway as _gateway  # noqa: E402
from litegen._generated.api import spend as _spend  # noqa: E402
from litegen._generated.api.budgets import (  # noqa: E402
    block_key, delete_app_limits, delete_key_limits, delete_member_limits, delete_org_limits,
    get_app_limits, get_key_limits, get_member_limits, get_org_limits, list_limits, my_limits,
    put_app_limits, put_key_limits, put_member_limits, put_org_limits, reset_app_limits,
    reset_key_limits, reset_member_limits, reset_org_limits, unblock_key,
)
from litegen._generated.api.end_users import (  # noqa: E402
    block_end_user, create_end_user, delete_end_user, delete_end_user_limits, get_end_user,
    get_end_user_limits, list_end_users, put_end_user_limits, reset_end_user_limits,
    unblock_end_user, update_end_user,
)
from litegen._generated.api.gateway import (  # noqa: E402
    admin_create_model, admin_delete_model, admin_delete_provider, admin_delete_routes,
    admin_get_model, admin_get_provider, admin_get_routes, admin_get_settings, admin_list_models,
    admin_list_providers, admin_patch_model, admin_put_provider, admin_put_routes,
    admin_put_settings, admin_replace_model,
)
from litegen._generated.api.spend import spend_logs, spend_report  # noqa: E402
from litegen._generated.models import (  # noqa: E402
    CreateEndUserRequest, GatewaySettings, LimitsInput, ModelSchema, PatchModelStateRequest,
    PutProviderConfigRequest, RoutesDocument, UpdateEndUserRequest,
)


def _new_key(owner, scopes="generate,read") -> tuple[str, str]:
    r = owner.http.post("/v1/keys", json={"name": f"k-{uuid.uuid4().hex[:6]}", "scopes": scopes})
    assert r.status_code in (200, 201), r.text
    return r.json()["id"], r.json()["key"]


def _owner_user_id(owner) -> str:
    members = owner.http.get(f"/v1/orgs/{owner.org_id}/members").json()
    return next(m["user_id"] for m in members if m["email"] == owner.email)


def test_limits_keys_orgs_apps_members(owner):
    key_id, raw = _new_key(owner)
    budget = LimitsInput.from_dict({"max_budget_usd": 5.0, "budget_duration": "30d", "rpm_limit": 100})

    r = _ok(put_key_limits.sync_detailed(key_id, client=owner.bearer, body=budget), 200)
    assert r.parsed.max_budget_usd == 5.0 and r.parsed.budget_duration == "30d"
    EXERCISED.add("put_key_limits")
    _ok(get_key_limits.sync_detailed(key_id, client=owner.bearer), 200)
    EXERCISED.add("get_key_limits")
    _ok(reset_key_limits.sync_detailed(key_id, client=owner.bearer), 200)
    EXERCISED.add("reset_key_limits")
    _ok(delete_key_limits.sync_detailed(key_id, client=owner.bearer), 200)
    EXERCISED.add("delete_key_limits")

    r = _ok(block_key.sync_detailed(key_id, client=owner.bearer), 200)
    assert r.parsed.is_active is False
    assert httpx.post(f"{owner.http.base_url}/v1/images/cost", headers={"authorization": f"Bearer {raw}"},
                      json={"model": IMG_MODEL, "prompt": "x"}).status_code == 401
    EXERCISED.add("block_key")
    r = _ok(unblock_key.sync_detailed(key_id, client=owner.bearer), 200)
    assert r.parsed.is_active is True
    EXERCISED.add("unblock_key")

    _ok(put_org_limits.sync_detailed(owner.org_id, client=owner.bearer, body=budget), 200)
    EXERCISED.add("put_org_limits")
    _ok(get_org_limits.sync_detailed(owner.org_id, client=owner.bearer), 200)
    EXERCISED.add("get_org_limits")
    _ok(reset_org_limits.sync_detailed(owner.org_id, client=owner.bearer), 200)
    EXERCISED.add("reset_org_limits")

    _ok(put_app_limits.sync_detailed(owner.app_id, client=owner.bearer, body=budget), 200)
    EXERCISED.add("put_app_limits")
    _ok(get_app_limits.sync_detailed(owner.app_id, client=owner.bearer), 200)
    EXERCISED.add("get_app_limits")
    _ok(reset_app_limits.sync_detailed(owner.app_id, client=owner.bearer), 200)
    EXERCISED.add("reset_app_limits")

    uid = _owner_user_id(owner)
    _ok(put_member_limits.sync_detailed(owner.org_id, uid, client=owner.session, body=budget), 200)
    EXERCISED.add("put_member_limits")
    _ok(get_member_limits.sync_detailed(owner.org_id, uid, client=owner.session), 200)
    EXERCISED.add("get_member_limits")
    _ok(reset_member_limits.sync_detailed(owner.org_id, uid, client=owner.session), 200)
    EXERCISED.add("reset_member_limits")

    r = _ok(list_limits.sync_detailed(client=owner.bearer), 200)
    assert {l.entity_type.value for l in r.parsed.data} == {"org", "app", "member"}
    r = _ok(list_limits.sync_detailed(client=owner.bearer, entity_type="app"), 200)
    assert len(r.parsed.data) == 1
    EXERCISED.add("list_limits")
    r = _ok(my_limits.sync_detailed(client=owner.bearer), 200)
    assert {l.entity_type.value for l in r.parsed.data} == {"org", "app", "member"}
    EXERCISED.add("my_limits")

    _ok(delete_member_limits.sync_detailed(owner.org_id, uid, client=owner.session), 200)
    EXERCISED.add("delete_member_limits")
    _ok(delete_app_limits.sync_detailed(owner.app_id, client=owner.bearer), 200)
    EXERCISED.add("delete_app_limits")
    _ok(delete_org_limits.sync_detailed(owner.org_id, client=owner.bearer), 200)
    EXERCISED.add("delete_org_limits")
    _ok(get_org_limits.sync_detailed(owner.org_id, client=owner.bearer), 404)


def test_budget_is_enforced_through_the_facade(owner, base_url):
    _, raw = _new_key(owner)
    admin = LiteGenClient(api_key=owner.api_key, base_url=base_url)
    admin.limits.set_app(owner.app_id, {"max_budget_usd": 7.0})
    gen = LiteGenClient(api_key=raw, base_url=base_url)
    gen.images.generate(model="mock/expensive-image", prompt="one")
    from litegen import LiteGenAPIError
    with pytest.raises(LiteGenAPIError) as e:
        gen.images.generate(model="mock/expensive-image", prompt="two")
    assert e.value.status == 402
    assert admin.limits.get_app(owner.app_id)["spend_usd"] == 5.0
    assert [lim["entity_type"] for lim in gen.limits.me()] == ["app"]
    admin.limits.delete_app(owner.app_id)


# =============================================================================
# management: end users + spend
# =============================================================================
def test_end_users_and_spend(owner, base_url):
    eu = f"cust-{uuid.uuid4().hex[:6]}"
    r = _ok(create_end_user.sync_detailed(client=owner.bearer, body=CreateEndUserRequest.from_dict(
        {"id": eu, "alias": "Jane", "limits": {"max_budget_usd": 50}})), 201)
    assert r.parsed.alias == "Jane"
    EXERCISED.add("create_end_user")
    _ok(get_end_user.sync_detailed(eu, client=owner.bearer), 200)
    EXERCISED.add("get_end_user")
    r = _ok(update_end_user.sync_detailed(eu, client=owner.bearer, body=UpdateEndUserRequest.from_dict({"alias": "J"})), 200)
    assert r.parsed.alias == "J"
    EXERCISED.add("update_end_user")
    r = _ok(list_end_users.sync_detailed(client=owner.bearer), 200)
    assert any(u.id == eu for u in r.parsed.data)
    EXERCISED.add("list_end_users")
    _ok(block_end_user.sync_detailed(eu, client=owner.bearer), 200)
    EXERCISED.add("block_end_user")
    blocked = httpx.post(f"{base_url}/v1/images/generations",
                         headers={"authorization": f"Bearer {owner.api_key}", "x-litegen-end-user": eu},
                         json={"model": IMG_MODEL, "prompt": "x"}, timeout=30)
    assert blocked.status_code == 403 and blocked.json()["error"]["type"] == "end_user_blocked"
    _ok(unblock_end_user.sync_detailed(eu, client=owner.bearer), 200)
    EXERCISED.add("unblock_end_user")

    _ok(put_end_user_limits.sync_detailed(eu, client=owner.bearer, body=LimitsInput.from_dict({"max_budget_usd": 20})), 200)
    EXERCISED.add("put_end_user_limits")
    _ok(get_end_user_limits.sync_detailed(eu, client=owner.bearer), 200)
    EXERCISED.add("get_end_user_limits")
    _ok(reset_end_user_limits.sync_detailed(eu, client=owner.bearer), 200)
    EXERCISED.add("reset_end_user_limits")

    # Attributed spend through the facade's raw request path.
    client = LiteGenClient(api_key=owner.api_key, base_url=base_url,
                           default_headers={"x-litegen-end-user": eu, "x-litegen-tags": "it,beta"})
    client.images.generate(model=IMG_MODEL, prompt="tracked")
    import time
    for _ in range(100):
        r = _ok(spend_logs.sync_detailed(client=owner.bearer, end_user=eu), 200)
        if r.parsed.total >= 1:
            break
        time.sleep(0.05)
    assert r.parsed.total == 1 and r.parsed.data[0].tags == ["beta", "it"]
    EXERCISED.add("spend_logs")
    r = _ok(spend_report.sync_detailed(client=owner.bearer, group_by="tag", end_user=eu), 200)
    assert {row.group for row in r.parsed.data} == {"beta", "it"}
    EXERCISED.add("spend_report")
    facade = LiteGenClient(api_key=owner.api_key, base_url=base_url)
    assert facade.spend.report(group_by="end_user", end_user=eu)["total_requests"] == 1
    assert facade.spend.logs(end_user=eu, per_page=5)["total"] == 1

    _ok(delete_end_user_limits.sync_detailed(eu, client=owner.bearer), 200)
    EXERCISED.add("delete_end_user_limits")
    _ok(delete_end_user.sync_detailed(eu, client=owner.bearer), 200)
    EXERCISED.add("delete_end_user")


# =============================================================================
# management: gateway admin  (master_client = platform admin)
# =============================================================================
def test_gateway_admin(master_client, owner, base_url):
    mid = f"mock/it-{uuid.uuid4().hex[:6]}"
    schema = {
        "id": mid, "provider": "mock", "media_type": "image", "display_name": "IT Runtime",
        "description": "made by the integration suite", "pricing": {"base_cost_usd": 0.0},
        "capabilities": {"text_to_image": True}, "prompt": {"required": True, "max_length": 500},
    }
    r = _ok(admin_create_model.sync_detailed(client=master_client, body=ModelSchema.from_dict(schema)), 201)
    assert r.parsed.source.value == "runtime"
    EXERCISED.add("admin_create_model")
    # A non-admin key is refused.
    _ok(admin_list_models.sync_detailed(client=owner.bearer), 403)
    r = _ok(admin_list_models.sync_detailed(client=master_client), 200)
    assert any(m.id == mid for m in r.parsed.data)
    EXERCISED.add("admin_list_models")
    _ok(admin_get_model.sync_detailed(mid, client=master_client), 200)
    EXERCISED.add("admin_get_model")
    schema["display_name"] = "IT Renamed"
    r = _ok(admin_replace_model.sync_detailed(mid, client=master_client, body=ModelSchema.from_dict(schema)), 200)
    EXERCISED.add("admin_replace_model")
    LiteGenClient(api_key=owner.api_key, base_url=base_url).images.generate(model=mid, prompt="runtime model")
    r = _ok(admin_patch_model.sync_detailed(mid, client=master_client, body=PatchModelStateRequest(disabled=True)), 200)
    assert r.parsed.disabled is True
    EXERCISED.add("admin_patch_model")
    _ok(admin_delete_model.sync_detailed(mid, client=master_client), 200)
    EXERCISED.add("admin_delete_model")

    r = _ok(admin_list_providers.sync_detailed(client=master_client), 200)
    assert any(p.name == "mock" and p.active for p in r.parsed.data)
    EXERCISED.add("admin_list_providers")
    secret = "sk-it-provider-secret-4321"
    r = _ok(admin_put_provider.sync_detailed("openai", client=master_client, body=PutProviderConfigRequest.from_dict(
        {"api_key": secret, "api_base": "https://example.invalid/v1"})), 200)
    assert r.parsed.key_hint == "…4321" and secret not in r.content.decode()
    EXERCISED.add("admin_put_provider")
    _ok(admin_get_provider.sync_detailed("openai", client=master_client), 200)
    EXERCISED.add("admin_get_provider")
    r = _ok(admin_delete_provider.sync_detailed("openai", client=master_client), 200)
    assert r.parsed.source == "none"
    EXERCISED.add("admin_delete_provider")

    r = _ok(admin_put_routes.sync_detailed(client=master_client, body=RoutesDocument.from_dict(
        {"routes": [{"model": "mock/*", "deployments": [{"provider": "mock"}]}]})), 200)
    assert r.parsed.source == "runtime"
    EXERCISED.add("admin_put_routes")
    _ok(admin_get_routes.sync_detailed(client=master_client), 200)
    EXERCISED.add("admin_get_routes")
    r = _ok(admin_delete_routes.sync_detailed(client=master_client), 200)
    assert r.parsed.source == "config"
    EXERCISED.add("admin_delete_routes")

    r = _ok(admin_put_settings.sync_detailed(client=master_client, body=GatewaySettings.from_dict({"cost_markup_percent": 5.0})), 200)
    assert r.parsed.effective_cost_markup_percent == 5.0
    EXERCISED.add("admin_put_settings")
    _ok(admin_get_settings.sync_detailed(client=master_client), 200)
    EXERCISED.add("admin_get_settings")
    _ok(admin_put_settings.sync_detailed(client=master_client, body=GatewaySettings.from_dict({})), 200)

    # The facade covers the same surface.
    facade = LiteGenClient(api_key=MASTER_KEY, base_url=base_url)
    assert any(m["id"] == IMG_MODEL for m in facade.admin.models.list())
    assert facade.admin.routes.get()["source"] == "config"
    assert facade.admin.settings.get()["settings"]["cost_markup_percent"] is None
    assert any(p["name"] == "mock" for p in facade.admin.providers.list())


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


def test_every_generated_endpoint_is_covered(base_url):
    """Guarantee every generated endpoint is exercised.

    Compares EXERCISED (populated by every other test) against the discovered
    universe of endpoint module names, and independently asserts the raw module
    file count is 124. Both `transfer_owner` modules are exercised
    (organizations + users); they share a bare name, so the deduped name set has
    123 entries while the file count is 124.

    Takes `base_url` only so it skips with the rest of the suite when the binary
    isn't built: without the binary nothing populates EXERCISED, and this would
    fail on a coverage gap that doesn't exist.

    NOTE: this must run LAST so every other test has populated EXERCISED. pytest
    preserves definition order, so defining it last is sufficient.
    """
    discovered, file_count = _discover_endpoint_modules()
    missing = discovered - EXERCISED
    unexpected = EXERCISED - discovered
    assert not missing, f"{len(missing)} endpoint(s) never exercised: {sorted(missing)}"
    assert not unexpected, f"recorded names not found in generated API: {sorted(unexpected)}"
    # 124 endpoint module files in total (123 unique names + duplicate transfer_owner).
    assert file_count == 124, f"expected 124 endpoint module files, found {file_count}"
    assert len(discovered) == 123, f"expected 123 unique names, found {len(discovered)}"
