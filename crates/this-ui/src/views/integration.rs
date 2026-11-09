//! The proof that the contract crosses: a module descriptor written as JSON — entities,
//! links, views, actions — is deserialised by `this-descriptor` **from this crate**, accepted
//! by the contract check, and placed by the renderer's own model.
//!
//! Nothing here touches the DOM: these are the parts of the rendering that can be measured.

use serde_json::json;
use this_descriptor::views::{Action, Brick, Node, Surface, ViewDesc};
use this_descriptor::{CheckProfile, CustomRegistry, FilterExpr, FilterOp, ModuleDescriptor};

use crate::views::data::{self, Feed, Reading, Snapshot};
use crate::views::placement::Portfolio;
use crate::views::state::{ViewState, brick_reading, view_state};

/// A billing module with a sidebar list, a detail view, a read-only replacement of it, a
/// header summary, and one action of every kind the contract allows.
const MODULE: &str = r#"
{
  "name": "billing",
  "version": "1.0.0",
  "entities": [
    {
      "name": "invoice",
      "fields": [
        { "name": "number", "ty": "string" },
        { "name": "amount", "ty": "f64" },
        { "name": "state", "ty": "string" }
      ]
    },
    { "name": "customer", "fields": [{ "name": "name", "ty": "string" }] }
  ],
  "links": [{ "link_type": "billed_to", "source": "invoice", "target": "customer" }],
  "actions": [
    { "kind": "create", "name": "create_invoice", "entity": "invoice",
      "fields": ["number", "amount"] },
    { "kind": "update", "name": "mark_paid", "entity": "invoice", "fields": ["state"] },
    { "kind": "delete", "name": "delete_invoice", "entity": "invoice" },
    { "kind": "navigate", "name": "show_detail", "view": "invoice_detail" },
    { "kind": "agent_tool", "name": "ask", "op": "send_message", "entity": "invoice", "text": "Quelles factures sont en retard ?" },
    { "kind": "device_capability", "name": "scan", "capability": "camera", "grant": "allow" }
  ],
  "views": [
    {
      "name": "invoice_list",
      "surface": "sidebar",
      "title": "Factures",
      "root": { "type": "leaf", "brick": {
        "type": "list", "entity": "invoice", "label": "number", "subtitle": "amount",
        "sort": [{ "field": "amount", "dir": "desc" }], "filterable": ["number"],
        "page_size": 20, "on_click": { "kind": "open_detail" } } }
    },
    {
      "name": "invoice_detail",
      "surface": "main",
      "root": { "type": "stack", "children": [
        { "type": "leaf", "brick": { "type": "card", "entity": "invoice",
          "fields": ["number", "amount", "state"] } },
        { "type": "leaf", "brick": { "type": "button", "entity": "invoice",
          "filter": { "cmp": { "field": "number", "op": "eq", "value": "F-1" } },
          "action": "mark_paid" } }
      ] }
    },
    {
      "name": "invoice_readonly",
      "surface": "main",
      "title": "Facture",
      "replaces": "invoice_detail",
      "root": { "type": "leaf", "brick": {
        "type": "text", "entity": "invoice", "field": "number" } }
    },
    {
      "name": "invoice_header",
      "surface": "header",
      "root": { "type": "row", "children": [
        { "type": "leaf", "brick": { "type": "metrics", "entity": "invoice" } },
        { "type": "leaf", "brick": { "type": "indicator", "signal": "connection" } }
      ] }
    }
  ]
}
"#;

fn descriptor() -> ModuleDescriptor {
    ModuleDescriptor::from_json(MODULE).expect("the module descriptor parses")
}

#[test]
fn a_module_json_with_views_is_accepted_by_the_contract_check() {
    let descriptor = descriptor();
    assert_eq!(descriptor.views.len(), 4);
    assert_eq!(descriptor.actions.len(), 6);
    descriptor
        .check(&CustomRegistry::default(), CheckProfile::Compiled)
        .expect("the contract check accepts this module");
}

