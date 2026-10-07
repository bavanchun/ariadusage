use thiserror::Error;

/// Errors arising from domain model invariants and validations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelError {
    #[error("value must be finite: {0}")]
    NonFiniteNumber(String),
    #[error("section count exceeds maximum of 8: got {0}")]
    TooManySections(usize),
}
