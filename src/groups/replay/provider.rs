use {
	super::{Config, LogReplaySync, LogReplaySyncMessage},
	crate::{
		groups::{
			Index,
			StateMachine,
			StateSyncProvider,
			SyncContext,
			SyncProviderContext,
		},
		primitives::{Pretty, Short},
	},
	core::marker::PhantomData,
};

/// An instance of this type runs on every peer in the group and is responsible
/// for responding to `AvailabilityRequest` and `FetchEntriesRequest` messages
/// from lagging followers during the catch-up process and providing the
/// requested information to help them synchronize with the current state of the
/// group.
pub struct LogReplayProvider<M: StateMachine> {
	config: Config,

	#[doc(hidden)]
	_marker: PhantomData<M>,
}

impl<M: StateMachine> LogReplayProvider<M> {
	pub(super) fn new(
		config: &Config,
		_: &dyn SyncContext<LogReplaySync<M>>,
	) -> Self {
		Self {
			config: config.clone(),
			_marker: PhantomData,
		}
	}
}

impl<M: StateMachine> StateSyncProvider for LogReplayProvider<M> {
	type Owner = LogReplaySync<M>;

	/// At the state sync provider level we only handle
	/// `AvailabilityRequest` and `FetchEntriesRequest` messages, other messages
	/// are forwarded to the state sync session of the follower.
	fn receive(
		&mut self,
		message: LogReplaySyncMessage<M::Command>,
		sender: crate::PeerId,
		cx: &mut dyn SyncProviderContext<Self::Owner>,
	) -> Result<(), LogReplaySyncMessage<M::Command>> {
		match message {
			LogReplaySyncMessage::AvailabilityRequest => {
				let available = cx.log().available();
				let committed = cx.committed().index();
				let available = *available.start()..=committed.min(*available.end());

				tracing::trace!(
					peer = %Short(sender),
					range = %Pretty(&available),
					group = %Short(cx.group_id()),
					network = %Short(cx.network_id()),
					"state availability confirmed for"
				);

				cx.send_to(
					sender,
					LogReplaySyncMessage::AvailabilityResponse(available),
				);
			}
			LogReplaySyncMessage::FetchEntriesRequest(range) => {
				if range.is_empty() {
					return Ok(());
				}

				// cap the range to the configured max batch size
				let len = range.end().distance(range.start()) + 1;
				let len = len.min(self.config.batch_size);
				let range = *range.start()..=(*range.start() + len - 1u32);

				let entries = cx.log().get_range(&range);

				if entries.is_empty() {
					cx.send_to(sender, LogReplaySyncMessage::FetchEntriesResponse {
						range: Index::zero()..=Index::zero(),
						entries: Vec::new(),
					});
					return Ok(());
				}

				// The actual range may differ from requested if some
				// prefix entries have been pruned from the log. Use the
				// indices reported by the storage to build the accurate
				// range so the session can place entries correctly.
				let actual_start = entries.first().unwrap().1;
				let actual_end = entries.last().unwrap().1;
				let actual_range = actual_start..=actual_end;

				tracing::trace!(
					peer = %Short(sender),
					range = %Pretty(&actual_range),
					group = %Short(cx.group_id()),
					network = %Short(cx.network_id()),
					"providing state to"
				);

				cx.send_to(sender, LogReplaySyncMessage::FetchEntriesResponse {
					range: actual_range,
					entries: entries
						.into_iter()
						.map(|(term, _, command)| (command, term))
						.collect(),
				});
			}
			message => return Err(message),
		}

		Ok(())
	}
}
