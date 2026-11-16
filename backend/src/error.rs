//! Error types shared by the storage, Discord and HTTP layers.

use std::fmt;

/// An internal failure described by a short message. Never shown to clients.
#[derive(Debug)]
pub struct Fail(pub String);

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<std::io::Error> for Fail {
    fn from(error: std::io::Error) -> Self {
        Self(format!("io:{error}"))
    }
}

impl From<serde_json::Error> for Fail {
    fn from(error: serde_json::Error) -> Self {
        Self(format!("json:{error}"))
    }
}

impl From<reqwest::Error> for Fail {
    fn from(error: reqwest::Error) -> Self {
        // `without_url` keeps query strings and tokens out of the logs.
        Self(format!("http:{}", error.without_url()))
    }
}
