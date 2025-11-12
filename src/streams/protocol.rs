use {
	super::StreamId,
	crate::{
		local::Local,
		streams::{
			Criteria,
			Error,
			error::{ProtocolError, SubscriptionError},
		},
	},
	core::fmt,
	futures::{SinkExt, StreamExt},
	iroh::{
		endpoint::{Connection, RecvStream, SendStream},
		protocol::{AcceptError, ProtocolHandler},
	},
	serde::{Deserialize, Serialize},
	tokio::io::Join,
	tokio_util::codec::{Framed, LengthDelimitedCodec},
};

pub struct Protocol {
	local: Local,
}

impl Protocol {
	pub const ALPN: &'static [u8] = b"/mosaik/streams/1";

	pub fn new(local: Local) -> Self {
		Self { local }
	}
}

impl ProtocolHandler for Protocol {
	async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
		let (tx, rx) = connection.accept_bi().await?;

		let mut wire =
			WireStream::new(tokio::io::join(rx, tx), LengthDelimitedCodec::new());

		let Some(handshake) = wire.next().await.transpose()? else {
			return Err(AcceptError::from_err(Error::Protocol(
				ProtocolError::ClosedBeforeHandshake,
			)));
		};

		let request: SubscriptionRequest = rmp_serde::from_slice(&handshake)
			.map_err(|e| {
				AcceptError::from_err(Error::Protocol(
					ProtocolError::InvalidHandshakeRequest(e),
				))
			})?;

		let Some(sink) = self.local.open_sink(&request.stream_id) else {
			let response = rmp_serde::to_vec(&SubscriptionResponse::Rejected(
				SubscriptionError::StreamNotFound(request.stream_id.clone()),
			))
			.expect("infallible; qed");

			wire.send(response.into()).await?;
			wire.close().await?;

			return Err(AcceptError::from_err(Error::Subscription(
				SubscriptionError::StreamNotFound(request.stream_id),
			)));
		};

		let link = Link { wire, connection };
		let criteria = request.criteria;

		sink
			.accept(link, criteria)
			.await
			.map_err(AcceptError::from_err)?;

		Ok(())
	}
}

impl fmt::Debug for Protocol {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "Streams({})", String::from_utf8_lossy(Self::ALPN))
	}
}

type WireStream = Framed<Join<RecvStream, SendStream>, LengthDelimitedCodec>;

pub struct Link {
	pub connection: Connection,
	pub wire: WireStream,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionRequest {
	pub stream_id: StreamId,
	pub criteria: Criteria,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubscriptionResponse {
	Accepted,
	Rejected(SubscriptionError),
}

impl core::fmt::Debug for Link {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		let Ok(remote_id) = self.connection.remote_id() else {
			return write!(f, "Link(to=Unknown, INVALID CONNECTION)");
		};
		write!(f, "Link(to={remote_id})")
	}
}
