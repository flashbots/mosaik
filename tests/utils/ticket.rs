use {
	core::time::Duration,
	mosaik::tickets::{Expiration, Hs256, Jwt, JwtTicketBuilder},
};

pub const DEFAULT_ISSUER: &str = "mosaik.test.jwt.issuer";
pub const DEFAULT_SECRET: &str = "mosaik.test.jwt.secret";

/// Derives a 32-byte key from a secret string (blake3 hash).
pub fn jwt_secret(secret: &str) -> [u8; 32] {
	*blake3::hash(secret.as_bytes()).as_bytes()
}

/// Creates a [`JwtTicketBuilder`] with the given issuer and secret.
pub fn jwt_builder(issuer: &str, secret: &str) -> JwtTicketBuilder {
	JwtTicketBuilder::new(Hs256::new(jwt_secret(secret)))
		.issuer(issuer)
		.expires_at(Expiration::in_1h())
}

/// Creates a [`Jwt`] validator matching the given issuer and secret.
pub fn jwt_validator(issuer: &str, secret: &str) -> Jwt {
	Jwt::with_key(Hs256::new(jwt_secret(secret))).allow_issuer(issuer)
}
