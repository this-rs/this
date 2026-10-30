//! `ValidatingCreator`: the descriptor's validators and transforms in the COMMON write
//! path (RFC module-descriptor §3.5).
//!
//! Every protocol that writes an entity goes through `ServerHost::entity_creators`
//! (GraphQL `mutation_executor` / `link_mutations`, gRPC `entity_service`, REST link
//! handlers that create the target). Wrapping those creators once, when the host learns its
//! descriptors, puts validation in front of all of them: no surface can forget it, and no
//! surface re-implements it.
//!
//! - Only entities WITH a descriptor are wrapped; the others keep their creator untouched
//!   (historical behaviour, bit for bit).
//! - Errors are a [`ContractError`] carried inside `anyhow::Error`; each surface recovers
//!   it with `this::core::errors::from_anyhow` (422 / `INVALID_ARGUMENT` /
//!   `BAD_USER_INPUT`).
//! - `delete` is delegated as is (nothing to validate).
//! - The custom registry is shared through an `ArcSwap` so that
//!   `ServerHost::with_custom_registry` reaches creators already wrapped.

// `ContractError` is the shared transport error (same choice as `this-descriptor`).
#![allow(clippy::result_large_err)]

use crate::core::module::EntityCreator;
use crate::descriptor::engine::validate::{self, Side, ValidateCtx, WriteOp};
use crate::descriptor::{CompiledEntity, ContractError, CustomRegistry, DescriptorSet};
use anyhow::Result;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Custom validators/transforms shared by the gate and the REST layer.
pub type SharedRegistry = Arc<ArcSwap<CustomRegistry>>;

/// A fresh registry holder (framework built-ins only).
pub fn shared_registry() -> SharedRegistry {
    Arc::new(ArcSwap::from_pointee(CustomRegistry::default()))
}

/// Run the engine on the server side.
pub fn validate_payload(
    entity: &CompiledEntity,
    op: WriteOp,
    payload: Value,
    registry: &CustomRegistry,
) -> Result<Value, ContractError> {
    let ctx = ValidateCtx {
        registry,
        side: Side::Server,
    };
    validate::apply(entity, op, payload, &ctx).map(|o| o.value)
}

/// An `EntityCreator` that validates (and transforms) the payload against the entity's
/// descriptor before delegating.
pub struct ValidatingCreator {
    inner: Arc<dyn EntityCreator>,
    entity: Arc<CompiledEntity>,
    registry: SharedRegistry,
}

impl ValidatingCreator {
    pub fn new(
        inner: Arc<dyn EntityCreator>,
        entity: Arc<CompiledEntity>,
        registry: SharedRegistry,
    ) -> Self {
        Self {
            inner,
            entity,
            registry,
        }
    }

    /// Entity validated by this creator.
    pub fn entity(&self) -> &CompiledEntity {
        &self.entity
    }

    fn check(&self, op: WriteOp, payload: Value) -> Result<Value> {
        let registry = self.registry.load();
        validate_payload(&self.entity, op, payload, &registry).map_err(anyhow::Error::new)
    }
}

#[async_trait]
impl EntityCreator for ValidatingCreator {
    async fn create_from_json(&self, entity_data: Value) -> Result<Value> {
        let checked = self.check(WriteOp::Create, entity_data)?;
        self.inner.create_from_json(checked).await
    }

    async fn update_from_json(&self, entity_id: &Uuid, entity_data: Value) -> Result<Value> {
        let checked = self.check(WriteOp::Update, entity_data)?;
        self.inner.update_from_json(entity_id, checked).await
    }

    async fn delete(&self, entity_id: &Uuid) -> Result<()> {
        self.inner.delete(entity_id).await
    }
}

