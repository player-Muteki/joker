#![forbid(unsafe_code)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

pub mod anthropic;
pub mod google;
pub mod model_discovery;
pub mod openai;
pub mod profiles;
pub mod protocol;
pub mod reconnect;
pub mod transform;

pub use openai::{OpenAiCompatibleConfig, OpenAiCompatibleModel, OpenAiProviderError};
pub use protocol::*;
pub use model_discovery::*;
pub use profiles::*;
pub use reconnect::ReconnectingModel;
