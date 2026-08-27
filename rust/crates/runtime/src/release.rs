pub const GITHUB_OWNER: &str = "bamm-squared";
pub const GITHUB_REPOSITORY: &str = "claw-code-bastion";
pub const RUNTIME_PACKAGE: &str = "claw-bastion-runtime";
pub const RUNTIME_REGISTRY: &str = "ghcr.io";

pub const RELEASE_REPOSITORY: &str = "bamm-squared/claw-code-bastion";
pub const RELEASE_REPOSITORY_URL: &str = "https://github.com/bamm-squared/claw-code-bastion";

#[must_use]
pub fn standard_runtime_image(version: &str) -> String {
    format!("{RUNTIME_REGISTRY}/{GITHUB_OWNER}/{RUNTIME_PACKAGE}:{version}")
}
