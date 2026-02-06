use {
	super::Datum,
	serde::{Deserialize, Serialize},
};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Criteria {}

impl Criteria {
	pub const fn matches<D: Datum>(&self, _item: &D) -> bool {
		true
	}
}
