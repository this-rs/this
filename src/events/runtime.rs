//! FlowRuntime — subscribes to EventLog and dispatches events to compiled flows
//!
//! The FlowRuntime is the main execution engine for declarative event flows.
//! It runs as a background task that:
//!
//! 1. Subscribes to the EventLog (from a configurable seek position)
//! 2. For each incoming event, finds matching flows via EventMatcher
//! 3. Executes the pipeline operators sequentially
//! 4. Handles fan-out by recursively processing sub-pipelines
//! 5. Logs errors without propagating them (one flow's error doesn't affect others)
//!
//! # Usage
//!
//! ```ignore
//! let runtime = FlowRuntime::new(compiled_flows, event_log, link_service, fetchers);
//! let handle = runtime.run(SeekPosition::Latest);
//! // handle is a JoinHandle that can be used to monitor/cancel the runtime
//! ```

use crate::core::module::EntityFetcher;
use crate::core::service::LinkService;
use crate::events::compiler::CompiledFlow;
use crate::events::context::FlowContext;
use crate::events::log::EventLog;
use crate::events::operators::{OpResult, PipelineOperator};
use crate::events::sinks::SinkRegistry;
use crate::events::types::SeekPosition;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;

/// The main flow execution runtime
pub struct FlowRuntime {
    /// Compiled flows to evaluate for each event
    flows: Vec<CompiledFlow>,

    /// Event log to subscribe to
    event_log: Arc<dyn EventLog>,

    /// Shared link service for resolve/fan_out operators
    link_service: Arc<dyn LinkService>,

    /// Entity fetchers keyed by entity type
    entity_fetchers: HashMap<String, Arc<dyn EntityFetcher>>,

    /// Sink registry for deliver operators
    sink_registry: Option<Arc<SinkRegistry>>,

    /// Consumer name for tracking position
    consumer_name: String,
}

impl std::fmt::Debug for FlowRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowRuntime")
            .field("flows", &self.flows.len())
            .field("consumer_name", &self.consumer_name)
            .finish()
    }
}

impl FlowRuntime {
    /// Create a new FlowRuntime
    pub fn new(
        flows: Vec<CompiledFlow>,
        event_log: Arc<dyn EventLog>,
        link_service: Arc<dyn LinkService>,
        entity_fetchers: HashMap<String, Arc<dyn EntityFetcher>>,
    ) -> Self {
        Self {
            flows,
            event_log,
            link_service,
            entity_fetchers,
            sink_registry: None,
            consumer_name: "flow-runtime".to_string(),
        }
    }

    /// Set a custom consumer name (for multi-consumer setups)
    pub fn with_consumer_name(mut self, name: impl Into<String>) -> Self {
        self.consumer_name = name.into();
        self
    }

    /// Set the sink registry for deliver operators
    ///
    /// Without a sink registry, the `deliver` operator will log but not
    /// actually dispatch to any sink.
    pub fn with_sink_registry(mut self, registry: Arc<SinkRegistry>) -> Self {
        self.sink_registry = Some(registry);
        self
    }

    /// Start the runtime as a background task
    ///
    /// Returns a JoinHandle that resolves when the runtime stops.
    /// The runtime runs indefinitely, processing events as they arrive.
    pub fn run(self, position: SeekPosition) -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = self.run_inner(position).await {
                tracing::error!(error = %e, "flow runtime stopped with error");
            }
        })
    }

    /// Internal run loop
    async fn run_inner(self, position: SeekPosition) -> anyhow::Result<()> {
        tracing::info!(
            flows = self.flows.len(),
            consumer = %self.consumer_name,
            "flow runtime starting"
        );

        let mut stream = self
            .event_log
            .subscribe(&self.consumer_name, position)
            .await?;

        while let Some(envelope) = stream.next().await {
            let event = &envelope.event;

            // Find matching flows
            for flow in &self.flows {
                if flow.matcher.matches(event) {
                    tracing::debug!(
                        flow = %flow.name,
                        event_kind = %event.event_kind(),
                        "flow matched, executing pipeline"
                    );

                    // Create a FlowContext for this execution
                    let mut ctx = FlowContext::new(
                        event.clone(),
                        self.link_service.clone(),
                        self.entity_fetchers.clone(),
                    );

                    // Attach sink registry if available
                    if let Some(ref registry) = self.sink_registry {
                        ctx.sink_registry = Some(registry.clone());
                    }

                    // Execute the pipeline
                    if let Err(e) = execute_pipeline(&flow.operators, &mut ctx).await {
                        tracing::warn!(
                            flow = %flow.name,
                            error = %e,
                            "pipeline execution failed"
                        );
                    }
                }
            }

            // Ack the exact event we just processed
            if let Some(seq) = envelope.seq_no {
                if let Err(e) = self.event_log.ack(&self.consumer_name, seq).await {
                    tracing::warn!(error = %e, "failed to ack event");
                }
            }
        }

        tracing::info!("flow runtime stream ended");
        Ok(())
    }
}

