use anyhow::Result;
use crate::tui::{TaskDef, run_tasks};

pub async fn clean() -> Result<()> {
    run_tasks(vec![
        TaskDef::new("Stopping Gradle daemon", "gradle", &["--stop"]),
        TaskDef {
            label: "Removing .gradle dirs".into(),
            program: "sh".into(),
            args: vec![
                "-c".into(),
                "find . -name .gradle -type d -maxdepth 5 -not -path '*/node_modules/*' -exec rm -rf {} +".into(),
            ],
        },
        TaskDef {
            label: "Removing build dirs".into(),
            program: "sh".into(),
            args: vec![
                "-c".into(),
                "find . -name build -type d -maxdepth 5 -not -path '*/node_modules/*' -exec rm -rf {} +".into(),
            ],
        },
    ])
    .await
}
