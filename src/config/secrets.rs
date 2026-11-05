//! 利用者の秘密値定義(`$HERDR_PLUGIN_CONFIG_DIR/secrets.yaml`)。
//!
//! 動作規則は`docs/specs/security.md#secrets`と`docs/specs/execution.md#secrets`を
//! 主文書とする。ファイルは現在の利用者が所有し、グループとその他の利用者が
//! 読み書きできない場合だけ読む。権限は変更しない。
//!
//! 秘密値はエラー、`Debug`表示のいずれにも含めない。YAMLの解析エラーも本文を
//! 引用しうるため、位置だけを報告する。

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::model::{EnvValue, TaskDef, WorkflowSpec, no_duplicate_map};
use crate::config::plugin::CONFIG_DIR_ENV;
use crate::config::validate::is_valid_resource_name;

pub const SECRETS_FILE_NAME: &str = "secrets.yaml";

/// グループとその他の利用者の読み取り・書き込み権限。
const GROUP_OTHER_READ_WRITE: u32 = 0o066;

/// 解決済みの秘密値。`Debug`では値を表示しない。
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    /// 子プロセスの環境変数へ渡すときだけ値を取り出す。
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SecretSource {
    Value(SecretValue),
    FromEnv(String),
}

/// 読み込んだ秘密値定義。ファイルがない場合は空として扱う。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SecretsFile {
    secrets: BTreeMap<String, SecretSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSecretsFile {
    version: u32,
    #[serde(default, deserialize_with = "no_duplicate_map")]
    secrets: BTreeMap<String, RawSecret>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawSecret {
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    from_env: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecretsError {
    #[error("failed to read secrets file {path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("secrets file {path} is not a regular file")]
    NotRegularFile { path: PathBuf },
    #[error(
        "secrets file {path} is owned by uid {owner}, not the current user (uid {current}); permissions were not changed"
    )]
    NotOwned {
        path: PathBuf,
        owner: u32,
        current: u32,
    },
    #[error(
        "secrets file {path} is readable or writable by group or others (mode {mode:o}); permissions were not changed"
    )]
    InsecurePermissions { path: PathBuf, mode: u32 },
    #[error("failed to parse secrets file {path}{}", location_suffix(*.line, *.column))]
    Parse {
        path: PathBuf,
        line: Option<usize>,
        column: Option<usize>,
    },
    #[error("unsupported secrets file version {0}; expected 1")]
    UnsupportedVersion(u32),
    #[error("secrets.{0}: invalid secret name (expected [a-z0-9][a-z0-9-]*)")]
    InvalidName(String),
    #[error("secrets.{0}: exactly one of value or fromEnv is required")]
    InvalidSource(String),
    #[error("secrets.{name}.fromEnv: invalid environment variable name {env:?}")]
    InvalidEnvName { name: String, env: String },
    #[error("secrets.{0}: value must not contain NUL")]
    ContainsNul(String),
    #[error("undefined secret reference(s): {}", .0.join(", "))]
    Undefined(Vec<String>),
    #[error("secret {name}: environment variable {env} is not set")]
    MissingEnv { name: String, env: String },
    #[error("secret {name}: environment variable {env} is not valid UTF-8 or contains NUL")]
    InvalidEnvValue { name: String, env: String },
}

fn location_suffix(line: Option<usize>, column: Option<usize>) -> String {
    match (line, column) {
        (Some(line), Some(column)) => format!(" at line {line} column {column}"),
        _ => String::new(),
    }
}

impl SecretsFile {
    /// 定義済みの秘密値名。
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.secrets.keys().map(String::as_str)
    }

    /// Run開始時の確認。参照した名前がすべて定義済みで、`fromEnv`の値が
    /// 起動元環境に存在することを確かめる。未定義の名前はまとめて報告する。
    pub fn check_references<'a, F>(
        &self,
        names: impl IntoIterator<Item = &'a str>,
        lookup: F,
    ) -> Result<(), SecretsError>
    where
        F: Fn(&str) -> Option<OsString>,
    {
        let names: BTreeSet<&str> = names.into_iter().collect();
        let undefined: Vec<String> = names
            .iter()
            .filter(|name| !self.secrets.contains_key(**name))
            .map(|name| (*name).to_string())
            .collect();
        if !undefined.is_empty() {
            return Err(SecretsError::Undefined(undefined));
        }
        for name in names {
            self.resolve(name, &lookup)?;
        }
        Ok(())
    }

    /// Attemptの起動直前に一つの秘密値を解決する。呼び出し側はこの直前に
    /// [`load_from_env`]等でファイルを読み直す。
    pub fn resolve<F>(&self, name: &str, lookup: F) -> Result<SecretValue, SecretsError>
    where
        F: Fn(&str) -> Option<OsString>,
    {
        match self.secrets.get(name) {
            None => Err(SecretsError::Undefined(vec![name.to_string()])),
            Some(SecretSource::Value(value)) => Ok(value.clone()),
            Some(SecretSource::FromEnv(env)) => {
                let value = lookup(env).ok_or_else(|| SecretsError::MissingEnv {
                    name: name.to_string(),
                    env: env.clone(),
                })?;
                value
                    .into_string()
                    .ok()
                    .filter(|value| !value.contains('\0'))
                    .map(SecretValue)
                    .ok_or_else(|| SecretsError::InvalidEnvValue {
                        name: name.to_string(),
                        env: env.clone(),
                    })
            }
        }
    }
}

