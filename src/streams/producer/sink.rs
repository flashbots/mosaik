use {
	super::{
		super::link::{CloseReason, Link},
		Producer,
		Status,
	},
	crate::datum::{Criteria, Datum, StreamId},
	bytes::Bytes,
	core::{any::Any, sync::atomic::Ordering},
	futures::{StreamExt, stream::FuturesUnordered},
	serde::Serialize,
	slotmap::DenseSlotMap,
	std::sync::Arc,
	tokio::sync::mpsc::{self, error::SendError},
	tokio_util::sync::{CancellationToken, DropGuard},
	tracing::{debug, info, warn},
};

/// A handle to a fanout loop for a specific stream on the local node.
///
/// Every stream produced by the local node has a corresponding async fanout
/// loop task that is responsible for distributing data produced locally to all
/// remote subscribers according to their subscription criteria.
///
/// Notes:
///
/// - All local stream producers for a given stream ID share the same fanout
///   loop.
///
/// - Remote subscribers to a given stream ID will receive data produced by all
///   instances of `Producer<D>` on the local node for that stream ID.
///
/// - This type is just a handle to the fanout loop; it is safe and cheap to
///   clone. All clones of this type control the same underlying fanout loop
///   task.
///
/// - Each `FanoutSink` may have many local `Producer<D>` instances, and all
///   such producers share the same fanout loop for distributing data to remote
///   subscribers.
///
/// - Each remote subscriber to a given stream ID will receive data produced by
///   all local producers for that stream ID.
///
/// - This struct is type-erased; but it must be instantiated with a type `D`
///   that matches the stream ID it represents. The `Producer<D>` instances
///   created by this sink will also be for the same type `D`. The reason why
///   this type is type-erased is to allow the local node to manage multiple
///   streams with different data types in a single registry.
pub struct FanoutSink(Arc<Handle>);

impl Clone for FanoutSink {
	fn clone(&self) -> Self {
		Self(Arc::clone(&self.0))
	}
}

impl FanoutSink {
	/// Creates a new `FanoutSink` for the given stream ID with no local producers
	/// or remote subscribers.
	pub fn new<D: Datum + Serialize>() -> Self {
		let (event_loop, handle) = EventLoop::<D>::new();
		let handle = Arc::new(handle);
		tokio::spawn(event_loop.run());
		Self(handle)
	}

	/// Returns the stream ID associated with this fanout sink.
	pub fn stream_id(&self) -> &StreamId {
		&self.0.stream_id
	}

	/// Creates a new `Producer<D>` instance for this fanout sink.
	///
	/// There may be multiple producers for the same stream ID on the local node;
	/// all such producers will share the same fanout loop for distributing data
	/// to remote subscribers.
	///
	/// # Panics
	///
	/// Since this is an internal API, this function will panic if the type `D`
	/// does not match the stream ID associated with this fanout sink.
	pub fn producer<D: Datum>(&self) -> Producer<D> {
		if !self.stream_id().is::<D>() {
			unreachable!(
				"stream-id mismatch; this is a bug; expected {}, found {}",
				self.stream_id(),
				StreamId::of::<D>()
			);
		}

		let handle = &self.0;
		let data_tx = handle
			.data_tx
			.downcast_ref::<mpsc::Sender<D>>()
			.expect("stream-id mismatch; this is a bug; qed")
			.clone();

		Producer::init(data_tx, handle.status.clone())
	}

	/// Accepts a new connected remote subscriber link to this fanout sink.
	///
	/// From this point onwards, this link will receive all data published to this
	/// sink that matches the given criteria.
	pub(crate) fn accept(&self, link: Link, criteria: Criteria) {
		if let Err(SendError(Command::Accept(link, _))) =
			self.0.cmd_tx.send(Command::Accept(link, criteria))
		{
			warn!(
				stream_id = %self.stream_id(),
				peer_id = %link.peer_id(),
				"Failed to accept new subscriber. FanoutSink event loop is terminated.",
			);
		}
	}
}