#[test]
fn the_portfolio_places_the_drawn_views_on_their_surface() {
    let descriptor = descriptor();
    let portfolio = Portfolio::of(&descriptor.views);
    let shown: Vec<&str> = portfolio
        .shown()
        .iter()
        .map(|view| view.name.as_str())
        .collect();
    assert_eq!(
        shown,
        vec!["invoice_list", "invoice_readonly", "invoice_header"]
    );
    assert_eq!(portfolio.hidden().len(), 1);
    assert_eq!(portfolio.hidden()[0].name, "invoice_detail");
    assert_eq!(portfolio.hidden()[0].by, "invoice_readonly");
    assert!(portfolio.warnings().is_empty());
    assert_eq!(portfolio.at(Surface::Sidebar).len(), 1);
    assert_eq!(portfolio.at(Surface::Main).len(), 1);
    assert_eq!(portfolio.at(Surface::Header).len(), 1);
    assert!(portfolio.at(Surface::Panel).is_empty());
    assert_eq!(
        portfolio.occupied(),
        vec![Surface::Sidebar, Surface::Header, Surface::Main]
    );
}

#[test]
fn the_rows_a_view_reads_come_from_the_host_snapshot() {
    let descriptor = descriptor();
    let list = &descriptor.views[0];
    let snapshot = Snapshot::new().with(
        "invoice",
        Feed::Ready(vec![
            json!({ "id": "1", "number": "F-1", "amount": 10.0 }),
            json!({ "id": "2", "number": "F-2", "amount": 30.0 }),
        ]),
    );
    assert_eq!(view_state(&list.root, &snapshot), ViewState::Ready);

    // The sort of the brick is the server's: amount descending.
    let (filter, sort) = match brick_of(list) {
        Brick::List { filter, sort, .. } => (filter, sort),
        _ => panic!("the first view holds a list"),
    };
    let reading = data::read(&snapshot, "invoice", filter.as_ref(), sort, 0, None);
    let numbers: Vec<String> = reading
        .rows()
        .expect("rows")
        .rows
        .iter()
        .map(|row| data::display(data::value_of(row, "number")))
        .collect();
    assert_eq!(numbers, vec!["F-2", "F-1"]);
}

/// The brick of a view whose root is a single leaf.
fn brick_of(view: &ViewDesc) -> &Brick {
    match &view.root {
        Node::Leaf { brick } => brick,
        _ => panic!("this view's root is a leaf"),
    }
}

#[test]
fn a_refusal_and_a_missing_entity_are_two_distinct_verdicts() {
    let descriptor = descriptor();
    let list = &descriptor.views[0];
    let denied = Snapshot::new().with("invoice", Feed::Denied("perm:invoice.list".into()));
    assert_eq!(
        view_state(&list.root, &denied),
        ViewState::Denied {
            entity: "invoice".into(),
            reason: "perm:invoice.list".into()
        }
    );
    let nothing = Snapshot::new();
    assert_eq!(
        view_state(&list.root, &nothing),
        ViewState::Missing {
            entity: "invoice".into()
        }
    );
    let empty = Snapshot::new().with("invoice", Feed::Ready(vec![]));
    assert_eq!(
        view_state(&list.root, &empty),
        ViewState::Empty {
            entity: Some("invoice".into())
        }
    );
}

#[test]
fn every_action_kind_of_the_contract_is_deserialised() {
    let descriptor = descriptor();
    let kinds: Vec<&str> = descriptor
        .actions
        .iter()
        .map(|action| {
            #[allow(unreachable_patterns)]
            match action {
                Action::Create { .. } => "create",
                Action::Update { .. } => "update",
                Action::Delete { .. } => "delete",
                Action::Navigate { .. } => "navigate",
                Action::AgentTool { .. } => "agent_tool",
                Action::DeviceCapability { .. } => "device_capability",
                _ => "unknown",
            }
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "create",
            "update",
            "delete",
            "navigate",
            "agent_tool",
            "device_capability"
        ]
    );
}

