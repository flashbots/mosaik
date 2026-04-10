fn main() {
	let output = mosaik::tdx::build::alpine().build();
	for line in format!("{output:#?}").lines() {
		println!("cargo:warning={line}");
	}
}
