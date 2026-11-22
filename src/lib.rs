pub mod config;
pub mod db;
pub mod models;
pub mod token;

// Touch dependencies required by layer 1 Cargo.toml that will be used in future layers
// to prevent rustc unused_dependencies warnings.
#[allow(unused_imports)]
use async_trait as _;
#[allow(unused_imports)]
use axum as _;
#[allow(unused_imports)]
use reqwest as _;
#[allow(unused_imports)]
use tokio_tungstenite as _;
#[allow(unused_imports)]
use tokio_util as _;
#[allow(unused_imports)]
use tracing_appender as _;
