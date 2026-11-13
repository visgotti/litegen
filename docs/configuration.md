# Configuration: where litegen reads its settings, and which one wins

litegen reads its settings once, at startup, from a config file and from
environment variables. This page says where it looks, which source wins when
two set the same setting, and how values are read. What the admin API and the
dashboard change while litegen runs is in
[gateway-settings.md](gateway-settings.md) and
[management-api.md](management-api.md).

## The config file

### Found in the working directory

Without a name (below), litegen looks in the directory it runs in for
`litegen.yaml`, or `litegen.<extension>` in another format it reads: `yml`,
`toml`, `json`, `json5`, `ini` or `ron`. It never takes the bare `litegen`,
which is the binary's own name in the Docker image (`/app/litegen`, beside
`/app/litegen.yaml`).

- **No file:** litegen starts on its built-in defaults and the environment.
- **One file:** it is read.
- **`litegen.yaml` and `litegen.yml`:** `litegen.yaml` is read, and a startup
  warning names `litegen.yml`.
- **Files in two formats** (`litegen.yaml` and `litegen.json`, say): litegen
  refuses to start and names them. Keep one, or name it.

### Named

| How | Example |
|---|---|
| `--config <file>`, `--config=<file>`, or LiteLLM's `-c <file>` | `litegen --config /etc/litegen/litegen.yaml` |
| `LITEGEN_CONFIG_FILE_PATH` | LiteLLM's `CONFIG_FILE_PATH`, with litegen's prefix. LiteLLM's bare name isn't read. |
| `LITEGEN_CONFIG_FILE` | The same setting, under the name the runbook once used. |

- A relative name starts in the working directory.
- A blank variable counts as unset, so compose's `${LITEGEN_CONFIG_FILE_PATH:-}`
  is harmless.
- If both variables are set to different files, litegen refuses to start and
  names both. `--config` beats them, and a warning names the variable it
  ignored.
- A named file must exist: otherwise litegen refuses to start, naming the file
  and what named it. (LiteLLM silently ignores a missing `CONFIG_FILE_PATH`.)
- The extension gives the format. A name with none, or with any other, is read
  as YAML, LiteLLM's format: a Kubernetes ConfigMap key such as `config` works.

`--config` is litegen's only argument. `litegen --help` prints the usage. Any
other argument, such as LiteLLM's `--port`, stops litegen with exit code 2,
rather than being ignored. Every other setting comes from the file or a
variable.

### A file that can't be read stops litegen

A file that is there but can't be used refuses to start, naming it, instead of
leaving litegen on its defaults (which would drop everything the file set, from
the bind address to the master key):

- a directory. Docker creates one when it mounts a file that doesn't exist on
  the host: `docker run -v ./litegen.yaml:/app/litegen.yaml` with no
  `./litegen.yaml`;
- a link to nothing;
- a file litegen may not read;
- a file that isn't valid in its format.

Links are followed, so a Kubernetes ConfigMap (`litegen.yaml -> ..data/litegen.yaml`)
works. The startup log says which file was read, or that there was none (at
info level: `config file: …`).

### Running it

- **From source:** the repository's quick start copies the example to the
  repository root, beside `models/`, so run the binary there
  (`./litegen-core/target/release/litegen`). From another directory, name both:
  `--config <file>`, and `LITEGEN_MODELS_DIR` for the models directory.
