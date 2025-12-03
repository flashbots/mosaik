#[derive(Clone, Debug)]
pub struct Transaction {
	inner: op_alloy_consensus::OpTxEnvelope,
}

impl Transaction {
	pub fn hash(&self) -> &alloy_primitives::B256 {
		self.inner.hash()
	}
}

impl serde::Serialize for Transaction {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		let tx: op_alloy_consensus::serde_bincode_compat::transaction::OpTxEnvelope =
            (&self.inner).into();
		tx.serialize(serializer)
	}
}

impl<'de> serde::Deserialize<'de> for Transaction {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: serde::Deserializer<'de>,
	{
		let tx: op_alloy_consensus::serde_bincode_compat::transaction::OpTxEnvelope =
            serde::Deserialize::deserialize(deserializer)?;
		let op_tx: op_alloy_consensus::OpTxEnvelope = tx.into();
		Ok(Transaction { inner: op_tx })
	}
}

impl From<op_alloy_consensus::OpTxEnvelope> for Transaction {
	fn from(inner: op_alloy_consensus::OpTxEnvelope) -> Self {
		Self { inner }
	}
}
