use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::ModelLimits;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum CompatibilityMode {
    #[serde(rename = "strict")]
    Strict,
    #[serde(rename = "balanced")]
    Balanced,
    #[default]
    #[serde(rename = "max_compat", alias = "max-compat")]
    MaxCompat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub enum ModelModality {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "image")]
    Image,
    #[serde(rename = "audio")]
    Audio,
    /// Narrow document capability for PDF inputs.
    #[serde(rename = "pdf")]
    Pdf,
    /// Generic file capability. This is a superset for PDF inputs at policy time.
    #[serde(rename = "file")]
    File,
    /// Video input capability. Phase one uses this for request gating only.
    #[serde(rename = "video")]
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ApplyPatchTransport {
    #[serde(rename = "function")]
    Function,
    #[serde(rename = "freeform")]
    Freeform,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelModalities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<ModelModality>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Vec<ModelModality>>,
}

impl ModelModalities {
    pub fn merged_with(&self, overrides: Option<&ModelModalities>) -> Option<ModelModalities> {
        let merged = ModelModalities {
            input: overrides
                .and_then(|item| item.input.clone())
                .or_else(|| self.input.clone()),
            output: overrides
                .and_then(|item| item.output.clone())
                .or_else(|| self.output.clone()),
        };
        if merged.input.is_none() && merged.output.is_none() {
            None
        } else {
            Some(merged)
        }
    }

    pub fn validate(&self, owner: &str) -> Result<(), String> {
        validate_modality_list(owner, "modalities.input", self.input.as_ref())?;
        validate_modality_list(owner, "modalities.output", self.output.as_ref())?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelToolSurface {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_search: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_view_image: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apply_patch_transport: Option<ApplyPatchTransport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_parallel_calls: Option<bool>,
}

impl ModelToolSurface {
    pub fn merged_with(&self, overrides: Option<&ModelToolSurface>) -> Option<ModelToolSurface> {
        let merged = ModelToolSurface {
            supports_search: overrides
                .and_then(|item| item.supports_search)
                .or(self.supports_search),
            supports_view_image: overrides
                .and_then(|item| item.supports_view_image)
                .or(self.supports_view_image),
            apply_patch_transport: overrides
                .and_then(|item| item.apply_patch_transport)
                .or(self.apply_patch_transport),
            supports_parallel_calls: overrides
                .and_then(|item| item.supports_parallel_calls)
                .or(self.supports_parallel_calls),
        };
        if merged.supports_search.is_none()
            && merged.supports_view_image.is_none()
            && merged.apply_patch_transport.is_none()
            && merged.supports_parallel_calls.is_none()
        {
            None
        } else {
            Some(merged)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSurfacePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<ModelModalities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ModelToolSurface>,
}

impl ModelSurfacePatch {
    pub fn merged_with(&self, overrides: Option<&ModelSurfacePatch>) -> ModelSurfacePatch {
        let modalities = match (
            &self.modalities,
            overrides.and_then(|item| item.modalities.as_ref()),
        ) {
            (Some(base), override_modalities) => base.merged_with(override_modalities),
            (None, Some(override_modalities)) => Some(override_modalities.clone()),
            (None, None) => None,
        };
        let tools = match (&self.tools, overrides.and_then(|item| item.tools.as_ref())) {
            (Some(base), override_tools) => base.merged_with(override_tools),
            (None, Some(override_tools)) => Some(override_tools.clone()),
            (None, None) => None,
        };
        ModelSurfacePatch { modalities, tools }
    }

    pub fn validate(&self, owner: &str) -> Result<(), String> {
        if let Some(modalities) = &self.modalities {
            modalities.validate(owner)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSurface {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ModelLimits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<ModelModalities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ModelToolSurface>,
}

fn validate_modality_list(
    owner: &str,
    field: &str,
    items: Option<&Vec<ModelModality>>,
) -> Result<(), String> {
    let Some(items) = items else {
        return Ok(());
    };
    if items.is_empty() {
        return Err(format!("{owner} {field} must not be empty"));
    }
    let mut seen = BTreeSet::new();
    for item in items {
        if !seen.insert(*item) {
            return Err(format!(
                "{owner} {field} contains duplicate modality `{item:?}`"
            ));
        }
    }
    Ok(())
}
