//! AGT-01 generative provider gateway.
//!
//! The gateway is the only path from runtime work to a Chat Completions
//! provider. It composes the pieces the contract requires without
//! owning any of them:
//!
//! - [`sse`] frames the transport stream; [`chat`] assembles and
//!   validates the Chat Completions response (tool-call arguments are
//!   JSON-validated before the call is exposed, so "validated tool
//!   calls are assembled before dispatch" is a property of this
//!   layer, not a caller convention).
//! - [`gateway`] runs the call: endpoint parsing, the #110 consent and
//!   address-classification rules, the #112 admission gate, typed
//!   timeouts, the retry rules the contract allows, and the pinned
//!   GenAI telemetry published through the existing `DiagnosticHub`.
//!
//! No credential value, prompt content or endpoint userinfo ever
//! enters an assembled value, a diagnostic record or an error string.

pub mod chat;
pub mod gateway;
pub mod sse;
