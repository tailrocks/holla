pub mod analyzer;
pub mod app;
pub mod cleanup_flow;
pub mod finder;
pub mod insights;
pub mod menu;

pub use app::{TaskDef, run_parallel_tasks, run_tasks};
