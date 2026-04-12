use {
	crate::{UniqueId, id},
	core::str::FromStr,
	tdx_quote::Quote,
};

/// A 48-byte TDX measurement register value (MRTD, RTMR0–3).
///
/// Used to express an expected measurement when configuring a
/// [`Tdx`](super::Tdx).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Measurement([u8; 48]);

impl Measurement {
	/// Creates a measurement from a 48-byte array.
	pub const fn new(bytes: [u8; 48]) -> Self {
		Self(bytes)
	}

	/// Creates a measurement from a 96-character hex string.
	///
	/// # Panics
	/// Panics if the string is not exactly 96 hex characters (48 bytes).
	pub const fn hex(input: &str) -> Self {
		const fn hex_nibble(b: u8) -> u8 {
			match b {
				b'0'..=b'9' => b - b'0',
				b'a'..=b'f' => b - b'a' + 10,
				b'A'..=b'F' => b - b'A' + 10,
				_ => panic!("Invalid hex character"),
			}
		}

		assert!(
			input.len() == 96,
			"Hex string must be exactly 96 characters (48 bytes)"
		);

		let bytes = input.as_bytes();

		let mut arr = [0u8; 48];

		let mut i = 0;
		while i < 48 {
			let hi = hex_nibble(bytes[i * 2]);
			let lo = hex_nibble(bytes[i * 2 + 1]);
			arr[i] = (hi << 4) | lo;
			i += 1;
		}

		Self(arr)
	}

	pub const fn as_bytes(&self) -> &[u8; 48] {
		&self.0
	}
}

impl From<[u8; 48]> for Measurement {
	fn from(bytes: [u8; 48]) -> Self {
		Self(bytes)
	}
}

impl FromStr for Measurement {
	type Err = hex::FromHexError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let bytes = hex::decode(s)?;
		if bytes.len() != 48 {
			return Err(hex::FromHexError::InvalidStringLength);
		}
		let mut arr = [0u8; 48];
		arr.copy_from_slice(&bytes);
		Ok(Self(arr))
	}
}

impl core::fmt::Debug for Measurement {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "Measurement({})", hex::encode(self.0))
	}
}

impl core::fmt::Display for Measurement {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "{}", hex::encode(self.0))
	}
}

pub trait IntoMeasurement {
	fn into_measurement(self) -> Measurement;
}

impl IntoMeasurement for [u8; 48] {
	fn into_measurement(self) -> Measurement {
		Measurement::new(self)
	}
}

impl IntoMeasurement for &str {
	fn into_measurement(self) -> Measurement {
		Measurement::hex(self)
	}
}

impl IntoMeasurement for Measurement {
	fn into_measurement(self) -> Measurement {
		self
	}
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Measurements {
	mrtd: Measurement,
	rtmr: [Measurement; 4],
}

impl Measurements {
	pub const fn from_quote(quote: &Quote) -> Self {
		Self {
			mrtd: Measurement::new(quote.body.mrtd),
			rtmr: [
				Measurement::new(quote.body.rtmr0),
				Measurement::new(quote.body.rtmr1),
				Measurement::new(quote.body.rtmr2),
				Measurement::new(quote.body.rtmr3),
			],
		}
	}

	pub const fn mrtd(&self) -> Measurement {
		self.mrtd
	}

	pub const fn rtmr(&self, index: usize) -> Measurement {
		self.rtmr[index]
	}

	pub const fn rtmr0(&self) -> Measurement {
		self.rtmr[0]
	}

	pub const fn rtmr1(&self) -> Measurement {
		self.rtmr[1]
	}

	pub const fn rtmr2(&self) -> Measurement {
		self.rtmr[2]
	}

	pub const fn rtmr3(&self) -> Measurement {
		self.rtmr[3]
	}
}

impl From<Quote> for Measurements {
	fn from(quote: Quote) -> Self {
		Self {
			mrtd: Measurement::new(quote.body.mrtd),
			rtmr: [
				Measurement::new(quote.body.rtmr0),
				Measurement::new(quote.body.rtmr1),
				Measurement::new(quote.body.rtmr2),
				Measurement::new(quote.body.rtmr3),
			],
		}
	}
}

impl From<&Quote> for Measurements {
	fn from(quote: &Quote) -> Self {
		Self::from_quote(quote)
	}
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MeasurementsCriteria {
	pub mrtd: Option<Measurement>,
	pub rtmr: [Option<Measurement>; 4],
}

impl MeasurementsCriteria {
	#[must_use]
	pub const fn new() -> Self {
		Self {
			mrtd: None,
			rtmr: [None, None, None, None],
		}
	}

	#[must_use]
	pub const fn require_mrtd(mut self, mrtd: Measurement) -> Self {
		self.mrtd = Some(mrtd);
		self
	}

	#[must_use]
	pub const fn require_rtmr0(mut self, rtmr: Measurement) -> Self {
		self.rtmr[0] = Some(rtmr);
		self
	}

	#[must_use]
	pub const fn require_rtmr1(mut self, rtmr: Measurement) -> Self {
		self.rtmr[1] = Some(rtmr);
		self
	}

	#[must_use]
	pub const fn require_rtmr2(mut self, rtmr: Measurement) -> Self {
		self.rtmr[2] = Some(rtmr);
		self
	}

	#[must_use]
	pub const fn require_rtmr3(mut self, rtmr: Measurement) -> Self {
		self.rtmr[3] = Some(rtmr);
		self
	}

	/// Checks if the given measurements match the criteria.
	pub fn matches(&self, measurements: &Measurements) -> bool {
		if let Some(expected) = &self.mrtd
			&& measurements.mrtd.as_bytes() != expected.as_bytes()
		{
			return false;
		}

		for i in 0..4 {
			if let Some(expected) = &self.rtmr[i]
				&& measurements.rtmr[i].as_bytes() != expected.as_bytes()
			{
				return false;
			}
		}
		true
	}

	pub fn signature(&self) -> UniqueId {
		id!("mosaik.tee.tdx.measurements-criteria.v1")
			.derive(self.mrtd.as_ref().map_or(&[9u8; 48], |m| m.as_bytes()))
			.derive(self.rtmr[0].as_ref().map_or(&[1u8; 48], |m| m.as_bytes()))
			.derive(self.rtmr[1].as_ref().map_or(&[2u8; 48], |m| m.as_bytes()))
			.derive(self.rtmr[2].as_ref().map_or(&[3u8; 48], |m| m.as_bytes()))
			.derive(self.rtmr[3].as_ref().map_or(&[4u8; 48], |m| m.as_bytes()))
	}
}

impl Default for MeasurementsCriteria {
	fn default() -> Self {
		Self::new()
	}
}

impl From<Measurements> for MeasurementsCriteria {
	fn from(measurements: Measurements) -> Self {
		Self::new()
			.require_mrtd(measurements.mrtd)
			.require_rtmr0(measurements.rtmr[0])
			.require_rtmr1(measurements.rtmr[1])
			.require_rtmr2(measurements.rtmr[2])
			.require_rtmr3(measurements.rtmr[3])
	}
}

impl From<&Measurements> for MeasurementsCriteria {
	fn from(measurements: &Measurements) -> Self {
		Self::new()
			.require_mrtd(measurements.mrtd)
			.require_rtmr0(measurements.rtmr[0])
			.require_rtmr1(measurements.rtmr[1])
			.require_rtmr2(measurements.rtmr[2])
			.require_rtmr3(measurements.rtmr[3])
	}
}
