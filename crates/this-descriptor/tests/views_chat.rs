//! La surface `chat` et la brique `message` : le vocabulaire du fil de conversation.
use this_descriptor::views::{Brick, Node, Surface, ViewDesc};
use this_descriptor::{CheckProfile, EntityDesc, FieldDesc, FieldType, ModuleDescriptor};

#[test]
fn une_vue_de_chat_avec_des_messages_passe_le_contrat() {
    let mut d = ModuleDescriptor::new("conversations", "0.1.0");
    let mut e = EntityDesc::new("message");
    e.fields.push(FieldDesc::new("role", FieldType::String));
    e.fields.push(FieldDesc::new("contenu", FieldType::Text));
    for op in this_descriptor::Operation::CRUD {
        e.permissions
            .ops
            .insert(op, this_descriptor::PolicyExpr::Authenticated);
    }
    d.entities.push(e);

    d.views.push(ViewDesc {
        name: "fil".into(),
        surface: Surface::Chat,
        title: Some("Conversation".into()),
        replaces: None,
        look: None,
        root: Node::Stack {
            children: vec![Node::Leaf {
                brick: Box::new(Brick::Message {
                    entity: "message".into(),
                    filter: None,
                }),
            }],
        },
    });

    d.normalize();
    let issues = d.check(&Default::default(), CheckProfile::Stored { allow_public: false });
    assert!(issues.is_ok(), "le fil de chat est un contrat valide : {issues:?}");
}
