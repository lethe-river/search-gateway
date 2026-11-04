use thiserror::Error;

/// Errors that prevent the gateway from starting or from serving a request.
///
/// Provider failures are not represented here: they are structured
/// [`crate::model::ProviderError`] values placed in a tool response.
#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("config error: {0}")]
    Config(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

impl GatewayError {
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }
}
