use {
	crate::{
		PeerId,
		UniqueId,
		groups::{Cursor, StateMachine},
	},
	core::task::{Context, Poll},
	serde::{Serialize, de::DeserializeOwned},
};

pub trait Consensus<M: StateMachine> {
	type QueryError: ProtocolError;
	type CommandError: ProtocolError;
	type Message: ProtocolMessage;
	type Ticket: Send + Sync + 'static;

	fn signature(&self) -> UniqueId;

	fn admit(&mut self, peer: PeerId, ticket: Self::Ticket) -> bool;

	fn dropped(&mut self, peer: PeerId);

	fn receive(&mut self, message: Self::Message, from: PeerId);

	fn feed(
		&mut self,
		commands: Vec<M::Command>,
	) -> impl Future<Output = Result<Cursor, Self::CommandError>> + Send + Sync + 'static;

	fn query(
		&mut self,
		query: M::Query,
	) -> impl Future<Output = Result<QueryResultAt<M>, Self::QueryError>>
	+ Send
	+ Sync
	+ 'static;

	fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()>;
}

pub struct QueryResultAt<M: StateMachine> {
	pub position: Cursor,
	pub result: M::QueryResult,
}

pub trait ProtocolMessage:
	Clone + Serialize + DeserializeOwned + Send + Sync + 'static
{
}

impl<T> ProtocolMessage for T where
	T: Clone + Serialize + DeserializeOwned + Send + Sync + 'static
{
}

pub trait ProtocolError: core::error::Error + Send + Sync + 'static {}
impl<T> ProtocolError for T where T: core::error::Error + Send + Sync + 'static {}
