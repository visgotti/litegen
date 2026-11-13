//! Where litegen's settings come from, and which one wins
//! (`docs/configuration.md`).

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{connections, load_provider_env_overrides, AppConfig};

/// What litegen was started with: `litegen [--config <file>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    /// The config file to read (`--config <file>` or `-c <file>`).
    pub config: Option<PathBuf>,
    /// `--help` or `-h`: print the usage and exit.
    pub help: bool,
}

/// What `litegen --help` prints, and what follows a refused command line.
pub const USAGE: &str = "\
Usage: litegen [--config <file>]

Options:
  -c, --config <file>  Read this config file instead of looking for litegen.yaml
                       in the working directory. Its extension gives its format
                       (yaml, yml, toml, json, json5, ini, ron); with none or any
                       other it is read as YAML. The environment variable
                       LITEGEN_CONFIG_FILE_PATH names it too.
  -h, --help           Print this help.

Every setting can also come from an environment variable: docs/configuration.md.
";

/// The command line, without the program's name. litegen takes `--config
/// <file>` (also `--config=<file>`, and LiteLLM's `-c <file>`) and `--help`;
/// anything else is an error, so that a mistyped or foreign flag (LiteLLM's
/// `--port`) is never silently ignored. Naming the file twice is an error too.
pub fn parse_args<I: IntoIterator<Item = OsString>>(args: I) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let Some(text) = arg.to_str() else {
            return Err(format!("unknown argument {}: litegen takes --config <file> and --help", arg.to_string_lossy()));
        };
        let file = match text {
            "-h" | "--help" => {
                parsed.help = true;
                continue;
            }
            "-c" | "--config" => match args.next() {
                Some(file) => PathBuf::from(file),
                None => return Err(format!("{text} needs a file: --config <file>")),
            },
            _ => match text.strip_prefix("--config=") {
                Some(file) => PathBuf::from(file),
                None => return Err(format!("unknown argument {text}: litegen takes --config <file> and --help")),
            },
        };
        if file.as_os_str().is_empty() {
            return Err("--config needs a file: --config <file>".to_string());
        }
        if parsed.config.is_some() {
            return Err("--config is given more than once: name one config file".to_string());
        }
        parsed.config = Some(file);
    }
    Ok(parsed)
}

/// A setting from an environment variable: the path it fills, as config-rs
/// writes it (`providers.openai.api_key`), the variable's name, and its text.
#[derive(Debug, Clone)]
pub(super) struct Setting {
    pub(super) path: String,
    pub(super) variable: String,
    pub(super) value: String,
}

/// The variables of `table` (a variable's name, then the path it fills) that
/// are set and not blank.
pub(super) fn named_settings(env: &HashMap<String, String>, table: &[(&str, &str)]) -> Vec<Setting> {
    table
        .iter()
        .filter_map(|(variable, path)| {
            let value = env.get(*variable).filter(|v| !v.trim().is_empty())?;
            Some(Setting { path: path.to_string(), variable: variable.to_string(), value: value.clone() })
        })
        .collect()
}

/// litegen's own single-underscore names for storage settings. They rank
/// above the `LITEGEN__` form of the same setting, as `LITEGEN_DATABASE_URL`
/// beats `LITEGEN__DATABASE_URL` (`docs/scaling.md`), so an image's
/// `ENV LITEGEN__IMAGE_STORAGE__LOCAL_DIR` can't beat an operator's
/// `LITEGEN_ASSET_DIR`. A blank one counts as unset.
const LITEGEN_ALIASES: &[(&str, &str)] = &[
    ("LITEGEN_S3_ACCESS_KEY_ID", "image_storage.s3.access_key_id"),
    ("LITEGEN_S3_SECRET_ACCESS_KEY", "image_storage.s3.secret_access_key"),
    ("LITEGEN_S3_BUCKET", "image_storage.s3.bucket_name"),
    ("LITEGEN_S3_REGION", "image_storage.s3.region"),
    ("LITEGEN_S3_ENDPOINT_URL", "image_storage.s3.endpoint_url"),
    ("LITEGEN_ASSET_DIR", "image_storage.local_dir"),
    ("LITEGEN_ASSET_RETENTION_DAYS", "image_storage.retention_days"),
];

/// The `LITEGEN__` variables, each filling the setting its name spells:
/// `LITEGEN__PROVIDERS__OPENAI__API_KEY` is `providers.openai.api_key`. An
/// empty value is still a value (`LITEGEN__PROVIDERS__MOCK__API_KEY=` turns
/// the mock vendor on).
fn litegen_variables(env: &HashMap<String, String>) -> Vec<Setting> {
    let mut settings: Vec<Setting> = env
        .iter()
        .filter_map(|(variable, value)| {
            let lower = variable.to_lowercase();
            let path = lower.strip_prefix("litegen__")?.replace("__", ".");
            Some(Setting { path, variable: variable.clone(), value: value.clone() })
        })
        .collect();
    // One name in two letter cases is rare; a fixed order picks the same one each time.
    settings.sort_by(|a, b| a.variable.cmp(&b.variable));
    settings
}

/// The settings that are lists of text, which a variable gives as a comma
/// list: `LITEGEN__CORS__ALLOWED_ORIGINS=https://a.example,https://b.example`.
/// These are every `Vec<String>` setting of `AppConfig`; a new one goes here.
const LIST_SETTINGS: [&str; 4] =
    ["cors.allowed_origins", "mcp.allowed_hosts", "mcp.allowed_origins", "mcp.oauth.trusted_redirect_hosts"];

/// A comma list's items: split on commas, trimmed, the empty ones dropped.
fn list_items(text: &str) -> Vec<&str> {
    text.split(',').map(str::trim).filter(|item| !item.is_empty()).collect()
}

impl Setting {
    fn is_list(&self) -> bool {
        LIST_SETTINGS.contains(&self.path.as_str())
    }

    /// The value as config-rs gets it: text, or a list of texts.
    fn value(&self) -> config::Value {
        if self.is_list() {
            let items = list_items(&self.value).into_iter().map(|item| config::Value::new(Some(&self.variable), item)).collect::<Vec<_>>();
            config::Value::new(Some(&self.variable), config::ValueKind::Array(items))
        } else {
            config::Value::new(Some(&self.variable), self.value.clone())
        }
    }

    /// The value as text, to compare with another source's ([`text_of`]).
    fn text(&self) -> String {
        if self.is_list() {
            list_items(&self.value).join(",")
        } else {
            self.value.clone()
        }
    }
}

/// Settings from environment variables as one config-rs source, each value
/// with its variable as its origin, so an error names the variable.
///
/// Every value stays text. config-rs turns text into the type each setting
/// has when it deserializes (`server.port`, a switch, a choice), whereas its
/// own environment source with `try_parsing` guessed a type for every value
/// first, which rewrote text settings: a key `0012345` became `12345`. Only
/// [`LIST_SETTINGS`] are split, into lists of text.
#[derive(Debug, Clone)]
struct Layer(Vec<Setting>);

impl config::Source for Layer {
    fn clone_into_box(&self) -> Box<dyn config::Source + Send + Sync> {
        Box::new(self.clone())
    }

    fn collect(&self) -> Result<config::Map<String, config::Value>, config::ConfigError> {
        // Of two settings for one path, the later wins.
        Ok(self.0.iter().map(|s| (s.path.clone(), s.value())).collect())
    }
}

/// A value's text, to compare with a variable's. A list is its items joined
/// by commas; a table has none, so it never matches a variable.
fn text_of(value: &config::Value) -> Option<String> {
    match &value.kind {
        config::ValueKind::Nil => Some(String::new()),
        config::ValueKind::Table(_) => None,
        config::ValueKind::Array(items) => items.iter().map(text_of).collect::<Option<Vec<_>>>().map(|items| items.join(",")),
        _ => value.clone().into_string().ok(),
    }
}

/// The value at `path` (dotted, in lower case as config-rs keeps keys) in
/// `tree`, with its origin.
fn value_at<'a>(tree: &'a config::Value, path: &str) -> Option<&'a config::Value> {
    path.split('.').try_fold(tree, |node, key| match &node.kind {
        config::ValueKind::Table(map) => map.get(key),
        _ => None,
    })
}

