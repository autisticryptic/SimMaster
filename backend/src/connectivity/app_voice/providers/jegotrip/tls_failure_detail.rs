//! Finite TLS failure categories, not peer values, certificates or trust grants.
//! Never format/store rustls errors: context variants can contain private names,
//! algorithms, certificate material or arbitrary provider text.
use super::session::AttemptOwner;
use serde::Serialize;
use std::sync::Mutex;
use tokio_rustls::rustls;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all="snake_case")]
pub enum TlsFailureReason {
    MalformedRecord, ReadFailed, NoPeerChain, NameMismatch, ExactNameMissing,
    UnknownIssuer, Expired, NotYetValid, Revoked, BadEncoding, BadSignature,
    UnsupportedSignature, InvalidPurpose, InvalidChain, NoProtocolVersion,
    NoCipherSuite, PeerAlert, PeerProtocol, DecryptionFailed, ProtocolOther,
}
impl TlsFailureReason {
    pub(super) fn from_rustls(error:&rustls::Error)->Self {
        use rustls::{CertificateError as C,Error as E,PeerIncompatible as P};
        match error {
            E::InvalidCertificate(value)=>match value {
                C::BadEncoding=>Self::BadEncoding,
                C::Expired|C::ExpiredContext{..}=>Self::Expired,
                C::NotValidYet|C::NotValidYetContext{..}=>Self::NotYetValid,
                C::Revoked=>Self::Revoked,
                C::UnknownIssuer=>Self::UnknownIssuer,
                C::BadSignature=>Self::BadSignature,
                C::UnsupportedSignatureAlgorithmContext{..}|C::UnsupportedSignatureAlgorithmForPublicKeyContext{..}=>Self::UnsupportedSignature,
                C::NotValidForName|C::NotValidForNameContext{..}=>Self::NameMismatch,
                C::InvalidPurpose|C::InvalidPurposeContext{..}=>Self::InvalidPurpose,
                _=>Self::InvalidChain,
            },
            E::NoCertificatesPresented=>Self::NoPeerChain,
            E::UnsupportedNameType=>Self::NameMismatch,
            E::PeerIncompatible(P::ServerDoesNotSupportTls12Or13|P::ServerTlsVersionIsDisabledByOurConfig|P::Tls12NotOffered|P::Tls12NotOfferedOrEnabled)=>Self::NoProtocolVersion,
            E::PeerIncompatible(P::NoCipherSuitesInCommon)=>Self::NoCipherSuite,
            E::PeerIncompatible(P::NoSignatureSchemesInCommon|P::NoCertificateRequestSignatureSchemesInCommon)=>Self::UnsupportedSignature,
            E::AlertReceived(_)=>Self::PeerAlert,
            E::InvalidMessage(_)|E::InappropriateMessage{..}|E::InappropriateHandshakeMessage{..}|E::PeerMisbehaved(_)=>Self::PeerProtocol,
            E::DecryptError=>Self::DecryptionFailed,
            _=>Self::ProtocolOther,
        }
    }
}
struct Recorded {owner:AttemptOwner,reason:TlsFailureReason}
/// One slot shared by clones of one original connection scope. It stores no
/// scope reference (no ownership cycle), raw error, time or reset/retry state.
pub(super) struct TlsDiagnosticStore(Mutex<Option<Recorded>>);
impl TlsDiagnosticStore {
    pub(super) fn original()->Self {Self(Mutex::new(None))}
    pub(super) fn record(&self,owner:&AttemptOwner,reason:TlsFailureReason){
        let mut first=self.0.lock().unwrap_or_else(|p|p.into_inner());
        if first.is_none(){*first=Some(Recorded{owner:owner.clone(),reason});}
    }
    pub(super) fn for_owner(&self,owner:&AttemptOwner)->Option<TlsFailureReason>{
        self.0.lock().unwrap_or_else(|p|p.into_inner()).as_ref().filter(|value|&value.owner==owner).map(|value|value.reason)
    }
}
#[cfg(test)]
#[path="tls_failure_detail_tests.rs"]
mod tests;
