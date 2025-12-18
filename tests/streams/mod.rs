use serde::{Deserialize, Serialize};

mod consumer;
mod producer;
mod smoke;
mod stats;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data1(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data2(pub String);
