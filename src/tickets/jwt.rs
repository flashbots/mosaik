use {
	crate::{
		Digest,
		PeerId,
		UniqueId,
		discovery::PeerEntry,
		id,
		tickets::{Expiration, InvalidTicket, Ticket, TicketValidator},
	},
	chrono::Utc,
	core::time::Duration,
	itertools::Itertools,
	jwt::{
		FromBase64,
		RegisteredClaims,
		SigningAlgorithm,
		ToBase64,
		VerifyingAlgorithm,
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
/// [`TicketValidator::signature`] is derived from the full
/// validator configuration — including the key fingerprints — so
/// two [`Jwt`] instances with different settings or different keys
/// will produce different signatures and therefore different group
/// ids.
///
/// # Example
///
/// ```ignore
/// use mosaik::tickets::{Jwt, Hs256};
///
/// let validator = Jwt::with_key(Hs256::hex("87e50edd90e2f9d53a7f2a9bd51c1069a454b827f0e1002577c54e1c2a598568"))
///     .allow_issuer("my-auth-service")
///     .max_lifetime(Duration::from_secs(3600));
/// ```
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
	/// If this list is non-empty, then the JWT must contain an `iss`
	/// claim that matches one of the issuers in the list.
	issuers: Vec<String>,

	/// Whether to allow non-expiring JWTs (i.e. those without an
	/// `exp` claim).
	allow_non_expiring: bool,

	/// The expected subject `sub` claim in the JWT.
	///
	/// If not set, then the subject must be the peer id in
	/// lower-case hex.
	subject: Option<String>,

	/// Required `aud` claim in the JWT with the specified value.
	///
	/// If this list is non-empty, then the JWT must contain an `aud`
	/// claim that matches one of the values in the list.
	audience: Vec<String>,

	/// A set of custom claims that must be present in the JWT with
	/// the specified values.
	custom_claims: BTreeMap<String, Value>,

	/// The maximum allowed remaining lifetime of a token. If set,
	/// tokens with `exp - now > max_lifetime` are rejected.
	max_lifetime: Option<Duration>,

	/// The maximum allowed age of a token since issuance. If set,
	/// the `iat` claim is required and tokens where
	/// `now - iat > max_age` are rejected.
	max_age: Option<Duration>,

	/// The verifying keys to use for validating the JWT signature.
	/// Multiple keys support zero-downtime key rotation.
	keys: Vec<VerifyingKey>,
}

impl Jwt {
	pub const CLASS: UniqueId = id!("mosaik.tickets.jwt.v1");

	/// Creates a new JWT ticket validator against the specified
	/// verifying key.
	#[must_use]
	pub fn with_key(key: impl Into<VerifyingKey>) -> Self {
		Self {
			keys: vec![key.into()],
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
	pub fn add_key(mut self, key: impl Into<VerifyingKey>) -> Self {
		self.keys.push(key.into());
		self
	}

	/// Requires the JWT token to contain a `sub` claim that matches
	/// the specified subject.
	///
	/// If not set, then the subject must be the peer id in
	/// lower-case hex.
	#[must_use]
	pub fn require_subject(mut self, subject: impl Into<String>) -> Self {
		self.subject = Some(subject.into());
		self
	}

	/// Requires the JWT token to contain custom claims with the
	/// specified names and values.
	#[must_use]
	pub fn require_claim(
		mut self,
		name: impl Into<String>,
		value: impl Into<Value>,
	) -> Self {
		self.custom_claims.insert(name.into(), value.into());
		self
	}

	/// Allows JWT tokens that do not contain an `exp` claim and
	/// therefore do not expire.
	#[must_use]
	pub const fn allow_non_expiring(mut self) -> Self {
		self.allow_non_expiring = true;
		self
	}

	/// Requires the JWT token to contain an `iss` claim that
	/// matches the specified issuer.
	///
	/// If called multiple times, the JWT must contain an `iss` claim
	/// that matches at least one of the specified issuers.
	///
	/// If never called, then no `iss` claim is required in the JWT.
	#[must_use]
	pub fn allow_issuer(mut self, issuer: impl Into<String>) -> Self {
		self.issuers.push(issuer.into());
		self
	}

	/// Requires the JWT token to contain an `aud` claim that
	/// matches the specified audience.
	///
	/// If called multiple times, the JWT must contain an `aud` claim
	/// that matches at least one of the specified audiences.
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
			sig = sig
				.derive(key.algorithm_name.as_bytes())
				.derive(key.fingerprint);
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

	#[allow(clippy::too_many_lines)]
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

		// Decode the header to extract the algorithm name. We parse
		// the raw JSON rather than using jwt::Header so that
		// non-standard algorithms (ES256K, EdDSA) are supported.
		let header_bytes =
			base64::decode_config(header_str, base64::URL_SAFE_NO_PAD)
				.map_err(|_| InvalidTicket)?;
		let header_json: Value =
			serde_json::from_slice(&header_bytes).map_err(|_| InvalidTicket)?;
		let alg = header_json
			.get("alg")
			.and_then(Value::as_str)
			.ok_or(InvalidTicket)?;

		// Reject if no configured key matches the header algorithm.
		if !self.keys.iter().any(|k| k.algorithm_name == alg) {
			return Err(InvalidTicket);
		}

		// Verify the cryptographic signature against each key with
		// a matching algorithm. Succeed if any key verifies.
		let signature_verified = self
			.keys
			.iter()
			.filter(|k| k.algorithm_name == alg)
			.any(|k| {
				k.algo
					.verify(header_str, claims_str, sig_str)
					.unwrap_or(false)
			});

		if !signature_verified {
			return Err(InvalidTicket);
		}

		// Decode the claims payload.
		let claims: jwt::Claims =
			FromBase64::from_base64(claims_str).map_err(|_| InvalidTicket)?;
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
			None if !self.allow_non_expiring => {
				return Err(InvalidTicket);
			}
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

		// Convert expiration timestamp. Reject rather than silently
		// accepting a non-representable timestamp (e.g. exp >
		// i64::MAX) as Expiration::Never.
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

/// A signing key ready for JWT ticket issuance.
///
/// Wraps a [`SigningAlgorithm`] implementation together with the
/// JWT algorithm name used in the token header. Prefer the
/// algorithm-specific types ([`Hs256`], [`Es256SigningKey`], etc.)
/// which convert into this type automatically.
pub struct SigningKey {
	key: Box<dyn SigningAlgorithm + Send + Sync>,
	algorithm_name: &'static str,
}

impl SigningKey {
	/// Creates a signing key from a raw [`SigningAlgorithm`] and
	/// the JWT algorithm name (e.g. `"RS256"`).
	pub fn custom(
		key: impl SigningAlgorithm + Send + Sync + 'static,
		algorithm_name: &'static str,
	) -> Self {
		Self {
			key: Box::new(key),
			algorithm_name,
		}
	}
}

/// Builds signed JWT [`Ticket`]s for peer authentication.
///
/// This is the issuing counterpart to the [`Jwt`] validator. Use it
/// to mint tickets that peers present during stream, collection, or
/// group authentication.
///
/// # Example
///
/// ```ignore
/// use mosaik::tickets::{JwtTicketBuilder, Jwt, Hs256};
///
/// let secret = [0xABu8; 32];
/// let builder = JwtTicketBuilder::new(Hs256::new(secret))
///     .issuer("my-auth-service");
///
/// let ticket = builder.build(&peer_id, Expiration::Never);
/// network.discovery().add_ticket(ticket);
/// ```
pub struct JwtTicketBuilder {
	key: SigningKey,
	issuer: Option<String>,
	subject: Option<String>,
	audience: Option<String>,
	not_before: Option<u64>,
	issued_at: Option<u64>,
	token_id: Option<String>,
	custom_claims: BTreeMap<String, Value>,
}

impl JwtTicketBuilder {
	/// Creates a new ticket builder with the given signing key.
	pub fn new(key: impl Into<SigningKey>) -> Self {
		Self {
			key: key.into(),
			issuer: None,
			subject: None,
			audience: None,
			not_before: None,
			issued_at: None,
			token_id: None,
			custom_claims: BTreeMap::new(),
		}
	}

	/// Sets the `iss` (issuer) claim on all built tickets.
	#[must_use]
	pub fn issuer(mut self, issuer: impl Into<String>) -> Self {
		self.issuer = Some(issuer.into());
		self
	}

	/// Overrides the `sub` (subject) claim on all built tickets.
	///
	/// If not set, the subject defaults to the peer id (lower-case
	/// hex) passed to [`build`](Self::build).
	#[must_use]
	pub fn subject(mut self, subject: impl Into<String>) -> Self {
		self.subject = Some(subject.into());
		self
	}

	/// Sets the `aud` (audience) claim on all built tickets.
	#[must_use]
	pub fn audience(mut self, audience: impl Into<String>) -> Self {
		self.audience = Some(audience.into());
		self
	}

	/// Sets the `iat` (issued-at) claim to a fixed timestamp.
	///
	/// If not set, `iat` defaults to the current time at
	/// [`build`](Self::build).
	#[must_use]
	pub const fn issued_at(mut self, ts: chrono::DateTime<Utc>) -> Self {
		self.issued_at = Some(ts.timestamp().cast_unsigned());
		self
	}

	/// Sets the `nbf` (not-before) claim on all built tickets.
	#[must_use]
	pub const fn not_before(mut self, ts: chrono::DateTime<Utc>) -> Self {
		self.not_before = Some(ts.timestamp().cast_unsigned());
		self
	}

	/// Sets the `jti` (JWT ID) claim on all built tickets.
	#[must_use]
	pub fn token_id(mut self, jti: impl Into<String>) -> Self {
		self.token_id = Some(jti.into());
		self
	}

	/// Adds a claim to all built tickets.
	///
	/// Standard claim names (`iss`, `sub`, `aud`, `exp`, `nbf`,
	/// `iat`, `jti`) are routed to their dedicated registered
	/// fields. All other names are added as private claims.
	#[must_use]
	pub fn claim(
		mut self,
		name: impl Into<String>,
		value: impl Into<Value>,
	) -> Self {
		let name = name.into();
		let value = value.into();
		match name.as_str() {
			"iss" => self.issuer = value.as_str().map(String::from),
			"sub" => self.subject = value.as_str().map(String::from),
			"aud" => self.audience = value.as_str().map(String::from),
			"nbf" => self.not_before = value.as_u64(),
			"iat" => self.issued_at = value.as_u64(),
			"jti" => {
				self.token_id = value.as_str().map(String::from);
			}
			_ => {
				self.custom_claims.insert(name, value);
			}
		}
		self
	}

	/// Builds a signed JWT [`Ticket`] for the given peer.
	///
	/// The `sub` claim defaults to the peer's id (lower-case hex)
	/// unless overridden via [`subject`](Self::subject). The `iat`
	/// claim defaults to the current time unless overridden via
	/// [`issued_at`](Self::issued_at). The `exp` claim is derived
	/// from the `expiration` parameter.
	///
	/// # Panics
	///
	/// Panics if the signing key fails to produce a signature.
	pub fn build(&self, peer_id: &PeerId, expiration: Expiration) -> Ticket {
		let now = Utc::now().timestamp().cast_unsigned();
		let claims = jwt::Claims {
			registered: RegisteredClaims {
				issuer: self.issuer.clone(),
				subject: Some(
					self
						.subject
						.clone()
						.unwrap_or_else(|| peer_id.to_string().to_lowercase()),
				),
				audience: self.audience.clone(),
				expiration: match expiration {
					Expiration::Never => None,
					Expiration::At(ts) => Some(ts.timestamp().cast_unsigned()),
				},
				not_before: self.not_before,
				issued_at: Some(self.issued_at.unwrap_or(now)),
				json_web_token_id: self.token_id.clone(),
			},
			private: self.custom_claims.clone(),
		};

		let header_json =
			format!(r#"{{"alg":"{}","typ":"JWT"}}"#, self.key.algorithm_name);
		let header_b64 =
			base64::encode_config(header_json.as_bytes(), base64::URL_SAFE_NO_PAD);
		let claims_b64 = claims.to_base64().expect("claims are serializable");
		let sig = self
			.key
			.key
			.sign(&header_b64, &claims_b64)
			.expect("signing succeeds");

		Ticket::new(
			Jwt::CLASS,
			format!("{header_b64}.{claims_b64}.{sig}").into(),
		)
	}
}

// Re-export built-in verifying key types for convenient construction of JWT
// validators.
pub use alg::{
	EdDsa,
	EdDsaSigningKey,
	Es256,
	Es256SigningKey,
	Es384,
	Es384SigningKey,
	Es512,
	Es512SigningKey,
	Hs256,
	Hs384,
	Hs512,
};

/// A processed verifying key ready for JWT signature validation.
///
/// Wraps a [`VerifyingAlgorithm`] implementation together with a
/// fingerprint derived from the raw key material. The fingerprint
/// is incorporated into [`TicketValidator::signature`] so that
/// validators configured with different keys produce different
/// group identities.
///
/// Prefer the algorithm-specific constructors ([`Hs256`], [`Hs384`],
/// [`Hs512`], [`Es256`], [`Es384`], [`EdDsa`]) for
/// common key types. Use [`VerifyingKey::custom`] for algorithms
/// not covered by the built-in types.
#[derive(Clone)]
pub struct VerifyingKey {
	algo: Arc<dyn VerifyingAlgorithm + Send + Sync + 'static>,
	fingerprint: Digest,
	algorithm_name: &'static str,
}

impl VerifyingKey {
	/// Creates a verifying key from a raw [`VerifyingAlgorithm`]
	/// implementation, key material bytes, and the JWT algorithm
	/// name (e.g. `"RS256"`).
	///
	/// The key material is hashed (blake3) to produce a fingerprint
	/// that uniquely identifies this key without retaining the raw
	/// secret in memory.
	pub fn custom(
		algo: impl VerifyingAlgorithm + Send + Sync + 'static,
		key_material: impl AsRef<[u8]>,
		algorithm_name: &'static str,
	) -> Self {
		Self {
			fingerprint: Digest::from(key_material.as_ref()),
			algo: Arc::new(algo),
			algorithm_name,
		}
	}
}

mod alg {
	use {
		super::{SigningKey, VerifyingKey},
		crate::{id, primitives::const_hex},
		hmac::{Hmac, digest::KeyInit},
		jwt::{SigningAlgorithm, VerifyingAlgorithm},
		std::sync::Arc,
	};

	/// HMAC-SHA256 (`HS256`) verifying key.
	///
	/// ```ignore
	/// use mosaik::tickets::{Jwt, Hs256};
	///
	/// let validator = Jwt::with_key(Hs256::hex("dd90e2..598568"))
	///     .allow_issuer("my-issuer");
	/// ```
	pub struct Hs256([u8; 32]);

	impl Hs256 {
		/// Creates a new HS256 verifying key from the shared secret.
		pub const fn new(secret: [u8; 32]) -> Self {
			Self(secret)
		}

		/// Creates a new HS256 verifying key from a hex string.
		pub const fn hex(input: &str) -> Self {
			Self(const_hex::<32>(input))
		}
	}

	impl SigningAlgorithm for Hs256 {
		fn algorithm_type(&self) -> jwt::AlgorithmType {
			jwt::AlgorithmType::Hs256
		}

		fn sign(
			&self,
			header: &str,
			claims: &str,
		) -> Result<String, jwt::error::Error> {
			Hmac::<sha2::Sha256>::new_from_slice(&self.0)
				.expect("HMAC accepts any key length")
				.sign(header, claims)
		}
	}

	impl From<Hs256> for VerifyingKey {
		fn from(key: Hs256) -> Self {
			Self {
				fingerprint: id!("hs256").derive(key.0),
				algo: Arc::new(
					Hmac::<sha2::Sha256>::new_from_slice(&key.0)
						.expect("HMAC accepts any key length"),
				),
				algorithm_name: "HS256",
			}
		}
	}

	impl From<Hs256> for SigningKey {
		fn from(key: Hs256) -> Self {
			Self {
				key: Box::new(key),
				algorithm_name: "HS256",
			}
		}
	}

	/// HMAC-SHA384 (`HS384`) verifying key.
	pub struct Hs384([u8; 48]);

	impl Hs384 {
		/// Creates a new HS384 verifying key from the shared secret.
		pub const fn new(secret: [u8; 48]) -> Self {
			Self(secret)
		}

		/// Creates a new HS384 verifying key from a hex string.
		pub const fn hex(input: &str) -> Self {
			Self(const_hex::<48>(input))
		}
	}

	impl SigningAlgorithm for Hs384 {
		fn algorithm_type(&self) -> jwt::AlgorithmType {
			jwt::AlgorithmType::Hs384
		}

		fn sign(
			&self,
			header: &str,
			claims: &str,
		) -> Result<String, jwt::error::Error> {
			Hmac::<sha2::Sha384>::new_from_slice(&self.0)
				.expect("HMAC accepts any key length")
				.sign(header, claims)
		}
	}

	impl From<Hs384> for VerifyingKey {
		fn from(key: Hs384) -> Self {
			Self {
				fingerprint: id!("hs384").derive(key.0),
				algo: Arc::new(
					Hmac::<sha2::Sha384>::new_from_slice(&key.0)
						.expect("HMAC accepts any key length"),
				),
				algorithm_name: "HS384",
			}
		}
	}

	impl From<Hs384> for SigningKey {
		fn from(key: Hs384) -> Self {
			Self {
				key: Box::new(key),
				algorithm_name: "HS384",
			}
		}
	}

	/// HMAC-SHA512 (`HS512`) verifying key.
	pub struct Hs512([u8; 64]);

	impl Hs512 {
		/// Creates a new HS512 verifying key from the shared secret.
		pub const fn new(secret: [u8; 64]) -> Self {
			Self(secret)
		}

		/// Creates a new HS512 verifying key from a hex string.
		pub const fn hex(input: &str) -> Self {
			Self(const_hex::<64>(input))
		}
	}

	impl SigningAlgorithm for Hs512 {
		fn algorithm_type(&self) -> jwt::AlgorithmType {
			jwt::AlgorithmType::Hs512
		}

		fn sign(
			&self,
			header: &str,
			claims: &str,
		) -> Result<String, jwt::error::Error> {
			Hmac::<sha2::Sha512>::new_from_slice(&self.0)
				.expect("HMAC accepts any key length")
				.sign(header, claims)
		}
	}

	impl From<Hs512> for VerifyingKey {
		fn from(key: Hs512) -> Self {
			Self {
				fingerprint: id!("hs512").derive(key.0),
				algo: Arc::new(
					Hmac::<sha2::Sha512>::new_from_slice(&key.0)
						.expect("HMAC accepts any key length"),
				),
				algorithm_name: "HS512",
			}
		}
	}

	impl From<Hs512> for SigningKey {
		fn from(key: Hs512) -> Self {
			Self {
				key: Box::new(key),
				algorithm_name: "HS512",
			}
		}
	}

	/// ECDSA P-256 (`ES256`) verifying key.
	///
	/// Expects a 33-byte SEC1 compressed public key (prefix byte +
	/// 32-byte x-coordinate).
	///
	/// ```ignore
	/// use mosaik::tickets::{Jwt, Es256};
	///
	/// let validator = Jwt::with_key(Es256::hex("02dd90e2..598568"))
	///     .allow_issuer("my-issuer");
	/// ```
	pub struct Es256([u8; 33]);

	impl Es256 {
		/// Creates a new ES256 verifying key from a compressed P-256
		/// public key (33 bytes, SEC1 format).
		pub const fn new(key: [u8; 33]) -> Self {
			Self(key)
		}

		/// Creates a new ES256 verifying key from a hex string.
		pub const fn hex(input: &str) -> Self {
			Self(const_hex::<33>(input))
		}
	}

	impl From<Es256> for VerifyingKey {
		fn from(key: Es256) -> Self {
			Self {
				fingerprint: id!("es256").derive(key.0),
				algo: Arc::new(Es256Verifier(
					p256::ecdsa::VerifyingKey::from_sec1_bytes(&key.0)
						.expect("valid compressed P-256 public key"),
				)),
				algorithm_name: "ES256",
			}
		}
	}

	/// P-256 ECDSA (`ES256`) signing key for issuing JWT tickets.
	///
	/// This is the private-key counterpart to [`Es256`]. Use it
	/// with [`JwtTicketBuilder`](super::JwtTicketBuilder) to mint
	/// tickets that can be verified by a validator configured with
	/// the corresponding [`Es256`] public key.
	///
	/// ```ignore
	/// use mosaik::tickets::{Jwt, Es256, Es256SigningKey, JwtTicketBuilder};
	///
	/// let sk = Es256SigningKey::random();
	/// let validator = Jwt::with_key(sk.verifying_key());
	/// let builder = JwtTicketBuilder::new(sk).issuer("my-issuer");
	/// ```
	pub struct Es256SigningKey(p256::ecdsa::SigningKey);

	impl Es256SigningKey {
		/// Creates a signing key from a 32-byte private key scalar.
		///
		/// # Panics
		///
		/// Panics if the bytes do not represent a valid P-256 scalar.
		pub fn from_bytes(bytes: [u8; 32]) -> Self {
			Self(
				p256::ecdsa::SigningKey::from_slice(&bytes)
					.expect("valid P-256 private key"),
			)
		}

		/// Generates a random P-256 signing key.
		pub fn random() -> Self {
			Self(p256::ecdsa::SigningKey::random(
				&mut p256::elliptic_curve::rand_core::OsRng,
			))
		}

		/// Returns the compressed public key for use with [`Es256`].
		///
		/// The returned key is the SEC1 compressed encoding of the
		/// public point (33 bytes).
		#[must_use]
		#[allow(clippy::missing_panics_doc)]
		pub fn verifying_key(&self) -> Es256 {
			let point = self.0.verifying_key().to_encoded_point(true);
			Es256(
				point
					.as_bytes()
					.try_into()
					.expect("compressed P-256 key is 33 bytes"),
			)
		}
	}

	impl SigningAlgorithm for Es256SigningKey {
		fn algorithm_type(&self) -> jwt::AlgorithmType {
			jwt::AlgorithmType::Es256
		}

		fn sign(
			&self,
			header: &str,
			claims: &str,
		) -> Result<String, jwt::error::Error> {
			use p256::ecdsa::signature::Signer;
			let message = format!("{header}.{claims}");
			let sig: p256::ecdsa::Signature = self.0.sign(message.as_bytes());
			Ok(base64::encode_config(
				sig.to_bytes(),
				base64::URL_SAFE_NO_PAD,
			))
		}
	}

	impl From<Es256SigningKey> for SigningKey {
		fn from(key: Es256SigningKey) -> Self {
			Self {
				key: Box::new(key),
				algorithm_name: "ES256",
			}
		}
	}

	/// ECDSA P-384 (`ES384`) verifying key.
	///
	/// Expects a 49-byte SEC1 compressed public key (prefix byte +
	/// 48-byte x-coordinate).
	///
	/// ```ignore
	/// use mosaik::tickets::{Jwt, Es384};
	///
	/// let validator = Jwt::with_key(Es384::hex("02dd90e2..598568"))
	///     .allow_issuer("my-issuer");
	/// ```
	pub struct Es384([u8; 49]);

	impl Es384 {
		/// Creates a new ES384 verifying key from a compressed P-384
		/// public key (49 bytes, SEC1 format).
		pub const fn new(key: [u8; 49]) -> Self {
			Self(key)
		}

		/// Creates a new ES384 verifying key from a hex string.
		pub const fn hex(input: &str) -> Self {
			Self(const_hex::<49>(input))
		}
	}

	impl From<Es384> for VerifyingKey {
		fn from(key: Es384) -> Self {
			Self {
				fingerprint: id!("es384").derive(key.0),
				algo: Arc::new(Es384Verifier(
					p384::ecdsa::VerifyingKey::from_sec1_bytes(&key.0)
						.expect("valid compressed P-384 public key"),
				)),
				algorithm_name: "ES384",
			}
		}
	}

	/// P-384 ECDSA (`ES384`) signing key for issuing JWT tickets.
	///
	/// This is the private-key counterpart to [`Es384`].
	pub struct Es384SigningKey(p384::ecdsa::SigningKey);

	impl Es384SigningKey {
		/// Creates a signing key from a 48-byte private key scalar.
		///
		/// # Panics
		///
		/// Panics if the bytes do not represent a valid P-384 scalar.
		pub fn from_bytes(bytes: [u8; 48]) -> Self {
			Self(
				p384::ecdsa::SigningKey::from_slice(&bytes)
					.expect("valid P-384 private key"),
			)
		}

		/// Generates a random P-384 signing key.
		pub fn random() -> Self {
			Self(p384::ecdsa::SigningKey::random(
				&mut p384::elliptic_curve::rand_core::OsRng,
			))
		}

		/// Returns the compressed public key for use with [`Es384`].
		#[must_use]
		#[allow(clippy::missing_panics_doc)]
		pub fn verifying_key(&self) -> Es384 {
			let point = self.0.verifying_key().to_encoded_point(true);
			Es384(
				point
					.as_bytes()
					.try_into()
					.expect("compressed P-384 key is 49 bytes"),
			)
		}
	}

	impl SigningAlgorithm for Es384SigningKey {
		fn algorithm_type(&self) -> jwt::AlgorithmType {
			jwt::AlgorithmType::Es384
		}

		fn sign(
			&self,
			header: &str,
			claims: &str,
		) -> Result<String, jwt::error::Error> {
			use p384::ecdsa::signature::Signer;
			let message = format!("{header}.{claims}");
			let sig: p384::ecdsa::Signature = self.0.sign(message.as_bytes());
			Ok(base64::encode_config(
				sig.to_bytes(),
				base64::URL_SAFE_NO_PAD,
			))
		}
	}

	impl From<Es384SigningKey> for SigningKey {
		fn from(key: Es384SigningKey) -> Self {
			Self {
				key: Box::new(key),
				algorithm_name: "ES384",
			}
		}
	}

	/// ECDSA P-521 (`ES512`) verifying key.
	///
	/// Expects a 67-byte SEC1 compressed public key (prefix byte +
	/// 66-byte x-coordinate).
	pub struct Es512([u8; 67]);

	impl Es512 {
		/// Creates a new ES512 verifying key from a compressed P-521
		/// public key (67 bytes, SEC1 format).
		pub const fn new(key: [u8; 67]) -> Self {
			Self(key)
		}

		/// Creates a new ES512 verifying key from a hex string.
		pub const fn hex(input: &str) -> Self {
			Self(const_hex::<67>(input))
		}
	}

	impl From<Es512> for VerifyingKey {
		fn from(key: Es512) -> Self {
			Self {
				fingerprint: id!("es512").derive(key.0),
				algo: Arc::new(Es512Verifier(
					p521::ecdsa::VerifyingKey::from_sec1_bytes(&key.0)
						.expect("valid compressed P-521 public key"),
				)),
				algorithm_name: "ES512",
			}
		}
	}

	/// P-521 ECDSA (`ES512`) signing key for issuing JWT tickets.
	///
	/// This is the private-key counterpart to [`Es512`].
	pub struct Es512SigningKey(p521::ecdsa::SigningKey);

	impl Es512SigningKey {
		/// Creates a signing key from a 66-byte private key scalar.
		///
		/// # Panics
		///
		/// Panics if the bytes do not represent a valid P-521 scalar.
		pub fn from_bytes(bytes: [u8; 66]) -> Self {
			Self(
				p521::ecdsa::SigningKey::from_slice(&bytes)
					.expect("valid P-521 private key"),
			)
		}

		/// Generates a random P-521 signing key.
		pub fn random() -> Self {
			Self(p521::ecdsa::SigningKey::random(
				&mut p521::elliptic_curve::rand_core::OsRng,
			))
		}

		/// Returns the compressed public key for use with [`Es512`].
		#[must_use]
		#[allow(clippy::missing_panics_doc)]
		pub fn verifying_key(&self) -> Es512 {
			let vk = p521::ecdsa::VerifyingKey::from(&self.0);
			let point = vk.to_encoded_point(true);
			Es512(
				point
					.as_bytes()
					.try_into()
					.expect("compressed P-521 key is 67 bytes"),
			)
		}
	}

	impl SigningAlgorithm for Es512SigningKey {
		fn algorithm_type(&self) -> jwt::AlgorithmType {
			jwt::AlgorithmType::Es512
		}

		fn sign(
			&self,
			header: &str,
			claims: &str,
		) -> Result<String, jwt::error::Error> {
			use p521::ecdsa::signature::Signer;
			let message = format!("{header}.{claims}");
			let sig: p521::ecdsa::Signature = self.0.sign(message.as_bytes());
			Ok(base64::encode_config(
				sig.to_bytes(),
				base64::URL_SAFE_NO_PAD,
			))
		}
	}

	impl From<Es512SigningKey> for SigningKey {
		fn from(key: Es512SigningKey) -> Self {
			Self {
				key: Box::new(key),
				algorithm_name: "ES512",
			}
		}
	}

	/// Ed25519 (`EdDSA`) verifying key.
	///
	/// Expects a 32-byte Ed25519 public key.
	///
	/// ```ignore
	/// use mosaik::tickets::{Jwt, EdDsa};
	///
	/// let validator = Jwt::with_key(EdDsa::new([1u8; 32]))
	///     .allow_issuer("my-issuer");
	/// ```
	pub struct EdDsa([u8; 32]);

	impl EdDsa {
		/// Creates a new `EdDSA` verifying key from a 32-byte Ed25519
		/// public key.
		pub const fn new(key: [u8; 32]) -> Self {
			Self(key)
		}

		/// Creates a new `EdDSA` verifying key from a hex string.
		pub const fn hex(input: &str) -> Self {
			Self(const_hex::<32>(input))
		}
	}

	impl From<EdDsa> for VerifyingKey {
		fn from(key: EdDsa) -> Self {
			Self {
				fingerprint: id!("EdDsa").derive(key.0),
				algo: Arc::new(EdDsaVerifier(
					ed25519_dalek::VerifyingKey::from_bytes(&key.0)
						.expect("valid Ed25519 public key"),
				)),
				algorithm_name: "EdDSA",
			}
		}
	}

	/// Ed25519 (`EdDSA`) signing key for issuing JWT tickets.
	///
	/// This is the private-key counterpart to [`EdDsa`].
	pub struct EdDsaSigningKey(ed25519_dalek::SigningKey);

	impl EdDsaSigningKey {
		/// Creates a signing key from a 32-byte Ed25519 private key.
		pub fn from_bytes(bytes: [u8; 32]) -> Self {
			Self(ed25519_dalek::SigningKey::from_bytes(&bytes))
		}

		/// Generates a random Ed25519 signing key.
		pub fn random() -> Self {
			Self(ed25519_dalek::SigningKey::from_bytes(&rand::random()))
		}

		/// Returns the public key for use with [`EdDsa`].
		#[must_use]
		pub fn verifying_key(&self) -> EdDsa {
			EdDsa(self.0.verifying_key().to_bytes())
		}
	}

	impl SigningAlgorithm for EdDsaSigningKey {
		fn algorithm_type(&self) -> jwt::AlgorithmType {
			jwt::AlgorithmType::None
		}

		fn sign(
			&self,
			header: &str,
			claims: &str,
		) -> Result<String, jwt::error::Error> {
			use ed25519_dalek::Signer;
			let message = format!("{header}.{claims}");
			let sig = self.0.sign(message.as_bytes());
			Ok(base64::encode_config(
				sig.to_bytes(),
				base64::URL_SAFE_NO_PAD,
			))
		}
	}

	impl From<EdDsaSigningKey> for SigningKey {
		fn from(key: EdDsaSigningKey) -> Self {
			Self {
				key: Box::new(key),
				algorithm_name: "EdDSA",
			}
		}
	}

	/// ECDSA P-256 verifier.
	struct Es256Verifier(p256::ecdsa::VerifyingKey);

	impl VerifyingAlgorithm for Es256Verifier {
		fn algorithm_type(&self) -> jwt::AlgorithmType {
			jwt::AlgorithmType::Es256
		}

		fn verify_bytes(
			&self,
			header: &str,
			claims: &str,
			signature: &[u8],
		) -> Result<bool, jwt::error::Error> {
			use p256::ecdsa::signature::Verifier;
			let Ok(sig) = p256::ecdsa::Signature::from_slice(signature) else {
				return Ok(false);
			};
			let message = format!("{header}.{claims}");
			Ok(self.0.verify(message.as_bytes(), &sig).is_ok())
		}
	}

	/// ECDSA P-384 verifier.
	struct Es384Verifier(p384::ecdsa::VerifyingKey);

	impl VerifyingAlgorithm for Es384Verifier {
		fn algorithm_type(&self) -> jwt::AlgorithmType {
			jwt::AlgorithmType::Es384
		}

		fn verify_bytes(
			&self,
			header: &str,
			claims: &str,
			signature: &[u8],
		) -> Result<bool, jwt::error::Error> {
			use p384::ecdsa::signature::Verifier;
			let Ok(sig) = p384::ecdsa::Signature::from_slice(signature) else {
				return Ok(false);
			};
			let message = format!("{header}.{claims}");
			Ok(self.0.verify(message.as_bytes(), &sig).is_ok())
		}
	}

	/// ECDSA P-521 verifier.
	struct Es512Verifier(p521::ecdsa::VerifyingKey);

	impl VerifyingAlgorithm for Es512Verifier {
		fn algorithm_type(&self) -> jwt::AlgorithmType {
			jwt::AlgorithmType::Es512
		}

		fn verify_bytes(
			&self,
			header: &str,
			claims: &str,
			signature: &[u8],
		) -> Result<bool, jwt::error::Error> {
			use p521::ecdsa::signature::Verifier;
			let Ok(sig) = p521::ecdsa::Signature::from_slice(signature) else {
				return Ok(false);
			};
			let message = format!("{header}.{claims}");
			Ok(self.0.verify(message.as_bytes(), &sig).is_ok())
		}
	}

	/// Ed25519 verifier wrapper.
	struct EdDsaVerifier(ed25519_dalek::VerifyingKey);

	impl VerifyingAlgorithm for EdDsaVerifier {
		fn algorithm_type(&self) -> jwt::AlgorithmType {
			// No EdDSA variant in jwt::AlgorithmType; the Jwt validator
			// uses algorithm_name for matching instead.
			jwt::AlgorithmType::None
		}

		fn verify_bytes(
			&self,
			header: &str,
			claims: &str,
			signature: &[u8],
		) -> Result<bool, jwt::error::Error> {
			use ed25519_dalek::Verifier;
			let Ok(sig) = ed25519_dalek::Signature::from_slice(signature) else {
				return Ok(false);
			};
			let message = format!("{header}.{claims}");
			Ok(self.0.verify(message.as_bytes(), &sig).is_ok())
		}
	}
}