#[test]
fn every_brick_of_the_contract_deserialises() {
    // One JSON form per brick. The renderer's dispatch is an exhaustive `match` on `Brick`,
    // so this list and the compile of that match keep each other honest.
    let bricks = [
        r#"{"type":"text","entity":"invoice","field":"number"}"#,
        r#"{"type":"button","entity":"invoice","action":"mark_paid"}"#,
        r#"{"type":"banner","tone":"warning","text":"attention"}"#,
        r#"{"type":"indicator","signal":"connection"}"#,
        r#"{"type":"list","entity":"invoice","label":"number"}"#,
        r#"{"type":"card","entity":"invoice","fields":["number"]}"#,
        r#"{"type":"form","entity":"invoice","fields":[{"field":"number"}],"submit":"create_invoice"}"#,
        r#"{"type":"field","entity":"invoice","field":"number","input":"text"}"#,
        r#"{"type":"relation","entity":"invoice","link":"billed_to","mode":"show"}"#,
        r#"{"type":"chat","entity":"invoice"}"#,
        r#"{"type":"tool_step","entity":"invoice","field":"state"}"#,
        r#"{"type":"queue","entity":"invoice","label":"number"}"#,
        r#"{"type":"code_block","entity":"invoice","field":"number","language":"json"}"#,
        r#"{"type":"result","entity":"invoice","field":"number"}"#,
        r#"{"type":"capability_request","capability":"camera","action":"scan"}"#,
        r#"{"type":"metrics","entity":"invoice"}"#,
        r#"{"type":"program","entity":"invoice","args":[{"name":"n","kind":"number"}],
           "run":"manual","columns":[{"name":"total","source":{"op":"sum","field":"amount"}}]}"#,
    ];
    let parsed: Vec<Brick> = bricks
        .iter()
        .map(|json| serde_json::from_str::<Brick>(json).expect("the brick parses"))
        .collect();
    assert_eq!(parsed.len(), 17);
    // Four bricks read no row: a banner, a device request, and an indicator of connection.
    let with_entity = parsed
        .iter()
        .filter(|brick| brick.entity().is_some())
        .count();
    assert_eq!(with_entity, 14);
    let Reading::Unknown = data::read(&Snapshot::new(), "invoice", None, &[], 0, None) else {
        panic!("an empty snapshot knows no entity");
    };
}

#[test]
fn a_reference_input_in_a_form_is_read_from_the_declared_field_type() {
    // The form brick carries no input kind: it comes from the entity's declared types. Here
    // `amount` is a number and `number` a text, which the renderer turns into two controls.
    let descriptor = descriptor();
    let invoice = descriptor
        .entities
        .iter()
        .find(|entity| entity.name == "invoice")
        .expect("invoice entity");
    let kinds: Vec<&str> = invoice
        .fields
        .iter()
        .map(|field| match crate::model::Kind::of(&field.ty) {
            crate::model::Kind::Int | crate::model::Kind::Decimal => "number",
            crate::model::Kind::Bool => "toggle",
            crate::model::Kind::Enum(_) => "choice",
            crate::model::Kind::Ref(_) => "reference",
            _ => "text",
        })
        .collect();
    assert_eq!(kinds, vec!["text", "number", "text"]);
}

