use crate::{
    model::{ActionSpec, Danger, GroupSpec},
    probe::Probe,
    providers::Provider,
};

pub struct SystemProvider;

impl Provider for SystemProvider {
    fn id(&self) -> &'static str {
        "system"
    }

    fn scan(&self) -> Option<GroupSpec> {
        group(&Probe::system())
    }
}

pub(super) fn group(probe: &Probe) -> Option<GroupSpec> {
    let mut actions = Vec::new();
    if probe.brew || probe.mise || probe.amp || probe.omz_dir.is_some() {
        actions.push(ActionSpec::new(
            "upgrade.all",
            "upgrade: everything",
            "Upgrade all detected tools in parallel",
            build_upgrade_preview(probe),
            &["update", "system", "packages"],
            Danger::Mutating,
            || Box::pin(crate::commands::upgrade::run_all()),
        ));
    }
    if probe.brew {
        actions.extend([
            ActionSpec::new(
                "upgrade.brew-packages",
                "upgrade: brew packages",
                "brew update && brew upgrade",
                "$ brew update\n$ brew upgrade --greedy\n$ brew cleanup\n$ brew autoremove\n$ brew doctor",
                &["update", "homebrew", "cleanup"],
                Danger::Mutating,
                || Box::pin(crate::commands::upgrade::run_brew()),
            ),
            ActionSpec::new(
                "upgrade.brew-casks",
                "upgrade: brew casks",
                "Upgrade GUI apps via Homebrew",
                "$ brew update\n$ brew upgrade --cask --greedy\n$ brew cleanup\n$ brew autoremove\n$ brew doctor",
                &["update", "homebrew", "apps", "cleanup"],
                Danger::Mutating,
                || Box::pin(crate::commands::upgrade::run_brew_casks()),
            ),
        ]);
    }
    if probe.mise {
        actions.push(ActionSpec::new(
            "upgrade.mise",
            "upgrade: mise tools",
            "Upgrade all mise-managed tools",
            "$ mise upgrade",
            &["update", "runtime", "versions"],
            Danger::Mutating,
            || Box::pin(crate::commands::upgrade::run_mise()),
        ));
    }
    if probe.amp {
        actions.push(ActionSpec::new(
            "upgrade.amp",
            "upgrade: amp",
            "Upgrade Amp CLI",
            "$ amp update",
            &["update", "cli"],
            Danger::Mutating,
            || Box::pin(crate::commands::upgrade::run_amp()),
        ));
    }
    if let Some(omz_dir) = &probe.omz_dir {
        let omz_dir = omz_dir.clone();
        actions.push(ActionSpec::new(
            "upgrade.oh-my-zsh",
            "upgrade: oh-my-zsh",
            "Update oh-my-zsh to latest version",
            "$ sh ~/.oh-my-zsh/tools/upgrade.sh",
            &["update", "shell", "zsh"],
            Danger::Mutating,
            move || Box::pin(crate::commands::upgrade::run_omz(omz_dir.clone())),
        ));
    }
    (!actions.is_empty()).then_some(GroupSpec {
        id: "system".into(),
        title: "System".into(),
        actions,
    })
}

fn build_upgrade_preview(probe: &Probe) -> String {
    let mut lines = vec!["Runs in parallel:".to_string()];
    if probe.omz_dir.is_some() {
        lines.push("  $ sh ~/.oh-my-zsh/tools/upgrade.sh".into());
    }
    if probe.mise {
        lines.push("  $ mise upgrade".into());
    }
    if probe.amp {
        lines.push("  $ amp update".into());
    }
    if probe.brew {
        lines.push("  $ brew update && brew upgrade --greedy && brew cleanup && brew autoremove && brew doctor".into());
    }
    lines.join("\n")
}
