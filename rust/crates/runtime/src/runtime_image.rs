use std::process::Command;

use crate::release::standard_runtime_image;

pub const DEFAULT_RUNTIME_IMAGE: &str = concat!(
    "ghcr.io/bamm-squared/claw-bastion-runtime:",
    env!("CARGO_PKG_VERSION")
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeImage {
    configured: String,
    resolved_id: Option<String>,
}

impl RuntimeImage {
    #[must_use]
    pub fn configured() -> String {
        std::env::var("CLAW_WORKER_IMAGE")
            .unwrap_or_else(|_| standard_runtime_image(env!("CARGO_PKG_VERSION")))
    }

    #[must_use]
    pub fn is_custom(&self) -> bool {
        std::env::var_os("CLAW_WORKER_IMAGE").is_some()
    }

    #[must_use]
    pub fn resolve() -> Self {
        let configured = Self::configured();
        let resolved_id = Command::new("podman")
            .args(["image", "inspect", "--format", "{{.Id}}", &configured])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty());
        Self {
            configured,
            resolved_id,
        }
    }

    #[must_use]
    pub fn configured_ref(&self) -> &str {
        &self.configured
    }

    #[must_use]
    pub fn resolved_id(&self) -> Option<&str> {
        self.resolved_id.as_deref()
    }

    #[must_use]
    pub fn is_available(&self) -> bool {
        self.resolved_id.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeImage, DEFAULT_RUNTIME_IMAGE};
    use crate::release::standard_runtime_image;

    #[test]
    fn default_runtime_image_is_stable_and_explicit() {
        assert_eq!(
            DEFAULT_RUNTIME_IMAGE,
            standard_runtime_image(env!("CARGO_PKG_VERSION"))
        );
        assert!(!RuntimeImage::configured().is_empty());
    }
}