/// workflow設定が`{secret: <名前>}`で参照する名前(`defaults.env`とcommandタスクの`env`)。
#[must_use]
pub fn referenced_names(spec: &WorkflowSpec) -> BTreeSet<String> {
    let defaults = spec
        .defaults
        .iter()
        .flat_map(|defaults| defaults.env.values());
    let tasks = spec.tasks.values().flat_map(|task| match task {
        TaskDef::Command { env, .. } => Some(env.values()),
        TaskDef::Agent { .. } => None,
    });
    defaults
        .chain(tasks.flatten())
        .filter_map(|value| match value {
            EnvValue::Secret(secret) => Some(secret.secret.clone()),
            EnvValue::Value(_) | EnvValue::Unset => None,
        })
        .collect()
}

/// Run開始時に、workflow設定が参照する秘密値を外部操作の前に確認する。
pub fn check_spec<F>(spec: &WorkflowSpec, file: &SecretsFile, lookup: F) -> Result<(), SecretsError>
where
    F: Fn(&str) -> Option<OsString>,
{
    let names = referenced_names(spec);
    file.check_references(names.iter().map(String::as_str), lookup)
}

fn parse(text: &str, path: &Path) -> Result<SecretsFile, SecretsError> {
    let raw: RawSecretsFile = serde_yaml_ng::from_str(text).map_err(|error| {
        let location = error.location();
        SecretsError::Parse {
            path: path.to_path_buf(),
            line: location.as_ref().map(serde_yaml_ng::Location::line),
            column: location.as_ref().map(serde_yaml_ng::Location::column),
        }
    })?;
    if raw.version != 1 {
        return Err(SecretsError::UnsupportedVersion(raw.version));
    }
    let mut secrets = BTreeMap::new();
    for (name, secret) in raw.secrets {
        if !is_valid_resource_name(&name) {
            return Err(SecretsError::InvalidName(name));
        }
        let source = match (secret.value, secret.from_env) {
            (Some(value), None) => {
                if value.contains('\0') {
                    return Err(SecretsError::ContainsNul(name));
                }
                SecretSource::Value(SecretValue(value))
            }
            (None, Some(env)) => {
                if env.is_empty() || env.contains('=') || env.contains('\0') {
                    return Err(SecretsError::InvalidEnvName { name, env });
                }
                SecretSource::FromEnv(env)
            }
            _ => return Err(SecretsError::InvalidSource(name)),
        };
        secrets.insert(name, source);
    }
    Ok(SecretsFile { secrets })
}

/// 所有者と権限の条件。ファイル操作から分離して単体試験する。
fn check_access(
    path: &Path,
    is_file: bool,
    owner: u32,
    mode: u32,
    current: u32,
) -> Result<(), SecretsError> {
    if !is_file {
        return Err(SecretsError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    if owner != current {
        return Err(SecretsError::NotOwned {
            path: path.to_path_buf(),
            owner,
            current,
        });
    }
    if mode & GROUP_OTHER_READ_WRITE != 0 {
        return Err(SecretsError::InsecurePermissions {
            path: path.to_path_buf(),
            mode: mode & 0o7777,
        });
    }
    Ok(())
}

fn current_euid() -> u32 {
    // SAFETY: geteuid は引数を取らず、常に成功し、メモリへ触れない。
    unsafe { libc::geteuid() }
}

fn io_error(path: &Path, error: &std::io::Error) -> SecretsError {
    SecretsError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

/// `dir/secrets.yaml`を読み込む。ファイルがなければ空の定義を返す。
/// 所有者と権限は開いたファイルの属性で確認し、確認と読み取りの間の差し替えを避ける。
pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<SecretsFile, SecretsError> {
    use std::os::unix::fs::MetadataExt;

    let path = dir.as_ref().join(SECRETS_FILE_NAME);
    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SecretsFile::default());
        }
        Err(error) => return Err(io_error(&path, &error)),
    };
    let metadata = file.metadata().map_err(|error| io_error(&path, &error))?;
    check_access(
        &path,
        metadata.is_file(),
        metadata.uid(),
        metadata.mode(),
        current_euid(),
    )?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| io_error(&path, &error))?;
    parse(&text, &path)
}

