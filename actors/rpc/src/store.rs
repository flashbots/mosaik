use {
	crate::sync::{Balance, BalancesUpdate, Nonce, NoncesUpdate},
	rblib::alloy::primitives::{Address, U256},
};

pub struct Store {
	_db: sled::Db,
	nonces: sled::Tree,
	balances: sled::Tree,
}

impl Store {
	pub fn open(data_dir: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
		let db = sled::open(data_dir)?;
		let nonces = db.open_tree("nonces")?;
		let balances = db.open_tree("balances")?;

		Ok(Self {
			_db: db,
			nonces,
			balances,
		})
	}

	pub fn insert_nonces(&self, update: NoncesUpdate) -> anyhow::Result<()> {
		for (address, nonce) in update.nonces {
			self.nonces.insert(address, &nonce.to_le_bytes()[..])?;
		}
		Ok(())
	}

	pub fn insert_balances(&self, update: BalancesUpdate) -> anyhow::Result<()> {
		for (address, balance) in update.balances {
			self.balances.insert(address, balance.as_le_slice())?;
		}
		Ok(())
	}

	pub fn get_nonce(&self, address: &Address) -> Option<Nonce> {
		self.nonces.get(address).ok()?.and_then(|ivec| {
			let bytes: [u8; 8] = ivec.as_ref().try_into().ok()?;
			Some(u64::from_le_bytes(bytes))
		})
	}

	pub fn get_balance(&self, address: &Address) -> Option<Balance> {
		self.balances.get(address).ok()?.and_then(|ivec| {
			let bytes: [u8; 32] = ivec.as_ref().try_into().ok()?;
			Some(U256::from_le_bytes(bytes))
		})
	}
}
