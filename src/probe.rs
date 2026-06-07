use which::which;

#[derive(Debug, Clone)]
pub struct Probe {
    pub git: bool,
    pub docker: bool,
    pub brew: bool,
    pub gradle: bool,
    pub mise: bool,
    pub amp: bool,
    pub idea: bool,
}

impl Probe {
    pub fn run() -> Self {
        Self {
            git: which("git").is_ok(),
            docker: which("docker").is_ok(),
            brew: which("brew").is_ok(),
            gradle: which("gradle").is_ok(),
            mise: which("mise").is_ok(),
            amp: which("amp").is_ok(),
            idea: which("idea").is_ok(),
        }
    }

    #[expect(dead_code)]
    pub fn any(&self) -> bool {
        self.git || self.docker || self.brew || self.gradle
    }
}
