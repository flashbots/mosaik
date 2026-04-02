use {
	core::time::Duration,
	hmac::{Hmac, digest::KeyInit},
	jwt::{RegisteredClaims, SignWithKey, VerifyWithKey},
	mosaik::{
		PeerId,
		Ticket,
		UniqueId,
		discovery::PeerEntry,
		id,
		tickets::{Expiration, InvalidTicket, TicketValidator},
	},
};

/// A JWT-based [`TicketValidator`] for producer-side stream authentication.
pub struct JwtValidator {
	key: Hmac<sha2::Sha256>,
	issuer: &'static str,
	secret: &'static str,
}

impl Default for JwtValidator {
	fn default() -> Self {
		Self::new("mosaik.test.jwt.issuer", "mosaik.test.jwt.secret")
	}
}

impl JwtValidator {
	pub fn new(issuer: &'static str, secret: &'static str) -> Self {
		Self {
			key: Hmac::new_from_slice(secret.as_bytes()).unwrap(),
			issuer,
			secret,
		}
	}

	pub fn make_ticket(
		&self,
		peer_id: &PeerId,
		expiration: Expiration,
	) -> Ticket {
		Ticket::new(
			self.class(),
			jwt::Claims::new(RegisteredClaims {
				issuer: Some(self.issuer.to_string()),
				subject: Some(peer_id.to_string()),
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

impl TicketValidator for JwtValidator {
	fn class(&self) -> UniqueId {
		id!("mosaik.test.jwt_ticket_validator")
	}

	fn signature(&self) -> UniqueId {
		self.class().derive(self.issuer).derive(self.secret)
	}

	fn validate(
		&self,
		ticket: &[u8],
		peer: &PeerEntry,
	) -> Result<Expiration, InvalidTicket> {
		let jwt_str = core::str::from_utf8(ticket).map_err(|_| InvalidTicket)?;
		let claims: jwt::Claims = jwt_str
			.verify_with_key(&self.key)
			.map_err(|_| InvalidTicket)?;
		let now = chrono::Utc::now().timestamp().cast_unsigned();
		let exp = claims.registered.expiration;
		if claims.registered.issuer.as_deref() != Some(self.issuer)
			|| claims.registered.subject != Some(peer.id().to_string())
			|| exp <= Some(now)
		{
			return Err(InvalidTicket);
		}
		Ok(
			exp
				.and_then(|ts| chrono::DateTime::from_timestamp(ts.cast_signed(), 0))
				.map_or(Expiration::Never, Expiration::At),
		)
	}
}
