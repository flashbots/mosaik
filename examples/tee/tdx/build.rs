fn main() {
	let output = mosaik::tdx::build::alpine().build();
	println!("cargo:warning=Alpine Image build output: {output:#?}");
}
