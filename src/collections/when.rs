use super::Version;

pub struct When {
	group: crate::groups::When,
}

impl When {
	pub(super) const fn new(group: crate::groups::When) -> Self {
		Self { group }
	}
}

impl When {
	pub async fn online(&self) {
		self.group.online().await;
	}

	pub async fn offline(&self) {
		self.group.offline().await;
	}

	pub async fn updated(&self) {
		self.group.committed().changed().await;
	}

	pub async fn reaches(&self, pos: Version) {
		self.group.committed().reaches(pos.0).await;
	}
}
