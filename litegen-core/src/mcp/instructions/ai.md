LiteGen generates images, video and 3D models through one API. To generate: 1) list_models (media_type image, video or model3d), 2) get_model for that model's parameters, 3) optionally estimate_image_cost / estimate_video_cost / estimate_3d_model_cost, 4) generate_image (returns images) or generate_video / generate_3d_model (return a job id), 5) wait_for_generation until it completes. Every call spends the budget of the key or account you connected with.

More:
- generate_image returns URLs plus small previews; pass preview: false to skip the previews.
- Video and 3D jobs take seconds to minutes. wait_for_generation waits up to timeout_seconds (default 45) and says when to call it again. list_generation_assets and get_generation_asset fetch a finished job's files.
- Each model takes its own parameters: read them with get_model. With strict: false, LiteGen drops parameters a model cannot use instead of refusing the call, and says which it dropped.
- Send idempotency_key on any generate call you might retry, so a retry never bills twice.
- end_user and tags attribute spend to your own customers and labels. Calls made through MCP are also tagged mcp.
- Errors come back as tool errors with LiteGen's HTTP status and code (for example budget_exceeded or model_not_allowed). Read the message, then change the call or tell the user.
- convert_3d_model converts a mesh between formats on LiteGen itself, with no AI model and no charge.
- get_my_limits shows the budgets that apply to you and what is left in each window.
- Only tools your credential can use are listed. If generation tools are missing, the key lacks the generate scope.
