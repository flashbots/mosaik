use {
	super::TicketValidator,
	crate::{
		UniqueId,
		discovery::PeerEntry,
		id,
		tickets::{Expiration, InvalidTicket},
	},
	chrono::Utc,
	itertools::Itertools,
	jwt::{FromBase64, Header, ToBase64, VerifyingAlgorithm},
	serde_json::Value,
	std::{collections::BTreeMap, sync::Arc},
};

/// A configurable JWT ticket validator.
///
/// Validates the `iss`, `sub`, `exp`, and `nbf` claims of a JWT ticket
/// presented by a peer. Build instances via the builder methods on this type
/// ([`with_key`], [`allow_issuer`], [`allow_audience`], [`require_subject`],
/// [`require_claim`], [`allow_non_expiring`]).
///
/// # Group identity
///
/// [`TicketValidator::signature`] is derived from the validator configuration,
/// so two [`JwtTicketValidator`] instances with different settings will produce
/// different signatures — and therefore different group ids.
///
/// [`with_key`]: JwtTicketValidator::with_key
/// [`allow_issuer`]: JwtTicketValidator::allow_issuer
/// [`allow_audience`]: JwtTicketValidator::allow_audience
/// [`require_subject`]: JwtTicketValidator::require_subject
/// [`require_claim`]: JwtTicketValidator::require_claim
/// [`allow_non_expiring`]: JwtTicketValidator::allow_non_expiring
#[derive(Clone)]
pub struct JwtTicketValidator {
	/// The expected issuer claim in the JWT.
	///
	/// If this list is non-empty, then the JWT must contain an "iss" claim that
	/// matches one of the issuers in the list.
	issuers: Vec<String>,

	/// Whether to allow non-expiring JWTs (i.e. those without an "exp" claim).
	allow_non_expiring: bool,

	/// The expected subject claim in the JWT.
	///
	/// If not set, then the subject must be the peer id in lower-case hex.
	subject: Option<String>,

	/// Required `aud` claim in the JWT with the specified value.
	///
	/// If this list is non-empty, then the JWT must contain an "aud" claim that
	/// matches one of the values in the list.
	audience: Vec<String>,

	/// A set of custom claims that must be present in the JWT with the specified
	/// values.
	custom_claims: BTreeMap<String, Value>,

	/// The verifying algorithm and key to use for validating the JWT signature.
	alg: Arc<dyn ::jwt::VerifyingAlgorithm + Send + Sync + 'static>,
}

impl JwtTicketValidator {
	/// Creates a new JWT ticket validator against the specified verifying key.
	#[must_use]
	pub fn with_key(
		key: impl VerifyingAlgorithm + Send + Sync + 'static,
	) -> Self {
		Self {
			alg: Arc::new(key),
			issuers: Vec::new(),
			allow_non_expiring: false,
			subject: None,
			audience: Vec::new(),
			custom_claims: BTreeMap::new(),
		}
	}

	/// Requires the JWT token to contain an `sub` claim that matches the
	/// specified subject.
	///
	/// If not set, then the subject must be the peer id in lower-case hex.
	#[must_use]
	pub fn require_subject(mut self, subject: impl Into<String>) -> Self {
		self.subject = Some(subject.into());
		self
	}

	/// Requires the JWT token to contain custom claims with the specified names
	/// and values.
	#[must_use]
	pub fn require_claim(
		mut self,
		name: impl Into<String>,
		value: impl Into<Value>,
	) -> Self {
		self.custom_claims.insert(name.into(), value.into());
		self
	}

	/// Allows JWT tokens that do not contain an `exp` claim and therefore do not
	/// expire.
	#[must_use]
	pub const fn allow_non_expiring(mut self) -> Self {
		self.allow_non_expiring = true;
		self
	}

	/// Requires the JWT token to contain an `iss` claim that matches the
	/// specified issuer.
	///
	/// If called multiple times, the JWT must contain an `iss` claim that matches
	/// at least one of the specified issuers.
	///
	/// If never called, then no `iss` claim is required in the JWT.
	#[must_use]
	pub fn allow_issuer(mut self, issuer: impl Into<String>) -> Self {
		self.issuers.push(issuer.into());
		self
	}

