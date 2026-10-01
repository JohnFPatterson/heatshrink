use std::fmt;

/// Failure from a heatshrink constructor or a streaming call on untrusted input.
///
/// Truncated or arbitrary compressed bytes are not an error: the decoder
/// suspends (`Poll::Empty` / `Finish::More`) the way the C library does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Window or lookahead (or a zero decoder input buffer) is outside the
    /// range enforced by `heatshrink_encoder_alloc` / `heatshrink_decoder_alloc`.
    InvalidParams,
    /// `sink` called while finishing, or before the previous buffer was polled.
    Misuse,
    /// Encoder `poll` was given an empty output buffer (`HSER_POLL_ERROR_MISUSE`).
    EmptyOutput,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidParams => write!(f, "invalid heatshrink parameters"),
            Error::Misuse => write!(f, "heatshrink streaming API misuse"),
            Error::EmptyOutput => write!(f, "encoder poll output buffer is empty"),
        }
    }
}

impl std::error::Error for Error {}
