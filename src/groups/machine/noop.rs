use {
	super::{
		super::replay::LogReplaySync,
		ApplyContext,
		LeadershipPreference,
		StateMachine,
	},
	crate::{
		PeerId,
		discovery::PeerEntry,
		primitives::{IntoIterOrSingle, UniqueId},
		unique_id,
	},
	std::collections::BTreeSet,
};

/// A no-op state machine that does nothing and can be used for testing or
/// creating groups that don't require any state machine logic, but want to form
/// bonds and have p2p connectivity with each other.
///
/// The `NoOp` state machine can be combined with ticket validators to create
/// groups that have custom authentication logic without needing to implement
/// any state machine behavior.
#[derive(Debug, Default)]
pub struct NoOp {
	leadership: LeadershipConfig,
}

impl NoOp {
	/// Creates a `NoOp` state machine with where nobody will attempt to be a
	/// leader. In this configuration, the group will never reach the `online`
	/// state, but peers can still form bonds and exchange messages with each
	/// other.
	pub const fn no_leaders() -> Self {
		Self {
			leadership: LeadershipConfig::NoLeaders,
		}
	}

	/// Creates a `NoOp` state machine with the default leadership configuration
	/// where all peers are equally eligible to be leaders. In this configuration,
	/// the group will reach the `online` state as long as a quorum of peers form
	/// bonds with each other.
	pub const fn anyone_can_lead() -> Self {
		Self {
			leadership: LeadershipConfig::AnyoneCanLead,
		}
	}

	/// Creates a `NoOp` state machine with a specified set of peers that are
	/// allowed to be leaders. The machine will reach the `online` state as long
	/// as at least one of the specified leaders is present in the group and forms
	/// a bond with the other peers.
	pub fn with_leaders<V>(leaders: impl IntoIterOrSingle<PeerId, V>) -> Self {
		Self {
			leadership: LeadershipConfig::SpecificLeaders(
				leaders.iterator().into_iter().collect(),
			),
		}
	}
}

impl StateMachine for NoOp {
	type Command = ();
	type Query = ();
	type QueryResult = ();
	type StateSync = LogReplaySync<Self>;

	fn signature(&self) -> UniqueId {
		unique_id!("mosaik.groups.machine.NoOp").derive(self.leadership.signature())
	}

	fn apply(&mut self, (): Self::Command, _: &dyn ApplyContext) {}

	fn query(&self, (): Self::Query) -> Self::QueryResult {}

	fn state_sync(&self) -> Self::StateSync {
		LogReplaySync::default()
	}

	fn leadership_preference(&self, local: &PeerEntry) -> LeadershipPreference {
		self.leadership.preference_for(local)
	}
}

#[derive(Debug, Default)]
enum LeadershipConfig {
	#[default]
	AnyoneCanLead,
	NoLeaders,
	SpecificLeaders(BTreeSet<PeerId>),
}

impl LeadershipConfig {
	fn signature(&self) -> UniqueId {
		match self {
			Self::NoLeaders => {
				unique_id!("no_leaders")
			}
			Self::AnyoneCanLead => {
				unique_id!("anyone_can_lead")
			}
			Self::SpecificLeaders(leaders) => {
				let mut base = unique_id!("specific");
				for leader in leaders {
					base = base.derive(leader);
				}
				base
			}
		}
	}

	fn preference_for(&self, local: &PeerEntry) -> LeadershipPreference {
		match self {
			Self::NoLeaders => LeadershipPreference::Observer,
			Self::AnyoneCanLead => LeadershipPreference::Normal,
			Self::SpecificLeaders(leaders) => {
				if leaders.contains(local.id()) {
					LeadershipPreference::Normal
				} else {
					LeadershipPreference::Observer
				}
			}
		}
	}
}
