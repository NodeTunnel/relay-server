use reqwest::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("http request failed to send: {0}")]
    SendFailure(#[from] reqwest::Error),
    
    #[error("unexpected status: {0}")]
    UnexpectedStatus(StatusCode)
}