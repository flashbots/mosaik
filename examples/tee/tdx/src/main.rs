use {
	mosaik::{tee::tdx::NetworkTicketExt, *},
	tdx_quote::{
		CertificationData,
		CertificationDataInner,
		Quote,
		QuoteVerificationError,
	},
};

/// Default URL for the host's PCCS, reachable from inside a QEMU
/// user-mode-networking guest at the gateway address.
const DEFAULT_PCCS_URL: &str = "https://10.0.2.2:8081";

/// Intel's public Provisioning Certification Service.
const INTEL_PCS_URL: &str = "https://api.trustedservices.intel.com";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	// use tracing_subscriber::prelude::*;
	// tracing_subscriber::registry()
	// 	.with(tracing_subscriber::fmt::layer())
	// 	.try_init()?;

	let network_id = NetworkId::from("tdx-example");
	let network = Network::new(network_id).await?;

	println!("Network {network_id} created.");

	println!("Retrieving TDX quote...");
	let raw_quote = configfs_tsm::create_tdx_quote([0u8; 64]).unwrap();

	let quote = Quote::from_bytes(raw_quote.as_slice()).unwrap();

	verify_quote(&quote).await?;

	let mrtd = network.tdx().mrtd().unwrap();
	println!("MR_TD measurement: {mrtd}");

	Ok(())
}

// ---------------------------------------------------------------------------
// Quote verification with PCS fallback
// ---------------------------------------------------------------------------

/// Verify the TDX quote's PCK chain back to Intel's root CA.
///
/// 1. Try using the embedded PCK certificate chain (if present).
/// 2. If the quote only contains the encrypted platform identity (type 1/2/3),
///    fetch the PCK cert from the host PCCS or Intel PCS, verify the chain,
///    then verify the QE report.
async fn verify_quote(quote: &Quote) -> anyhow::Result<()> {
	// Fast path: embedded cert chain
	match quote.verify() {
		Ok(_pck) => {
			println!("Quote verified (embedded PCK chain)");
			return Ok(());
		}
		Err(QuoteVerificationError::NoPckCertChain) => {
			println!("No embedded PCK chain — fetching from PCS...");
		}
		Err(e) => {
			anyhow::bail!("Quote verification failed: {e}")
		}
	}

	// Extract platform identity from the certification data
	let (encrypted_ppid, cpusvn, pcesvn, pceid) = extract_platform_id(quote)?;

	// Fetch the PCK certificate chain
	let pck_pem =
		fetch_pck_cert_chain(&encrypted_ppid, &cpusvn, &pcesvn, &pceid).await?;

	// Verify the certificate chain roots at Intel's CA
	let pck = tdx_quote::pck::verify_pck_certificate_chain_pem(pck_pem)
		.map_err(|e| anyhow::anyhow!("PCK chain verification failed: {e:?}"))?;
	println!("PCK certificate chain verified against Intel root CA");

	// Verify the QE report signature with the PCK
	quote
		.verify_with_pck(&pck)
		.map_err(|e| anyhow::anyhow!("QE report verification failed: {e:?}"))?;
	println!("QE report signature verified with PCK");

	Ok(())
}

// ---------------------------------------------------------------------------
// Platform identity extraction
// ---------------------------------------------------------------------------

/// Parse the encrypted PPID, CPUSVN, PCESVN, and PCEID from the
/// quote's certification data.
///
/// Supports types 1 (plain), 2 (RSA-2048), and 3 (RSA-3072).
/// Layout: `encrypted_ppid (var) || cpusvn (16) || pcesvn (2) || pceid (2)`
fn extract_platform_id(
	quote: &Quote,
) -> anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>)> {
	let raw = match &quote.certification_data {
		CertificationData::QeReportCertificationData(qe) => {
			match &qe.certification_data {
				CertificationDataInner::PckIdPpidPlainCpusvnPcesvn(d)
				| CertificationDataInner::PckIdPpidRSA2048CpusvnPcesvn(d)
				| CertificationDataInner::PckIdPpidRSA3072CpusvnPcesvn(d) => d,
				_ => anyhow::bail!(
					"Inner certification data does not contain a platform identity"
				),
			}
		}
		CertificationData::PckIdPpidPlainCpusvnPcesvn(d)
		| CertificationData::PckIdPpidRSA2048CpusvnPcesvn(d)
		| CertificationData::PckIdPpidRSA3072CpusvnPcesvn(d) => d,
		_ => {
			anyhow::bail!("Certification data does not contain a platform identity")
		}
	};

	// encrypted_ppid is variable-length; the last 20 bytes
	// are cpusvn(16) + pcesvn(2) + pceid(2).
	anyhow::ensure!(
		raw.len() > 20,
		"Platform identity data too short: {} bytes",
		raw.len()
	);

	let ppid_len = raw.len() - 20;
	Ok((
		raw[..ppid_len].to_vec(),
		raw[ppid_len..ppid_len + 16].to_vec(),
		raw[ppid_len + 16..ppid_len + 18].to_vec(),
		raw[ppid_len + 18..ppid_len + 20].to_vec(),
	))
}

