use serde::{Deserialize, Serialize};

use crate::Money;

/// The capability vocabulary of `docs/contracts/provider.md`.
///
/// A provider declares the subset it implements. The runtime records a missing
/// capability instead of assuming a false universal endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapability {
    Search,
    Fetch,
    Query,
    /// Execute a declared entrypoint and return what it produced. The one
    /// capability whose result is produced rather than retrieved, which is
    /// why it is its own word: a skill is supply the operator invokes, not a
    /// document anybody published (`docs/contracts/provider.md`).
    Invoke,
    Quote,
    Licensed,
    Report,
    Corroborate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRef {
    pub name: String,
    pub adapter_version: String,
    #[serde(default)]
    pub capabilities: Vec<ProviderCapability>,
}

/// One acquisition against one provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplyStep {
    pub provider: ProviderRef,
    pub capability: ProviderCapability,
    /// Maximum results requested. Fixed across a comparison so that one
    /// provider is not asked for more evidence than another.
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupplyPlan {
    pub name: String,
    pub version: String,
    pub steps: Vec<SupplyStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_cost: Option<Money>,
}

/// The model half of a job's policy, deliberately separate from the supply
/// half so an operator can tell whether an improvement came from better
/// evidence or a better model.
///
/// The model name is the whole of it. The gateway request carries nothing
/// else, so a provider pin, a fallback list or a per-model cap declared here
/// would be sealed into the manifest as though it governed the crossing while
/// changing nothing. Unknown fields are load errors, so a job file declaring
/// one is told the control does not exist rather than left believing it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPlan {
    pub name: String,
    pub version: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    EmptyPlan,
    EmptyModel,
    ZeroLimit,
    MissingCapability {
        provider: String,
        capability: ProviderCapability,
    },
}

impl SupplyPlan {
    /// A plan is valid only if every provider it names declares the capability
    /// the step needs. Catching this before execution is what keeps a run from
    /// discovering at the crossing that it planned an operation nobody offers.
    pub fn validate(&self) -> Result<(), PlanError> {
        if self.steps.is_empty() {
            return Err(PlanError::EmptyPlan);
        }
        for step in &self.steps {
            if step.limit == 0 {
                return Err(PlanError::ZeroLimit);
            }
            if !step.provider.capabilities.contains(&step.capability) {
                return Err(PlanError::MissingCapability {
                    provider: step.provider.name.clone(),
                    capability: step.capability,
                });
            }
        }
        Ok(())
    }
}

impl ModelPlan {
    pub fn validate(&self) -> Result<(), PlanError> {
        if self.model.trim().is_empty() {
            return Err(PlanError::EmptyModel);
        }
        Ok(())
    }
}