- **Docker:** mount the file over `/app/litegen.yaml`, or mount a folder and
  name the file:

  ```bash
  docker run -v "$PWD/litegen.yaml:/app/litegen.yaml:ro" -p 4000:4000 litegen
  docker run -v "$PWD/conf:/etc/litegen:ro" -p 4000:4000 litegen --config /etc/litegen/litegen.yaml
  ```

  A variable beats the file (below), so a compose file's `environment:` entry
  for a setting wins over a mounted file's value for it. The image itself sets
  no litegen setting. More on the image: [the README](../README.md#run-the-docker-image).

### Hook paths

A hook's `path` (`docs/hooks.md`) starts in the folder of the config file that
declares it, as LiteLLM's custom callbacks do. For a `litegen.yaml` found in
the working directory, that is the working directory.

## `os.environ/NAME` in the file

A value in the file whose whole text is `os.environ/NAME` takes the text of the
environment variable `NAME`, as in LiteLLM:

```yaml
master_key: os.environ/LITEGEN_MASTER_SECRET
providers:
  openai:
    api_key: os.environ/OPENAI_API_KEY
hooks:
  - path: hooks/custom_auth.py
    env:
      ACME_TOKENS: os.environ/ACME_TOKENS
```

- It works anywhere in the file, in nested sections and lists too.
- The value is the variable's text exactly as written. An empty variable is an
  empty value.
- A variable that isn't set stops litegen, naming the setting, the file and the
  variable (never a value). A setting such as `storage_access.mode` must never
  fall back to its default unnoticed. (LiteLLM uses no value there.)
- References are read in the file only, not in variables.

In YAML, an unquoted value that looks like a number is a number:
`master_key: 0012345` is the number 12345. Quote such a value
(`master_key: "0012345"`), or take it from a variable with `os.environ/`.

## Which source wins

Lowest first. A higher source wins for each setting it sets:

1. The built-in defaults.
2. The config file, with its `os.environ/` references read.
3. The vendors' own variables: `OPENAI_API_KEY`, `FAL_KEY`, `REPLICATE_API_TOKEN`
   and the rest of the README's
   [environment variable reference](../README.md#environment-variable-reference).
   A blank one counts as unset, so compose's `${OPENAI_API_KEY:-}` never blanks
   a key set elsewhere. Of two names for one setting, the later wins:
   `ARK_API_KEY` over `BYTEDANCE_API_KEY`.
4. `LITEGEN__` variables: `LITEGEN__PROVIDERS__OPENAI__API_KEY` sets
   `providers.openai.api_key`, each `__` a level down. An empty one counts as
   set: `LITEGEN__PROVIDERS__MOCK__API_KEY=` turns the mock vendor on.
5. litegen's own single-underscore names for storage: `LITEGEN_S3_BUCKET`,
   `LITEGEN_S3_REGION`, `LITEGEN_S3_ENDPOINT_URL`, `LITEGEN_S3_ACCESS_KEY_ID`,
   `LITEGEN_S3_SECRET_ACCESS_KEY`, `LITEGEN_ASSET_DIR` and
   `LITEGEN_ASSET_RETENTION_DAYS`. A blank one counts as unset.

The database and Redis come after these, from their own variables
(`LITEGEN_DATABASE_URL` beats `LITEGEN__DATABASE_URL`, which beats the parts and
the file): [scaling.md](scaling.md#connection-settings). While litegen runs,
what the admin API or the dashboard stored (provider configs, routes, gateway
settings) beats all of these.

With no database setting at all, the database is SQLite in `data/litegen.db`
in the working directory (`sqlite://data/litegen.db?mode=rwc`), beside the kept
files in `data/assets`, and litegen creates it, and its directory, on the first
start. In the Docker image that is `/app/data/litegen.db`, inside the
`/app/data` volume. The old default was `sqlite://litegen.db`, a file litegen
could not create: a `litegen.db` in the working directory is still used, with a
startup warning, until `data/litegen.db` exists. Any SQLite URL with
`?mode=rwc` creates its file and directory; without it the file must exist. A
SQLite database litegen can't open, or can't write when its migrations run,
stops it at startup with an error that names the file. A database URL with
another scheme, such as `mysql://`, refuses to start, naming only the scheme
([scaling.md](scaling.md#connection-settings)).

When a source hides a different value from a lower one, a startup warning
names both, never the values. For example:
`OPENAI_API_KEY is ignored because LITEGEN__PROVIDERS__OPENAI__API_KEY is set`,
or `providers.openai.api_key in litegen.yaml is ignored because OPENAI_API_KEY is set`.
A lower value that is blank, or the same, isn't named.

S3 storage reads no `AWS_*` variable into the config. With no key set
(`LITEGEN_S3_ACCESS_KEY_ID` or `image_storage.s3.access_key_id`), the S3 client
takes its credentials from the AWS chain, in this order: `AWS_ACCESS_KEY_ID`,
`AWS_SECRET_ACCESS_KEY` and `AWS_SESSION_TOKEN`; then a profile; then web
identity, when `AWS_ROLE_ARN` and `AWS_WEB_IDENTITY_TOKEN_FILE` are set (EKS);
then a container or instance role. A blank key, secret or session token counts
as unset, wherever it is set, so `access_key_id: ""` in the file leaves the
chain to sign. With `image_storage.backend: s3`, litegen refuses to start
without a bucket (naming `LITEGEN_S3_BUCKET`), or with a key but no secret
(naming `LITEGEN_S3_SECRET_ACCESS_KEY`): the chain never fills in a configured
key's secret.

## How values are read

A variable's value is taken exactly as written: a key `0012345`, `True` or
`1e5` stays as it is. A setting that is a number or a switch reads it from
text:

- switches take `true` or `false`, `1` or `0`, `yes` or `no`, `on` or `off`;
- a whole-number setting refuses a fraction (`LITEGEN__SERVER__PORT=4000.0`);
- a value of the wrong kind, or out of a setting's range, stops litegen with a
  message that names the setting.

Three settings are lists, and a variable gives them as a comma list, trimmed,
with empty items dropped:

| setting | variable |
|---|---|
| `cors.allowed_origins` | `LITEGEN__CORS__ALLOWED_ORIGINS=https://a.example,https://b.example` (or `*`) |
| `mcp.allowed_hosts` | `LITEGEN__MCP__ALLOWED_HOSTS` |
| `mcp.allowed_origins` | `LITEGEN__MCP__ALLOWED_ORIGINS` |

`LITEGEN_CORS_ORIGINS`, the same comma list, replaces the CORS list when set.
The other lists, `model_routes` and `hooks`, are set in the file only.

## Variables that only the environment sets

These are read from litegen's environment (or `.env`) and nowhere else.
Written in the config file they do nothing, and `os.environ/` can't feed them:

| variable | what it does |
|---|---|
| `LITEGEN__ALLOW_NO_AUTH` | `true` starts single-tenant litegen with no master key on a public address (README, security note). |
| `LITEGEN__OAUTH__GITHUB__CLIENT_ID`, `LITEGEN__OAUTH__GITHUB__CLIENT_SECRET`, `LITEGEN__OAUTH__GOOGLE__CLIENT_ID`, `LITEGEN__OAUTH__GOOGLE__CLIENT_SECRET`, `LITEGEN__OAUTH__CALLBACK_BASE` | The dashboard's GitHub and Google sign-in. |
| `LITEGEN__COOKIE_INSECURE_DEV` | `true` drops `Secure` from the sign-in cookies, for local HTTP only. |
| `LITEGEN__OWNER_EMAIL` | The only email that may sign up as a single-tenant deployment's Owner. |
| `LITEGEN__DEV__EXPOSE_RESET_TOKENS` | `true` returns password-reset tokens in API answers (development only). `dev.expose_reset_tokens` in the file does nothing. |
| `LITEGEN_CORS_ORIGINS` | The CORS origins, a comma list (above). |
| `LITEGEN_MODELS_DIR` | The model registry directory (default `models`). |
| `OTEL_EXPORTER_OTLP_ENDPOINT`, `RUST_LOG` | Trace export, and the log filter. |

## `.env`

litegen loads a `.env` file from the directory it runs in (or the nearest
folder above it that has one) before it reads anything else. A variable already
set in the environment beats the same one in `.env`.

## What needs a restart

The file and the variables are read once, at startup. A change to either takes
a restart: `SIGTERM` (what `docker stop` and Kubernetes send) or ctrl-c drains
in-flight requests first, within the shutdown grace (`server.shutdown_grace_secs`;
see [gateway-settings.md](gateway-settings.md#server-limits-set-at-startup)).
`SIGHUP` reloads nothing; it stops litegen at once, with no drain.

Routes, provider configs, gateway settings and runtime models can change while
litegen runs, through the admin API (`PUT /v1/admin/routes`,
`PUT /v1/admin/providers/{name}`, `PUT /v1/admin/settings`) or the dashboard.
Every replica picks a change up within 15 seconds, or about a second with Redis
([scaling.md](scaling.md)).
