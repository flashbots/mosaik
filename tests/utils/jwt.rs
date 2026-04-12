use {
	chrono::Utc,
	core::time::Duration,
	itertools::Itertools,
	jwt::{FromBase64, Header, ToBase64, VerifyingAlgorithm},
	mosaik::{
		UniqueId,
		discovery::PeerEntry,
		id,
		primitives::{Expiration, InvalidTicket, TicketValidator},
	},
	serde_json::Value,
	std::{collections::BTreeMap, sync::Arc},
};

/// A configurable JWT ticket validator.
///
/// Validates the `iss`, `sub`, `exp`, `nbf`, and `iat` claims of a
/// JWT ticket presented by a peer. Build instances via the builder
/// methods on this type ([`with_key`], [`add_key`],
/// [`allow_issuer`], [`allow_audience`], [`require_subject`],
/// [`require_claim`], [`allow_non_expiring`], [`max_lifetime`],
/// [`max_age`]).
///
/// # Group identity
///
/// [`TicketValidator::signature`] is derived from the validator
/// configuration, so two [`Jwt`] instances with
/// different settings will produce different signatures — and
/// therefore different group ids.
///
/// [`with_key`]: Jwt::with_key
/// [`add_key`]: Jwt::add_key
/// [`allow_issuer`]: Jwt::allow_issuer
/// [`allow_audience`]: Jwt::allow_audience
/// [`require_subject`]: Jwt::require_subject
/// [`require_claim`]: Jwt::require_claim
/// [`allow_non_expiring`]: Jwt::allow_non_expiring
/// [`max_lifetime`]: Jwt::max_lifetime
/// [`max_age`]: Jwt::max_age
#[derive(Clone)]
pub struct Jwt {
	/// The expected issuer `iss` claim in the JWT.
	///
	/// If this list is non-empty, then the JWT must contain an `iss` claim that
	/// matches one of the issuers in the list.
	issuers: Vec<String>,

	/// Whether to allow non-expiring JWTs (i.e. those without an `exp` claim).
	allow_non_expiring: bool,

	/// The expected subject `sub` claim in the JWT.
	///
	/// If not set, then the subject must be the peer id in lower-case hex.
	subject: Option<String>,

	/// Required `aud` claim in the JWT with the specified value.
	///
	/// If this list is non-empty, then the JWT must contain an `aud` claim that
	/// matches one of the values in the list.
	audience: Vec<String>,

	/// A set of custom claims that must be present in the JWT with the specified
	/// values.
	custom_claims: BTreeMap<String, Value>,

	/// The maximum allowed remaining lifetime of a token. If set,
	/// tokens with `exp - now > max_lifetime` are rejected.
	max_lifetime: Option<Duration>,

	/// The maximum allowed age of a token since issuance. If set, the
	/// `iat` claim is required and tokens where `now - iat > max_age`
	/// are rejected.
	max_age: Option<Duration>,

	/// The verifying algorithms and keys to use for validating the JWT
	/// signature. Multiple keys support zero-downtime key rotation.
	keys: Vec<Arc<dyn ::jwt::VerifyingAlgorithm + Send + Sync + 'static>>,
}

impl Jwt {
	pub const CLASS: UniqueId = id!("mosaik.tickets.jwt.v1");

	/// Creates a new JWT ticket validator against the specified verifying key.
	#[must_use]
	pub fn with_key(
		key: impl VerifyingAlgorithm + Send + Sync + 'static,
	) -> Self {
		Self {
			keys: vec![Arc::new(key)],
			issuers: Vec::new(),
			allow_non_expiring: false,
			subject: None,
			audience: Vec::new(),
			custom_claims: BTreeMap::new(),
			max_lifetime: None,
			max_age: None,
		}
	}

	/// Adds an additional verifying key for signature validation.
	///
	/// When multiple keys are configured, the JWT signature is
	/// verified against each key with a matching algorithm type
	/// until one succeeds. This enables zero-downtime key rotation
	/// by accepting tokens signed with either the current or
	/// previous key.
	#[must_use]
	pub fn add_key(
		mut self,
		key: impl VerifyingAlgorithm + Send + Sync + 'static,
	) -> Self {
		self.keys.push(Arc::new(key));
		self
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

	/// Sets the maximum allowed remaining lifetime of a token.
	///
	/// If set, tokens with an `exp` claim where `exp - now` exceeds
	/// this duration are rejected. This prevents acceptance of
	/// tokens with unreasonably distant expiration times.
	#[must_use]
	pub const fn max_lifetime(mut self, duration: Duration) -> Self {
		self.max_lifetime = Some(duration);
		self
	}

	/// Sets the maximum allowed age of a token since issuance.
	///
	/// If set, the `iat` (issued-at) claim is required and tokens
	/// where `now - iat` exceeds this duration are rejected. This
	/// prevents acceptance of tokens that were issued too far in
	/// the past.
	#[must_use]
	pub const fn max_age(mut self, duration: Duration) -> Self {
		self.max_age = Some(duration);
		self
	}
}

impl TicketValidator for Jwt {
	fn class(&self) -> UniqueId {
		Self::CLASS
	}

	fn signature(&self) -> UniqueId {
		let mut sig = self.class();

		for key in &self.keys {
			sig = sig.derive(
				key
					.algorithm_type()
					.to_base64()
					.expect("algorithm type is base64 encodable")
					.as_bytes(),
			);
		}

		sig = sig
			.derive(if self.allow_non_expiring { [1] } else { [0] })
			.derive(self.issuers.iter().join(","))
			.derive(self.audience.iter().join(","))
			.derive(self.subject.as_deref().unwrap_or(""))
			.derive(self.max_lifetime.map_or(0, |d| d.as_secs()).to_le_bytes())
			.derive(self.max_age.map_or(0, |d| d.as_secs()).to_le_bytes());

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

		// Reject if the header declares an algorithm that none of the
		// configured keys support.
		let header = Header::from_base64(header_str).map_err(|_| InvalidTicket)?;
		if !self
			.keys
			.iter()
			.any(|k| k.algorithm_type() == header.algorithm)
		{
			return Err(InvalidTicket);
		}

		// Verify the cryptographic signature against each key with a
		// matching algorithm type. Succeed if any key verifies.
		let signature_verified = self
			.keys
			.iter()
			.filter(|k| k.algorithm_type() == header.algorithm)
			.any(|k| k.verify(header_str, claims_str, sig_str).unwrap_or(false));

		if !signature_verified {
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

		// Validate maximum token lifetime.
		if let Some(max_lifetime) = self.max_lifetime
			&& let Some(e) = exp
		{
			let remaining = e.saturating_sub(now);
			if remaining > max_lifetime.as_secs() {
				return Err(InvalidTicket);
			}
		}

		// Validate issued-at age.
		if let Some(max_age) = self.max_age {
			match reg.issued_at {
				Some(iat) => {
					let age = now.saturating_sub(iat);
					if age > max_age.as_secs() {
						return Err(InvalidTicket);
					}
				}
				None => return Err(InvalidTicket),
			}
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