	/// Requires the JWT token to contain an `aud` claim that matches the
	/// specified audience.
	///
	/// If called multiple times, the JWT must contain an `aud` claim that matches
	/// at least one of the specified audiences.
	///
	/// If never called, then no `aud` claim is required in the JWT.
	#[must_use]
	pub fn allow_audience(mut self, audience: impl Into<String>) -> Self {
		self.audience.push(audience.into());
		self
	}
}

impl JwtTicketValidator {
	pub const CLASS: UniqueId = id!("mosaik.tickets.jwt.v1");
}

impl TicketValidator for JwtTicketValidator {
	fn class(&self) -> UniqueId {
		Self::CLASS
	}

	fn signature(&self) -> UniqueId {
		let mut sig = self
			.class()
			.derive(
				self
					.alg
					.algorithm_type()
					.to_base64()
					.expect("algorithm type is base64 encodable")
					.as_bytes(),
			)
			.derive(if self.allow_non_expiring { [1] } else { [0] })
			.derive(self.issuers.iter().join(","))
			.derive(self.audience.iter().join(","))
			.derive(self.subject.as_deref().unwrap_or(""));

		for (key, value) in &self.custom_claims {
			sig = sig.derive(key).derive(value.to_string());
		}

		sig
	}

	fn validate(
		&self,
		ticket: &[u8],
		peer: &PeerEntry,
	) -> Result<Expiration, InvalidTicket> {
		let expected_subject = self
			.subject
			.clone()
			.unwrap_or_else(|| peer.id().to_string().to_lowercase());

		let jwt_str = core::str::from_utf8(ticket).map_err(|_| InvalidTicket)?;

		// Split the JWT into its three dot-separated components.
		let mut parts = jwt_str.splitn(3, '.');
		let header_str = parts.next().ok_or(InvalidTicket)?;
		let claims_str = parts.next().ok_or(InvalidTicket)?;
		let sig_str = parts.next().ok_or(InvalidTicket)?;

		// Reject if the header declares a different algorithm than the key.
		let header = Header::from_base64(header_str).map_err(|_| InvalidTicket)?;
		if header.algorithm != self.alg.algorithm_type() {
			return Err(InvalidTicket);
		}

		// Verify the cryptographic signature.
		if !self
			.alg
			.verify(header_str, claims_str, sig_str)
			.map_err(|_| InvalidTicket)?
		{
			return Err(InvalidTicket);
		}

		// Decode the claims payload.
		let claims =
			jwt::Claims::from_base64(claims_str).map_err(|_| InvalidTicket)?;
		let reg = &claims.registered;
		let now = Utc::now().timestamp().cast_unsigned();

		// Validate issuer.
		if !self.issuers.is_empty() {
			match reg.issuer.as_deref() {
				Some(iss) if self.issuers.iter().any(|i| i == iss) => {}
				_ => return Err(InvalidTicket),
			}
		}

		// Validate subject.
		if reg.subject.as_deref() != Some(expected_subject.as_str()) {
			return Err(InvalidTicket);
		}

		// Validate not-before.
		if let Some(nbf) = reg.not_before
			&& nbf > now
		{
			return Err(InvalidTicket);
		}

		// Validate expiration.
		let exp = reg.expiration;
		match exp {
			None if !self.allow_non_expiring => return Err(InvalidTicket),
			Some(e) if e <= now => return Err(InvalidTicket),
			_ => {}
		}

		// Validate audience.
		if !self.audience.is_empty() {
			match reg.audience.as_deref() {
				Some(aud) if self.audience.iter().any(|a| a == aud) => {}
				_ => return Err(InvalidTicket),
			}
		}

		// Validate custom claims.
		for (key, expected_value) in &self.custom_claims {
			if claims.private.get(key) != Some(expected_value) {
				return Err(InvalidTicket);
			}
		}

		// Convert expiration timestamp. Reject rather than silently accepting
		// a non-representable timestamp (e.g. exp > i64::MAX) as Expiration::Never.
		Ok(match exp {
			None => Expiration::Never,
			Some(ts) => {
				let ts_i64 = i64::try_from(ts).map_err(|_| InvalidTicket)?;
				let dt =
					chrono::DateTime::from_timestamp(ts_i64, 0).ok_or(InvalidTicket)?;
				Expiration::At(dt)
			}
		})
	}
}
