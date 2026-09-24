//! Convert failure into process exit code.

use crate::common::{ProviderError, ProviderFailure, ResolveError};

pub const EXIT_GENERIC: u8 = 1;
pub const EXIT_NOT_FOUND: u8 = 3;
pub const EXIT_BLOCKED: u8 = 4;
pub const EXIT_NETWORK: u8 = 5;
/// Ctrl+C, by the usual `128 + SIGINT` convention.
pub const EXIT_CANCELLED: u8 = 130;

/// CLI error carrying the process exit code to use.
pub struct AppError {
	pub code: u8,
	pub source: anyhow::Error,
}

impl From<serde_json::Error> for AppError {
	fn from(e: serde_json::Error) -> Self {
		Self {
			code: EXIT_GENERIC,
			source: e.into(),
		}
	}
}

/// Exit code for one provider's failure.
pub fn provider_error_code(e: &ProviderError) -> u8 {
	match e {
		ProviderError::NotFound(_) => EXIT_NOT_FOUND,
		ProviderError::Blocked => EXIT_BLOCKED,
		ProviderError::Network(_) => EXIT_NETWORK,
		ProviderError::Cancelled => EXIT_CANCELLED,
		ProviderError::ParseError(_) => EXIT_GENERIC,
	}
}

impl From<ProviderFailure> for AppError {
	fn from(f: ProviderFailure) -> Self {
		Self {
			code: provider_error_code(&f.source),
			source: anyhow::anyhow!("{f}"),
		}
	}
}

impl From<ResolveError> for AppError {
	fn from(e: ResolveError) -> Self {
		// If every provider failed the same way, that's the reason; otherwise a
		// network failure is the one the user can act on.
		let codes: Vec<u8> = e.attempts.iter().map(|f| provider_error_code(&f.source)).collect();
		let code = match codes.first() {
			Some(&c) if codes.iter().all(|&x| x == c) => c,
			_ if codes.contains(&EXIT_NETWORK) => EXIT_NETWORK,
			_ => EXIT_GENERIC,
		};
		Self {
			code,
			source: anyhow::anyhow!("{e}"),
		}
	}
}
