use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    probe::Probe,
    providers::Provider,
};

pub struct GradleProvider;

impl Provider for GradleProvider {
    fn id(&self) -> &'static str {
        "gradle"
    }

    fn scan(&self) -> Option<GroupSpec> {
        group(&Probe::gradle())
    }
}

pub(super) fn group(probe: &Probe) -> Option<GroupSpec> {
    probe.gradle.then(|| GroupSpec {
        id: "system",
        title: "System".into(),
        actions: vec![ActionSpec::new(
            "gradle.clean-all",
            "gradle: clean all",
            "Stop daemon and clean all build dirs recursively",
            "Stop Gradle, enumerate .gradle/build directories to depth 5, then move them to Trash through holla's validated cleanup core",
            &["cleanup", "build", "cache"],
            Danger::Destructive,
            || Box::pin(crate::commands::gradle::clean()),
        )],
    })
}
