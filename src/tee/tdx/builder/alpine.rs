pub struct AlpineBuilder;

impl AlpineBuilder {
	#[must_use]
	pub fn with_major_version(self, _version: &str) -> Self {
		self
	}

	#[must_use]
	pub fn with_minor_version(self, _version: &str) -> Self {
		self
	}

	#[must_use]
	pub fn with_custom_minirootfs(self, bytes: &[u8]) -> Self {
		self
	}

	#[must_use]
	pub fn with_custom_vmlinuz(self, bytes: &[u8]) -> Self {
		self
	}

	#[must_use]
	pub fn with_custom_ovmf(self, bytes: &[u8]) -> Self {
		self
	}

	// Defaults to no ssh access
	#[must_use]
	pub fn with_ssh_access(self, pubkey: &str) -> Self {
		self
	}

	#[must_use]
	pub fn with_default_cpu_count(self, count: u32) -> Self {
		self
	}

	#[must_use]
	pub fn with_default_memory_size(self, size: u64) -> Self {
		self
	}

	#[must_use]
	pub fn with_bundle_runner(self) -> Self {
		self
	}

	#[must_use]
	pub fn without_bundle_runner(self) -> Self {
		self
	}

	pub fn build(self) {}
}

impl Default for AlpineBuilder {
	fn default() -> Self {
		Self
	}
}