/// One startup warning per setting a higher source hides from a lower one
/// that set a different value, not blank (the higher one may be blank). It
/// names the sources, never a value. `file` is the config file alone, whose
/// values name the file as their origin.
fn hidden_settings(file: &config::Config, layers: &[&[Setting]]) -> Vec<String> {
    let mut by_path: BTreeMap<&str, Vec<(String, Option<String>)>> = BTreeMap::new();
    for layer in layers {
        for s in layer.iter() {
            by_path.entry(s.path.as_str()).or_default().push((s.variable.clone(), Some(s.text())));
        }
    }
    let mut warnings = Vec::new();
    for (path, mut sources) in by_path {
        if let Some(value) = value_at(&file.cache, path) {
            let name = format!("{path} in {}", value.origin().unwrap_or("the config file"));
            sources.insert(0, (name, text_of(value)));
        }
        let Some((winner, won)) = sources.pop() else { continue };
        let mut losers: Vec<&str> = Vec::new();
        for (name, text) in &sources {
            let blank = text.as_deref().is_some_and(|t| t.trim().is_empty());
            if !blank && *text != won && !losers.contains(&name.as_str()) {
                losers.push(name);
            }
        }
        if !losers.is_empty() {
            warnings.push(connections::ignored(&losers, &winner));
        }
    }
    warnings
}

/// The `image_storage` section, merged from every source, checked once:
/// storage on S3 needs a bucket, so litegen refuses to start without one,
/// rather than keep files on local disk where no other replica sees them.
/// While storage is not on S3, an `s3` section that names no bucket (one
/// `LITEGEN_S3_REGION`, say) is unused, so it is dropped with a warning.
///
/// A blank key, secret or session token is no key, wherever it was set (the
/// old example's `access_key_id: ""`, a blank `LITEGEN__` variable): the S3
/// client then takes its credentials from the AWS chain. aws-creds takes any
/// key it is given as the key, so an empty one would sign every request. On
/// S3, a key without its secret refuses to start: rust-s3 would send its
/// requests unsigned, and panic presigning a link.
fn settle_image_storage(config: &mut AppConfig) -> Result<(), config::ConfigError> {
    let storage = &mut config.image_storage;
    if let Some(s3) = storage.s3.as_mut() {
        for key in [&mut s3.access_key_id, &mut s3.secret_access_key, &mut s3.session_token] {
            if key.as_deref().is_some_and(|k| k.trim().is_empty()) {
                *key = None;
            }
        }
    }
    let bucket = storage.s3.as_ref().map(|s3| s3.bucket_name.trim()).filter(|b| !b.is_empty());
    if storage.backend == "s3" {
        if bucket.is_none() {
            return Err(config::ConfigError::Message(
                "image_storage.backend is s3 but no bucket is named: set LITEGEN_S3_BUCKET or image_storage.s3.bucket_name".to_string(),
            ));
        }
        // The AWS chain reads AWS_SECRET_ACCESS_KEY only beside
        // AWS_ACCESS_KEY_ID, never for a key set here.
        if storage.s3.as_ref().is_some_and(|s3| s3.access_key_id.is_some() && s3.secret_access_key.is_none()) {
            return Err(config::ConfigError::Message(
                "image_storage.s3 has an access key but no secret (a blank one counts as unset): set LITEGEN_S3_SECRET_ACCESS_KEY or image_storage.s3.secret_access_key, or remove the key (LITEGEN_S3_ACCESS_KEY_ID or image_storage.s3.access_key_id) so the AWS credential chain signs".to_string(),
            ));
        }
    } else if storage.s3.is_some() && bucket.is_none() {
        let backend = storage.backend.clone();
        storage.s3 = None;
        config.startup_warnings.push(format!(
            "image_storage.s3 is ignored: it names no bucket (LITEGEN_S3_BUCKET or image_storage.s3.bucket_name), and image_storage.backend is {backend}, not s3"
        ));
    }
    Ok(())
}

/// The formats a config file can be in, by extension, in the order litegen
/// looks for `litegen.<extension>` in its working directory.
const FORMATS: &[(&str, config::FileFormat)] = &[
    ("yaml", config::FileFormat::Yaml),
    ("yml", config::FileFormat::Yaml),
    ("toml", config::FileFormat::Toml),
    ("json", config::FileFormat::Json),
    ("json5", config::FileFormat::Json5),
    ("ini", config::FileFormat::Ini),
    ("ron", config::FileFormat::Ron),
];

fn format_name(format: config::FileFormat) -> &'static str {
    match format {
        config::FileFormat::Yaml => "YAML",
        config::FileFormat::Toml => "TOML",
        config::FileFormat::Json => "JSON",
        config::FileFormat::Json5 => "JSON5",
        config::FileFormat::Ini => "INI",
        config::FileFormat::Ron => "RON",
    }
}

/// The config file litegen reads, parsed once: every reader of the file
/// (the loader, and the checks that decide how a failed load is reported)
/// sees the same file.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    /// Where it is, as litegen opened it.
    pub path: PathBuf,
    /// How it was read: by its extension, and YAML for any other or none.
    pub format: config::FileFormat,
    /// The file as messages name it: `litegen.yaml` when found in the working
    /// directory, else the path as given.
    pub name: String,
    /// Its settings.
    tree: config::Map<String, config::Value>,
    /// What litegen noticed on the way (a candidate it skipped), for the
    /// startup log.
    warnings: Vec<String>,
}

impl ConfigFile {
    /// The file alone, without defaults or variables.
    fn alone(&self) -> Result<config::Config, config::ConfigError> {
        config::Config::builder().add_source(Tree(self.tree.clone())).build()
    }

