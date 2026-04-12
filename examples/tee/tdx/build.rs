use std::env::VarError;

fn main() {
	// Generate a random build ID so every compilation produces a unique
	// network.  Both the host binary and the TDX-packaged binary share
	// this value because they originate from the same build invocation.
	//
	// The TDX builder spawns a nested `cargo build` for the musl target,
	// which runs this build script a second time.  We set BUILD_ID as an
	// OS env var so the child cargo process inherits it and the inner
	// invocation reuses the same ID instead of generating a new one.
	let build_id = std::env::var("BUILD_ID").unwrap_or_else(|_| {
		let id = mosaik::UniqueId::random().to_string();
		// SAFETY: build scripts are single-threaded.
		unsafe { std::env::set_var("BUILD_ID", &id) };
		id
	});

	println!("cargo:rustc-env=BUILD_ID={build_id}");
	println!("cargo:warning=Build ID: {build_id}");

	let build_output = match std::env::var("BUILD_TYPE").as_deref() {
		Err(VarError::NotPresent) | Ok("alpine") => {
			println!("cargo:warning=Building Alpine-based MUSL TDX image...");
			mosaik::tee::tdx::build::alpine().build()
		}
		Ok("ubuntu") => {
			println!("cargo:warning=Building Ubuntu-based GLIBC TDX image...");
			mosaik::tee::tdx::build::ubuntu().build()
		}
		_ => panic!("Unknown BUILD_TYPE, expected 'alpine' or 'ubuntu'"),
	};

	for line in format!("{build_output:#?}").lines() {
		println!("cargo:warning={line}");
	}
}
