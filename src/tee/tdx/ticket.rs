use {
	crate::{
		NetworkId,
		PeerId,
		primitives::{Expiration, encoding},
		tee::tdx::Measurements,
	},
	bytes::Bytes,
	core::cell::OnceCell,
	derive_more::{AsMut, AsRef, Deref, DerefMut},
	lsmtree::{BadProof, KVStore, SparseMerkleTree},
	serde::{Deserialize, Serialize, de},
	std::collections::BTreeMap,
	tdx_quote::Quote,
};

#[derive(Debug, Serialize)]
pub struct TdxTicket {
	system: SystemData,
	user: UserData,
	quote_bytes: Vec<u8>,

	#[serde(skip)]
	quote: OnceCell<Quote>,
}

impl TdxTicket {
	pub const fn system(&self) -> &SystemData {
		&self.system
	}

	pub const fn user(&self) -> &UserData {
		&self.user
	}

	pub fn quote(&self) -> &Quote {
		self.quote.get().expect("quote should be initialized")
	}

	pub fn measurements(&self) -> Measurements {
		Measurements::from(self.quote())
	}
}

impl<'de> Deserialize<'de> for TdxTicket {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: de::Deserializer<'de>,
	{
		#[derive(Deserialize)]
		struct Raw {
			system: SystemData,
			user: UserData,
			quote_bytes: Vec<u8>,
		}

		let raw = Raw::deserialize(deserializer)?;

		let parsed_quote =
			Quote::from_bytes(&raw.quote_bytes).map_err(de::Error::custom)?;
		parsed_quote.verify().map_err(de::Error::custom)?;

		// The report data in the TDX quote is a 64-byte array where:
		//   [0..32]  = merkle root of the system data
		//   [32..64] = merkle root of the user data
		let report_data = parsed_quote.report_input_data();
		let expected_system_root = &report_data[..32];
		let expected_user_root = &report_data[32..];

		let quote = OnceCell::new();
		quote
			.set(parsed_quote)
			.expect("freshly created OnceCell cannot be full");

		let system_tree =
			build_system_tree(&raw.system).map_err(de::Error::custom)?;
		let user_tree = build_user_tree(&raw.user).map_err(de::Error::custom)?;

		if system_tree.root_ref() != expected_system_root {
			return Err(de::Error::custom(
				"system data merkle root does not match quote report data",
			));
		}

		if user_tree.root_ref() != expected_user_root {
			return Err(de::Error::custom(
				"user data merkle root does not match quote report data",
			));
		}

		let system_proof = OnceCell::new();
		system_proof
			.set(system_tree)
			.expect("freshly created OnceCell cannot be full");

		let user_proof = OnceCell::new();
		user_proof
			.set(user_tree)
			.expect("freshly created OnceCell cannot be full");

		Ok(Self {
			system: raw.system,
			user: raw.user,
			quote_bytes: raw.quote_bytes,
			quote,
		})
	}
}

#[derive(
	Debug, Clone, Default, Serialize, Deserialize, Deref, DerefMut, AsRef, AsMut,
)]
pub struct UserData(BTreeMap<Vec<u8>, Vec<u8>>);

impl KVStore for UserData {
	type Error = BadProof;
	type Hasher = blake3::Hasher;

	fn get(&self, key: &[u8]) -> Result<Option<Bytes>, Self::Error> {
		Ok(self.0.get(key).map(|v| Bytes::from(v.clone())))
	}

	fn set(&mut self, key: Bytes, value: Bytes) -> Result<(), Self::Error> {
		self.0.insert(key.to_vec(), value.to_vec());
		Ok(())
	}

	fn remove(&mut self, key: &[u8]) -> Result<Bytes, Self::Error> {
		self.0.remove(key).map(Bytes::from).ok_or(BadProof)
	}

	fn contains(&self, key: &[u8]) -> Result<bool, Self::Error> {
		Ok(self.0.contains_key(key))
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemData {
	pub peer_id: PeerId,
	pub network_id: NetworkId,
	pub expires_at: Expiration,
	#[serde(skip)]
	store: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl KVStore for SystemData {
	type Error = BadProof;
	type Hasher = blake3::Hasher;

	fn get(&self, key: &[u8]) -> Result<Option<Bytes>, Self::Error> {
		Ok(self.store.get(key).map(|v| Bytes::from(v.clone())))
	}

	fn set(&mut self, key: Bytes, value: Bytes) -> Result<(), Self::Error> {
		self.store.insert(key.to_vec(), value.to_vec());
		Ok(())
	}

	fn remove(&mut self, key: &[u8]) -> Result<Bytes, Self::Error> {
		self.store.remove(key).map(Bytes::from).ok_or(BadProof)
	}

	fn contains(&self, key: &[u8]) -> Result<bool, Self::Error> {
		Ok(self.store.contains_key(key))
	}
}

fn build_system_tree(
	system: &SystemData,
) -> Result<SparseMerkleTree<SystemData>, BadProof> {
	let mut smt = SparseMerkleTree::new_with_stores(
		SystemData {
			peer_id: system.peer_id,
			network_id: system.network_id,
			expires_at: system.expires_at,
			store: BTreeMap::new(),
		},
		SystemData {
			peer_id: system.peer_id,
			network_id: system.network_id,
			expires_at: system.expires_at,
			store: BTreeMap::new(),
		},
	);

	smt.update(b"peer_id", encoding::serialize(&system.peer_id))?;
	smt.update(b"network_id", encoding::serialize(&system.network_id))?;
	smt.update(b"expires_at", encoding::serialize(&system.expires_at))?;

	Ok(smt)
}

fn build_user_tree(
	user: &UserData,
) -> Result<SparseMerkleTree<UserData>, BadProof> {
	let mut smt = SparseMerkleTree::<UserData>::new();

	for (key, value) in &user.0 {
		smt.update(key, Bytes::from(value.clone()))?;
	}

	Ok(smt)
}
