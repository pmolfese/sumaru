// Developed by P. Molfese, National Institutes of Health (NIH).
// See LICENSE for the public-domain notice, warranty disclaimer, and attribution.

//! Parent vertex correspondence. This layer never performs geometric alignment
//! or changes local triangle indices. Parent identity checks are distinct from
//! the structural checks required to safely address data.

use std::collections::HashSet;

use anyhow::{Result, ensure};

use crate::surface::SurfaceSide;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParentDomainIdentity {
    pub vertex_count: Option<usize>,
    pub hemisphere: Option<SurfaceSide>,
    pub subject: Option<String>,
    /// Reserved for a documented parent topology AND vertex-order fingerprint.
    /// Do not substitute a coordinate hash, filename, or vertex count.
    pub vertex_order_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParentVertexMapping {
    pub parent: ParentDomainIdentity,
    /// Entry i is the parent vertex for local vertex/data row i.
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParentMismatchPolicy {
    #[default]
    Warn,
    Strict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParentVerification {
    /// Subject, hemisphere, and parent count agree. This is metadata agreement,
    /// not independent verification of geometry or vertex order.
    MetadataMatched,
    Verified,
    Unverified,
    Mismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentAssessment {
    pub verification: ParentVerification,
    /// Callers must surface these diagnostics when applying the mapping.
    pub warnings: Vec<String>,
}

impl ParentVertexMapping {
    pub fn new(parent: ParentDomainIdentity, indices: Vec<u32>, rows: usize) -> Result<Self> {
        let mapping = Self { parent, indices };
        mapping.validate(rows)?;
        Ok(mapping)
    }

    pub fn validate(&self, rows: usize) -> Result<()> {
        ensure!(!self.indices.is_empty(), "parent vertex mapping is empty");
        ensure!(
            self.indices.len() == rows,
            "parent vertex mapping has {} IDs but geometry/data has {rows} rows",
            self.indices.len()
        );
        let mut seen = HashSet::new();
        for &id in &self.indices {
            ensure!(
                seen.insert(id),
                "parent vertex mapping contains duplicate ID {id}"
            );
            if let Some(count) = self.parent.vertex_count {
                ensure!(
                    (id as usize) < count,
                    "parent vertex ID {id} is outside declared parent count {count}"
                );
            }
        }
        Ok(())
    }

    /// Assess an explicitly selected parent. This does not choose a display
    /// domain, apply a mapping, or interpret a std. filename as registration.
    pub fn assess_parent(
        &self,
        target: &ParentDomainIdentity,
        target_node_count: usize,
        policy: ParentMismatchPolicy,
    ) -> Result<ParentAssessment> {
        self.validate(self.indices.len())?;
        for &id in &self.indices {
            ensure!(
                (id as usize) < target_node_count,
                "parent vertex ID {id} is outside target node count {target_node_count}"
            );
        }
        if let Some(count) = target.vertex_count {
            ensure!(
                count == target_node_count,
                "target identity count disagrees with target geometry"
            );
        }
        let mut warnings = Vec::new();
        if let Some(count) = self.parent.vertex_count
            && count != target_node_count
        {
            warnings.push(format!(
                "parent vertex count {count} differs from target count {target_node_count}"
            ));
        }
        if let (Some(a), Some(b)) = (&self.parent.hemisphere, &target.hemisphere)
            && a != b
        {
            warnings.push(format!("parent hemisphere {a:?} differs from target {b:?}"));
        }
        if let (Some(a), Some(b)) = (&self.parent.subject, &target.subject)
            && a != b
        {
            warnings.push(format!("parent subject {a} differs from target {b}"));
        }
        let fingerprints_match = match (
            &self.parent.vertex_order_fingerprint,
            &target.vertex_order_fingerprint,
        ) {
            (Some(a), Some(b)) if a == b && !a.is_empty() => true,
            (Some(a), Some(b)) if a != b => {
                warnings.push("parent vertex-order fingerprints differ".into());
                false
            }
            _ => false,
        };
        let metadata_matches = self.parent.vertex_count == Some(target_node_count)
            && target.vertex_count == Some(target_node_count)
            && self.parent.subject.as_ref().is_some_and(|subject| {
                !subject.trim().is_empty() && target.subject.as_ref() == Some(subject)
            })
            && matches!(
                self.parent.hemisphere,
                Some(SurfaceSide::Left | SurfaceSide::Right)
            )
            && self.parent.hemisphere == target.hemisphere;
        let verification = if !warnings.is_empty() {
            ParentVerification::Mismatch
        } else if fingerprints_match {
            ParentVerification::Verified
        } else if metadata_matches {
            ParentVerification::MetadataMatched
        } else {
            warnings.push("parent identity metadata is incomplete; subject, hemisphere, and parent count could not all be matched".into());
            ParentVerification::Unverified
        };
        ensure!(
            policy != ParentMismatchPolicy::Strict
                || matches!(
                    verification,
                    ParentVerification::Verified | ParentVerification::MetadataMatched
                ),
            "strict parent validation: {}",
            warnings.join("; ")
        );
        if !warnings.is_empty() {
            warnings.push(
                "using vertex IDs as supplied; no geometric alignment or resampling is implied"
                    .into(),
            );
        }
        Ok(ParentAssessment {
            verification,
            warnings,
        })
    }
}