/// `$HERDR_PLUGIN_CONFIG_DIR/secrets.yaml`を読み込む。変数が未設定・空なら空の定義を返す。
pub fn load_from_env() -> Result<SecretsFile, SecretsError> {
    let Some(dir) = std::env::var_os(CONFIG_DIR_ENV).filter(|value| !value.is_empty()) else {
        return Ok(SecretsFile::default());
    };
    load_from_dir(PathBuf::from(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SECRET: &str = "postgres://user:hunter2@localhost/db";

    fn parse_str(yaml: &str) -> Result<SecretsFile, SecretsError> {
        parse(yaml, Path::new(SECRETS_FILE_NAME))
    }

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    fn sample() -> SecretsFile {
        parse_str(&format!(
            "version: 1\nsecrets:\n  db-url:\n    value: '{SECRET}'\n  api-token:\n    fromEnv: MY_API_TOKEN\n"
        ))
        .expect("sample should parse")
    }

    /// 試験ごとに別のディレクトリを作る。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "herdr-workflow-secrets-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, text: &str, mode: u32) {
            let path = self.0.join(SECRETS_FILE_NAME);
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn documented_example_parses() {
        let file = sample();
        assert_eq!(file.names().collect::<Vec<_>>(), ["api-token", "db-url"]);
        assert_eq!(file.resolve("db-url", no_env).unwrap().expose(), SECRET);
    }

    #[test]
    fn debug_output_does_not_contain_values() {
        let file = sample();
        let value = file.resolve("db-url", no_env).unwrap();
        assert!(!format!("{file:?}").contains("hunter2"));
        assert!(!format!("{value:?}").contains("hunter2"));
    }

    #[test]
    fn from_env_is_read_from_lookup() {
        let file = sample();
        let value = file
            .resolve("api-token", |name| {
                (name == "MY_API_TOKEN").then(|| OsString::from("token-value"))
            })
            .unwrap();
        assert_eq!(value.expose(), "token-value");
    }

    #[test]
    fn missing_from_env_fails() {
        assert_eq!(
            sample().resolve("api-token", no_env),
            Err(SecretsError::MissingEnv {
                name: "api-token".to_string(),
                env: "MY_API_TOKEN".to_string(),
            })
        );
    }

    #[test]
    fn env_value_with_nul_is_rejected() {
        let err = sample()
            .resolve("api-token", |_| Some(OsString::from("a\0b")))
            .unwrap_err();
        assert!(matches!(err, SecretsError::InvalidEnvValue { .. }));
    }

    #[test]
    fn check_references_reports_all_undefined_names() {
        let err = sample()
            .check_references(["db-url", "zeta", "alpha"], no_env)
            .unwrap_err();
        assert_eq!(
            err,
            SecretsError::Undefined(vec!["alpha".to_string(), "zeta".to_string()])
        );
    }

    #[test]
    fn check_references_requires_from_env_values() {
        let file = sample();
        assert!(matches!(
            file.check_references(["api-token"], no_env),
            Err(SecretsError::MissingEnv { .. })
        ));
        assert_eq!(
            file.check_references(["db-url", "api-token"], |_| Some(OsString::from("x"))),
            Ok(())
        );
    }

    #[test]
    fn both_or_neither_source_is_rejected() {
        assert_eq!(
            parse_str("version: 1\nsecrets:\n  a:\n    value: x\n    fromEnv: X\n"),
            Err(SecretsError::InvalidSource("a".to_string()))
        );
        assert_eq!(
            parse_str("version: 1\nsecrets:\n  a: {}\n"),
            Err(SecretsError::InvalidSource("a".to_string()))
        );
    }

    #[test]
    fn invalid_names_and_env_names_are_rejected() {
        assert_eq!(
            parse_str("version: 1\nsecrets:\n  Bad_Name:\n    value: x\n"),
            Err(SecretsError::InvalidName("Bad_Name".to_string()))
        );
        assert!(matches!(
            parse_str("version: 1\nsecrets:\n  a:\n    fromEnv: 'A=B'\n"),
            Err(SecretsError::InvalidEnvName { .. })
        ));
        assert!(matches!(
            parse_str("version: 1\nsecrets:\n  a:\n    fromEnv: ''\n"),
            Err(SecretsError::InvalidEnvName { .. })
        ));
    }

    #[test]
    fn version_must_be_one() {
        assert_eq!(
            parse_str("version: 2\n"),
            Err(SecretsError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn parse_errors_do_not_quote_values() {
        // 型の誤りや未知キーのエラー本文は値を引用しうるため、位置だけを返す。
        for yaml in [
            format!("version: 1\nsecrets:\n  a:\n    value: ['{SECRET}']\n"),
            format!("version: 1\nsecrets:\n  a:\n    value: '{SECRET}'\n    extra: '{SECRET}'\n"),
            format!("version: 1\nsecrets:\n  a:\n    value: '{SECRET}'\n  a:\n    value: x\n"),
        ] {
            let err = parse_str(&yaml).unwrap_err();
            assert!(matches!(err, SecretsError::Parse { .. }), "{err:?}");
            assert!(!err.to_string().contains("hunter2"), "{err}");
        }
    }

    #[test]
    fn access_check_requires_owner_and_private_mode() {
        let path = Path::new("secrets.yaml");
        assert_eq!(check_access(path, true, 1000, 0o100600, 1000), Ok(()));
        assert_eq!(check_access(path, true, 1000, 0o100400, 1000), Ok(()));
        // 実行権限は読み書きではないため条件に含めない。
        assert_eq!(check_access(path, true, 1000, 0o100711, 1000), Ok(()));
        assert!(matches!(
            check_access(path, true, 0, 0o100600, 1000),
            Err(SecretsError::NotOwned { .. })
        ));
        for mode in [0o100640, 0o100620, 0o100604, 0o100602] {
            assert!(matches!(
                check_access(path, true, 1000, mode, 1000),
                Err(SecretsError::InsecurePermissions { .. })
            ));
        }
        assert!(matches!(
            check_access(path, false, 1000, 0o040700, 1000),
            Err(SecretsError::NotRegularFile { .. })
        ));
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = TempDir::new();
        assert_eq!(load_from_dir(&dir.0), Ok(SecretsFile::default()));
    }

    #[test]
    fn private_file_is_loaded() {
        let dir = TempDir::new();
        dir.write("version: 1\nsecrets:\n  db-url:\n    value: x\n", 0o600);
        let file = load_from_dir(&dir.0).unwrap();
        assert_eq!(file.names().collect::<Vec<_>>(), ["db-url"]);
    }

    #[test]
    fn group_readable_file_is_rejected_without_changing_mode() {
        let dir = TempDir::new();
        dir.write("version: 1\n", 0o640);
        let err = load_from_dir(&dir.0).unwrap_err();
        assert!(matches!(
            err,
            SecretsError::InsecurePermissions { mode: 0o640, .. }
        ));
        let mode = std::fs::metadata(dir.0.join(SECRETS_FILE_NAME))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o640);
    }

    #[test]
    fn directory_in_place_of_file_is_rejected() {
        let dir = TempDir::new();
        std::fs::create_dir(dir.0.join(SECRETS_FILE_NAME)).unwrap();
        assert!(matches!(
            load_from_dir(&dir.0),
            Err(SecretsError::NotRegularFile { .. })
        ));
    }

    #[test]
    fn referenced_names_cover_defaults_and_command_tasks() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml");
        let mut spec = crate::config::load::load_file(path).unwrap();
        let secret = |name: &str| {
            EnvValue::Secret(crate::config::model::SecretRef {
                secret: name.to_string(),
            })
        };
        spec.defaults
            .as_mut()
            .expect("example has defaults")
            .env
            .insert("TOKEN".to_string(), secret("api-token"));
        let Some(TaskDef::Command { env, .. }) = spec
            .tasks
            .values_mut()
            .find(|task| matches!(task, TaskDef::Command { .. }))
        else {
            panic!("example has a command task");
        };
        env.insert("DATABASE_URL".to_string(), secret("db-url"));
        let names = referenced_names(&spec);
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            ["api-token", "db-url"]
        );
        assert_eq!(
            check_spec(&spec, &sample(), |_| Some(OsString::from("x"))),
            Ok(())
        );
        assert!(matches!(
            check_spec(&spec, &SecretsFile::default(), no_env),
            Err(SecretsError::Undefined(_))
        ));
    }
}
