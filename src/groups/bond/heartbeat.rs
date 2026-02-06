use {
	crate::groups::Config,
	core::time::Duration,
	rand::random,
	std::{sync::Arc, time::Instant},
	tokio::{
		sync::{Notify, futures::OwnedNotified},
		time::{Interval, MissedTickBehavior, interval},
	},
};

/// Heartbeats are messages that are sent periodically over a bond connection to
/// the remote peer when no other messages are being exchanged to ensure that
/// the connection is still alive and responsive.
///
/// When a heartbeat is sent, the heartbeat timer is started, and if no message
/// is received from the remote peer before the next heartbeat interval, a
/// missed heartbeat is recorded. If the number of consecutive missed heartbeats
/// exceeds the configured maximum, then the heartbeat is considered failed.
///
/// The heartbeat message in times of idleness is a simple `Ping` message, if
/// the remote peer is alive it should respond with a `Pong` message or any
/// other message which will also reset the heartbeat timer.
pub struct Heartbeat {
	tick: Interval,
	last_recv: Instant,
	missed: usize,
	max_missed: usize,
	alert: Arc<Notify>,
	base: Duration,
	jitter: Duration,
}

impl Heartbeat {
	pub fn new(config: &Config) -> Self {
		let next_tick_at = Self::next_tick_at(
			&config.heartbeat_interval, //
			&config.heartbeat_jitter,
		);

		let mut tick = interval(next_tick_at);
		tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

		Self {
			tick,
			missed: 0,
			last_recv: Instant::now(),
			max_missed: config.max_missed_heartbeats,
			alert: Arc::new(Notify::new()),
			base: config.heartbeat_interval,
			jitter: config.heartbeat_jitter,
		}
	}

	/// Completes when the next heartbeat is due.
	pub async fn tick(&mut self) {
		self.tick.tick().await;

		let max_gap = self.base.saturating_add(self.jitter);
		if self.last_recv.elapsed() > max_gap {
			self.missed += 1;
		}

		if self.missed >= self.max_missed {
			self.alert.notify_waiters();
		}
	}

	/// Resets the heartbeat timer after any received message over the bond link.
	pub fn reset(&mut self) {
		self
			.tick
			.reset_after(Self::next_tick_at(&self.base, &self.jitter));

		self.last_recv = Instant::now();
		self.missed = 0;
	}

	/// Completes when the heartbeat has failed due to too many missed heartbeats.
	pub fn failed(&self) -> OwnedNotified {
		Arc::clone(&self.alert).notified_owned()
	}

	fn next_tick_at(base: &Duration, jitter: &Duration) -> Duration {
		#[expect(clippy::cast_possible_truncation)]
		let millis_sub = random::<u64>() % jitter.as_millis() as u64;
		let sub = Duration::from_millis(millis_sub);
		base.saturating_sub(sub)
	}
}