    /// The value at `key` (a dotted path) in this file alone; `Ok(None)` when
    /// the file doesn't set it.
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>, config::ConfigError> {
        match self.alone()?.get::<T>(key) {
            Ok(value) => Ok(Some(value)),
            Err(config::ConfigError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// A config file's settings as a config-rs source.
#[derive(Debug, Clone)]
struct Tree(config::Map<String, config::Value>);

impl config::Source for Tree {
    fn clone_into_box(&self) -> Box<dyn config::Source + Send + Sync> {
        Box::new(self.clone())
    }

    fn collect(&self) -> Result<config::Map<String, config::Value>, config::ConfigError> {
        Ok(self.0.clone())
    }
}

/// Whether there is anything at `path`: a file, a directory, or a link,
/// even one to nothing (which must refuse, not count as no file).
fn entry_exists(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// The config file found in a directory, and a warning for each other
/// candidate it beat.
struct Found {
    path: PathBuf,
    name: String,
    format: config::FileFormat,
    warnings: Vec<String>,
}

/// `litegen.<extension>` in `dir`, for each format litegen reads. Never the
/// bare `litegen`: in the image that is the binary itself (`/app/litegen`,
/// beside `/app/litegen.yaml`). One file is read. `litegen.yaml` beside
/// `litegen.yml` reads `litegen.yaml` and names the other in a warning; two
/// files in different formats refuse, since neither can be chosen safely.
fn find_config_file(dir: &Path) -> Result<Option<Found>, String> {
    let found: Vec<(String, config::FileFormat)> = FORMATS
        .iter()
        .map(|(extension, format)| (format!("litegen.{extension}"), *format))
        .filter(|(name, _)| entry_exists(&dir.join(name)))
        .collect();
    let Some((name, format)) = found.first().cloned() else { return Ok(None) };
    let others: Vec<&str> = found[1..].iter().map(|(name, _)| name.as_str()).collect();
    let mut warnings = Vec::new();
    if found.iter().any(|(_, f)| *f != format) {
        let all: Vec<&str> = found.iter().map(|(name, _)| name.as_str()).collect();
        return Err(format!(
            "{} are all in {}, and litegen reads one config file: remove all but one, or name the one to read with --config or LITEGEN_CONFIG_FILE_PATH",
            connections::list(&all, "and"),
            dir.display()
        ));
    }
    if !others.is_empty() {
        warnings.push(format!("{} is ignored: litegen reads {name}", connections::list(&others, "and")));
    }
    Ok(Some(Found { path: dir.join(&name), name, format, warnings }))
}

/// Read and parse the config file at `path`, named `name` in messages. A
/// link is followed (a Kubernetes ConfigMap mounts its keys as links). Any
/// file that can't be read refuses to start: a skipped file would leave
/// litegen on its defaults, from the bind address to the master key.
fn read_config_file(path: &Path, name: &str, format: config::FileFormat) -> Result<config::Map<String, config::Value>, String> {
    let metadata = std::fs::metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => match std::fs::read_link(path) {
            Ok(target) => format!("the config file {name} is a link to {}, which does not exist", target.display()),
            Err(_) => format!("the config file {name} does not exist"),
        },
        _ => format!("the config file {name} cannot be read: {e}"),
    })?;
    if metadata.is_dir() {
        return Err(format!(
            "the config file {name} is a directory, not a file. Docker makes one when it mounts a file that does not exist on the host: create the file there first, or mount the right path"
        ));
    }
    if !metadata.is_file() {
        return Err(format!("the config file {name} is not a regular file"));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("the config file {name} cannot be read: {e}"))?;
    config::Format::parse(&format, Some(&name.to_string()), &text).map_err(|e| format!("the config file {name} is not valid {}: {e}", format_name(format)))
}

/// The variables that name the config file: LiteLLM's `CONFIG_FILE_PATH`
/// with litegen's prefix, then the spelling the runbook once published. Both
/// set must agree.
const CONFIG_FILE_VARIABLES: [&str; 2] = ["LITEGEN_CONFIG_FILE_PATH", "LITEGEN_CONFIG_FILE"];

/// A named file's format, from its extension; YAML (LiteLLM's format) for
/// none or any other, such as a Kubernetes ConfigMap key.
fn format_of(path: &Path) -> config::FileFormat {
    let extension = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    FORMATS
        .iter()
        .find(|(known, _)| extension.as_deref() == Some(*known))
        .map_or(config::FileFormat::Yaml, |(_, format)| *format)
}

/// Replace every string in `value` (at `at`, in `file`) whose whole text is
/// `os.environ/NAME` with the text of the variable `NAME`, as LiteLLM does in
/// its config file. The text stays text, and an empty variable is an empty
/// value. An unset variable refuses to start, naming the setting and the
/// variable: a setting such as `storage_access.mode` must never fall back to
/// its default unnoticed.
fn resolve_env_references(value: &mut config::Value, at: &str, file: &str, env: &HashMap<String, String>) -> Result<(), String> {
    match &mut value.kind {
        config::ValueKind::String(text) => {
            let reference = text.trim().strip_prefix("os.environ/").map(|name| name.trim().to_string());
            if let Some(name) = reference {
                if name.is_empty() {
                    return Err(format!("{at} in {file} is os.environ/ with no variable name after it"));
                }
                match env.get(&name) {
                    Some(variable) => *text = variable.clone(),
                    None => return Err(format!("{at} in {file} is os.environ/{name}, but {name} is not set")),
                }
            }
        }
        config::ValueKind::Table(table) => {
            for (key, value) in table.iter_mut() {
                resolve_env_references(value, &format!("{at}.{key}"), file, env)?;
            }
        }
        config::ValueKind::Array(items) => {
            for (i, value) in items.iter_mut().enumerate() {
                resolve_env_references(value, &format!("{at}[{i}]"), file, env)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The config file litegen reads in `cwd`, with `env` as its environment and
/// `config_flag` as its `--config` argument, parsed; `Ok(None)` when there is
/// none. An `Err` is the message to refuse to start with.
///
/// `--config`, else `LITEGEN_CONFIG_FILE_PATH` or `LITEGEN_CONFIG_FILE` (a
/// blank one is unset), names the file, relative to `cwd` unless absolute,
/// and a named file must be there. Without a name, litegen looks for
/// `litegen.<extension>` in `cwd`.
pub fn config_file(cwd: &Path, config_flag: Option<&Path>, env: &HashMap<String, String>) -> Result<Option<ConfigFile>, String> {
    let variables: Vec<(&str, &str)> = CONFIG_FILE_VARIABLES
        .iter()
        .filter_map(|name| env.get(*name).map(|v| v.trim()).filter(|v| !v.is_empty()).map(|v| (*name, v)))
        .collect();
    if let [(first, a), (second, b)] = variables[..] {
        if a != b {
            return Err(format!("{first} and {second} name different config files: set only one of them"));
        }
    }
    let mut warnings = Vec::new();
    let named = match (config_flag, variables.first()) {
        (Some(flag), _) => {
            for (variable, value) in &variables {
                if Path::new(value) != flag {
                    warnings.push(format!("{variable} is ignored because --config is set"));
                }
            }
            Some((flag.to_path_buf(), "--config"))
        }
        (None, Some((variable, value))) => Some((PathBuf::from(value), *variable)),
        (None, None) => None,
    };
    let (path, name, format) = match named {
        Some((given, named_by)) => {
            let path = cwd.join(&given);
            if !entry_exists(&path) {
                return Err(format!("the config file {} that {named_by} names does not exist", given.display()));
            }
            (path, given.display().to_string(), format_of(&given))
        }
        None => match find_config_file(cwd)? {
            Some(found) => {
                warnings.extend(found.warnings);
                (found.path, found.name, found.format)
            }
            None => return Ok(None),
        },
    };
    let mut tree = read_config_file(&path, &name, format)?;
    for (key, value) in tree.iter_mut() {
        resolve_env_references(value, key, &name, env)?;
    }
    Ok(Some(ConfigFile { path, format, name, tree, warnings }))
}

/// Load litegen's configuration from `file` (from [`config_file`]) and the
/// environment `env`, for a litegen running in `cwd`.
///
/// Lowest first: the built-in defaults, the config file, the vendors' own
/// variables (`OPENAI_API_KEY`), `LITEGEN__` variables, then litegen's own
/// single-underscore names (`LITEGEN_S3_BUCKET`); after them the database and
/// Redis settings (`connections.rs`). Nothing is written to the process
/// environment.
pub fn load_config_with(cwd: &Path, file: Option<&ConfigFile>, env: &HashMap<String, String>) -> Result<AppConfig, config::ConfigError> {
    let vendor = load_provider_env_overrides(env);
    let litegen = litegen_variables(env);
    let aliases = named_settings(env, LITEGEN_ALIASES);
    let mut builder = config::Config::builder().add_source(config::Config::try_from(&AppConfig::default())?);
    if let Some(file) = file {
        builder = builder.add_source(Tree(file.tree.clone()));
    }
    let mut config: AppConfig = builder
        .add_source(Layer(vendor.clone()))
        .add_source(Layer(litegen.clone()))
        .add_source(Layer(aliases.clone()))
        .build()?
        .try_deserialize()?;
    config.config_file = file.map(|f| f.path.clone());
    let file_alone = match file {
        Some(file) => {
            config.startup_warnings.extend(file.warnings.iter().cloned());
            file.alone()?
        }
        None => config::Config::default(),
    };
    config.startup_warnings.extend(hidden_settings(&file_alone, &[&vendor, &litegen, &aliases]));
    let retired = config.retired_setting_warnings();
    config.startup_warnings.extend(retired);
    settle_image_storage(&mut config)?;

    // The database and Redis connection settings, which have a URL form and a
    // parts form (`connections.rs`).
    let connections =
        connections::resolve_connections(env, &config.database_url, file.map(|f| f.name.as_str())).map_err(config::ConfigError::Message)?;
    config.database_url = connections.database_url;
    config.redis = connections.redis;
    config.startup_warnings.extend(connections.warnings);
    // No setting names a database: data/litegen.db, unless the old default's
    // litegen.db is still in use in `cwd`. A file that names the default's
    // own URL has chosen it.
    let file_names_a_database = file.is_some_and(|f| !matches!(f.get::<config::Value>("database_url"), Ok(None)));
    if connections.database_is_default && !file_names_a_database {
        let (url, warning) = connections::default_database_in(cwd);
        config.database_url = url;
        config.startup_warnings.extend(warning);
    }
    Ok(config)
}

/// Load litegen's configuration as it would start in `cwd`, with `env` as its
/// environment and `config_flag` as its `--config` argument.
pub fn load_config_from(cwd: &Path, config_flag: Option<&Path>, env: &HashMap<String, String>) -> Result<AppConfig, config::ConfigError> {
    let file = config_file(cwd, config_flag, env).map_err(config::ConfigError::Message)?;
    load_config_with(cwd, file.as_ref(), env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::ConfigError;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// A directory holding `files`, each a relative path and its text.
    fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    fn load(dir: &Path, pairs: &[(&str, &str)]) -> Result<AppConfig, ConfigError> {
        load_config_from(dir, None, &env(pairs))
    }

    /// The config of a load that must work.
    fn loaded(r: Result<AppConfig, ConfigError>, what: &str) -> AppConfig {
        r.unwrap_or_else(|e| panic!("{what}: litegen must start, but its config did not load: {e}"))
    }

    /// The message of a load that must be refused.
    fn refused(r: Result<AppConfig, ConfigError>, what: &str) -> String {
        assert!(r.is_err(), "{what}: litegen loaded its config, but must refuse to start");
        r.err().map(|e| e.to_string()).unwrap_or_default()
    }

    fn openai_key(c: &AppConfig) -> Option<&str> {
        c.providers.get("openai").and_then(|p| p.api_key.as_deref())
    }

    // ── S4: values are taken as written ──────────────────────────────────

    #[test]
    fn litegen_variables_keep_their_text() {
        // Each value in a load of its own: two names for one setting would
        // hide each other.
        let mut rewritten = Vec::new();
        for text in ["0012345", "True", "1e5", "1.50", "12345678901234567890123", "Infinity", "nan", "+5"] {
            let dir = tempfile::tempdir().unwrap();
            let c = loaded(load(dir.path(), &[("LITEGEN__MASTER_KEY", text)]), text);
            if c.master_key.as_deref() != Some(text) {
                rewritten.push(format!("LITEGEN__MASTER_KEY={text} -> {:?}", c.master_key));
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let c = loaded(load(dir.path(), &[("LITEGEN__PROVIDERS__OPENAI__API_KEY", "0042")]), "api_key");
        if openai_key(&c) != Some("0042") {
            rewritten.push(format!("LITEGEN__PROVIDERS__OPENAI__API_KEY=0042 -> {:?}", openai_key(&c)));
        }
        let c = loaded(load(dir.path(), &[("OPENAI_API_KEY", "007")]), "vendor key");
        if openai_key(&c) != Some("007") {
            rewritten.push(format!("OPENAI_API_KEY=007 -> {:?}", openai_key(&c)));
        }
        let c = loaded(load(dir.path(), &[("LITEGEN__PROVIDERS__X__MODEL_MAPPING__M", "2.0")]), "model_mapping");
        let mapped = c.providers.get("x").and_then(|p| p.model_mapping.get("m")).cloned();
        if mapped.as_deref() != Some("2.0") {
            rewritten.push(format!("LITEGEN__PROVIDERS__X__MODEL_MAPPING__M=2.0 -> {mapped:?}"));
        }
        let c = loaded(load(dir.path(), &[("LITEGEN__PROVIDERS__X__EXTRA_HEADERS__V", "1.0")]), "extra_headers");
        let header = c.providers.get("x").and_then(|p| p.extra_headers.get("v")).cloned();
        if header.as_deref() != Some("1.0") {
            rewritten.push(format!("LITEGEN__PROVIDERS__X__EXTRA_HEADERS__V=1.0 -> {header:?}"));
        }
        assert!(rewritten.is_empty(), "values rewritten on the way in: {rewritten:#?}");
    }

    #[test]
    fn provider_options_from_variables_stay_text() {
        let dir = tempfile::tempdir().unwrap();
        let c = loaded(load(dir.path(), &[("LITEGEN__PROVIDERS__FAL__OPTIONS__SYNC_API_BASE", "8080")]), "options");
        let options = c.providers.get("fal").and_then(|p| p.options.clone()).unwrap_or_default();
        assert_eq!(options["sync_api_base"].as_str(), Some("8080"), "{options}");
    }

    /// S7: `server.request_timeout_seconds` was never enforced. It is retired:
    /// set, it is named in one startup warning that points at the settings
    /// that do limit a request; unset, nothing is said.
    #[test]
    fn a_request_timeout_in_the_config_is_named_as_retired() {
        let dir = tempfile::tempdir().unwrap();
        let set = loaded(load(dir.path(), &[("LITEGEN__SERVER__REQUEST_TIMEOUT_SECONDS", "300")]), "request_timeout_seconds set");
        let named: Vec<&String> = set.startup_warnings.iter().filter(|w| w.contains("request_timeout_seconds")).collect();
        assert_eq!(named.len(), 1, "{:?}", set.startup_warnings);
        for setting in ["server.vendor_timeout_secs", "server.header_read_timeout_secs", "server.request_body_timeout_secs", "server.shutdown_grace_secs"] {
            assert!(named[0].contains(setting), "the warning names {setting}: {}", named[0]);
        }
        let unset = loaded(load(dir.path(), &[]), "a default start");
        assert!(!unset.startup_warnings.iter().any(|w| w.contains("request_timeout_seconds")), "{:?}", unset.startup_warnings);
    }

    /// Guard: every typed setting still loads from the text of its variable,
    /// so taking values as written breaks no number, switch or choice.
    #[test]
    fn every_typed_setting_still_loads_from_its_variable() {
        use crate::access::{AccessMode, OwnerScope, SignedLinks};
        let dir = tempfile::tempdir().unwrap();
        let one = |name: &str, value: &str| loaded(load(dir.path(), &[(name, value)]), &format!("{name}={value}"));
        assert_eq!(one("LITEGEN__SERVER__PORT", "4321").server.port, 4321);
        assert_eq!(one("LITEGEN__SERVER__REQUEST_TIMEOUT_SECONDS", "77").server.request_timeout_seconds, Some(77));
        assert_eq!(one("LITEGEN__SERVER__VENDOR_TIMEOUT_SECS", "30").server.vendor_timeout_secs, Some(30));
        assert_eq!(one("LITEGEN__SERVER__HEADER_READ_TIMEOUT_SECS", "45").server.header_read_timeout_secs, Some(45));
        assert_eq!(one("LITEGEN__SERVER__REQUEST_BODY_TIMEOUT_SECS", "90").server.request_body_timeout_secs, Some(90));
        assert_eq!(one("LITEGEN__SERVER__SHUTDOWN_GRACE_SECS", "20").server.shutdown_grace_secs, Some(20));
        assert_eq!(one("LITEGEN__COST_MARKUP_PERCENT", "12.5").cost_markup_percent, 12.5);
        for (text, on) in [("true", true), ("1", true), ("yes", true), ("on", true), ("TRUE", true), ("false", false), ("0", false), ("no", false), ("off", false)] {
            assert_eq!(one("LITEGEN__CACHE__ENABLED", text).cache.enabled, on, "cache.enabled={text}");
            assert_eq!(one("LITEGEN__AUTH__ALLOW_PASSWORD", text).auth.allow_password, on, "auth.allow_password={text}");
        }
        assert_eq!(one("LITEGEN__CACHE__DEFAULT_TTL_SECONDS", "60").cache.default_ttl_seconds, 60);
        assert_eq!(one("LITEGEN__CACHE__MAX_ITEMS", "5").cache.max_items, 5);
        assert_eq!(one("LITEGEN__CACHE__MAX_BYTES", "1048576").cache.max_bytes, 1_048_576);
        assert_eq!(one("LITEGEN__CIRCUIT_BREAKER__THRESHOLD", "3").circuit_breaker.threshold, 3);
        assert_eq!(one("LITEGEN__CIRCUIT_BREAKER__OPEN_FOR_SECONDS", "9").circuit_breaker.open_for_seconds, 9);
        assert_eq!(one("LITEGEN__BACKPRESSURE__MAX_IN_FLIGHT", "7").backpressure.max_in_flight, 7);
        assert_eq!(one("LITEGEN__MODE", "hosted").mode, crate::config::Mode::Hosted);
        assert!(one("LITEGEN__DEV__EXPOSE_INVITE_TOKENS", "true").dev.expose_invite_tokens);
        assert!(one("LITEGEN__OBSERVABILITY__JSON_LOGS", "true").observability.json_logs);
        assert!(!one("LITEGEN__OBSERVABILITY__METRICS_ENABLED", "false").observability.metrics_enabled);
        assert!(one("LITEGEN__OBSERVABILITY__METRICS_REQUIRE_AUTH", "true").observability.metrics_require_auth);
        assert_eq!(one("LITEGEN__OBSERVABILITY__METRICS_PATH", "/internal/metrics").observability.metrics_path, "/internal/metrics");
        assert!(!one("LITEGEN__PROVIDERS__OPENAI__ENABLED", "false").providers["openai"].enabled);
        assert_eq!(one("LITEGEN__STORAGE_ACCESS__MODE", "private").storage_access.mode, AccessMode::Private);
        assert_eq!(one("LITEGEN__STORAGE_ACCESS__OWNER_SCOPE", "org").storage_access.owner_scope, OwnerScope::Org);
        assert_eq!(one("LITEGEN__STORAGE_ACCESS__SIGNED_URL_TTL_SECONDS", "600").storage_access.signed_url_ttl_seconds, 600);
        assert!(!one("LITEGEN__STORAGE_ACCESS__KEEP_OUTPUTS", "false").storage_access.keep_outputs);
        assert_eq!(one("LITEGEN__STORAGE_ACCESS__RETENTION_DAYS", "30").storage_access.retention_days, 30);
        assert_eq!(one("LITEGEN__STORAGE_ACCESS__SIGNED_LINKS", "litegen").storage_access.signed_links, SignedLinks::Litegen);
        assert_eq!(one("LITEGEN__IMAGE_STORAGE__RETENTION_DAYS", "9").image_storage.retention_days, Some(9));
        assert_eq!(one("LITEGEN_ASSET_RETENTION_DAYS", "10").image_storage.retention_days, Some(10));
        assert_eq!(one("LITEGEN__RETRIES__MAX_RETRIES", "4").retries.max_retries, 4);
        assert_eq!(one("LITEGEN__RETRIES__BACKOFF_MS", "100").retries.backoff_ms, 100);
        assert_eq!(one("LITEGEN__RETRIES__MAX_BACKOFF_MS", "2000").retries.max_backoff_ms, 2000);
        assert!(!one("LITEGEN__MCP__ENABLED", "false").mcp.enabled);
        assert!(!one("LITEGEN__MCP__ADMIN_ENABLED", "false").mcp.admin_enabled);
        assert!(!one("LITEGEN__MCP__OAUTH__ENABLED", "false").mcp.oauth.enabled);
        assert_eq!(one("LITEGEN__MCP__OAUTH__ACCESS_TOKEN_TTL_SECS", "60").mcp.oauth.access_token_ttl_secs, 60);
        assert_eq!(one("LITEGEN__MCP__OAUTH__REFRESH_TOKEN_TTL_DAYS", "2").mcp.oauth.refresh_token_ttl_days, 2);
        assert_eq!(one("LITEGEN__MCP__OAUTH__RATE_LIMIT_PER_MINUTE", "0").mcp.oauth.rate_limit_per_minute, 0);
        // A value out of range is still refused, naming the setting.
        let e = refused(load(dir.path(), &[("LITEGEN__CACHE__MAX_ITEMS", "0")]), "max_items 0");
        assert!(e.contains("max_items"), "{e}");
        let e = refused(load(dir.path(), &[("LITEGEN__SERVER__PORT", "http")]), "port http");
        assert!(e.contains("server.port"), "{e}");
    }

    // ── S6: each source has its place, and a setting that loses is named ─

    #[test]
    fn a_litegen_variable_beats_the_vendors_own() {
        let dir = tempfile::tempdir().unwrap();
        let c = loaded(load(dir.path(), &[("OPENAI_API_KEY", "sk-bare"), ("LITEGEN__PROVIDERS__OPENAI__API_KEY", "sk-explicit")]), "both");
        assert_eq!(openai_key(&c), Some("sk-explicit"));
    }

    #[test]
    fn a_blank_vendor_variable_counts_as_unset() {
        let dir = dir_with(&[("litegen.yaml", "providers:\n  openai:\n    api_key: sk-yaml\n")]);
        for blank in ["", "  ", "\n"] {
            let c = loaded(load(dir.path(), &[("OPENAI_API_KEY", blank)]), "blank vendor variable");
            assert_eq!(openai_key(&c), Some("sk-yaml"), "OPENAI_API_KEY={blank:?} must not blank the file's key");
            let c = loaded(load(dir.path(), &[("OPENAI_API_KEY", blank), ("LITEGEN__PROVIDERS__OPENAI__API_KEY", "sk-explicit")]), "blank vendor variable");
            assert_eq!(openai_key(&c), Some("sk-explicit"), "OPENAI_API_KEY={blank:?} must not blank LITEGEN__'s key");
        }
        // litegen's own single-underscore names: blank is unset too.
        let empty = tempfile::tempdir().unwrap();
        let s3 = ["LITEGEN_S3_BUCKET", "LITEGEN_S3_REGION", "LITEGEN_S3_ENDPOINT_URL", "LITEGEN_S3_ACCESS_KEY_ID", "LITEGEN_S3_SECRET_ACCESS_KEY"];
        let all_blank: Vec<(&str, &str)> = s3.iter().map(|n| (*n, "")).collect();
        let c = loaded(load(empty.path(), &all_blank), "every LITEGEN_S3_* blank");
        assert!(c.image_storage.s3.is_none(), "{:?}", c.image_storage.s3);
        let c = loaded(load(empty.path(), &[("LITEGEN_S3_REGION", " ")]), "LITEGEN_S3_REGION blank");
        assert!(c.image_storage.s3.is_none(), "{:?}", c.image_storage.s3);
        let c = loaded(load(empty.path(), &[("LITEGEN_ASSET_RETENTION_DAYS", ""), ("LITEGEN_ASSET_DIR", " ")]), "LITEGEN_ASSET_* blank");
        assert_eq!((c.image_storage.retention_days, c.image_storage.local_dir), (None, None));
    }

    /// Guard: a vendor's variable still beats the file, which the example's
    /// `api_key: ""  # or set OPENAI_API_KEY` relies on.
    #[test]
    fn a_vendor_variable_beats_the_file() {
        let dir = dir_with(&[("litegen.yaml", "providers:\n  openai:\n    api_key: sk-yaml\n")]);
        let c = loaded(load(dir.path(), &[("OPENAI_API_KEY", "sk-bare")]), "vendor over file");
        assert_eq!(openai_key(&c), Some("sk-bare"));
        let c = loaded(load(dir.path(), &[]), "file only");
        assert_eq!(openai_key(&c), Some("sk-yaml"));
    }

    /// Guard: litegen's single-underscore names beat the `LITEGEN__` form of
    /// the same setting, so the image's `ENV` can't beat an operator's.
    #[test]
    fn litegen_aliases_beat_the_double_underscore_form() {
        let dir = tempfile::tempdir().unwrap();
        let c = loaded(load(dir.path(), &[("LITEGEN__IMAGE_STORAGE__LOCAL_DIR", "a"), ("LITEGEN_ASSET_DIR", "b")]), "asset dir");
        assert_eq!(c.image_storage.local_dir.as_deref(), Some("b"));
        let c = loaded(load(dir.path(), &[("LITEGEN__IMAGE_STORAGE__S3__BUCKET_NAME", "aa"), ("LITEGEN_S3_BUCKET", "bb")]), "bucket");
        assert_eq!(c.image_storage.s3.as_ref().map(|s| s.bucket_name.as_str()), Some("bb"));
    }

    #[test]
    fn a_setting_that_loses_is_named_never_its_value() {
        let dir = tempfile::tempdir().unwrap();
        let warned = |c: &AppConfig, names: &[&str]| -> String {
            c.startup_warnings
                .iter()
                .find(|w| names.iter().all(|n| w.contains(n)))
                .cloned()
                .unwrap_or_else(|| panic!("no warning names {names:?}: {:?}", c.startup_warnings))
        };
        // The explicit LITEGEN__ name wins, and the vendor's is named.
        let c = loaded(load(dir.path(), &[("OPENAI_API_KEY", "sk-bare-123"), ("LITEGEN__PROVIDERS__OPENAI__API_KEY", "sk-explicit-456")]), "both");
        let w = warned(&c, &["OPENAI_API_KEY", "LITEGEN__PROVIDERS__OPENAI__API_KEY"]);
        assert!(!w.contains("sk-bare-123") && !w.contains("sk-explicit-456"), "a value leaked: {w}");
        // A blank LITEGEN__ value still wins (it is set), and still says so.
        let c = loaded(load(dir.path(), &[("OPENAI_API_KEY", "sk-bare-123"), ("LITEGEN__PROVIDERS__OPENAI__API_KEY", "")]), "blank explicit");
        assert_eq!(openai_key(&c), Some(""));
        warned(&c, &["OPENAI_API_KEY", "LITEGEN__PROVIDERS__OPENAI__API_KEY"]);
        // Two vendor names for one setting: the later in the table wins.
        let c = loaded(load(dir.path(), &[("BYTEDANCE_API_KEY", "byte-1"), ("ARK_API_KEY", "ark-2")]), "two vendor names");
        assert_eq!(c.providers.get("bytedance").and_then(|p| p.api_key.as_deref()), Some("ark-2"));
        let w = warned(&c, &["BYTEDANCE_API_KEY", "ARK_API_KEY"]);
        assert!(!w.contains("byte-1") && !w.contains("ark-2"), "a value leaked: {w}");
        // The file's value, hidden by a variable.
        let file = dir_with(&[("litegen.yaml", "providers:\n  openai:\n    api_key: sk-yaml-789\n")]);
        let c = loaded(load(file.path(), &[("OPENAI_API_KEY", "sk-bare-123")]), "file and vendor");
        let w = warned(&c, &["providers.openai.api_key", "litegen.yaml", "OPENAI_API_KEY"]);
        assert!(!w.contains("sk-yaml-789") && !w.contains("sk-bare-123"), "a value leaked: {w}");
        // Nothing to say: the same value twice, a blank one below, or nothing below.
        for pairs in [
            &[("OPENAI_API_KEY", "same"), ("LITEGEN__PROVIDERS__OPENAI__API_KEY", "same")][..],
            &[("OPENAI_API_KEY", ""), ("LITEGEN__PROVIDERS__OPENAI__API_KEY", "sk-explicit")][..],
            &[("LITEGEN__PROVIDERS__MOCK__API_KEY", "")][..],
        ] {
            let c = loaded(load(dir.path(), pairs), "no clash");
            assert!(c.startup_warnings.is_empty(), "{pairs:?}: {:?}", c.startup_warnings);
        }
    }

    /// Loading reads `env` and nothing else: it never copies a variable into
    /// the process environment, which is unsound once other threads run.
    /// A sentinel, so whatever the shell (or another test) set can't decide it.
    #[test]
    fn loading_leaves_the_process_environment_alone() {
        let sentinel = format!("sentinel-{}-{:?}", std::process::id(), std::time::SystemTime::now());
        let dir = tempfile::tempdir().unwrap();
        let c = loaded(load(dir.path(), &[("OPENAI_API_KEY", &sentinel), ("LITEGEN_S3_BUCKET", &sentinel)]), "sentinel");
        assert_eq!(openai_key(&c), Some(sentinel.as_str()));
        for name in ["LITEGEN__PROVIDERS__OPENAI__API_KEY", "LITEGEN__IMAGE_STORAGE__S3__BUCKET_NAME"] {
            assert_ne!(std::env::var(name).ok().as_deref(), Some(sentinel.as_str()), "{name} was written into the process environment");
        }
    }

    // ── S3: ambient AWS credentials, and the image_storage.s3 section ─────

    #[test]
    fn aws_credentials_leave_image_storage_alone() {
        let dir = tempfile::tempdir().unwrap();
        let c = loaded(
            load(dir.path(), &[("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE"), ("AWS_SECRET_ACCESS_KEY", "x"), ("AWS_SESSION_TOKEN", "y")]),
            "AWS_* with local storage",
        );
        assert!(c.image_storage.s3.is_none(), "{:?}", c.image_storage.s3);
        let c = loaded(load(dir.path(), &[("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE")]), "AWS_ACCESS_KEY_ID alone");
        assert!(c.image_storage.s3.is_none(), "{:?}", c.image_storage.s3);
    }

    /// Guard: the five LITEGEN_S3_* variables still make the section.
    #[test]
    fn litegen_s3_variables_still_fill_image_storage() {
        let dir = tempfile::tempdir().unwrap();
        let c = loaded(
            load(dir.path(), &[
                ("LITEGEN__IMAGE_STORAGE__BACKEND", "s3"),
                ("LITEGEN_S3_BUCKET", "bkt"),
                ("LITEGEN_S3_REGION", "fr-par"),
                ("LITEGEN_S3_ENDPOINT_URL", "https://s3.example"),
                ("LITEGEN_S3_ACCESS_KEY_ID", "kid"),
                ("LITEGEN_S3_SECRET_ACCESS_KEY", "sec"),
            ]),
            "LITEGEN_S3_*",
        );
        let s3 = c.image_storage.s3.expect("the s3 section");
        assert_eq!(
            (s3.bucket_name.as_str(), s3.region.as_str(), s3.endpoint_url.as_deref(), s3.access_key_id.as_deref(), s3.secret_access_key.as_deref()),
            ("bkt", "fr-par", Some("https://s3.example"), Some("kid"), Some("sec"))
        );
    }

    #[test]
    fn a_partial_s3_section_is_ignored_while_storage_is_local() {
        let dir = tempfile::tempdir().unwrap();
        let c = loaded(load(dir.path(), &[("LITEGEN_S3_REGION", "fr-par")]), "LITEGEN_S3_REGION alone");
        assert!(c.image_storage.s3.is_none(), "{:?}", c.image_storage.s3);
        assert!(c.startup_warnings.iter().any(|w| w.contains("image_storage.s3")), "{:?}", c.startup_warnings);
        let file = dir_with(&[("litegen.yaml", "image_storage:\n  backend: local\n  s3:\n    region: fr-par\n")]);
        let c = loaded(load(file.path(), &[]), "a partial s3 section in the file");
        assert!(c.image_storage.s3.is_none(), "{:?}", c.image_storage.s3);
        assert!(c.startup_warnings.iter().any(|w| w.contains("image_storage.s3")), "{:?}", c.startup_warnings);
    }

    #[test]
    fn s3_storage_without_a_bucket_names_the_variable() {
        let dir = tempfile::tempdir().unwrap();
        for pairs in [
            &[("LITEGEN__IMAGE_STORAGE__BACKEND", "s3"), ("LITEGEN_S3_REGION", "x")][..],
            &[("LITEGEN__IMAGE_STORAGE__BACKEND", "s3"), ("LITEGEN__IMAGE_STORAGE__S3__BUCKET_NAME", " ")][..],
            // N4: no s3 section at all no longer falls back to local disk.
            &[("LITEGEN__IMAGE_STORAGE__BACKEND", "s3")][..],
        ] {
            let e = refused(load(dir.path(), pairs), &format!("{pairs:?}"));
            assert!(e.contains("LITEGEN_S3_BUCKET") && e.contains("image_storage.s3.bucket_name"), "{pairs:?}: {e}");
        }
    }

    /// `image_storage` on S3, with bucket `bkt` and the `s3` lines `keys`.
    fn s3_file(keys: &str) -> String {
        format!("image_storage:\n  backend: s3\n  s3:\n    bucket_name: bkt\n{keys}")
    }

    /// M1: a blank key, secret or session token is no key, wherever it is
    /// set, so the AWS chain signs. aws-creds takes any key it is given as
    /// the key, even an empty one: the old example's `access_key_id: ""`
    /// beside `AWS_*` would sign every upload with an empty key id.
    #[test]
    fn a_blank_s3_key_counts_as_unset() {
        let blank = dir_with(&[("litegen.yaml", &s3_file("    access_key_id: \"\"\n    secret_access_key: \"\"\n    session_token: \" \"\n"))]);
        let references = dir_with(&[("litegen.yaml", &s3_file("    access_key_id: os.environ/E2E_KEY\n    secret_access_key: os.environ/E2E_SECRET\n"))]);
        let empty = tempfile::tempdir().unwrap();
        let variables = [
            ("LITEGEN__IMAGE_STORAGE__BACKEND", "s3"),
            ("LITEGEN_S3_BUCKET", "bkt"),
            ("LITEGEN__IMAGE_STORAGE__S3__ACCESS_KEY_ID", ""),
            ("LITEGEN__IMAGE_STORAGE__S3__SECRET_ACCESS_KEY", " "),
            ("LITEGEN__IMAGE_STORAGE__S3__SESSION_TOKEN", ""),
        ];
        let mut taken = Vec::new();
        for (what, dir, pairs) in [
            ("blank in the file, as the old example wrote them", blank.path(), &[][..]),
            ("blank LITEGEN__IMAGE_STORAGE__S3__* variables", empty.path(), &variables[..]),
            ("os.environ/ references to empty variables", references.path(), &[("E2E_KEY", ""), ("E2E_SECRET", "")][..]),
        ] {
            let s3 = loaded(load(dir, pairs), what).image_storage.s3.unwrap_or_else(|| panic!("{what}: the s3 section is gone"));
            let keys = (s3.access_key_id, s3.secret_access_key, s3.session_token);
            if keys != (None, None, None) {
                taken.push(format!("{what}: {keys:?}"));
            }
        }
        assert!(taken.is_empty(), "blank S3 keys taken as keys: {taken:#?}");
    }

    /// Guard: keys that are there are used as written, the session token too.
    #[test]
    fn s3_keys_that_are_set_are_kept() {
        let dir = dir_with(&[("litegen.yaml", &s3_file("    access_key_id: AKIAFILE\n    secret_access_key: file-secret\n    session_token: file-token\n"))]);
        let s3 = loaded(load(dir.path(), &[]), "keys in the file").image_storage.s3.expect("the s3 section");
        assert_eq!(
            (s3.access_key_id.as_deref(), s3.secret_access_key.as_deref(), s3.session_token.as_deref()),
            (Some("AKIAFILE"), Some("file-secret"), Some("file-token"))
        );
    }

    /// M1, the other half: with storage on S3, a key without its secret
    /// refuses to start. rust-s3 sends that key's requests unsigned and
    /// panics presigning a link. A key in the file whose secret is in
    /// `AWS_SECRET_ACCESS_KEY` worked while the `AWS_*` rows filled the
    /// config; the AWS chain only ever reads both variables together.
    #[test]
    fn an_s3_key_without_its_secret_refuses_to_start() {
        let mut started = Vec::new();
        for (what, keys, pairs) in [
            ("no secret", "    access_key_id: AKIAFILE\n", &[][..]),
            ("a blank secret", "    access_key_id: AKIAFILE\n    secret_access_key: \"\"\n", &[][..]),
            ("the secret in AWS_SECRET_ACCESS_KEY", "    access_key_id: AKIAFILE\n", &[("AWS_SECRET_ACCESS_KEY", "aws-secret")][..]),
            ("LITEGEN_S3_ACCESS_KEY_ID alone", "", &[("LITEGEN_S3_ACCESS_KEY_ID", "AKIAVAR")][..]),
        ] {
            let dir = dir_with(&[("litegen.yaml", &s3_file(keys))]);
            match load(dir.path(), pairs) {
                Ok(_) => started.push(what),
                Err(e) => {
                    let e = e.to_string();
                    assert!(e.contains("LITEGEN_S3_SECRET_ACCESS_KEY") && e.contains("image_storage.s3.secret_access_key"), "{what}: {e}");
                    assert!(!e.contains("AKIAFILE") && !e.contains("AKIAVAR") && !e.contains("aws-secret"), "{what}: a value leaked: {e}");
                }
            }
        }
        assert!(started.is_empty(), "litegen must refuse to start with a key but no secret: {started:?}");
        // Still starts: storage that isn't on S3 never uses the section, and a
        // secret without its key leaves the AWS chain to sign, as before.
        let local = dir_with(&[("litegen.yaml", "image_storage:\n  backend: local\n  s3:\n    bucket_name: bkt\n    access_key_id: AKIAFILE\n")]);
        loaded(load(local.path(), &[]), "a key without its secret, storage local");
        let stale = dir_with(&[("litegen.yaml", &s3_file("    secret_access_key: stale\n"))]);
        let c = loaded(load(stale.path(), &[("AWS_ACCESS_KEY_ID", "AKIAENV"), ("AWS_SECRET_ACCESS_KEY", "env-secret")]), "a secret without its key");
        assert_eq!(c.image_storage.s3.and_then(|s| s.access_key_id), None);
    }

    // ── S1: the config file is found by name, and never skipped silently ──

    #[test]
    fn the_file_beside_a_file_named_litegen_is_read() {
        // The image's layout: the binary is /app/litegen, beside /app/litegen.yaml.
        let dir = dir_with(&[("litegen", ""), ("litegen.yaml", "server:\n  port: 4321\n")]);
        assert_eq!(loaded(load(dir.path(), &[]), "beside litegen").server.port, 4321);
    }

    #[test]
    fn two_config_files_refuse_to_start() {
        let dir = dir_with(&[("litegen.yaml", "server:\n  port: 4321\n"), ("litegen.json", "{\"server\": {\"port\": 1234}}")]);
        let e = refused(load(dir.path(), &[]), "litegen.yaml and litegen.json");
        assert!(e.contains("litegen.yaml") && e.contains("litegen.json"), "{e}");
    }

    #[test]
    fn a_yml_beside_a_yaml_is_named_and_the_yaml_read() {
        let dir = dir_with(&[("litegen.yaml", "server:\n  port: 4321\n"), ("litegen.yml", "server:\n  port: 1234\n")]);
        let c = loaded(load(dir.path(), &[]), "litegen.yaml and litegen.yml");
        assert_eq!(c.server.port, 4321);
        assert!(c.startup_warnings.iter().any(|w| w.contains("litegen.yml") && w.contains("ignored")), "{:?}", c.startup_warnings);
    }

    #[test]
    fn a_config_file_that_is_a_directory_refuses_to_start() {
        // What `docker run -v ./litegen.yaml:/app/litegen.yaml` makes when the host file is missing.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("litegen.yaml")).unwrap();
        let e = refused(load(dir.path(), &[]), "litegen.yaml is a directory");
        assert!(e.contains("litegen.yaml") && e.contains("directory"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_config_file_refuses_to_start() {
        use std::os::unix::fs::PermissionsExt;
        let dir = dir_with(&[("litegen.yaml", "server:\n  port: 4321\n")]);
        let path = dir.path().join("litegen.yaml");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            eprintln!("SKIPPED: this user reads a mode-000 file (root)");
            return;
        }
        let e = refused(load(dir.path(), &[]), "an unreadable litegen.yaml");
        assert!(e.contains("litegen.yaml"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_config_symlink_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(dir.path().join("..data/litegen.yaml"), dir.path().join("litegen.yaml")).unwrap();
        let e = refused(load(dir.path(), &[]), "litegen.yaml links to nothing");
        assert!(e.contains("litegen.yaml"), "{e}");
    }

    /// Guard: a Kubernetes ConfigMap mounts its keys as links
    /// (`litegen.yaml -> ..data/litegen.yaml`), and they are followed.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_file_is_read() {
        let dir = dir_with(&[("..data/litegen.yaml", "server:\n  port: 4321\n")]);
        std::os::unix::fs::symlink(dir.path().join("..data/litegen.yaml"), dir.path().join("litegen.yaml")).unwrap();
        assert_eq!(loaded(load(dir.path(), &[]), "a linked litegen.yaml").server.port, 4321);
    }

    /// Guard: no file at all is no file, not an error.
    #[test]
    fn no_config_file_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        let c = loaded(load(dir.path(), &[]), "an empty directory");
        assert_eq!((c.server.port, c.server.host.as_str()), (4000, "0.0.0.0"));
        assert!(c.startup_warnings.is_empty(), "{:?}", c.startup_warnings);
    }

    // ── S5: naming the file, and os.environ/ references ──────────────────

    #[test]
    fn the_named_file_is_read() {
        let dir = dir_with(&[
            ("conf/site.yaml", "server:\n  port: 4321\n"),
            ("conf/site", "server:\n  port: 4322\n"),
            ("litegen.yaml", "server:\n  port: 5555\n"),
        ]);
        let port = |flag: Option<&str>, pairs: &[(&str, &str)]| {
            loaded(load_config_from(dir.path(), flag.map(Path::new), &env(pairs)), &format!("{flag:?} {pairs:?}")).server.port
        };
        assert_eq!(port(Some("conf/site.yaml"), &[]), 4321, "--config");
        let absolute = dir.path().join("conf/site.yaml");
        assert_eq!(port(Some(absolute.to_str().unwrap()), &[]), 4321, "--config, absolute");
        assert_eq!(port(None, &[("LITEGEN_CONFIG_FILE_PATH", "conf/site.yaml")]), 4321, "LITEGEN_CONFIG_FILE_PATH");
        assert_eq!(port(None, &[("LITEGEN_CONFIG_FILE", "conf/site.yaml")]), 4321, "LITEGEN_CONFIG_FILE");
        // No extension: YAML, LiteLLM's format (a Kubernetes ConfigMap key).
        assert_eq!(port(Some("conf/site"), &[]), 4322, "no extension");
        // A blank variable is unset: the file in the directory is read.
        assert_eq!(port(None, &[("LITEGEN_CONFIG_FILE_PATH", " "), ("LITEGEN_CONFIG_FILE", "")]), 5555, "blank variables");
    }

    #[test]
    fn a_named_file_that_is_missing_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let e = refused(load_config_from(dir.path(), Some(Path::new("nope.yaml")), &env(&[])), "--config nope.yaml");
        assert!(e.contains("nope.yaml") && e.contains("--config"), "{e}");
        for name in ["LITEGEN_CONFIG_FILE_PATH", "LITEGEN_CONFIG_FILE"] {
            let e = refused(load(dir.path(), &[(name, "nope.yaml")]), name);
            assert!(e.contains("nope.yaml") && e.contains(name), "{e}");
        }
        std::fs::create_dir(dir.path().join("conf")).unwrap();
        let e = refused(load_config_from(dir.path(), Some(Path::new("conf")), &env(&[])), "--config conf (a directory)");
        assert!(e.contains("conf") && e.contains("directory"), "{e}");
    }

    #[test]
    fn the_two_config_variables_must_agree() {
        let dir = dir_with(&[("a.yaml", "server:\n  port: 4321\n"), ("b.yaml", "server:\n  port: 1234\n")]);
        let e = refused(load(dir.path(), &[("LITEGEN_CONFIG_FILE_PATH", "a.yaml"), ("LITEGEN_CONFIG_FILE", "b.yaml")]), "two different files");
        assert!(e.contains("LITEGEN_CONFIG_FILE_PATH") && e.contains("LITEGEN_CONFIG_FILE "), "{e}");
        let c = loaded(load(dir.path(), &[("LITEGEN_CONFIG_FILE_PATH", "a.yaml"), ("LITEGEN_CONFIG_FILE", "a.yaml")]), "the same file");
        assert_eq!(c.server.port, 4321);
        // --config beats a variable, and the variable is named.
        let c = loaded(load_config_from(dir.path(), Some(Path::new("b.yaml")), &env(&[("LITEGEN_CONFIG_FILE_PATH", "a.yaml")])), "--config over a variable");
        assert_eq!(c.server.port, 1234);
        assert!(c.startup_warnings.iter().any(|w| w.contains("LITEGEN_CONFIG_FILE_PATH") && w.contains("--config")), "{:?}", c.startup_warnings);
    }

    #[test]
    fn os_environ_references_take_the_variables_text() {
        let dir = dir_with(&[(
            "litegen.yaml",
            "master_key: os.environ/K\n\
             providers:\n  openai:\n    api_key: \"os.environ/EMPTY\"\n    extra_headers:\n      X-A: os.environ/ H \n\
             hooks:\n  - path: hooks/a.py\n    env:\n      TOKEN: os.environ/T\n\
             model_routes:\n  - model: m\n    deployments:\n      - provider: os.environ/P\n",
        )]);
        let c = loaded(load(dir.path(), &[("K", "0012345"), ("H", "hv"), ("T", "tok"), ("P", "mock"), ("EMPTY", "")]), "references");
        assert_eq!(c.master_key.as_deref(), Some("0012345"), "kept as written");
        assert_eq!(openai_key(&c), Some(""), "an empty variable is an empty value");
        assert_eq!(c.providers["openai"].extra_headers.get("x-a").map(String::as_str), Some("hv"));
        assert_eq!(c.hooks[0].env.get("TOKEN").map(String::as_str), Some("tok"));
        assert_eq!(c.model_routes[0].deployments[0].provider, "mock");
    }

    #[test]
    fn an_os_environ_reference_to_an_unset_variable_is_refused() {
        let dir = dir_with(&[("litegen.yaml", "master_key: os.environ/E2E_UNSET_K\n")]);
        let e = refused(load(dir.path(), &[("OTHER", "hunter2")]), "an unset variable");
        assert!(e.contains("master_key") && e.contains("E2E_UNSET_K") && e.contains("litegen.yaml"), "{e}");
        assert!(!e.contains("hunter2"), "{e}");
    }

    #[test]
    fn parse_args_takes_only_config_and_help() {
        let parse = |args: &[&str]| parse_args(args.iter().map(OsString::from));
        let config = |args: &[&str]| parse(args).map(|a| a.config);
        assert_eq!(config(&[]), Ok(None));
        assert_eq!(config(&["--config", "a.yaml"]), Ok(Some(PathBuf::from("a.yaml"))));
        assert_eq!(config(&["--config=a.yaml"]), Ok(Some(PathBuf::from("a.yaml"))));
        assert_eq!(config(&["-c", "a.yaml"]), Ok(Some(PathBuf::from("a.yaml"))), "LiteLLM's short form");
        assert_eq!(parse(&["--help"]).map(|a| a.help), Ok(true));
        assert_eq!(parse(&["-h"]).map(|a| a.help), Ok(true));
        for bad in [&["--config"][..], &["--config="][..], &["-c"][..], &["--config", ""][..], &["--config", "a", "--config", "b"][..], &["-c", "a", "--config=b"][..]] {
            let e = parse(bad).err().unwrap_or_else(|| panic!("{bad:?} must be refused"));
            assert!(e.contains("--config"), "{bad:?}: {e}");
        }
        for (bad, named) in [(&["--port", "1"][..], "--port"), (&["serve"][..], "serve"), (&["--config", "a", "-v"][..], "-v")] {
            let e = parse(bad).err().unwrap_or_else(|| panic!("{bad:?} must be refused"));
            assert!(e.contains(named), "{bad:?}: {e}");
        }
    }

    // ── N1: list settings from LITEGEN__ variables ───────────────────────

    #[test]
    fn cors_allowed_origins_takes_a_comma_list() {
        let dir = tempfile::tempdir().unwrap();
        let c = loaded(load(dir.path(), &[("LITEGEN__CORS__ALLOWED_ORIGINS", "https://a.example, https://b.example,,")]), "a list");
        assert_eq!(c.cors.allowed_origins, ["https://a.example", "https://b.example"]);
        let c = loaded(load(dir.path(), &[("LITEGEN__CORS__ALLOWED_ORIGINS", "*")]), "*");
        assert_eq!(c.cors.allowed_origins, ["*"]);
        // Beside a list in the file: the same list is no clash, another one is named.
        let file = dir_with(&[("litegen.yaml", "cors:\n  allowed_origins: [\"https://a.example\", \"https://b.example\"]\n")]);
        let c = loaded(load(file.path(), &[("LITEGEN__CORS__ALLOWED_ORIGINS", "https://a.example,https://b.example")]), "the same list");
        assert!(c.startup_warnings.is_empty(), "{:?}", c.startup_warnings);
        let c = loaded(load(file.path(), &[("LITEGEN__CORS__ALLOWED_ORIGINS", "https://c.example")]), "another list");
        assert_eq!(c.cors.allowed_origins, ["https://c.example"]);
        assert!(c.startup_warnings.iter().any(|w| w.contains("cors.allowed_origins in litegen.yaml")), "{:?}", c.startup_warnings);
    }

    #[test]
    fn mcp_allowed_hosts_takes_a_comma_list() {
        let dir = tempfile::tempdir().unwrap();
        let c = loaded(load(dir.path(), &[("LITEGEN__MCP__ALLOWED_HOSTS", "litegen.example,  api.example ")]), "a list");
        assert_eq!(c.mcp.allowed_hosts, ["litegen.example", "api.example"]);
        let c = loaded(load(dir.path(), &[("LITEGEN__MCP__ALLOWED_HOSTS", "")]), "empty");
        assert!(c.mcp.allowed_hosts.is_empty(), "{:?}", c.mcp.allowed_hosts);
    }

    #[test]
    fn mcp_allowed_origins_takes_a_comma_list() {
        let dir = tempfile::tempdir().unwrap();
        let c = loaded(load(dir.path(), &[("LITEGEN__MCP__ALLOWED_ORIGINS", "https://chatgpt.com,https://claude.ai")]), "a list");
        assert_eq!(c.mcp.allowed_origins, ["https://chatgpt.com", "https://claude.ai"]);
    }

    // ── N2: the shipped example loads ────────────────────────────────────

    #[test]
    fn the_example_config_loads() {
        let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../litegen.example.yaml");
        let text = std::fs::read_to_string(&example).unwrap();
        let dir = dir_with(&[("litegen.yaml", &text)]);
        loaded(load(dir.path(), &[]), "litegen.example.yaml copied to litegen.yaml");
        // `models_dir` is no setting (LITEGEN_MODELS_DIR is): the example must not suggest one.
        assert!(!text.lines().any(|l| l.trim_start().starts_with("models_dir:")), "litegen.example.yaml sets models_dir, which litegen never reads");
    }

    // ── S2: the database when no setting names one ───────────────────────

    #[test]
    fn the_old_defaults_litegen_db_is_used_only_while_nothing_names_a_database() {
        let dir = dir_with(&[("litegen.db", "")]);
        let c = loaded(load(dir.path(), &[]), "no database setting, beside litegen.db");
        assert_eq!(c.database_url, "sqlite://litegen.db", "the file the old default opened");
        assert!(c.startup_warnings.iter().any(|w| w.contains("data/litegen.db")), "{:?}", c.startup_warnings);

        // A config file that names the default's own URL has chosen it.
        let named = format!("database_url: \"{}\"\n", crate::config::DEFAULT_DATABASE_URL);
        let dir = dir_with(&[("litegen.db", ""), ("litegen.yaml", &named)]);
        let c = loaded(load(dir.path(), &[]), "a file that names the default");
        assert_eq!(c.database_url, crate::config::DEFAULT_DATABASE_URL);
        assert!(!c.startup_warnings.iter().any(|w| w.contains("litegen.db")), "{:?}", c.startup_warnings);
    }
}
