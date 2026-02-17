// Unix socket and gRPC server

pub mod agents;
pub mod awareness;
pub mod llm;
pub mod protocol;
pub mod socket;
pub mod state;
pub mod subscriptions;

pub use agents::{AgentActivity, AgentState, AgentTracker};
pub use awareness::{build_awareness, AwarenessError};
pub use llm::{LlmClient, LlmConfig, LlmError};
pub use socket::{SocketServer, SocketError};
pub use state::ServerState;
pub use subscriptions::{SubscriptionManager, SubscriptionId};

