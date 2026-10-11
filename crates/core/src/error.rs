use std::path::PathBuf;

/// Errors raised by pure core logic: config loading, validation, planning.
///
/// Every variant carries enough context to act on without a stack trace. A
/// config failure in particular must name the file and the reason, because the
/// only place it surfaces is a server that refuses to boot.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("config file {path} could not be read: {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("config file {path} is not valid TOML: {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("config is invalid: {0}")]
    ConfigInvalid(String),

    #[error("validation failed for {field}: {reason}")]
    Validation { field: String, reason: String },
}

impl CoreError {
    /// Convenience for the common `Validation` construction.
    pub fn validation(field: impl Into<String>, reason: impl Into<String>) -> Self {
        CoreError::Validation {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

pub type CoreResult<T> = Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_errors_render_a_useful_message() {
        let missing = CoreError::ConfigRead {
            path: PathBuf::from("/etc/stvdy/plans.toml"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        };
        let message = missing.to_string();
        assert!(
            message.contains("/etc/stvdy/plans.toml"),
            "the message must name the file: {message}"
        );
        assert!(
            message.contains("no such file"),
            "the message must carry the cause: {message}"
        );

        let invalid = CoreError::ConfigInvalid("plans.toml must define a [free] plan".into());
        assert!(invalid.to_string().contains("[free]"));

        let validation = CoreError::validation("exam_date", "must be in the future");
        assert!(validation.to_string().contains("exam_date"));
        assert!(validation.to_string().contains("must be in the future"));
    }

    #[test]
    fn config_read_exposes_its_source_for_error_chains() {
        use std::error::Error;

        let error = CoreError::ConfigRead {
            path: PathBuf::from("config/plans.toml"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        };
        // `#[source]` must be wired up, or `anyhow`'s chain loses the cause.
        let source = error.source().expect("source must be exposed");
        assert_eq!(source.to_string(), "denied");
    }

    fn parse(flag: bool) -> Result<u8, CoreError> {
        if flag {
            Ok(7)
        } else {
            Err(CoreError::ConfigInvalid("off".into()))
        }
    }

    #[test]
    fn unwrap_is_allowed_in_test_code() {
        // Verifies clippy.toml's `allow-unwrap-in-tests` actually applies to a
        // realistic call-result unwrap. (A literal construction such as
        // `Some(1).unwrap()` trips the unrelated `unnecessary_literal_unwrap`
        // lint instead, which is not part of the test relaxation.)
        assert_eq!(parse(true).unwrap(), 7);
        assert!(parse(false).unwrap_err().to_string().contains("off"));
    }
}