/// A long running async task that manages distributing data published to a
/// stream type `D` to all remote subscribers according to their subscription
/// criteria.
///
/// Notes:
///
/// - There is one fanout loop per stream ID on the local node.
///
/// - The event loop is controlled via a handle that is accessible from the
///   `FanoutSink` type.
struct EventLoop<D: Datum> {
	/// Stream ID associated with this fanout loop.
	stream_id: StreamId,

	/// Channel for receiving published data items of type `D`.
	data_rx: mpsc::Receiver<D>,

	/// Channel for receiving control commands for this fanout loop.
	cmds: mpsc::UnboundedReceiver<Command>,

	/// Used to keep track of statistics and status updates for local producers
	/// of this stream.
	status: Status,

	/// A list of remote subscribers to this stream
	subs: DenseSlotMap<SubscriptionId, Subscriber>,

	/// Cancellation token for terminating this event loop.
	cancel: CancellationToken,
}

impl<D: Datum> EventLoop<D> {
	fn new() -> (Self, Handle) {
		let stream_id = StreamId::of::<D>();
		let (data_tx, data_rx) = mpsc::channel(1);
		let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
		let status = Status::new();

		let handle = Handle {
			stream_id: stream_id.clone(),
			data_tx: Box::new(data_tx),
			cmd_tx,
			status: status.clone(),
			_abort: status.0.cancel.clone().drop_guard(),
		};

		let event_loop = Self {
			stream_id,
			cancel: status.0.cancel.clone(),
			data_rx,
			cmds: cmd_rx,
			status,
			subs: DenseSlotMap::with_key(),
		};

		(event_loop, handle)
	}
}

impl<D: Datum + Serialize> EventLoop<D> {
	async fn run(mut self) {
		self.signal_online_status();

		loop {
			tokio::select! {
				// Stream is terminated.
				() = self.cancel.cancelled() => {
					self.on_terminated();
					break;
				}

				// Control command received from the `Handle`.
				Some(command) = self.cmds.recv() => {
					self.on_command(command);
				}

				// New data item was produced by local producers.
				Some(item) = self.data_rx.recv() => {
					self.on_data(item).await;
				}
			}
		}
	}

	fn on_command(&mut self, cmd: Command) {
		match cmd {
			Command::Accept(link, criteria) => {
				self.accept_subscriber(link, criteria);
			}
		}
	}

	/// Invoked when a new remote subscriber requests to subscribe to this stream.
	fn accept_subscriber(&mut self, link: Link, criteria: Criteria) {
		tracing::info!(
			stream_id = %self.stream_id,
			peer_id = %link.peer_id(),
			criteria = ?criteria,
			existing_subs = %self.subs.len(),
			"Accepted new subscriber",
		);

		self.subs.insert(Subscriber { link, criteria });

		self
			.status
			.0
			.subscribers_count
			.fetch_add(1, Ordering::Relaxed);

		self.status.0.notify.notify_waiters();
	}

