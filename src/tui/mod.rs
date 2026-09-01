pub mod analyzer;
pub mod app;
pub mod browser;
pub mod cleanup_flow;
mod file_preview;
pub mod finder;
pub mod insights;
pub mod menu;
pub mod session;
pub mod trust;

pub use app::{TaskDef, run_parallel_tasks, run_tasks};