/// The chat surface: one `message` brick per message, each filtered on its own key. A
/// brick reads one row, so its filter is what says which message it draws.
#[test]
fn a_chat_thread_is_one_message_brick_per_message_in_order() {
    let keys = ["m1", "m2", "m3"];
    let thread = ViewDesc {
        name: "fil".into(),
        surface: Surface::Chat,
        title: None,
        replaces: None,
        look: None,
        root: Node::Stack {
            children: keys
                .iter()
                .map(|key| Node::Leaf {
                    brick: Box::new(Brick::Message {
                        entity: "message".into(),
                        filter: Some(FilterExpr::Cmp {
                            field: "local".into(),
                            op: FilterOp::Eq,
                            value: json!(key),
                        }),
                    }),
                })
                .collect(),
        },
    };
    let portfolio = Portfolio::of(std::slice::from_ref(&thread));
    assert_eq!(portfolio.occupied(), vec![Surface::Chat]);
    assert_eq!(portfolio.at(Surface::Chat).len(), 1);

    let snapshot = Snapshot::new().with(
        "message",
        Feed::Ready(vec![
            json!({ "local": "m1", "role": "utilisateur", "contenu": "bonjour" }),
            json!({ "local": "m2", "role": "agent", "contenu": "que crée-t-on ?" }),
            json!({ "local": "m3", "role": "utilisateur", "contenu": "une liste de courses" }),
        ]),
    );
    let Node::Stack { children } = &thread.root else {
        panic!("the thread is a stack of bubbles");
    };
    let read: Vec<String> = children
        .iter()
        .map(|node| {
            let Node::Leaf { brick } = node else {
                panic!("each bubble is a leaf");
            };
            let reading = brick_reading(brick, &snapshot).expect("a message brick reads an entity");
            let rows = reading
                .rows()
                .expect("the filter keeps exactly one message");
            assert_eq!(rows.rows.len(), 1);
            data::display(data::value_of(rows.rows[0], "contenu"))
        })
        .collect();
    assert_eq!(
        read,
        vec!["bonjour", "que crée-t-on ?", "une liste de courses"]
    );
}

/// The chat surface as a module declares it: ONE `message` brick, filtered on the open
/// conversation through a context reference. The host provides the conversation; the
/// module never learns which one it is, and the brick reads every message of it, in order.
#[test]
fn a_chat_thread_is_one_message_brick_on_the_open_conversation() {
    let thread = ViewDesc {
        name: "fil".into(),
        surface: Surface::Chat,
        title: None,
        replaces: None,
        look: None,
        root: Node::Leaf {
            brick: Box::new(Brick::Message {
                entity: "message".into(),
                filter: Some(FilterExpr::Cmp {
                    field: "conversation".into(),
                    op: FilterOp::Eq,
                    value: json!({ "$context": "conversation" }),
                }),
            }),
        },
    };
    let portfolio = Portfolio::of(std::slice::from_ref(&thread));
    assert_eq!(portfolio.at(Surface::Chat).len(), 1);

    let rows = vec![
        json!({ "local": "m1", "conversation": "c-1", "role": "utilisateur", "contenu": "bonjour" }),
        json!({ "local": "m2", "conversation": "c-2", "role": "utilisateur", "contenu": "autre fil" }),
        json!({ "local": "m3", "conversation": "c-1", "role": "agent", "contenu": "que crée-t-on ?" }),
    ];
    let Node::Leaf { brick } = &thread.root else {
        panic!("the thread is one brick");
    };
    let read_in = |conversation: &str| -> Vec<String> {
        let snapshot = Snapshot::new()
            .with("message", Feed::Ready(rows.clone()))
            .with_context("conversation", json!(conversation));
        let reading = brick_reading(brick, &snapshot).expect("a message brick reads an entity");
        reading
            .rows()
            .expect("the open conversation has messages")
            .rows
            .iter()
            .map(|row| data::display(data::value_of(row, "contenu")))
            .collect()
    };
    assert_eq!(read_in("c-1"), vec!["bonjour", "que crée-t-on ?"]);
    assert_eq!(read_in("c-2"), vec!["autre fil"]);

    // No open conversation: the brick is empty, not the union of every conversation.
    let closed = Snapshot::new().with("message", Feed::Ready(rows.clone()));
    assert_eq!(brick_reading(brick, &closed), Some(Reading::Empty));
}
