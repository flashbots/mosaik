use {
	crate::utils::Jwt,
	core::time::Duration,
	hmac::{Hmac, digest::KeyInit},
	jwt::{RegisteredClaims, SignWithKey, VerifyWithKey},
	mosaik::{
		PeerId,
		UniqueId,
		discovery::PeerEntry,
		id,
		tickets::{Expiration, InvalidTicket, Ticket, TicketValidator},
	},
};

/// A JWT-based [`TicketValidator`] for producer-side stream authentication.
pub struct JwtIssuer {
	key: Hmac<sha2::Sha256>,
	issuer: &'static str,
	secret: &'static str,
}

impl Default for JwtIssuer {
	fn default() -> Self {
		Self::new("mosaik.test.jwt.issuer", "mosaik.test.jwt.secret")
	}
}

impl JwtIssuer {
	pub fn new(issuer: &'static str, secret: &'static str) -> Self {
		Self {
			key: Hmac::new_from_slice(secret.as_bytes()).unwrap(),
			issuer,
			secret,
		}
	}

	pub const fn secret(&self) -> &[u8] {
		self.secret.as_bytes()
	}

	pub const fn issuer(&self) -> &str {
		self.issuer
	}

	pub fn key(&self) -> Hmac<sha2::Sha256> {
		self.key.clone()
	}

	pub fn make_ticket(
		&self,
		peer_id: &PeerId,
		expiration: Expiration,
	) -> Ticket {
		Ticket::new(
			Jwt::CLASS,
			jwt::Claims::new(RegisteredClaims {
				issuer: Some(self.issuer.to_string()),
				subject: Some(peer_id.to_string().to_lowercase()),
				expiration: match expiration {
					Expiration::Never => None,
					Expiration::At(ts) => Some(ts.timestamp().cast_unsigned()),
				},
				..Default::default()
			})
			.sign_with_key(&self.key)
			.unwrap()
			.into(),
		)
	}

	pub fn make_ticket_expiring_in(
		&self,
		peer_id: &PeerId,
		duration: Duration,
	) -> Ticket {
		let expiration_time =
			chrono::Utc::now() + chrono::Duration::from_std(duration).unwrap();
		self.make_ticket(peer_id, Expiration::At(expiration_time))
	}

	pub fn make_expired_ticket(&self, peer_id: &PeerId) -> Ticket {
		let expiration_time = chrono::Utc::now() - chrono::Duration::hours(1);
		self.make_ticket(peer_id, Expiration::At(expiration_time))
	}

	pub fn make_valid_ticket(&self, peer_id: &PeerId) -> Ticket {
		let expiration_time = chrono::Utc::now() + chrono::Duration::hours(1);
		self.make_ticket(peer_id, Expiration::At(expiration_time))
	}
}
