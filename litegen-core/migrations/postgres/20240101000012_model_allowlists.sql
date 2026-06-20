CREATE TABLE org_allowed_models (
    org_id   TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    PRIMARY KEY (org_id, model_id)
);

CREATE TABLE app_model_access (
    app_id TEXT NOT NULL PRIMARY KEY REFERENCES applications(id) ON DELETE CASCADE,
    mode   TEXT NOT NULL DEFAULT 'all'
);

CREATE TABLE app_model_access_ids (
    app_id   TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    PRIMARY KEY (app_id, model_id)
);
