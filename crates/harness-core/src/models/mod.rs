//! Capability-based model registry (§7, §55).
//!
//! The registry answers "which configured model can do X?" from roles and advertised capabilities,
//! never from model names, so replacing Qwen/LFM/Bonsai is a config change.

use std::collections::BTreeMap;

use crate::config::{Capabilities, Config, ModelConfig, ModelRole};

/// A capability query. `None` fields are "don't care"; `Some(true)` requires the capability.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CapabilityRequirement {
    pub role: Option<ModelRole>,
    pub tool_calling: bool,
    pub vision: bool,
    pub reasoning: bool,
    pub structured_output: bool,
    pub streaming: bool,
    /// Minimum configured context window, in tokens.
    pub min_context: u32,
    /// Only models running on this machine.
    pub local_only: bool,
}

impl CapabilityRequirement {
    /// Returns true when `model` satisfies every requirement.
    pub fn is_met_by(&self, model: &ModelConfig) -> bool {
        let c: &Capabilities = &model.capabilities;
        self.role.is_none_or(|r| r == model.role)
            && (!self.tool_calling || c.tool_calling)
            && (!self.vision || c.vision)
            && (!self.reasoning || c.reasoning)
            && (!self.structured_output || c.structured_output)
            && (!self.streaming || c.streaming)
            && model.context >= self.min_context
            && (!self.local_only || c.local)
    }
}

/// Read-only view over the configured model slots.
#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    models: BTreeMap<String, ModelConfig>,
}

impl ModelRegistry {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            models: cfg.models.clone(),
        }
    }

    pub fn get(&self, slot: &str) -> Option<&ModelConfig> {
        self.models.get(slot)
    }

    /// All slots in deterministic (sorted) order.
    pub fn slots(&self) -> impl Iterator<Item = (&str, &ModelConfig)> {
        self.models.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Slots matching a capability query, in deterministic order.
    pub fn find(&self, req: &CapabilityRequirement) -> Vec<(&str, &ModelConfig)> {
        self.slots().filter(|(_, m)| req.is_met_by(m)).collect()
    }

    /// First slot with the given role, if any.
    pub fn first_with_role(&self, role: ModelRole) -> Option<(&str, &ModelConfig)> {
        self.slots().find(|(_, m)| m.role == role)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::from_toml_str;

    fn registry() -> ModelRegistry {
        let cfg = from_toml_str(
            r#"
            [models.main]
            provider = "llama_cpp"
            id = "engineer-9b"
            role = "engineer"
            endpoint = "http://127.0.0.1:8001/v1"
            context = 16384
            hardware = "gpu"
            capabilities = { tool_calling = true, reasoning = true, structured_output = true, gpu = true }

            [models.micro]
            provider = "llama_cpp"
            id = "micro-1b"
            role = "decision"
            endpoint = "http://127.0.0.1:8002/v1"
            context = 8192
            hardware = "cpu"
            capabilities = { tool_calling = true, structured_output = true }
            "#,
        )
        .expect("valid config");
        ModelRegistry::from_config(&cfg)
    }

    #[test]
    fn finds_by_capability_not_name() {
        let r = registry();
        let req = CapabilityRequirement {
            reasoning: true,
            ..Default::default()
        };
        let hits: Vec<_> = r.find(&req).into_iter().map(|(s, _)| s).collect();
        assert_eq!(hits, vec!["main"]);
    }

    #[test]
    fn min_context_filters() {
        let r = registry();
        let req = CapabilityRequirement {
            min_context: 10_000,
            ..Default::default()
        };
        assert_eq!(r.find(&req).len(), 1);
        let req = CapabilityRequirement {
            min_context: 1_000,
            ..Default::default()
        };
        assert_eq!(r.find(&req).len(), 2);
    }

    #[test]
    fn role_lookup() {
        let r = registry();
        assert_eq!(
            r.first_with_role(ModelRole::Decision).map(|(s, _)| s),
            Some("micro")
        );
        assert!(r.first_with_role(ModelRole::DeepReasoning).is_none());
    }
}
