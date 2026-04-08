fn main() {
	let output = mosaik::tee::tdx::ImageBuilder::alpine().build();
	println!("cargo:warning=Alpine Image build output: {output:#?}");
}