// ---------------------------------------------------------------------------
// PCK certificate fetching
// ---------------------------------------------------------------------------

fn hex_encode(bytes: &[u8]) -> String {
	bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn url_decode(input: &str) -> String {
	let mut out = Vec::with_capacity(input.len());
	let mut bytes = input.bytes();
	while let Some(b) = bytes.next() {
		if b == b'%' {
			if let (Some(hi), Some(lo)) = (bytes.next(), bytes.next()) {
				let hex = [hi, lo].map(|c| c as char);
				if let Ok(decoded) = u8::from_str_radix(&String::from_iter(hex), 16) {
					out.push(decoded);
					continue;
				}
			}
		}
		out.push(b);
	}
	String::from_utf8_lossy(&out).into_owned()
}

/// Fetch the PCK certificate chain from the host PCCS or Intel
/// PCS.
///
/// Tries the host PCCS first (`PCCS_URL` env or default gateway
/// `10.0.2.2:8081`), then falls back to Intel's public PCS
/// (requires `INTEL_PCS_API_KEY` env).
async fn fetch_pck_cert_chain(
	encrypted_ppid: &[u8],
	cpusvn: &[u8],
	pcesvn: &[u8],
	pceid: &[u8],
) -> anyhow::Result<Vec<u8>> {
	let query = format!(
		"/sgx/certification/v4/pckcert?encrypted_ppid={}&cpusvn={}&pcesvn={}&\
		 pceid={}",
		hex_encode(encrypted_ppid),
		hex_encode(cpusvn),
		hex_encode(pcesvn),
		hex_encode(pceid),
	);

	// ---- Try Intel PCS first (only service that can decrypt encrypted PPID)
	// ----
	if let Ok(api_key) = std::env::var("INTEL_PCS_API_KEY") {
		let pcs_client = reqwest::Client::new();

		println!("Trying Intel PCS...");
		match try_fetch_pck(
			&pcs_client,
			&format!("{INTEL_PCS_URL}{query}"),
			Some(&api_key),
		)
		.await
		{
			Ok(chain) => return Ok(chain),
			Err(e) => println!("  Intel PCS failed: {e:#}"),
		}
	}

	// ---- Try host PCCS (only works if platform was pre-registered via
	//      PCKIDRetrievalTool — PCCS cannot decrypt the encrypted PPID) ----
	let pccs_url =
		std::env::var("PCCS_URL").unwrap_or_else(|_| DEFAULT_PCCS_URL.to_string());

	let pccs_client = reqwest::Client::builder()
		// PCCS typically uses a self-signed certificate
		.danger_accept_invalid_certs(true)
		.build()?;

	println!("Trying host PCCS at {pccs_url}...");
	match try_fetch_pck(&pccs_client, &format!("{pccs_url}{query}"), None).await {
		Ok(chain) => return Ok(chain),
		Err(e) => println!("  PCCS failed: {e:#}"),
	}

	anyhow::bail!(
		"Could not fetch PCK certificate. Either:\n\
		 1. Set INTEL_PCS_API_KEY (get one free at \
		    https://api.portal.trustedservices.intel.com)\n\
		 2. Run PCKIDRetrievalTool on the host and restart qgsd"
	)
}

/// GET the PCK cert from a PCS-compatible endpoint.
///
/// Returns the full PEM chain (PCK leaf + issuer chain) suitable
/// for [`tdx_quote::pck::verify_pck_certificate_chain_pem`].
async fn try_fetch_pck(
	client: &reqwest::Client,
	url: &str,
	api_key: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
	let mut req = client.get(url);
	if let Some(key) = api_key {
		req = req.header("Ocp-Apim-Subscription-Key", key);
	}

	let resp = req.send().await?.error_for_status()?;

	// The issuer chain (intermediate + root CA) is returned
	// URL-encoded in a response header.
	let issuer_chain = resp
		.headers()
		.get("SGX-PCK-Certificate-Issuer-Chain")
		.and_then(|v| v.to_str().ok())
		.map(url_decode)
		.unwrap_or_default();

	// The PCK leaf cert is the response body (PEM).
	let pck_cert = resp.text().await?;

	// Full chain: leaf → intermediate → root
	let full_chain = format!("{pck_cert}{issuer_chain}");
	Ok(full_chain.into_bytes())
}
