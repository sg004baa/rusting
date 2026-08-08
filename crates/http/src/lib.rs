//! Building and sending a request, and the types the result is reported in.

pub mod client;
pub mod send;
pub mod timing;

pub mod types;

pub use types::{Phase, PhaseEvent, PhaseOutcome, Response, SendError, SentRequest, Timings};
