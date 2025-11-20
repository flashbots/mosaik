use {
	crate::state::Store,
	bytes::Bytes,
	jsonrpsee::{
		core::async_trait,
		proc_macros::rpc,
		server::Server,
		types::ErrorObjectOwned,
	},
	rblib::{
		alloy::{
			consensus::{Transaction, transaction::TxHashRef},
			primitives::{TxHash, U256},
		},
		prelude::*,
		reth::{
			core::primitives::constants::MINIMUM_GAS_LIMIT,
			ethereum::rpc::eth::utils::recover_raw_transaction,
			primitives::Recovered,
		},
	},
	std::sync::Arc,
	tokio::{
		net::ToSocketAddrs,
		sync::mpsc::{self, Sender},
	},
};

#[rpc(server, namespace = "eth")]
pub trait EthApi<P: Platform> {
	#[method(name = "sendRawTransaction")]
	async fn send_raw_transaction(
		&self,
		tx: Bytes,
	) -> Result<TxHash, ErrorObjectOwned>;

	#[method(name = "sendBundle")]
	async fn send_bundle(
		&self,
		bundle: types::Bundle<P>,
	) -> Result<String, ErrorObjectOwned>;
}

struct EthApiImpl<P: Platform> {
	store: Arc<Store>,
	tx_sender: Sender<types::Transaction<P>>,
	_bundle_sender: Sender<types::Bundle<P>>,
}

#[async_trait]
impl<P: Platform> EthApiServer<P> for EthApiImpl<P> {
	async fn send_raw_transaction(
		&self,
		tx: Bytes,
	) -> Result<TxHash, ErrorObjectOwned> {
		let recovered: Recovered<types::Transaction<P>> =
			recover_raw_transaction(&tx)?;

		let min_nonce = self
			.store
			.get_nonce(recovered.signer_ref())
			.unwrap_or_default();

		if recovered.nonce() < min_nonce {
			return Err(ErrorObjectOwned::owned(
				-32000,
				"Nonce too low",
				Some(min_nonce),
			));
		}

		let gas_price = recovered.effective_gas_price(None);
		let min_gas = u128::from(MINIMUM_GAS_LIMIT) * gas_price;
		let min_balance = recovered.value() + U256::from(min_gas);

		let actual_balance = self
			.store
			.get_balance(recovered.signer_ref())
			.unwrap_or_default();

		if actual_balance < min_balance {
			return Err(ErrorObjectOwned::owned(
				-32001,
				"Insufficient funds",
				Some(min_balance),
			));
		}

		let txhash = *recovered.tx_hash();

		self
			.tx_sender
			.send(recovered.into_inner())
			.await
			.map_err(|_| {
				ErrorObjectOwned::owned(-32002, "Internal error", None::<()>)
			})?;

		Ok(txhash)
	}

	async fn send_bundle(
		&self,
		_bundle: types::Bundle<P>,
	) -> Result<String, ErrorObjectOwned> {
		todo!()
	}
}

pub struct RpcEndpoints<P: Platform> {
	pub bundles: mpsc::Receiver<types::Bundle<P>>,
	pub transactions: mpsc::Receiver<types::Transaction<P>>,
}

impl<P: Platform> RpcEndpoints<P> {
	pub async fn start(
		store: Arc<Store>,
		listen_addr: impl ToSocketAddrs,
	) -> anyhow::Result<Self> {
		let (tx_sender, transactions) = mpsc::channel(1);
		let (bundle_sender, bundles) = mpsc::channel(1);

		let api = EthApiImpl::<P> {
			store: store.clone(),
			tx_sender,
			_bundle_sender: bundle_sender,
		};

		Server::builder()
			.build(listen_addr)
			.await?
			.start(api.into_rpc());

		Ok(Self {
			bundles,
			transactions,
		})
	}
}
