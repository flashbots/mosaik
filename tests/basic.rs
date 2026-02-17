// tests can describe long scenarios
#![allow(clippy::too_many_lines)]

use serde::{Deserialize, Serialize};

mod collections;
mod discovery;
mod groups;
mod streams;
mod utils;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data1(pub String);

impl Data1 {
	#[allow(dead_code)]
	pub fn new(s: &str) -> Self {
		Self(s.to_string())
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data2(pub String);

#[allow(dead_code)]
impl Data2 {
	pub fn new(s: &str) -> Self {
		Self(s.to_string())
	}
}
