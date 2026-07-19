use std::future::Future;
use std::pin::Pin;

use codex_protocol::ThreadId;
use http::HeaderValue;

pub(crate) const X_OAI_ATTESTATION_HEADER: &str = "x-oai-attestation";

pub type GenerateAttestationFuture<'a> =
    Pin<Box<dyn Future<Output = Option<HeaderValue>> + Send + 'a>>;

pub type BypassSandboxAttestationFuture<'a> =
    Pin<Box<dyn Future<Output = Option<BypassSandboxAttestation>> + Send + 'a>>;

/// Backend-created sandbox/isolation attestation for entering Bypass mode.
///
/// This is deliberately separate from the upstream `x-oai-attestation` header:
/// model request attestation is not enough to prove local tool sandboxing. A
/// host integration must validate local isolation and return this proof for the
/// current thread.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BypassSandboxAttestation {
    thread_id: ThreadId,
    isolated: bool,
}

impl BypassSandboxAttestation {
    pub fn backend_sandbox_isolation(thread_id: ThreadId, isolated: bool) -> Self {
        Self {
            thread_id,
            isolated,
        }
    }

    pub fn mode_attestation_for(
        self,
        thread_id: ThreadId,
    ) -> Option<cccx_mode::BypassModeAttestation> {
        if self.isolated && self.thread_id == thread_id {
            Some(cccx_mode::BypassModeAttestation::backend_sandbox_isolation())
        } else {
            None
        }
    }
}

/// Request context that host integrations can use when deciding whether to
/// generate an attestation header value.
#[derive(Clone, Copy, Debug)]
pub struct AttestationContext {
    /// Thread whose upstream request is being prepared.
    pub thread_id: ThreadId,
}

/// Host integration boundary for just-in-time attestation header values.
///
/// Implementations own the policy for when attestation should be attempted and
/// return the upstream `x-oai-attestation` header value when one should be sent.
pub trait AttestationProvider: std::fmt::Debug + Send + Sync {
    fn header_for_request(&self, context: AttestationContext) -> GenerateAttestationFuture<'_>;

    fn bypass_sandbox_attestation_for(
        &self,
        _context: AttestationContext,
    ) -> BypassSandboxAttestationFuture<'_> {
        Box::pin(async { None })
    }
}
