use {
	super::{Fanout, StreamId},
	crate::streams::{Criteria, fanout::Subscription},
	core::fmt,
	dashmap::DashMap,
	futures::{SinkExt, StreamExt},
	iroh::{
		Endpoint,
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	serde::{Deserialize, Serialize},
	std::{io, sync::Arc},
	tokio_util::codec::{Framed, LengthDelimitedCodec},
	tracing::info,
};

pub struct Protocol {
	endpoint: Endpoint,
	producers: Arc<DashMap<StreamId, Fanout>>,
}

impl Protocol {
	pub const ALPN: &'static [u8] = b"/mosaik/streams/1";

	pub fn new(
		endpoint: Endpoint,
		producers: Arc<DashMap<StreamId, Fanout>>,
	) -> Self {
		Self {
			endpoint,
			producers,
		}
	}
}

impl ProtocolHandler for Protocol {
	async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
		let (tx, rx) = connection.accept_bi().await?;

		let wire_stream = tokio::io::join(rx, tx);
		let mut wire = Framed::new(wire_stream, LengthDelimitedCodec::new());

		let Some(handshake) = wire.next().await.transpose()? else {
			return Err(
				std::io::Error::new(
					std::io::ErrorKind::UnexpectedEof,
					"Connection closed before handshake",
				)
				.into(),
			);
		};

		let request: SubscriptionRequest = rmp_serde::from_slice(&handshake)
			.map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

		let Some(fanout) = self.producers.get(&request.stream_id) else {
			let error = SubscriptionError::StreamNotFound;
			let response = SubscriptionResponse::Rejected(error.clone());
			let response_bytes = rmp_serde::to_vec(&response).expect("infallible");

			wire.send(response_bytes.into()).await?;
			wire.close().await?;

			return Err(io::Error::new(io::ErrorKind::NotFound, error).into());
		};

		info!(
			"New subscription for stream {} from {}",
			request.stream_id,
			connection.remote_id()?
		);

		let response = SubscriptionResponse::Accepted;
		let response_bytes = rmp_serde::to_vec(&response).expect("infallible");

		wire.send(response_bytes.into()).await?;

		fanout
			.subscribe(Subscription {
				connection,
				criteria: request.criteria,
				wire,
			})
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

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
pub enum SubscriptionError {
	#[error("Stream not found")]
	StreamNotFound,

	#[error("Max subscriber capacity reached")]
	AtCapacity,
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
