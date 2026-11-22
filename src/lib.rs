pub mod app_state;
pub mod config;
pub mod db;
pub mod discord;
pub mod mcp;
pub mod models;
pub mod token;
pub mod tray;
pub mod ui;

// Touch dependencies to prevent unused_dependencies warnings
#[allow(unused_imports)]
use async_trait as _;
#[allow(unused_imports)]
use tokio_util as _;
