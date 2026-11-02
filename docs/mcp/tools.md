# LiteGen MCP tool reference

Generated from LiteGen's tool registry by `cargo run --example mcp_tools_doc` (run in `litegen-core/`). A test fails when this file is out of date, so don't edit it by hand. Connecting: [README](README.md). What each requirement means: [permissions](permissions.md).

## `/mcp`: the AI API (22 tools)

### Toolset `images`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`estimate_image_cost`](#estimate_image_cost) | Estimate image cost | `POST /v1/images/cost` | the `generate` scope | yes | no |
| [`generate_image`](#generate_image) | Generate image | `POST /v1/images/generations` | the `generate` scope | no | no |
| [`segment_image`](#segment_image) | Segment image | `POST /v1/images/segmentations` | the `generate` scope | no | no |

### Toolset `videos`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`estimate_video_cost`](#estimate_video_cost) | Estimate video cost | `POST /v1/videos/cost` | the `generate` scope | yes | no |
| [`generate_video`](#generate_video) | Generate video | `POST /v1/videos/generations` | the `generate` scope | no | no |
| [`get_video_status`](#get_video_status) | Get video status | `GET /v1/videos/{id}` | the `generate` scope | yes | no |

### Toolset `models3d`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`convert_3d_model`](#convert_3d_model) | Convert 3D model | `POST /v1/utils/models3d/convert` | the `generate` scope | no | no |
| [`estimate_3d_model_cost`](#estimate_3d_model_cost) | Estimate 3D model cost | `POST /v1/models3d/cost` | the `generate` scope | yes | no |
| [`generate_3d_model`](#generate_3d_model) | Generate 3D model | `POST /v1/models3d/generations` | the `generate` scope | no | no |
| [`get_3d_model_status`](#get_3d_model_status) | Get 3D model status | `GET /v1/models3d/{id}` | the `generate` scope | yes | no |
| [`list_3d_convert_formats`](#list_3d_convert_formats) | List 3D convert formats | `GET /v1/utils/models3d/formats` | the `generate` or `read` scope | yes | no |

### Toolset `catalog`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`get_model`](#get_model) | Get model | `GET /v1/models/{id}` | the `generate` or `read` scope | yes | no |
| [`list_models`](#list_models) | List models | `GET /v1/models` | the `generate` or `read` scope | yes | no |
| [`list_providers`](#list_providers) | List providers | `GET /v1/providers` | the `generate` or `read` scope | yes | no |

### Toolset `generations`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`cancel_generation`](#cancel_generation) | Cancel generation | `PATCH /v1/generations/{id}` | the `generate` or `read` scope | no | no |
| [`get_generation`](#get_generation) | Get generation | `GET /v1/generations/{id}` | the `generate` or `read` scope | yes | no |
| [`get_generation_asset`](#get_generation_asset) | Get generation asset | `GET /v1/generations/{id}/assets/{index}` | the `generate` or `read` scope | yes | no |
| [`list_generation_assets`](#list_generation_assets) | List generation assets | `GET /v1/generations/{id}/assets` | the `generate` or `read` scope | yes | no |
| [`list_generations`](#list_generations) | List generations | `GET /v1/generations` | the `generate` or `read` scope | yes | no |
| [`wait_for_generation`](#wait_for_generation) | Wait for generation | polls the job's status route | the `generate` or `read` scope | yes | no |

### Toolset `account`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`get_current_user`](#get_current_user) | Get current user | `GET /v1/auth/me` | a signed-in user | yes | no |
| [`get_my_limits`](#get_my_limits) | Get my limits | `GET /v1/limits/me` | any credential | yes | no |

## `/admin/mcp`: the management API (93 tools)

### Toolset `keys`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`block_api_key`](#block_api_key) | Block API key | `POST /v1/keys/{id}/block` | org write access (`admin` scope, or a member role with `key:write:own`) | no | yes |
| [`create_api_key`](#create_api_key) | Create API key | `POST /v1/keys` | the `admin` scope | no | no |
| [`get_api_key`](#get_api_key) | Get API key | `GET /v1/keys/{id}` | the `admin` scope | yes | no |
| [`list_api_keys`](#list_api_keys) | List API keys | `GET /v1/keys` | the `admin` scope | yes | no |
| [`list_webhook_deliveries`](#list_webhook_deliveries) | List webhook deliveries | `GET /v1/keys/{id}/webhook-deliveries` | the `admin` scope | yes | no |
| [`revoke_api_key`](#revoke_api_key) | Revoke API key | `DELETE /v1/keys/{id}` | the `admin` scope | no | yes |
| [`rotate_api_key`](#rotate_api_key) | Rotate API key | `POST /v1/keys/{id}/rotate` | the `admin` scope | no | yes |
| [`test_api_key_webhook`](#test_api_key_webhook) | Test API key webhook | `POST /v1/keys/{id}/test-webhook` | the `admin` scope | no | no |
| [`unblock_api_key`](#unblock_api_key) | Unblock API key | `POST /v1/keys/{id}/unblock` | org write access (`admin` scope, or a member role with `key:write:own`) | no | no |
| [`update_api_key`](#update_api_key) | Update API key | `PATCH /v1/keys/{id}` | the `admin` scope | no | no |

### Toolset `budgets`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`delete_app_limits`](#delete_app_limits) | Delete app limits | `DELETE /v1/apps/{app_id}/limits` | org write access (`admin` scope, or a member role with `app:write`) | no | yes |
| [`delete_key_limits`](#delete_key_limits) | Delete key limits | `DELETE /v1/keys/{id}/limits` | org write access (`admin` scope, or a member role with `key:write:own`) | no | yes |
| [`delete_member_limits`](#delete_member_limits) | Delete member limits | `DELETE /v1/orgs/{id}/members/{user_id}/limits` | org write access (`admin` scope, or a member role with `member:write`) | no | yes |
| [`delete_org_limits`](#delete_org_limits) | Delete org limits | `DELETE /v1/orgs/{id}/limits` | org write access (`admin` scope, or a member role with `org:write`) | no | yes |
| [`get_app_limits`](#get_app_limits) | Get app limits | `GET /v1/apps/{app_id}/limits` | org read access (`read` or `admin` scope, or a member role with `app:read`) | yes | no |
| [`get_key_limits`](#get_key_limits) | Get key limits | `GET /v1/keys/{id}/limits` | org read access (`read` or `admin` scope, or a member role with `key:read:own`) | yes | no |
| [`get_member_limits`](#get_member_limits) | Get member limits | `GET /v1/orgs/{id}/members/{user_id}/limits` | org read access (`read` or `admin` scope, or a member role with `member:read`) | yes | no |
| [`get_org_limits`](#get_org_limits) | Get org limits | `GET /v1/orgs/{id}/limits` | org read access (`read` or `admin` scope, or a member role with `org:read`) | yes | no |
| [`list_limits`](#list_limits) | List limits | `GET /v1/limits` | org read access (`read` or `admin` scope, or a member role with `org:read`) | yes | no |
| [`reset_app_limits`](#reset_app_limits) | Reset app limits | `POST /v1/apps/{app_id}/limits/reset` | org write access (`admin` scope, or a member role with `app:write`) | no | yes |
| [`reset_key_limits`](#reset_key_limits) | Reset key limits | `POST /v1/keys/{id}/limits/reset` | org write access (`admin` scope, or a member role with `key:write:own`) | no | yes |
| [`reset_member_limits`](#reset_member_limits) | Reset member limits | `POST /v1/orgs/{id}/members/{user_id}/limits/reset` | org write access (`admin` scope, or a member role with `member:write`) | no | yes |
| [`reset_org_limits`](#reset_org_limits) | Reset org limits | `POST /v1/orgs/{id}/limits/reset` | org write access (`admin` scope, or a member role with `org:write`) | no | yes |
| [`set_app_limits`](#set_app_limits) | Set app limits | `PUT /v1/apps/{app_id}/limits` | org write access (`admin` scope, or a member role with `app:write`) | no | no |
| [`set_key_limits`](#set_key_limits) | Set key limits | `PUT /v1/keys/{id}/limits` | org write access (`admin` scope, or a member role with `key:write:own`) | no | no |
| [`set_member_limits`](#set_member_limits) | Set member limits | `PUT /v1/orgs/{id}/members/{user_id}/limits` | org write access (`admin` scope, or a member role with `member:write`) | no | no |
| [`set_org_limits`](#set_org_limits) | Set org limits | `PUT /v1/orgs/{id}/limits` | org write access (`admin` scope, or a member role with `org:write`) | no | no |

### Toolset `end_users`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`block_end_user`](#block_end_user) | Block end user | `POST /v1/end-users/{end_user_id}/block` | org write access (`admin` scope, or a member role with `org:write`) | no | yes |
| [`create_end_user`](#create_end_user) | Create end user | `POST /v1/end-users` | org write access (`admin` scope, or a member role with `org:write`) | no | no |
| [`delete_end_user`](#delete_end_user) | Delete end user | `DELETE /v1/end-users/{end_user_id}` | org write access (`admin` scope, or a member role with `org:write`) | no | yes |
| [`delete_end_user_limits`](#delete_end_user_limits) | Delete end user limits | `DELETE /v1/end-users/{end_user_id}/limits` | org write access (`admin` scope, or a member role with `org:write`) | no | yes |
| [`get_end_user`](#get_end_user) | Get end user | `GET /v1/end-users/{end_user_id}` | org read access (`read` or `admin` scope, or a member role with `org:read`) | yes | no |
| [`get_end_user_limits`](#get_end_user_limits) | Get end user limits | `GET /v1/end-users/{end_user_id}/limits` | org read access (`read` or `admin` scope, or a member role with `org:read`) | yes | no |
| [`list_end_users`](#list_end_users) | List end users | `GET /v1/end-users` | org read access (`read` or `admin` scope, or a member role with `org:read`) | yes | no |
| [`reset_end_user_limits`](#reset_end_user_limits) | Reset end user limits | `POST /v1/end-users/{end_user_id}/limits/reset` | org write access (`admin` scope, or a member role with `org:write`) | no | yes |
| [`set_end_user_limits`](#set_end_user_limits) | Set end user limits | `PUT /v1/end-users/{end_user_id}/limits` | org write access (`admin` scope, or a member role with `org:write`) | no | no |
| [`unblock_end_user`](#unblock_end_user) | Unblock end user | `POST /v1/end-users/{end_user_id}/unblock` | org write access (`admin` scope, or a member role with `org:write`) | no | no |
| [`update_end_user`](#update_end_user) | Update end user | `PATCH /v1/end-users/{end_user_id}` | org write access (`admin` scope, or a member role with `org:write`) | no | no |

### Toolset `spend`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`get_spend_report`](#get_spend_report) | Get spend report | `GET /v1/spend/report` | org read access (`read` or `admin` scope, or a member role with `org:read`) | yes | no |
| [`list_spend_logs`](#list_spend_logs) | List spend logs | `GET /v1/spend/logs` | org read access (`read` or `admin` scope, or a member role with `org:read`) | yes | no |

### Toolset `logs`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`get_request_log_artifact`](#get_request_log_artifact) | Get request log artifact | `GET /v1/logs/{id}/artifact` | the `read` scope | yes | no |
| [`get_usage_stats`](#get_usage_stats) | Get usage stats | `GET /v1/stats` | the `read` scope | yes | no |
| [`list_request_logs`](#list_request_logs) | List request logs | `GET /v1/logs` | the `read` scope | yes | no |

### Toolset `audit`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`list_audit_log`](#list_audit_log) | List audit log | `GET /v1/audit` | the `admin` scope | yes | no |

### Toolset `orgs`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`create_org`](#create_org) | Create org | `POST /v1/orgs` | a signed-in user | no | no |
| [`delete_org`](#delete_org) | Delete org | `DELETE /v1/orgs/{id}` | a signed-in member with `org:delete`, or the platform admin | no | yes |
| [`delete_org_provider_credential`](#delete_org_provider_credential) | Delete org provider credential | `DELETE /v1/orgs/{org_id}/provider-credentials/{provider}` | a signed-in member with `provider_cred:delete`, or the platform admin | no | yes |
| [`get_org`](#get_org) | Get org | `GET /v1/orgs/{id}` | a signed-in member with `org:read`, or the platform admin | yes | no |
| [`get_org_allowed_models`](#get_org_allowed_models) | Get org allowed models | `GET /v1/orgs/{id}/allowed-models` | a signed-in member with `org:read`, or the platform admin | yes | no |
| [`list_org_provider_credentials`](#list_org_provider_credentials) | List org provider credentials | `GET /v1/orgs/{org_id}/provider-credentials` | a signed-in member with `provider_cred:read`, or the platform admin | yes | no |
| [`list_orgs`](#list_orgs) | List orgs | `GET /v1/orgs` | a signed-in user | yes | no |
| [`set_org_allowed_models`](#set_org_allowed_models) | Set org allowed models | `PUT /v1/orgs/{id}/allowed-models` | a signed-in member with `org:write`, or the platform admin | no | no |
| [`set_org_provider_credential`](#set_org_provider_credential) | Set org provider credential | `POST /v1/orgs/{org_id}/provider-credentials` | a signed-in member with `provider_cred:write`, or the platform admin | no | no |
| [`transfer_org_ownership`](#transfer_org_ownership) | Transfer org ownership | `POST /v1/orgs/{id}/transfer-owner` | a signed-in member with `org:transfer_owner`, or the platform admin | no | yes |
| [`update_org`](#update_org) | Update org | `PATCH /v1/orgs/{id}` | a signed-in member with `org:write`, or the platform admin | no | no |

### Toolset `members`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`invite_member`](#invite_member) | Invite member | `POST /v1/orgs/{id}/members` | a signed-in member with `member:invite`, or the platform admin | no | no |
| [`list_members`](#list_members) | List members | `GET /v1/orgs/{id}/members` | a signed-in member with `member:read`, or the platform admin | yes | no |
| [`remove_member`](#remove_member) | Remove member | `DELETE /v1/orgs/{id}/members/{user_id}` | a signed-in member with `member:remove`, or the platform admin | no | yes |
| [`update_member`](#update_member) | Update member | `PATCH /v1/orgs/{id}/members/{user_id}` | a signed-in member with `member:write`, or the platform admin | no | no |

### Toolset `apps`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`create_app`](#create_app) | Create app | `POST /v1/orgs/{id}/apps` | a signed-in member with `app:write`, or the platform admin | no | no |
| [`delete_app`](#delete_app) | Delete app | `DELETE /v1/apps/{app_id}` | a signed-in member with `app:delete`, or the platform admin | no | yes |
| [`delete_app_storage`](#delete_app_storage) | Delete app storage | `DELETE /v1/apps/{app_id}/storage` | a signed-in member with `storage_cred:delete`, or the platform admin | no | yes |
| [`get_app`](#get_app) | Get app | `GET /v1/apps/{app_id}` | a signed-in member with `app:read`, or the platform admin | yes | no |
| [`get_app_allowed_models`](#get_app_allowed_models) | Get app allowed models | `GET /v1/apps/{app_id}/allowed-models` | a signed-in member with `app:read`, or the platform admin | yes | no |
| [`get_app_storage`](#get_app_storage) | Get app storage | `GET /v1/apps/{app_id}/storage` | a signed-in member with `storage_cred:read`, or the platform admin | yes | no |
| [`list_apps`](#list_apps) | List apps | `GET /v1/orgs/{id}/apps` | a signed-in member with `app:read`, or the platform admin | yes | no |
| [`set_app_allowed_models`](#set_app_allowed_models) | Set app allowed models | `PUT /v1/apps/{app_id}/allowed-models` | a signed-in member with `app:write`, or the platform admin | no | no |
| [`set_app_storage`](#set_app_storage) | Set app storage | `PUT /v1/apps/{app_id}/storage` | a signed-in member with `storage_cred:write`, or the platform admin | no | no |
| [`update_app`](#update_app) | Update app | `PATCH /v1/apps/{app_id}` | a signed-in member with `app:write`, or the platform admin | no | no |

### Toolset `users`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`delete_user`](#delete_user) | Delete user | `DELETE /v1/users/{id}` | the platform admin (single-tenant: a user with `user:delete:any`) | no | yes |
| [`invite_user`](#invite_user) | Invite user | `POST /v1/users` | a signed-in user with `invitation:send` | no | no |
| [`list_users`](#list_users) | List users | `GET /v1/users` | the platform admin (single-tenant: a user with `user:read:any`) | yes | no |
| [`transfer_platform_ownership`](#transfer_platform_ownership) | Transfer platform ownership | `POST /v1/users/transfer-owner` | the platform admin (single-tenant: a user with `system:transfer_owner`) | no | yes |
| [`update_user`](#update_user) | Update user | `PATCH /v1/users/{id}` | the platform admin (single-tenant: a user with `user:write:any`) | no | no |

### Toolset `gateway`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`create_gateway_model`](#create_gateway_model) | Create gateway model | `POST /v1/admin/models` | the platform admin (single-tenant: owners, admins and `admin` keys) | no | no |
| [`delete_gateway_model`](#delete_gateway_model) | Delete gateway model | `DELETE /v1/admin/models/{id}` | the platform admin (single-tenant: owners, admins and `admin` keys) | no | yes |
| [`delete_gateway_provider`](#delete_gateway_provider) | Delete gateway provider | `DELETE /v1/admin/providers/{name}` | the platform admin (single-tenant: owners, admins and `admin` keys) | no | yes |
| [`delete_gateway_routes`](#delete_gateway_routes) | Delete gateway routes | `DELETE /v1/admin/routes` | the platform admin (single-tenant: owners, admins and `admin` keys) | no | yes |
| [`get_gateway_model`](#get_gateway_model) | Get gateway model | `GET /v1/admin/models/{id}` | the platform admin (single-tenant: owners, admins and `admin` keys) | yes | no |
| [`get_gateway_provider`](#get_gateway_provider) | Get gateway provider | `GET /v1/admin/providers/{name}` | the platform admin (single-tenant: owners, admins and `admin` keys) | yes | no |
| [`get_gateway_routes`](#get_gateway_routes) | Get gateway routes | `GET /v1/admin/routes` | the platform admin (single-tenant: owners, admins and `admin` keys) | yes | no |
| [`get_gateway_settings`](#get_gateway_settings) | Get gateway settings | `GET /v1/admin/settings` | the platform admin (single-tenant: owners, admins and `admin` keys) | yes | no |
| [`list_gateway_models`](#list_gateway_models) | List gateway models | `GET /v1/admin/models` | the platform admin (single-tenant: owners, admins and `admin` keys) | yes | no |
| [`list_gateway_providers`](#list_gateway_providers) | List gateway providers | `GET /v1/admin/providers` | the platform admin (single-tenant: owners, admins and `admin` keys) | yes | no |
| [`replace_gateway_model`](#replace_gateway_model) | Replace gateway model | `PUT /v1/admin/models/{id}` | the platform admin (single-tenant: owners, admins and `admin` keys) | no | no |
| [`set_gateway_provider`](#set_gateway_provider) | Set gateway provider | `PUT /v1/admin/providers/{name}` | the platform admin (single-tenant: owners, admins and `admin` keys) | no | no |
| [`set_gateway_routes`](#set_gateway_routes) | Set gateway routes | `PUT /v1/admin/routes` | the platform admin (single-tenant: owners, admins and `admin` keys) | no | no |
| [`set_gateway_settings`](#set_gateway_settings) | Set gateway settings | `PUT /v1/admin/settings` | the platform admin (single-tenant: owners, admins and `admin` keys) | no | no |
| [`update_gateway_model`](#update_gateway_model) | Update gateway model | `PATCH /v1/admin/models/{id}` | the platform admin (single-tenant: owners, admins and `admin` keys) | no | no |

### Toolset `system`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`clear_cache`](#clear_cache) | Clear cache | `DELETE /v1/cache` | the `admin` scope | no | yes |
| [`get_provider_health`](#get_provider_health) | Get provider health | `GET /health` | the `read` scope | yes | no |

### Toolset `account`

| Tool | What it does | API operation | Needs | Read-only | Destructive |
|---|---|---|---|---|---|
| [`get_current_user`](#get_current_user) | Get current user | `GET /v1/auth/me` | a signed-in user | yes | no |
| [`get_my_limits`](#get_my_limits) | Get my limits | `GET /v1/limits/me` | any credential | yes | no |

## Tools

### `get_model`

One model's full schema: every parameter it takes (types, ranges, defaults), its pricing, capabilities and accepted inputs. Read it before using a model for the first time.

`GET /v1/models/{id}` · on `/mcp` · needs the `generate` or `read` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Model ID (may contain slashes, e.g. |

### `list_models`

The models you may use, with provider, media type, status and pricing. Filter with media_type (image, video, model3d).

`GET /v1/models` · on `/mcp` · needs the `generate` or `read` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `media_type` | string | no | Filter by media type |

### `list_providers`

Catalog of supported providers and the credential fields each one needs. Drives the dashboard's dynamic credential form.

`GET /v1/providers` · on `/mcp` · needs the `generate` or `read` scope

No arguments.

### `cancel_generation`

Soft-cancel a generation.

`PATCH /v1/generations/{id}` · on `/mcp` · needs the `generate` or `read` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Generation ID (litegen-vid-...) |
| `status` | string | yes | Target status; only `"cancelled"` is honored. |

### `get_generation`

Poll DB-backed generation status.

`GET /v1/generations/{id}` · on `/mcp` · needs the `generate` or `read` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Generation ID (litegen-vid-...) |

### `get_generation_asset`

The bytes of one stored file.

`GET /v1/generations/{id}/assets/{index}` · on `/mcp` · needs the `generate` or `read` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Generation ID |
| `index` | integer | yes | The asset's `index` in `GET /v1/generations/{id}/assets` |
| `preview` | boolean | no | Also return small JPEG previews of the images (longest side 512 px, at most 2). |

### `list_generation_assets`

Every file litegen kept for a generation.

`GET /v1/generations/{id}/assets` · on `/mcp` · needs the `generate` or `read` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Generation ID |

### `list_generations`

Paginated list of generations.

`GET /v1/generations` · on `/mcp` · needs the `generate` or `read` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `page` | integer | no | Page number (default 1) |
| `per_page` | integer | no | Items per page (default 50) |

### `estimate_image_cost`

Quote what generate_image would cost with the same arguments. Generates nothing and spends nothing.

`POST /v1/images/cost` · on `/mcp` · needs the `generate` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `model` | string | yes |  |
| `aspect_ratio` | string | no |  |
| `background` | one of `transparent`, `opaque` | no | `transparent` for a real alpha channel where the model supports it (`supports_transparent_background`); needs `png` or `webp`. |
| `directions` | integer | no | A character in 4 or 8 facings, one labelled image each. |
| `expand` | object | no | Grow the init image's canvas by this many pixels a side; the model's `expand` spec says how much and whether it only centres. |
| `extra` | any | no |  |
| `feather_px` | integer | no | With `preserve_unmasked`: blend from the init at the edit area's edge to the vendor's pixels this many pixels inside it (0, a hard edge, by default; at most 256). |
| `guidance_scale` | number | no |  |
| `layout` | array of object | no | Boxes for a text-to-image canvas: objects and words placed by fractions of the canvas. |
| `mask_grow_px` | integer | no | Grow the edit area outward by this many pixels: the vendor's own setting where it has one (sent explicitly, even as 0), else litegen dilates the mask itself. |
| `mask_prompt` | string | no | The area an edit replaces, named in text rather than drawn as a mask . |
| `metadata` | object | no | Your own labels for the job: a map of string values, keys of letters, digits, `_`, `.` and `-`, at most 1 KB as JSON. |
| `n` | integer | no |  |
| `negative_prompt` | string | no |  |
| `output_format` | one of `png`, `webp`, `jpeg` | no | The image container to return: one of the model's `capabilities.output_formats`. |
| `palette` | array of string | no | Colours as `#rrggbb`: every pixel one of them on a model whose palette is forced, a hint elsewhere. |
| `preserve_unmasked` | boolean | no | Paste the init image's pixels back outside the edit area, so they come back byte-identical from any vendor. |
| `prompt` | string | no | Whether a prompt is required is decided per model, by its `prompt.required` in `GET /v1/models/{id}` — never by the wire format. |
| `quality` | string | no |  |
| `reference_images` | array of object | no |  |
| `region` | object | no | Edit only this rectangle of the `init` image. |
| `response_format` | string | no |  |
| `seed` | integer | no |  |
| `size` | string | no |  |
| `steps` | integer | no |  |
| `strength` | number | no |  |
| `strict` | boolean | no |  |
| `style` | string | no |  |
| `views` | array of string | no | The views a `multiview` model makes, one image each, in this order. |

### `generate_image`

Generate or edit images with any image model LiteGen serves. Returns the finished images as URLs plus small previews. Find a model with list_models (media_type image) and its parameters with get_model; quote first with estimate_image_cost. Spends your budget.

`POST /v1/images/generations` · on `/mcp` · needs the `generate` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `model` | string | yes |  |
| `aspect_ratio` | string | no |  |
| `background` | one of `transparent`, `opaque` | no | `transparent` for a real alpha channel where the model supports it (`supports_transparent_background`); needs `png` or `webp`. |
| `directions` | integer | no | A character in 4 or 8 facings, one labelled image each. |
| `end_user` | string | no | Your own customer's id, so spend and budgets can be tracked per customer (sent as x-litegen-end-user). |
| `expand` | object | no | Grow the init image's canvas by this many pixels a side; the model's `expand` spec says how much and whether it only centres. |
| `extra` | any | no |  |
| `feather_px` | integer | no | With `preserve_unmasked`: blend from the init at the edit area's edge to the vendor's pixels this many pixels inside it (0, a hard edge, by default; at most 256). |
| `guidance_scale` | number | no |  |
| `idempotency_key` | string | no | 1–255 visible ASCII characters. |
| `layout` | array of object | no | Boxes for a text-to-image canvas: objects and words placed by fractions of the canvas. |
| `mask_grow_px` | integer | no | Grow the edit area outward by this many pixels: the vendor's own setting where it has one (sent explicitly, even as 0), else litegen dilates the mask itself. |
| `mask_prompt` | string | no | The area an edit replaces, named in text rather than drawn as a mask . |
| `metadata` | object | no | Your own labels for the job: a map of string values, keys of letters, digits, `_`, `.` and `-`, at most 1 KB as JSON. |
| `n` | integer | no |  |
| `negative_prompt` | string | no |  |
| `output_format` | one of `png`, `webp`, `jpeg` | no | The image container to return: one of the model's `capabilities.output_formats`. |
| `palette` | array of string | no | Colours as `#rrggbb`: every pixel one of them on a model whose palette is forced, a hint elsewhere. |
| `preserve_unmasked` | boolean | no | Paste the init image's pixels back outside the edit area, so they come back byte-identical from any vendor. |
| `preview` | boolean | no | Also return small JPEG previews of the images (longest side 512 px, at most 2). |
| `prompt` | string | no | Whether a prompt is required is decided per model, by its `prompt.required` in `GET /v1/models/{id}` — never by the wire format. |
| `quality` | string | no |  |
| `reference_images` | array of object | no |  |
| `region` | object | no | Edit only this rectangle of the `init` image. |
| `response_format` | string | no |  |
| `seed` | integer | no |  |
| `size` | string | no |  |
| `steps` | integer | no |  |
| `strength` | number | no |  |
| `strict` | boolean | no |  |
| `style` | string | no |  |
| `tags` | array of string | no | Spend tags for reporting (sent as x-litegen-tags). |
| `views` | array of string | no | The views a `multiview` model makes, one image each, in this order. |

### `segment_image`

Cut an image into masks, or remove its background, with a segmentation model. Spends your budget.

`POST /v1/images/segmentations` · on `/mcp` · needs the `generate` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `image` | string | yes | The image: base64, or a `data:` URI. |
| `model` | string | yes | A model whose `task` is `segment`. |
| `prompts` | array | yes | What to select. |
| `end_user` | string | no | Your own customer's id, so spend and budgets can be tracked per customer (sent as x-litegen-end-user). |
| `idempotency_key` | string | no | 1–255 visible ASCII characters. |
| `metadata` | object | no | Your own labels for the call, as on a generation. |
| `tags` | array of string | no | Spend tags for reporting (sent as x-litegen-tags). |

### `convert_3d_model`

Convert a mesh between formats on LiteGen itself: no AI model, no charge. Give the mesh as source_url or data_base64; list_3d_convert_formats says what converts to what.

`POST /v1/utils/models3d/convert` · on `/mcp` · needs the `generate` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `from` | one of `glb`, `gltf`, `obj`, `stl`, `ply`, `fbx`, `usdz`, `usda`, … | yes | Container the request body is in. |
| `to` | one of `glb`, `gltf`, `obj`, `stl`, `ply`, `fbx`, `usdz`, `usda`, … | yes | Container to convert to. |
| `data_base64` | string | no | The file itself, base64-encoded. |
| `source_url` | string | no | URL LiteGen downloads the file from. |

### `estimate_3d_model_cost`

Quote what generate_3d_model would cost with the same arguments. Generates nothing and spends nothing.

`POST /v1/models3d/cost` · on `/mcp` · needs the `generate` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `model` | string | yes |  |
| `animations` | array of object | no | The clips to make, in order: one glTF animation per entry, named exactly its `name`. |
| `center` | boolean | no | Centre the mesh's bounds on x and z. |
| `extra` | any | no |  |
| `fps` | integer | no | Frames per second of the delivered clips. |
| `ground` | boolean | no | Put the mesh's lowest point at y = 0. |
| `height_m` | number | no | The delivered mesh's height in metres: litegen scales any mesh it delivers to it, and a rig model whose vendor takes a height gets it too. |
| `joint_naming` | one of `vendor`, `vrm`, `mixamo`, `unity`, `unreal` | no | The joint names to deliver the rig with. |
| `keep_uvs` | boolean | no | Paint the source's own UV layout rather than a new one. |
| `metadata` | object | no | Your own labels for the job: a map of string values, keys of letters, digits, `_`, `.` and `-`, at most 1 KB as JSON. |
| `n` | integer | no |  |
| `negative_prompt` | string | no |  |
| `output_format` | one of `glb`, `gltf`, `obj`, `stl`, `ply`, `fbx`, `usdz`, `usda`, … | no | Superseded by `output_formats`. |
| `output_formats` | array of string | no | Mesh containers to deliver, e.g. |
| `pbr` | boolean | no | PBR materials. |
| `prompt` | string | no | Whether a prompt is required is decided per model, by its `prompt.required` in `GET /v1/models/{id}` — never by the wire format. |
| `reference_images` | array of object | no |  |
| `rig` | boolean | no | Auto-rig: deliver the mesh with a skeleton. |
| `seed` | integer | no |  |
| `skeleton` | one of `humanoid`, `quadruped`, `hexapod`, `octopod`, `avian`, `serpentine`, `aquatic`, `other` | no | The skeleton to rig for, from the model's `skeleton` enum. |
| `source_model` | object | no | The mesh an operation (`rig`, `animate`, `texture`, `remesh`, `segment`) works on, required by every model whose task is not `generate`. |
| `strict` | boolean | no |  |
| `symmetry` | one of `off`, `auto`, `on` | no | Symmetry handling. |
| `target_polycount` | integer | no | Approximate triangle budget for the mesh, within the model's declared `target_polycount` range. |
| `texture` | boolean | no | Texture the mesh, rather than returning bare geometry. |
| `texture_resolution` | integer | no | A `texture` model's map size in pixels, one of its `texture_resolutions`. |
| `topology` | one of `triangle`, `quad` | no | Face topology of the output mesh. |

### `generate_3d_model`

Start a 3D model generation (text-to-3D, image-to-3D, multiview, rigging and other 3D operations). Returns a job right away: pass its id to wait_for_generation to get the mesh files. Spends your budget.

`POST /v1/models3d/generations` · on `/mcp` · needs the `generate` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `model` | string | yes |  |
| `animations` | array of object | no | The clips to make, in order: one glTF animation per entry, named exactly its `name`. |
| `center` | boolean | no | Centre the mesh's bounds on x and z. |
| `end_user` | string | no | Your own customer's id, so spend and budgets can be tracked per customer (sent as x-litegen-end-user). |
| `extra` | any | no |  |
| `fps` | integer | no | Frames per second of the delivered clips. |
| `ground` | boolean | no | Put the mesh's lowest point at y = 0. |
| `height_m` | number | no | The delivered mesh's height in metres: litegen scales any mesh it delivers to it, and a rig model whose vendor takes a height gets it too. |
| `idempotency_key` | string | no | 1–255 visible ASCII characters. |
| `joint_naming` | one of `vendor`, `vrm`, `mixamo`, `unity`, `unreal` | no | The joint names to deliver the rig with. |
| `keep_uvs` | boolean | no | Paint the source's own UV layout rather than a new one. |
| `metadata` | object | no | Your own labels for the job: a map of string values, keys of letters, digits, `_`, `.` and `-`, at most 1 KB as JSON. |
| `n` | integer | no |  |
| `negative_prompt` | string | no |  |
| `output_format` | one of `glb`, `gltf`, `obj`, `stl`, `ply`, `fbx`, `usdz`, `usda`, … | no | Superseded by `output_formats`. |
| `output_formats` | array of string | no | Mesh containers to deliver, e.g. |
| `pbr` | boolean | no | PBR materials. |
| `prompt` | string | no | Whether a prompt is required is decided per model, by its `prompt.required` in `GET /v1/models/{id}` — never by the wire format. |
| `reference_images` | array of object | no |  |
| `rig` | boolean | no | Auto-rig: deliver the mesh with a skeleton. |
| `seed` | integer | no |  |
| `skeleton` | one of `humanoid`, `quadruped`, `hexapod`, `octopod`, `avian`, `serpentine`, `aquatic`, `other` | no | The skeleton to rig for, from the model's `skeleton` enum. |
| `source_model` | object | no | The mesh an operation (`rig`, `animate`, `texture`, `remesh`, `segment`) works on, required by every model whose task is not `generate`. |
| `strict` | boolean | no |  |
| `symmetry` | one of `off`, `auto`, `on` | no | Symmetry handling. |
| `tags` | array of string | no | Spend tags for reporting (sent as x-litegen-tags). |
| `target_polycount` | integer | no | Approximate triangle budget for the mesh, within the model's declared `target_polycount` range. |
| `texture` | boolean | no | Texture the mesh, rather than returning bare geometry. |
| `texture_resolution` | integer | no | A `texture` model's map size in pixels, one of its `texture_resolutions`. |
| `topology` | one of `triangle`, `quad` | no | Face topology of the output mesh. |

### `get_3d_model_status`

The current state of a 3D job; when completed, assets lists the mesh files in each requested format.

`GET /v1/models3d/{id}` · on `/mcp` · needs the `generate` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | 3D generation ID |

### `list_3d_convert_formats`

Containers the standalone converter reads and writes.

`GET /v1/utils/models3d/formats` · on `/mcp` · needs the `generate` or `read` scope

No arguments.

### `estimate_video_cost`

Quote what generate_video would cost with the same arguments. Generates nothing and spends nothing.

`POST /v1/videos/cost` · on `/mcp` · needs the `generate` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `model` | string | yes |  |
| `aspect_ratio` | string | no |  |
| `audio` | boolean | no | `false` for a silent clip, where the model can leave audio out (`supports_audio_toggle`). |
| `camera` | one of `fixed`, `auto` | no | `fixed` for a still camera, where the model takes one (`supports_fixed_camera`, PixelGenie L10). |
| `direction` | string | no | An `extend`: add before (`backward`) or after (`forward`) the source. |
| `duration_seconds` | number | no | Absent means the model's own `duration_seconds` default (resolved by `resolve_duration_seconds`), not a fixed 5s: models whose minimum is higher (fal/ltx-2.3, bedrock nova-reel) rejected every… |
| `extra` | any | no |  |
| `fps` | integer | no |  |
| `loop` | boolean | no | `true` for a clip whose last frame is its first (`supports_loop`). |
| `metadata` | object | no | Your own labels for the job: a map of string values, keys of letters, digits, `_`, `.` and `-`, at most 1 KB as JSON. |
| `n` | integer | no |  |
| `negative_prompt` | string | no |  |
| `prompt` | string | no | Whether a prompt is required is decided per model, by its `prompt.required` in `GET /v1/models/{id}` — never by the wire format. |
| `reference_images` | array of object | no |  |
| `replace` | string | no | A `retake`'s track: `video`, `audio` or `both`. |
| `resolution` | string | no |  |
| `seed` | integer | no |  |
| `source_video` | object | no | The clip an operation (`extend`, `retake`, `video_edit`) works on: a litegen generation id, a URL or base64, as `source_model`. |
| `strict` | boolean | no |  |
| `time_range` | object | no | A `retake`'s window, or a `video_edit`'s where the model takes one. |

### `generate_video`

Start a video generation (text-to-video, image-to-video, keyframes, edits). Returns a job right away: pass its id to wait_for_generation to get the video. Spends your budget.

`POST /v1/videos/generations` · on `/mcp` · needs the `generate` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `model` | string | yes |  |
| `aspect_ratio` | string | no |  |
| `audio` | boolean | no | `false` for a silent clip, where the model can leave audio out (`supports_audio_toggle`). |
| `camera` | one of `fixed`, `auto` | no | `fixed` for a still camera, where the model takes one (`supports_fixed_camera`, PixelGenie L10). |
| `direction` | string | no | An `extend`: add before (`backward`) or after (`forward`) the source. |
| `duration_seconds` | number | no | Absent means the model's own `duration_seconds` default (resolved by `resolve_duration_seconds`), not a fixed 5s: models whose minimum is higher (fal/ltx-2.3, bedrock nova-reel) rejected every… |
| `end_user` | string | no | Your own customer's id, so spend and budgets can be tracked per customer (sent as x-litegen-end-user). |
| `extra` | any | no |  |
| `fps` | integer | no |  |
| `idempotency_key` | string | no | 1–255 visible ASCII characters. |
| `loop` | boolean | no | `true` for a clip whose last frame is its first (`supports_loop`). |
| `metadata` | object | no | Your own labels for the job: a map of string values, keys of letters, digits, `_`, `.` and `-`, at most 1 KB as JSON. |
| `n` | integer | no |  |
| `negative_prompt` | string | no |  |
| `prompt` | string | no | Whether a prompt is required is decided per model, by its `prompt.required` in `GET /v1/models/{id}` — never by the wire format. |
| `reference_images` | array of object | no |  |
| `replace` | string | no | A `retake`'s track: `video`, `audio` or `both`. |
| `resolution` | string | no |  |
| `seed` | integer | no |  |
| `source_video` | object | no | The clip an operation (`extend`, `retake`, `video_edit`) works on: a litegen generation id, a URL or base64, as `source_model`. |
| `strict` | boolean | no |  |
| `tags` | array of string | no | Spend tags for reporting (sent as x-litegen-tags). |
| `time_range` | object | no | A `retake`'s window, or a `video_edit`'s where the model takes one. |

### `get_video_status`

The current state of a video job: queued, processing, completed (with video_url) or failed.

`GET /v1/videos/{id}` · on `/mcp` · needs the `generate` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Video generation ID |

### `get_current_user`

The account you are acting as, and its organizations.

`GET /v1/auth/me` · on `/mcp` and `/admin/mcp` · needs a signed-in user

No arguments.

### `get_my_limits`

The budgets and limits that apply to you: spend so far in each window and when it resets.

`GET /v1/limits/me` · on `/mcp` and `/admin/mcp` · needs any credential

No arguments.

### `create_app`

Create an application (requires app:write).

`POST /v1/orgs/{id}/apps` · on `/admin/mcp` · needs a signed-in member with `app:write`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization ID |
| `name` | string | yes |  |

### `delete_app`

Delete an application (app:delete via its org).

`DELETE /v1/apps/{app_id}` · on `/admin/mcp` · needs a signed-in member with `app:delete`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | yes | Application ID |

### `delete_app_storage`

remove BYO storage config (storage_cred:delete).

`DELETE /v1/apps/{app_id}/storage` · on `/admin/mcp` · needs a signed-in member with `storage_cred:delete`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | yes | Application ID |

### `get_app`

Get an application; authorizes via its org (app:read).

`GET /v1/apps/{app_id}` · on `/admin/mcp` · needs a signed-in member with `app:read`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | yes | Application ID |

### `get_app_allowed_models`

Which org models this app's keys may call (app read).

`GET /v1/apps/{app_id}/allowed-models` · on `/admin/mcp` · needs a signed-in member with `app:read`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | yes | Application ID |

### `get_app_storage`

read BYO storage config (storage_cred:read). No secret.

`GET /v1/apps/{app_id}/storage` · on `/admin/mcp` · needs a signed-in member with `storage_cred:read`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | yes | Application ID |

### `list_apps`

List applications in an org (requires app:read).

`GET /v1/orgs/{id}/apps` · on `/admin/mcp` · needs a signed-in member with `app:read`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization ID |

### `set_app_allowed_models`

Set this app's model access (app write).

`PUT /v1/apps/{app_id}/allowed-models` · on `/admin/mcp` · needs a signed-in member with `app:write`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | yes | Application ID |
| `mode` | string | yes | Must be "all" or "select". |
| `models` | array of string | no |  |

### `set_app_storage`

upsert BYO storage config (storage_cred:write).

`PUT /v1/apps/{app_id}/storage` · on `/admin/mcp` · needs a signed-in member with `storage_cred:write`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | yes | Application ID |
| `bucket_name` | string | yes |  |
| `access_key_id` | string | no | Write-only. |
| `backend` | string | no |  |
| `custom_public_url` | string | no |  |
| `endpoint_url` | string | no |  |
| `path_prefix` | string | no |  |
| `region` | string | no |  |
| `secret_access_key` | string | no | Write-only. |

### `update_app`

Rename an application (app:write via its org).

`PATCH /v1/apps/{app_id}` · on `/admin/mcp` · needs a signed-in member with `app:write`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | yes | Application ID |
| `name` | string | yes |  |

### `list_audit_log`

List audit log entries (admin scope required). Supports `?format=csv`.

`GET /v1/audit` · on `/admin/mcp` · needs the `admin` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `action` | string | no | Filter by action, e.g. |
| `actor_key_id` | string | no | Filter by actor API key ID |
| `format` | string | no | Set to `csv` for a CSV export |
| `from` | string | no | ISO 8601 lower bound (inclusive) |
| `page` | integer | no | Page number (default 1) |
| `per_page` | integer | no | Items per page (default 50) |
| `to` | string | no | ISO 8601 upper bound (inclusive) |

### `delete_app_limits`

Delete app limits

`DELETE /v1/apps/{app_id}/limits` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `app:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | yes | Application id |

### `delete_key_limits`

Delete key limits

`DELETE /v1/keys/{id}/limits` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `key:write:own`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | API key id |

### `delete_member_limits`

Delete member limits

`DELETE /v1/orgs/{id}/members/{user_id}/limits` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `member:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization id |
| `user_id` | string | yes | Member user id |

### `delete_org_limits`

Delete org limits

`DELETE /v1/orgs/{id}/limits` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `org:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization id |

### `get_app_limits`

Get app limits

`GET /v1/apps/{app_id}/limits` · on `/admin/mcp` · needs org read access (`read` or `admin` scope, or a member role with `app:read`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | yes | Application id |

### `get_key_limits`

Get key limits

`GET /v1/keys/{id}/limits` · on `/admin/mcp` · needs org read access (`read` or `admin` scope, or a member role with `key:read:own`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | API key id |

### `get_member_limits`

Get member limits

`GET /v1/orgs/{id}/members/{user_id}/limits` · on `/admin/mcp` · needs org read access (`read` or `admin` scope, or a member role with `member:read`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization id |
| `user_id` | string | yes | Member user id |

### `get_org_limits`

Get org limits

`GET /v1/orgs/{id}/limits` · on `/admin/mcp` · needs org read access (`read` or `admin` scope, or a member role with `org:read`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization id |

### `list_limits`

Every limits row in the caller's organization.

`GET /v1/limits` · on `/admin/mcp` · needs org read access (`read` or `admin` scope, or a member role with `org:read`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `entity_type` | string | no | key \| org \| app \| member \| end_user |
| `org_id` | string | no | Platform admin only: list another org's limits. |

### `reset_app_limits`

Reset app limits

`POST /v1/apps/{app_id}/limits/reset` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `app:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | yes | Application id |

### `reset_key_limits`

Zero the key's window spend.

`POST /v1/keys/{id}/limits/reset` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `key:write:own`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | API key id |

### `reset_member_limits`

Reset member limits

`POST /v1/orgs/{id}/members/{user_id}/limits/reset` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `member:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization id |
| `user_id` | string | yes | Member user id |

### `reset_org_limits`

Reset org limits

`POST /v1/orgs/{id}/limits/reset` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `org:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization id |

### `set_app_limits`

Budget and caps for one application (LiteLLM team budget).

`PUT /v1/apps/{app_id}/limits` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `app:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | yes | Application id |
| `allowed_models` | array of string | no |  |
| `budget_duration` | string | no |  |
| `max_budget_usd` | number | no |  |
| `max_parallel_requests` | integer | no |  |
| `metadata` | any | no |  |
| `rpm_limit` | integer | no |  |
| `soft_budget_usd` | number | no |  |

### `set_key_limits`

Set the key's budget window, soft budget, model allowlist, rpm and parallel caps. Independent of the key's lifetime `token_quota`; both apply.

`PUT /v1/keys/{id}/limits` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `key:write:own`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | API key id |
| `allowed_models` | array of string | no |  |
| `budget_duration` | string | no |  |
| `max_budget_usd` | number | no |  |
| `max_parallel_requests` | integer | no |  |
| `metadata` | any | no |  |
| `rpm_limit` | integer | no |  |
| `soft_budget_usd` | number | no |  |

### `set_member_limits`

A member's budget inside the org, covering their session generations and every key they own.

`PUT /v1/orgs/{id}/members/{user_id}/limits` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `member:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization id |
| `user_id` | string | yes | Member user id |
| `allowed_models` | array of string | no |  |
| `budget_duration` | string | no |  |
| `max_budget_usd` | number | no |  |
| `max_parallel_requests` | integer | no |  |
| `metadata` | any | no |  |
| `rpm_limit` | integer | no |  |
| `soft_budget_usd` | number | no |  |

### `set_org_limits`

Budget and caps shared by everything in the org.

`PUT /v1/orgs/{id}/limits` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `org:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization id |
| `allowed_models` | array of string | no |  |
| `budget_duration` | string | no |  |
| `max_budget_usd` | number | no |  |
| `max_parallel_requests` | integer | no |  |
| `metadata` | any | no |  |
| `rpm_limit` | integer | no |  |
| `soft_budget_usd` | number | no |  |

### `block_end_user`

Refuse this end user's requests (403 `end_user_blocked`).

`POST /v1/end-users/{end_user_id}/block` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `org:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `end_user_id` | string | yes | End user id |
| `org_id` | string | no | Platform admin only: act on another org. |

### `create_end_user`

Register an end user ahead of its first request, optionally with limits.

`POST /v1/end-users` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `org:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes |  |
| `alias` | string | no |  |
| `blocked` | boolean | no |  |
| `limits` | object | no | Optional limits to set in the same call. |
| `metadata` | any | no |  |
| `org_id` | string | no | Platform admin only: act on another org. |

### `delete_end_user`

Remove the end user and its limits. Its spend history stays in the request logs.

`DELETE /v1/end-users/{end_user_id}` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `org:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `end_user_id` | string | yes | End user id |
| `org_id` | string | no | Platform admin only: act on another org. |

### `delete_end_user_limits`

Delete end user limits

`DELETE /v1/end-users/{end_user_id}/limits` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `org:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `end_user_id` | string | yes | End user id |
| `org_id` | string | no | Platform admin only: act on another org. |

### `get_end_user`

Get end user

`GET /v1/end-users/{end_user_id}` · on `/admin/mcp` · needs org read access (`read` or `admin` scope, or a member role with `org:read`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `end_user_id` | string | yes | End user id |
| `org_id` | string | no | Platform admin only: act on another org. |

### `get_end_user_limits`

Get end user limits

`GET /v1/end-users/{end_user_id}/limits` · on `/admin/mcp` · needs org read access (`read` or `admin` scope, or a member role with `org:read`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `end_user_id` | string | yes | End user id |
| `org_id` | string | no | Platform admin only: act on another org. |

### `list_end_users`

End users seen in, or created for, the organization.

`GET /v1/end-users` · on `/admin/mcp` · needs org read access (`read` or `admin` scope, or a member role with `org:read`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `org_id` | string | no | Platform admin only: act on another org. |

### `reset_end_user_limits`

Reset end user limits

`POST /v1/end-users/{end_user_id}/limits/reset` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `org:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `end_user_id` | string | yes | End user id |
| `org_id` | string | no | Platform admin only: act on another org. |

### `set_end_user_limits`

An end user's budget and caps. Registers the end user if it was never seen.

`PUT /v1/end-users/{end_user_id}/limits` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `org:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `end_user_id` | string | yes | End user id |
| `allowed_models` | array of string | no |  |
| `budget_duration` | string | no |  |
| `max_budget_usd` | number | no |  |
| `max_parallel_requests` | integer | no |  |
| `metadata` | any | no |  |
| `org_id` | string | no | Platform admin only: act on another org. |
| `rpm_limit` | integer | no |  |
| `soft_budget_usd` | number | no |  |

### `unblock_end_user`

Unblock end user

`POST /v1/end-users/{end_user_id}/unblock` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `org:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `end_user_id` | string | yes | End user id |
| `org_id` | string | no | Platform admin only: act on another org. |

### `update_end_user`

Change alias, blocked or metadata.

`PATCH /v1/end-users/{end_user_id}` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `org:write`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `end_user_id` | string | yes | End user id |
| `alias` | string | no |  |
| `blocked` | boolean | no |  |
| `metadata` | any | no |  |
| `org_id` | string | no | Platform admin only: act on another org. |

### `create_gateway_model`

Add a model at runtime (LiteLLM `/model/new`).

The body is a full model schema, the same shape as a `models/*.yaml` row, and passes the same validation. The vendor model called is the part of the id after `<provider>/` (or the provider's `model_mapping`).

`POST /v1/admin/models` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

| Argument | Type | Required | Description |
|---|---|---|---|
| `capabilities` | object | yes |  |
| `display_name` | string | yes |  |
| `id` | string | yes |  |
| `media_type` | one of `image`, `video`, `model3d` | yes |  |
| `pricing` | object | yes |  |
| `prompt` | object | yes | How a model treats the request's `prompt`. |
| `provider` | string | yes |  |
| `deprecated_at` | string | no | When the vendor deprecated the model (`YYYY-MM-DD`); it still serves it . |
| `description` | string | no |  |
| `docs_url` | string | no | The vendor's own API reference page for the endpoint this model calls — the page to re-read when a field mapping is questioned. |
| `extra_allowlist` | array of string | no |  |
| `native_orientation` | object | no | 3D only: the axes the vendor builds its meshes along, declared once a run has shown them. |
| `params` | object | no |  |
| `preserves_unmasked` | one of `exact`, `similar`, `unspecified` | no | Image edits: what the vendor promises about pixels outside the edit area, in its own words. |
| `ref_inputs` | object | no |  |
| `replaced_by` | string | no | The catalog model to move to. |
| `retires_at` | string | no | The vendor's announced shutdown date (`YYYY-MM-DD`). |
| `tags` | array of string | no |  |
| `task` | one of `generate`, `rig`, `animate`, `texture`, `remesh`, `segment`, `multiview`, `background_swap`, … | no | What the model does: `generate`, or an operation on an existing asset such as `rig` or `animate`. |

### `delete_gateway_model`

Remove a runtime model (LiteLLM `/model/delete`).

`DELETE /v1/admin/models/{id}` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Model id (may contain slashes) may contain '/' (send it unescaped). |

### `delete_gateway_provider`

Drop the runtime config; the provider reverts to its env/YAML config, or goes out of service if it has none.

`DELETE /v1/admin/providers/{name}` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Provider adapter name, e.g. |

### `delete_gateway_routes`

Drop the runtime routes and go back to litegen.yaml's.

`DELETE /v1/admin/routes` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

No arguments.

### `get_gateway_model`

Get gateway model

`GET /v1/admin/models/{id}` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Model id (may contain slashes) may contain '/' (send it unescaped). |

### `get_gateway_provider`

Get gateway provider

`GET /v1/admin/providers/{name}` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Provider adapter name, e.g. |

### `get_gateway_routes`

Model routes in effect (fallbacks, load balancing, caching).

`GET /v1/admin/routes` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

No arguments.

### `get_gateway_settings`

Get gateway settings

`GET /v1/admin/settings` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

No arguments.

### `list_gateway_models`

Every model (catalog and runtime, enabled or not).

`GET /v1/admin/models` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

No arguments.

### `list_gateway_providers`

Every provider adapter with its effective config. Secrets are masked.

`GET /v1/admin/providers` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

No arguments.

### `replace_gateway_model`

Replace a runtime model's schema (LiteLLM `/model/update`).

`PUT /v1/admin/models/{id}` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

| Argument | Type | Required | Description |
|---|---|---|---|
| `body` | object | yes |  |
| `id` | string | yes | Model id (may contain slashes) may contain '/' (send it unescaped). |

### `set_gateway_provider`

Set a provider's keys, api_base, model mapping, headers, options or enabled flag at runtime. Secret fields left out keep their stored value; every unset field falls back to env/YAML. In-flight jobs finish on the instance they started with.

`PUT /v1/admin/providers/{name}` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Provider adapter name, e.g. |
| `api_base` | string | no |  |
| `api_key` | string | no |  |
| `api_keys` | string | no |  |
| `enabled` | boolean | no | False takes the provider out of service until re-enabled. |
| `extra_headers` | object | no |  |
| `key_id` | string | no |  |
| `key_secret` | string | no |  |
| `model_mapping` | object | no |  |
| `options` | any | no |  |
| `region` | string | no |  |

### `set_gateway_routes`

Replace every model route at runtime. Order matters: the first route whose pattern matches a model wins.

`PUT /v1/admin/routes` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

| Argument | Type | Required | Description |
|---|---|---|---|
| `routes` | array of object | yes |  |
| `source` | string | no | `config` while the routes come from litegen.yaml, `runtime` once replaced through the API. |

### `set_gateway_settings`

Replace the gateway settings (markup, budget alert webhook, default end-user budget). An omitted field is cleared.

`PUT /v1/admin/settings` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

| Argument | Type | Required | Description |
|---|---|---|---|
| `alert_webhook_url` | string | no | URL that receives budget alerts (soft budget crossed, hard budget hit). |
| `cost_markup_percent` | number | no | Percent added on top of provider cost when billing. |
| `default_end_user_budget_duration` | string | no | Window for that default end-user budget. |
| `default_end_user_max_budget_usd` | number | no | Default hard budget (USD) for an end user with no limits of its own. |

### `update_gateway_model`

Enable or disable any model platform-wide (LiteLLM `/model/block`). Disabled models disappear from `/v1/models` and generation requests for them answer 404 `model_not_found`.

`PATCH /v1/admin/models/{id}` · on `/admin/mcp` · needs the platform admin (single-tenant: owners, admins and `admin` keys)

| Argument | Type | Required | Description |
|---|---|---|---|
| `disabled` | boolean | yes |  |
| `id` | string | yes | Model id (may contain slashes) may contain '/' (send it unescaped). |

### `block_api_key`

Refuse every request made with this key until unblocked.

`POST /v1/keys/{id}/block` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `key:write:own`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | API key id |

### `create_api_key`

Create a new API key.

`POST /v1/keys` · on `/admin/mcp` · needs the `admin` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes |  |
| `expires_at` | string | no | Optional expiry; after this instant the key is rejected at auth. |
| `rpm_limit` | integer | no | Requests-per-minute cap; None = unlimited. |
| `scopes` | string | no | CSV of scopes (default: "generate,read"). |
| `token_quota` | number | no | USD budget cap; None = unlimited. |
| `webhook_url` | string | no | Webhook URL for async callbacks. |

### `get_api_key`

Get a single API key by ID.

`GET /v1/keys/{id}` · on `/admin/mcp` · needs the `admin` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | API key ID |

### `list_api_keys`

List API keys.

`GET /v1/keys` · on `/admin/mcp` · needs the `admin` scope

No arguments.

### `list_webhook_deliveries`

List webhook deliveries for a key (admin scope).

`GET /v1/keys/{id}/webhook-deliveries` · on `/admin/mcp` · needs the `admin` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | API key ID |
| `page` | integer | no | Page number (default 1) |
| `per_page` | integer | no | Items per page (default 50) |

### `revoke_api_key`

Revoke an API key.

`DELETE /v1/keys/{id}` · on `/admin/mcp` · needs the `admin` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | API key ID |

### `rotate_api_key`

Issue a new secret for an existing key in place. Keeps the same row id and all settings; the new `sk_live_` secret is returned once.

`POST /v1/keys/{id}/rotate` · on `/admin/mcp` · needs the `admin` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | API key ID |

### `test_api_key_webhook`

Fire one synthetic webhook and return the result.

`POST /v1/keys/{id}/test-webhook` · on `/admin/mcp` · needs the `admin` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | API key ID |

### `unblock_api_key`

Unblock API key

`POST /v1/keys/{id}/unblock` · on `/admin/mcp` · needs org write access (`admin` scope, or a member role with `key:write:own`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | API key id |

### `update_api_key`

Update an API key's quota/rpm/scopes/etc.

`PATCH /v1/keys/{id}` · on `/admin/mcp` · needs the `admin` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | API key ID |
| `expires_at` | string | no |  |
| `is_active` | boolean | no |  |
| `name` | string | no |  |
| `rpm_limit` | integer | no |  |
| `scopes` | string | no | Replace the key's scopes: the same values and the same privilege check as on create (an unknown scope is a 400, one the caller may not grant a 403). |
| `token_quota` | number | no |  |
| `webhook_url` | string | no |  |

### `get_request_log_artifact`

Retrieve the stored artifact for a request log.

`GET /v1/logs/{id}/artifact` · on `/admin/mcp` · needs the `read` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Request log / generation ID |

### `get_usage_stats`

Get aggregate usage statistics.

`metadata.<key>=<value>` (repeatable) narrows them to the requests whose `metadata` holds every pair: spend per project, per asset.

`GET /v1/stats` · on `/admin/mcp` · needs the `read` scope

No arguments.

### `list_request_logs`

Paginated + filtered request logs. Supports `?format=csv`.

`metadata.<key>=<value>` (repeatable) keeps the requests whose `metadata` holds every pair.

`GET /v1/logs` · on `/admin/mcp` · needs the `read` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `page` | integer | no | Page number |
| `per_page` | integer | no | Items per page |

### `invite_member`

Invite a member to this org (requires member:invite).

`POST /v1/orgs/{id}/members` · on `/admin/mcp` · needs a signed-in member with `member:invite`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `email` | string | yes |  |
| `id` | string | yes | Organization ID |
| `role` | one of `owner`, `admin`, `member`, `viewer` | yes |  |

### `list_members`

List org members (requires member:read membership).

`GET /v1/orgs/{id}/members` · on `/admin/mcp` · needs a signed-in member with `member:read`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization ID |

### `remove_member`

Remove a member (requires member:remove).

`DELETE /v1/orgs/{id}/members/{user_id}` · on `/admin/mcp` · needs a signed-in member with `member:remove`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization ID |
| `user_id` | string | yes | Member user ID |

### `update_member`

Change a member's role (requires member:write).

`PATCH /v1/orgs/{id}/members/{user_id}` · on `/admin/mcp` · needs a signed-in member with `member:write`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization ID |
| `role` | one of `owner`, `admin`, `member`, `viewer` | yes |  |
| `user_id` | string | yes | Member user ID |

### `create_org`

Create an organization owned by the caller (any session user).

`POST /v1/orgs` · on `/admin/mcp` · needs a signed-in user

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes |  |

### `delete_org`

Delete an organization (requires org:delete membership).

`DELETE /v1/orgs/{id}` · on `/admin/mcp` · needs a signed-in member with `org:delete`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization ID |

### `delete_org_provider_credential`

Delete a credential for an org (provider_cred:delete).

`DELETE /v1/orgs/{org_id}/provider-credentials/{provider}` · on `/admin/mcp` · needs a signed-in member with `provider_cred:delete`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `org_id` | string | yes | Organization ID |
| `provider` | string | yes | Provider name |

### `get_org`

Get an organization (requires org:read membership).

`GET /v1/orgs/{id}` · on `/admin/mcp` · needs a signed-in member with `org:read`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization ID |

### `get_org_allowed_models`

The org's model pool (org read).

`GET /v1/orgs/{id}/allowed-models` · on `/admin/mcp` · needs a signed-in member with `org:read`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization ID |

### `list_org_provider_credentials`

List BYO credentials for an org (provider_cred:read). Never returns plaintext — only `ProviderCredentialInfo`.

`GET /v1/orgs/{org_id}/provider-credentials` · on `/admin/mcp` · needs a signed-in member with `provider_cred:read`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `org_id` | string | yes | Organization ID |

### `list_orgs`

List the orgs the caller belongs to (any session user).

`GET /v1/orgs` · on `/admin/mcp` · needs a signed-in user

No arguments.

### `set_org_allowed_models`

Replace the org's model pool (org write).

`PUT /v1/orgs/{id}/allowed-models` · on `/admin/mcp` · needs a signed-in member with `org:write`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization ID |
| `models` | array of string | yes |  |

### `set_org_provider_credential`

Store a BYO credential for an org (provider_cred:write). Encrypts the credential JSON; returns `ProviderCredentialInfo` (no plaintext).

`POST /v1/orgs/{org_id}/provider-credentials` · on `/admin/mcp` · needs a signed-in member with `provider_cred:write`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `credentials` | any | yes | The provider's secret fields, e.g. |
| `org_id` | string | yes | Organization ID |
| `provider` | string | yes |  |

### `transfer_org_ownership`

Transfer ownership (requires org:transfer_owner).

`POST /v1/orgs/{id}/transfer-owner` · on `/admin/mcp` · needs a signed-in member with `org:transfer_owner`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization ID |
| `new_owner_user_id` | string | yes |  |

### `update_org`

Rename an organization (requires org:write membership).

`PATCH /v1/orgs/{id}` · on `/admin/mcp` · needs a signed-in member with `org:write`, or the platform admin

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Organization ID |
| `name` | string | yes |  |

### `get_spend_report`

Spend and request counts grouped by one dimension.

`GET /v1/spend/report` · on `/admin/mcp` · needs org read access (`read` or `admin` scope, or a member role with `org:read`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | no |  |
| `end_user` | string | no |  |
| `from` | string | no | Inclusive lower bound: RFC 3339 timestamp or `YYYY-MM-DD`. |
| `group_by` | string | no | day \| model \| provider \| media_type \| status \| org \| app \| key \| user \| end_user \| tag (report only; default day) |
| `key_id` | string | no |  |
| `model` | string | no |  |
| `org_id` | string | no | Platform admin only: another org. |
| `page` | integer | no | Spend logs only (default 1). |
| `per_page` | integer | no | Spend logs only (default 50, max 500). |
| `provider` | string | no |  |
| `tag` | string | no |  |
| `to` | string | no | Inclusive upper bound: RFC 3339 timestamp or `YYYY-MM-DD` (the whole day). |
| `user_id` | string | no |  |

### `list_spend_logs`

Attributed request log (key, user, end user, tags), newest first.

`GET /v1/spend/logs` · on `/admin/mcp` · needs org read access (`read` or `admin` scope, or a member role with `org:read`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `app_id` | string | no |  |
| `end_user` | string | no |  |
| `from` | string | no | Inclusive lower bound: RFC 3339 timestamp or `YYYY-MM-DD`. |
| `group_by` | string | no | day \| model \| provider \| media_type \| status \| org \| app \| key \| user \| end_user \| tag (report only; default day) |
| `key_id` | string | no |  |
| `model` | string | no |  |
| `org_id` | string | no | Platform admin only: another org. |
| `page` | integer | no | Spend logs only (default 1). |
| `per_page` | integer | no | Spend logs only (default 50, max 500). |
| `provider` | string | no |  |
| `tag` | string | no |  |
| `to` | string | no | Inclusive upper bound: RFC 3339 timestamp or `YYYY-MM-DD` (the whole day). |
| `user_id` | string | no |  |

### `clear_cache`

Clear the generation cache.

`DELETE /v1/cache` · on `/admin/mcp` · needs the `admin` scope

No arguments.

### `get_provider_health`

Health check for all providers.

`GET /health` · on `/admin/mcp` · needs the `read` scope

No arguments.

### `delete_user`

Soft-delete (deactivate) a user (requires user:delete:any).

`DELETE /v1/users/{id}` · on `/admin/mcp` · needs the platform admin (single-tenant: a user with `user:delete:any`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | User ID |

### `invite_user`

Invite a user (requires invitation:send).

`POST /v1/users` · on `/admin/mcp` · needs a signed-in user with `invitation:send`

| Argument | Type | Required | Description |
|---|---|---|---|
| `email` | string | yes |  |
| `role` | one of `owner`, `admin`, `member`, `viewer` | yes |  |

### `list_users`

List all users (requires user:read:any).

`GET /v1/users` · on `/admin/mcp` · needs the platform admin (single-tenant: a user with `user:read:any`)

No arguments.

### `transfer_platform_ownership`

Transfer ownership to another user (Owner only).

`POST /v1/users/transfer-owner` · on `/admin/mcp` · needs the platform admin (single-tenant: a user with `system:transfer_owner`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `new_owner_id` | string | yes |  |

### `update_user`

Update user role or active status (requires user:write:any).

`PATCH /v1/users/{id}` · on `/admin/mcp` · needs the platform admin (single-tenant: a user with `user:write:any`)

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | User ID |
| `is_active` | boolean | no |  |
| `role` | one of `owner`, `admin`, `member`, `viewer` | no |  |

### `wait_for_generation`

Wait for a video, 3D or other generation to finish, up to timeout_seconds (default 45, max 230). Returns the final result, or the current progress if it is still running; then call it again.

polls the job's status route · on `/mcp` · needs the `generate` or `read` scope

| Argument | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | The job id generate_video, generate_3d_model or another generation returned. |
| `timeout_seconds` | integer | no | How long to wait before returning the job's current state. |

