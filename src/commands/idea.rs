
pub async fn clean() -> Result<()> {
    run_tasks(vec![
        TaskDef {
            label: "Removing .idea dirs".into(),
            program: "sh".into(),
            args: vec![
                "-c".into(),
                "find . -name .idea -type d -maxdepth 5 -not -path '*/node_modules/*' -exec rm -rf {} +".into(),
            ],
        },
        TaskDef {
            label: "Removing *.iml files".into(),
            program: "sh".into(),
            args: vec![
                "-c".into(),
                "find . -name '*.iml' -type f -not -path '*/node_modules/*' -exec rm -f {} +".into(),
            ],
        },
    ])
    .await
}