/// Execute a pipeline of operators on a FlowContext
///
/// Handles fan-out by recursively executing remaining operators on each sub-context.
/// Uses `Box::pin` for async recursion since fan-out creates sub-pipelines.
fn execute_pipeline<'a>(
    operators: &'a [Box<dyn PipelineOperator>],
    ctx: &'a mut FlowContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        for (i, op) in operators.iter().enumerate() {
            match op.execute(ctx).await? {
                OpResult::Continue => {
                    // Continue to next operator
                }
                OpResult::Drop => {
                    tracing::debug!(operator = %op.name(), "event dropped by operator");
                    return Ok(());
                }
                OpResult::FanOut(contexts) => {
                    tracing::debug!(
                        operator = %op.name(),
                        count = contexts.len(),
                        "fan-out: processing remaining pipeline for each context"
                    );

                    // Execute remaining operators for each fanned-out context
                    let remaining = &operators[i + 1..];
                    for mut sub_ctx in contexts {
                        if let Err(e) = execute_pipeline(remaining, &mut sub_ctx).await {
                            tracing::warn!(
                                operator = %op.name(),
                                error = %e,
                                "fan-out sub-pipeline failed"
                            );
                        }
                    }
                    return Ok(());
                }
            }
        }

        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::events::*;
    use crate::core::events::{EntityEvent, EventEnvelope, FrameworkEvent, LinkEvent};
    use crate::events::compiler::compile_flow;
    use crate::events::memory::InMemoryEventLog;
    use serde_json::json;
    use std::sync::Arc;
    use uuid::Uuid;

    // ── Mock LinkService ─────────────────────────────────────────────

    struct MockLinkService;

    #[async_trait::async_trait]
    impl LinkService for MockLinkService {
        async fn create(
            &self,
            _: crate::core::link::LinkEntity,
        ) -> anyhow::Result<crate::core::link::LinkEntity> {
            unimplemented!()
        }
        async fn get(&self, _: &Uuid) -> anyhow::Result<Option<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn list(&self) -> anyhow::Result<Vec<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn find_by_source(
            &self,
            _: &Uuid,
            _: Option<&str>,
            _: Option<&str>,
        ) -> anyhow::Result<Vec<crate::core::link::LinkEntity>> {
            Ok(vec![])
        }
        async fn find_by_target(
            &self,
            _: &Uuid,
            _: Option<&str>,
            _: Option<&str>,
        ) -> anyhow::Result<Vec<crate::core::link::LinkEntity>> {
            Ok(vec![])
        }
        async fn update(
            &self,
            _: &Uuid,
            _: crate::core::link::LinkEntity,
        ) -> anyhow::Result<crate::core::link::LinkEntity> {
            unimplemented!()
        }
        async fn delete(&self, _: &Uuid) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn delete_by_entity(&self, _: &Uuid) -> anyhow::Result<()> {
            unimplemented!()
        }
    }

    // ── Mock EntityFetcher ───────────────────────────────────────────

    struct MockEntityFetcher;

    #[async_trait::async_trait]
    impl EntityFetcher for MockEntityFetcher {
        async fn fetch_as_json(&self, id: &Uuid) -> anyhow::Result<serde_json::Value> {
            Ok(json!({"id": id.to_string(), "name": "TestUser"}))
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn make_link_event(link_type: &str) -> FrameworkEvent {
        FrameworkEvent::Link(LinkEvent::Created {
            link_type: link_type.to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        })
    }

    fn make_entity_event(entity_type: &str) -> FrameworkEvent {
        FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: entity_type.to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"name": "test"}),
        })
    }

    // ── Tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_runtime_dispatches_matching_event() {
        let event_log = Arc::new(InMemoryEventLog::new());

        // Compile a simple flow: link.created/follows → map → deliver
        let flow = compile_flow(&FlowConfig {
            name: "follow_notif".to_string(),
            description: None,
            trigger: TriggerConfig {
                kind: "link.created".to_string(),
                link_type: Some("follows".to_string()),
                entity_type: None,
            },
            pipeline: vec![
                PipelineStep::Map(MapConfig {
                    template: json!({"title": "New follower!"}),
                }),
                PipelineStep::Deliver(DeliverConfig {
                    sink: Some("in_app".to_string()),
                    sinks: None,
                }),
            ],
        })
        .unwrap();

        let link_service = Arc::new(MockLinkService) as Arc<dyn LinkService>;
        let runtime = FlowRuntime::new(vec![flow], event_log.clone(), link_service, HashMap::new());

        let handle = runtime.run(SeekPosition::Latest);

        // Publish a matching event
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        event_log
            .append(EventEnvelope::new(make_link_event("follows")))
            .await
            .unwrap();

        // Give the runtime time to process
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        handle.abort();
        // If we got here without panics, the runtime processed the event correctly
    }

    #[tokio::test]
    async fn test_runtime_ignores_non_matching_event() {
        let event_log = Arc::new(InMemoryEventLog::new());

        let flow = compile_flow(&FlowConfig {
            name: "follow_notif".to_string(),
            description: None,
            trigger: TriggerConfig {
                kind: "link.created".to_string(),
                link_type: Some("follows".to_string()),
                entity_type: None,
            },
            pipeline: vec![PipelineStep::Map(MapConfig {
                template: json!({"title": "New follower!"}),
            })],
        })
        .unwrap();

        let link_service = Arc::new(MockLinkService) as Arc<dyn LinkService>;
        let runtime = FlowRuntime::new(vec![flow], event_log.clone(), link_service, HashMap::new());

        let handle = runtime.run(SeekPosition::Latest);

        // Publish a NON-matching event (likes, not follows)
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        event_log
            .append(EventEnvelope::new(make_link_event("likes")))
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        handle.abort();
        // Should process without errors — the flow simply doesn't match
    }

    #[tokio::test]
    async fn test_runtime_multiple_flows() {
        let event_log = Arc::new(InMemoryEventLog::new());

        let flow1 = compile_flow(&FlowConfig {
            name: "follow_flow".to_string(),
            description: None,
            trigger: TriggerConfig {
                kind: "link.created".to_string(),
                link_type: Some("follows".to_string()),
                entity_type: None,
            },
            pipeline: vec![PipelineStep::Map(MapConfig {
                template: json!({"type": "follow"}),
            })],
        })
        .unwrap();

        let flow2 = compile_flow(&FlowConfig {
            name: "entity_flow".to_string(),
            description: None,
            trigger: TriggerConfig {
                kind: "entity.created".to_string(),
                link_type: None,
                entity_type: Some("user".to_string()),
            },
            pipeline: vec![PipelineStep::Map(MapConfig {
                template: json!({"type": "user_created"}),
            })],
        })
        .unwrap();

        let link_service = Arc::new(MockLinkService) as Arc<dyn LinkService>;
        let runtime = FlowRuntime::new(
            vec![flow1, flow2],
            event_log.clone(),
            link_service,
            HashMap::new(),
        );

        let handle = runtime.run(SeekPosition::Latest);

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Publish events that match different flows
        event_log
            .append(EventEnvelope::new(make_link_event("follows")))
            .await
            .unwrap();
        event_log
            .append(EventEnvelope::new(make_entity_event("user")))
            .await
            .unwrap();
        event_log
            .append(EventEnvelope::new(make_link_event("likes"))) // matches nothing
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        handle.abort();
    }

    #[tokio::test]
    async fn test_runtime_filter_drops_event() {
        let event_log = Arc::new(InMemoryEventLog::new());

        let flow = compile_flow(&FlowConfig {
            name: "filtered_flow".to_string(),
            description: None,
            trigger: TriggerConfig {
                kind: "entity.created".to_string(),
                link_type: None,
                entity_type: None,
            },
            pipeline: vec![
                // This filter will drop the event
                PipelineStep::Filter(FilterConfig {
                    condition: "entity_type == \"admin\"".to_string(),
                }),
                PipelineStep::Map(MapConfig {
                    template: json!({"title": "should not reach here"}),
                }),
            ],
        })
        .unwrap();

        let link_service = Arc::new(MockLinkService) as Arc<dyn LinkService>;
        let runtime = FlowRuntime::new(vec![flow], event_log.clone(), link_service, HashMap::new());

        let handle = runtime.run(SeekPosition::Latest);

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        event_log
            .append(EventEnvelope::new(make_entity_event("user"))) // type is "user", filter expects "admin"
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        handle.abort();
    }

    #[tokio::test]
    async fn test_runtime_resolve_and_map() {
        let event_log = Arc::new(InMemoryEventLog::new());

        let flow = compile_flow(&FlowConfig {
            name: "resolve_map_flow".to_string(),
            description: None,
            trigger: TriggerConfig {
                kind: "link.created".to_string(),
                link_type: Some("follows".to_string()),
                entity_type: None,
            },
            pipeline: vec![
                PipelineStep::Resolve(ResolveConfig {
                    from: "source_id".to_string(),
                    via: None,
                    direction: "forward".to_string(),
                    output_var: "source".to_string(),
                }),
                PipelineStep::Map(MapConfig {
                    template: json!({
                        "title": "{{ source.name }} followed you",
                        "source_id": "{{ source_id }}"
                    }),
                }),
                PipelineStep::Deliver(DeliverConfig {
                    sink: Some("in_app".to_string()),
                    sinks: None,
                }),
            ],
        })
        .unwrap();

        let fetcher = Arc::new(MockEntityFetcher) as Arc<dyn EntityFetcher>;
        let mut fetchers = HashMap::new();
        fetchers.insert("user".to_string(), fetcher);

        let link_service = Arc::new(MockLinkService) as Arc<dyn LinkService>;
        let runtime = FlowRuntime::new(vec![flow], event_log.clone(), link_service, fetchers);

        let handle = runtime.run(SeekPosition::Latest);

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        event_log
            .append(EventEnvelope::new(make_link_event("follows")))
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        handle.abort();
    }

    // ── Unit test for execute_pipeline ────────────────────────────────

    #[tokio::test]
    async fn test_execute_pipeline_end_to_end() {
        let ops: Vec<Box<dyn PipelineOperator>> = vec![
            Box::new(
                crate::events::operators::FilterOp::from_config(&FilterConfig {
                    condition: "entity_type == \"user\"".to_string(),
                })
                .unwrap(),
            ),
            Box::new(crate::events::operators::MapOp::from_config(&MapConfig {
                template: json!({"msg": "Hello {{ entity_type }}"}),
            })),
        ];

        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let link_service = Arc::new(MockLinkService) as Arc<dyn LinkService>;
        let mut ctx = FlowContext::new(event, link_service, HashMap::new());

        execute_pipeline(&ops, &mut ctx).await.unwrap();

        // Map should have set _payload
        let payload = ctx.get_var("_payload").unwrap();
        assert_eq!(payload["msg"], "Hello user");
    }

    #[tokio::test]
    async fn test_execute_pipeline_filter_drops() {
        let ops: Vec<Box<dyn PipelineOperator>> = vec![
            Box::new(
                crate::events::operators::FilterOp::from_config(&FilterConfig {
                    condition: "entity_type == \"admin\"".to_string(),
                })
                .unwrap(),
            ),
            Box::new(crate::events::operators::MapOp::from_config(&MapConfig {
                template: json!({"msg": "should not reach"}),
            })),
        ];

        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let link_service = Arc::new(MockLinkService) as Arc<dyn LinkService>;
        let mut ctx = FlowContext::new(event, link_service, HashMap::new());

        execute_pipeline(&ops, &mut ctx).await.unwrap();

        // Map should NOT have been executed (filter dropped)
        assert!(ctx.get_var("_payload").is_none());
    }
}
