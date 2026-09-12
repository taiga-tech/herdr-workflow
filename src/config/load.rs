use std::path::{Path, PathBuf};

use crate::config::model::WorkflowSpec;

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse workflow config: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
}

pub fn load_str(yaml: &str) -> Result<WorkflowSpec, LoadError> {
    serde_yaml_ng::from_str(yaml).map_err(LoadError::from)
}

pub fn load_file(path: impl AsRef<Path>) -> Result<WorkflowSpec, LoadError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    load_str(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow.yaml")
    }

    #[test]
    fn load_file_reads_and_parses_the_example_config() {
        let spec = load_file(example_path()).expect("example config should load");
        assert_eq!(spec.name, "web-development");
    }

    #[test]
    fn load_file_reports_io_error_for_missing_file() {
        let err = load_file("does/not/exist.yaml").expect_err("missing file should error");
        assert!(matches!(err, LoadError::Io { .. }));
    }

    #[test]
    fn load_str_reports_yaml_error_for_invalid_syntax() {
        let err = load_str("tasks: [").expect_err("invalid yaml should error");
        assert!(matches!(err, LoadError::Yaml(_)));
    }
}
