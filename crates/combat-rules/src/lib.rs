pub mod combat;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FactionId(pub SmolStr);