/// Wrap every creator whose entity has a descriptor in `set`. Pass the RAW creators (as
/// registered by the modules): wrapping an already wrapped map validates twice, which is
/// harmless (transforms are idempotent, RFC §3.5) but wasted work.
pub fn wrap_creators(
    creators: &HashMap<String, Arc<dyn EntityCreator>>,
    set: &DescriptorSet,
    registry: &SharedRegistry,
) -> HashMap<String, Arc<dyn EntityCreator>> {
    creators
        .iter()
        .map(|(entity, creator)| {
            let wrapped = match set.compiled(entity) {
                Some(compiled) => Arc::new(ValidatingCreator::new(
                    creator.clone(),
                    compiled.clone(),
                    registry.clone(),
                )) as Arc<dyn EntityCreator>,
                None => creator.clone(),
            };
            (entity.clone(), wrapped)
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::descriptor::framework::codes;
    use crate::descriptor::{
        Applies, EntityDesc, FieldDesc, FieldType, ModuleDescriptor, TransformSpec, ValidatorSpec,
    };
    use serde_json::json;
    use std::sync::Mutex;

    /// Records what reaches the store.
    #[derive(Default)]
    pub(crate) struct RecordingCreator {
        pub(crate) seen: Mutex<Vec<Value>>,
    }

    #[async_trait]
    impl EntityCreator for RecordingCreator {
        async fn create_from_json(&self, mut v: Value) -> Result<Value> {
            self.seen.lock().unwrap().push(v.clone());
            v["id"] = json!(Uuid::new_v4().to_string());
            Ok(v)
        }
        async fn update_from_json(&self, id: &Uuid, mut v: Value) -> Result<Value> {
            self.seen.lock().unwrap().push(v.clone());
            v["id"] = json!(id.to_string());
            Ok(v)
        }
    }

    /// `billing` module: `invoice { number: trim+uppercase, len 3..=50; amount > 0 }`.
    pub(crate) fn billing() -> ModuleDescriptor {
        let mut e = EntityDesc::new("invoice");
        e.fields = vec![
            FieldDesc::new("number", FieldType::String)
                .transform(TransformSpec::Trim, Applies::Always)
                .transform(TransformSpec::Uppercase, Applies::Always)
                .validator(
                    ValidatorSpec::StringLength { min: 3, max: 50 },
                    Applies::Always,
                ),
            FieldDesc::new("amount", FieldType::F64)
                .validator(ValidatorSpec::Positive, Applies::Always),
        ];
        let mut m = ModuleDescriptor::new("billing", "1.0.0");
        m.entities.push(e);
        m
    }

    pub(crate) fn billing_set() -> DescriptorSet {
        let mut set = DescriptorSet::new();
        set.insert(billing()).expect("valid descriptor");
        set
    }

    fn gate() -> (Arc<RecordingCreator>, ValidatingCreator) {
        let rec = Arc::new(RecordingCreator::default());
        let set = billing_set();
        let g = ValidatingCreator::new(
            rec.clone(),
            set.compiled("invoice").unwrap().clone(),
            shared_registry(),
        );
        (rec, g)
    }

    #[tokio::test]
    async fn validating_creator_rejects_before_store_and_transforms_on_success() {
        let (rec, g) = gate();
        let err = g
            .create_from_json(json!({"number": "abc", "amount": -1}))
            .await
            .unwrap_err();
        let c = crate::core::errors::from_anyhow(&err);
        assert_eq!(c.code.as_str(), codes::VALIDATION_FAILED);
        assert_eq!(c.fields[0].code.as_str(), codes::VALIDATION_POSITIVE);
        assert!(rec.seen.lock().unwrap().is_empty(), "store never reached");

        g.create_from_json(json!({"number": " ab1 ", "amount": 2}))
            .await
            .unwrap();
        assert_eq!(rec.seen.lock().unwrap()[0]["number"], "AB1");
    }

    #[tokio::test]
    async fn validating_creator_update_checks_present_fields_only() {
        let (rec, g) = gate();
        let id = Uuid::new_v4();
        g.update_from_json(&id, json!({"amount": 5})).await.unwrap();
        let err = g
            .update_from_json(&id, json!({"number": "x"}))
            .await
            .unwrap_err();
        assert_eq!(
            crate::core::errors::from_anyhow(&err).fields[0]
                .code
                .as_str(),
            codes::VALIDATION_STRING_LENGTH
        );
        assert_eq!(rec.seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn wrap_creators_only_wraps_described_entities() {
        let mut raw: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        let legacy: Arc<dyn EntityCreator> = Arc::new(RecordingCreator::default());
        raw.insert("invoice".into(), Arc::new(RecordingCreator::default()));
        raw.insert("legacy".into(), legacy.clone());
        let wrapped = wrap_creators(&raw, &billing_set(), &shared_registry());
        assert!(!Arc::ptr_eq(&wrapped["invoice"], &raw["invoice"]));
        assert!(
            Arc::ptr_eq(&wrapped["legacy"], &legacy),
            "undescribed: untouched"
        );
    }
}