	/// Invoked when any of the local producers publish a new data item.
	async fn on_data(&mut self, item: D) {
		let mut serialized: Option<Bytes> = None;
		let mut sends = FuturesUnordered::new();

		for (sub_id, sub) in &mut self.subs {
			if sub.criteria.matches(&item) {
				// serialize the datum only once when we identify the first
				// matching subscriber
				let bytes = match &mut serialized {
					Some(b) => b.clone(),
					None => match rmp_serde::to_vec(&item) {
						Ok(bytes) => {
							let b = Bytes::from(bytes);
							serialized.replace(b.clone());
							b
						}
						Err(e) => {
							warn!(
								stream_id = %self.stream_id,
								error = %e,
								"Failed to serialize datum for stream",
							);
							self.status.0.items_dropped.fetch_add(1, Ordering::Relaxed);
							return;
						}
					},
				};

				// Initiate send to this subscriber and track the subscription ID
				// to identify which send operation corresponds to which subscriber
				// if a failure occurs.
				sends.push(async move {
					let result = sub.link.send(bytes).await;
					let peer = *sub.link.peer_id();
					(sub_id, peer, result)
				});
			}
		}

		// if there was at least one matching subscriber
		if let Some(bytes) = serialized {
			let packet_len = bytes.len() as u64;

			let mut failed_subs = Vec::new();
			let mut first_success = false;

			while let Some(result) = sends.next().await {
				let (sub_id, peer, result) = result;

				if let Err(e) = result {
					failed_subs.push(sub_id);

					warn!(
						stream_id = %self.stream_id,
						peer = %peer,
						error = %e,
						"Failed to send data item to subscriber",
					);
				} else {
					if !first_success {
						first_success = true;

						self.status.0.items_sent.fetch_add(1, Ordering::Relaxed);
						self
							.status
							.0
							.bytes_sent
							.fetch_add(packet_len, Ordering::Relaxed);
					}

					self
						.status
						.0
						.cumulative_bytes_sent
						.fetch_add(packet_len, Ordering::Relaxed);
				}
			}

			drop(sends);

			// remove failed subscribers
			for sub_id in failed_subs {
				self.disconnect(sub_id, CloseReason::DataSendError).await;
			}
		} else {
			// no subscribers matched the datum
			self.status.0.items_dropped.fetch_add(1, Ordering::Relaxed);
		}
	}

	fn on_terminated(&mut self) {
		info!("Fanout loop for stream {} is terminating", self.stream_id);
	}

	/// Disconnects a remote subscriber from this fanout loop.
	async fn disconnect(&mut self, sub_id: SubscriptionId, reason: CloseReason) {
		if let Some(sub) = self.subs.remove(sub_id) {
			let _ = sub.link.close_with_reason(reason).await;

			self
				.status
				.0
				.subscribers_count
				.fetch_sub(1, Ordering::Relaxed);
			self.status.0.notify.notify_waiters();
		}
	}

	/// Signals to all local producers that this fanout loop is now online
	/// and ready to accept subscriptions from consumers.
	fn signal_online_status(&self) {
		self.status.0.online.store(true, Ordering::SeqCst);
		self.status.0.notify.notify_waiters();
		debug!(stream_id = %self.stream_id, "Producer for stream is now online");
	}
}

/// A handle for controlling a fanout loop for a specific stream ID on the
/// local node.
struct Handle {
	/// Stream ID associated with this fanout sink.
	stream_id: StreamId,

	/// Type-erased sender channel for publishing data items of type `D`.
	/// This is stored as a `Box<dyn Any + Send + Sync>` to allow type-erasure,
	/// but it is downcasted to specific `mpsc::Sender<D>` when creating
	/// `Producer<D>` instances.
	///
	/// The data channel is a bounded channel to provide feedback to producers
	/// when the fanout loop is overwhelmed with data so they can apply
	/// backpressure themselves.
	data_tx: Box<dyn Any + Send + Sync>,

	/// Command channel for sending control commands to the fanout loop.
	/// It is unbounded because it is only used internally for control messages.
	cmd_tx: mpsc::UnboundedSender<Command>,

	/// Statistics about this fanout loop. Used to inform local producers.
	/// about the number of remote subscribers. Updated by the fanout loop.
	status: Status,

	/// Terminates the fanout loop when `FanoutSink` is dropped.
	_abort: DropGuard,
}

slotmap::new_key_type! {
	/// A unique identifier for a remote subscription for one stream.
	///
	/// One remote node may have multiple subscriptions to the same stream
	/// with different criteria. Each subscription is identified by a unique
	/// `SubscriptionId` and is managed independently.
	///
	/// This identifier is internal to the fanout loop and is not exposed outside
	/// of this module.
	struct SubscriptionId;
}

#[derive(Debug)]
enum Command {
	Accept(Link, Criteria),
}

struct Subscriber {
	link: Link,
	criteria: Criteria,
}
