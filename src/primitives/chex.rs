/// Utility for converting hex string literals to byte arrays at compile time.
///
/// # Panics
/// Panics if the input string is not a valid hex string of the expected length.
pub const fn const_hex<const N: usize>(s: &str) -> [u8; N] {
	const fn hex_nibble(b: u8) -> u8 {
		match b {
			b'0'..=b'9' => b - b'0',
			b'a'..=b'f' => b - b'a' + 10,
			b'A'..=b'F' => b - b'A' + 10,
			_ => panic!("Invalid hex character"),
		}
	}

	assert!(N > 0, "Array length must be greater than 0");
	assert!(s.len() == N * 2, "Invalid hex string length");

	let bytes = s.as_bytes();

	let mut arr = [0u8; N];

	let mut i = 0;
	while i < N {
		let hi = hex_nibble(bytes[i * 2]);
		let lo = hex_nibble(bytes[i * 2 + 1]);
		arr[i] = (hi << 4) | lo;
		i += 1;
	}
	arr
}
