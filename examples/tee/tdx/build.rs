fn main() {
	let output = mosaik::tee::tdx::build::alpine().build();
	println!("cargo:warning=Alpine Image build output: {output:#?}");
}
