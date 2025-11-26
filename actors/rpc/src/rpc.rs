use {
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
			primitives::{Recovered, SealedHeader},
			rpc::types::mev::EthBundleHash,
		},
	},
	std::sync::Arc,
	tokio::{
		net::ToSocketAddrs,
		sync::{
			mpsc::{self, Sender},
			watch,
		},
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
	) -> Result<EthBundleHash, ErrorObjectOwned>;
}

struct EthApiImpl<P: Platform> {
	store: Arc<Store>,
	latest_header: watch::Receiver<SealedHeader<types::Header<P>>>,
	tx_sender: Sender<types::Transaction<P>>,
	bundle_sender: Sender<types::Bundle<P>>,
}

impl<P: Platform> EthApiImpl<P> {
	fn validate_transaction(
		&self,
		recovered: &Recovered<types::Transaction<P>>,
	) -> Result<(), ErrorObjectOwned> {
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

		Ok(())
	}
}

#[async_trait]
impl<P: Platform> EthApiServer<P> for EthApiImpl<P> {
	async fn send_raw_transaction(
		&self,
		tx: Bytes,
	) -> Result<TxHash, ErrorObjectOwned> {
		let recovered: Recovered<types::Transaction<P>> =
			recover_raw_transaction(&tx)?;

		self.validate_transaction(&recovered)?;

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
		bundle: types::Bundle<P>,
	) -> Result<EthBundleHash, ErrorObjectOwned> {
		let header = self.latest_header.borrow();

		// run platform specific bundle eligibility checks
		if bundle.is_permanently_ineligible(&*header) {
			return Err(ErrorObjectOwned::owned(
				-32003,
				"Bundle is permanently ineligible",
				None::<()>,
			));
		}

		// validate all transactions in the bundle
		// todo: account for txs that fund subsequent txs in the bundle
		for tx in bundle.transactions() {
			self.validate_transaction(tx)?;
		}

		let hash = EthBundleHash {
			bundle_hash: bundle.hash(),
		};

		Ok(hash)
	}
}

pub struct RpcEndpoints<P: Platform> {
	pub bundles: mpsc::Receiver<types::Bundle<P>>,
	pub transactions: mpsc::Receiver<types::Transaction<P>>,
	pub headers: watch::Sender<SealedHeader<types::Header<P>>>,
}

impl<P: Platform> RpcEndpoints<P> {
	pub async fn start(
		store: Arc<Store>,
		latest_header: types::Header<P>,
		listen_addr: impl ToSocketAddrs,
	) -> anyhow::Result<Self> {
		let latest_header = SealedHeader::new_unhashed(latest_header);
		let (tx_sender, transactions) = mpsc::channel(1);
		let (bundle_sender, bundles) = mpsc::channel(1);
		let (headers, _) = watch::channel(latest_header);

		let api = EthApiImpl::<P> {
			store: store.clone(),
			latest_header: headers.subscribe(),
			tx_sender,
			bundle_sender,
		};

		Server::builder()
			.build(listen_addr)
			.await?
			.start(api.into_rpc());

		Ok(Self {
			bundles,
			transactions,
			headers,
		})
	}
}
