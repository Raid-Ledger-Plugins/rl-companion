//! Hand-written LSX scanner (ROK-2081). Documents are hand-written in the
//! LSLib LSX shape with invented ids and handles.

use std::fmt::Write as _;

use super::bg3_loca_tests::h;
use crate::{
    CatalogueError, LSX_MAX_ATTRS, LSX_MAX_DEPTH, LSX_MAX_NODES, LSX_ORIGINS, LSX_QUESTS, LsxAttr,
    lsx_id_handles, read_lsx_nodes,
};

fn origins_doc() -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<!-- <node id="Origin"> inside a comment is not a node -->
<save>
  <version major="4" minor="0" revision="9" build="0" />
  <region id="Origins">
    <node id="root">
      <children>
        <node id="Origin">
          <attribute id="Name" type="FixedString" value="TestOriginA" />
          <attribute id="DisplayName" type="TranslatedString" handle="{a}" version="1" />
          <children>
            <node id="Nested">
              <attribute id="Name" type="FixedString" value="LeakedName" />
            </node>
          </children>
        </node>
        <node id='Origin'>
          <attribute id='Name' type='FixedString' value='TestOriginB'/>
          <attribute id="DisplayName" type="TranslatedString" value="{b}" />
        </node>
        <node id="Origin">
          <attribute id="Name" type="FixedString" value="NoHandle" />
          <attribute id="DisplayName" type="TranslatedString" handle="not-a-handle" />
        </node>
        <node id="Origin" />
      </children>
    </node>
  </region>
</save>
"#,
        a = h(0xa),
        b = h(0xb)
    )
}

fn parse_reason(r: Result<impl std::fmt::Debug, CatalogueError>) -> &'static str {
    match r {
        Err(CatalogueError::Parse { reason, .. }) => reason,
        other => panic!("expected a Parse error, got {other:?}"),
    }
}

rl_test! {
    fn lsx_reads_direct_attributes_of_matching_nodes_only() {
        let nodes = read_lsx_nodes(&origins_doc(), "Origin").unwrap();
        assert_eq!(nodes.len(), 4, "comment and nested nodes are not matches: {nodes:?}");
        assert_eq!(nodes[0].attrs.len(), 2, "the nested node's attribute leaked: {:?}", nodes[0]);
        assert_eq!(
            nodes[0].attr("DisplayName"),
            Some(&LsxAttr {
                id: "DisplayName".into(),
                kind: Some("TranslatedString".into()),
                value: None,
                handle: Some(h(0xa)),
            })
        );
        assert!(nodes[3].attrs.is_empty(), "a self-closing node has no attributes");
        let pairs = lsx_id_handles(&nodes, &LSX_ORIGINS);
        assert_eq!(pairs, [("TestOriginA".to_owned(), h(0xa)), ("TestOriginB".to_owned(), h(0xb))]);
    }
}

rl_test! {
    fn lsx_quest_titles() {
        let doc = format!(
            "<save><region id=\"quest_prototypes\"><node id=\"root\"><children>\
             <node id=\"Quest\"><attribute id=\"QuestID\" type=\"FixedString\" value=\"Q_TestQuest\"/>\
             <attribute id=\"QuestTitle\" type=\"TranslatedString\" handle=\"{}\" version=\"2\"/></node>\
             </children></node></region></save>",
            h(0x51)
        );
        let pairs = lsx_id_handles(&read_lsx_nodes(&doc, LSX_QUESTS.node_id).unwrap(), &LSX_QUESTS);
        assert_eq!(pairs, [("Q_TestQuest".to_owned(), h(0x51))]);
    }
}

rl_test! {
    fn lsx_attribute_escaping_is_decoded() {
        let doc = r#"<node id="N"><attribute id="Name" value="Tav &amp; &quot;Co&quot; &lt;3 &gt; &apos;x&apos; &#65;&#x42;&#X43; a>b" /></node>"#;
        let nodes = read_lsx_nodes(doc, "N").unwrap();
        let value = nodes[0].attr("Name").and_then(|a| a.value.as_deref());
        assert_eq!(value, Some(r#"Tav & "Co" <3 > 'x' ABC a>b"#));
        let bad = |v: &str| read_lsx_nodes(&format!(r#"<node id="N"><attribute id="A" value="{v}"/></node>"#), "N");
        assert_eq!(parse_reason(bad("&nbsp;")), "unknown or invalid entity");
        assert_eq!(parse_reason(bad("&#0;")), "unknown or invalid entity");
        assert_eq!(parse_reason(bad("&#xD800;")), "unknown or invalid entity");
        assert_eq!(parse_reason(bad("a & b")), "unterminated entity");
        assert_eq!(parse_reason(bad("&amp")), "unterminated entity");
    }
}

rl_test! {
    fn lsx_malformed_documents_are_errors() {
        let cases = [
            ("<node id=\"N\">", "a node is never closed"),
            ("</node>", "</node> without an open node"),
            ("<node id=\"N\"", "tag runs past the end"),
            ("<node id=\"N>", "unterminated attribute value"),
            ("<node id=N>", "malformed tag attribute"),
            ("<node id=\"N\"><attribute type=\"x\"/></node>", "attribute without an id"),
            ("< node/>", "tag without a name"),
            ("<!-- never closed", "unterminated markup"),
            ("<node id=\"N\"></node", "unterminated tag"),
        ];
        for (doc, want) in cases {
            assert_eq!(parse_reason(read_lsx_nodes(doc, "N")), want, "doc {doc:?}");
        }
    }
}

fn nested(depth: usize) -> String {
    format!(
        "{}{}",
        "<node id=\"D\">".repeat(depth),
        "</node>".repeat(depth)
    )
}

rl_test! {
    fn lsx_depth_cap_at_edge_and_edge_plus_one() {
        assert_eq!(read_lsx_nodes(&nested(LSX_MAX_DEPTH), "D").unwrap().len(), LSX_MAX_DEPTH);
        let over = read_lsx_nodes(&nested(LSX_MAX_DEPTH + 1), "D");
        assert_eq!(parse_reason(over), "nodes nest too deep");
    }
}

rl_test! {
    fn lsx_node_count_cap_at_edge_and_edge_plus_one() {
        let mut doc = "<node id=\"Q\"/>".repeat(LSX_MAX_NODES);
        assert_eq!(read_lsx_nodes(&doc, "Q").unwrap().len(), LSX_MAX_NODES);
        doc.push_str("<node id=\"Q\"/>");
        assert_eq!(
            read_lsx_nodes(&doc, "Q"),
            Err(CatalogueError::TooLarge {
                what: "lsx matching node count",
                size: LSX_MAX_NODES as u64 + 1,
                limit: LSX_MAX_NODES as u64,
            })
        );
        // Non-matching nodes never count against the cap.
        assert!(read_lsx_nodes(&doc, "Other").unwrap().is_empty());
    }
}

fn with_attrs(n: usize) -> String {
    let mut doc = String::from("<node id=\"A\">");
    for i in 0..n {
        write!(doc, "<attribute id=\"K{i}\" value=\"{i}\"/>").unwrap();
    }
    doc.push_str("</node>");
    doc
}

rl_test! {
    fn lsx_attribute_cap_at_edge_and_edge_plus_one() {
        assert_eq!(read_lsx_nodes(&with_attrs(LSX_MAX_ATTRS), "A").unwrap()[0].attrs.len(), LSX_MAX_ATTRS);
        assert_eq!(
            read_lsx_nodes(&with_attrs(LSX_MAX_ATTRS + 1), "A"),
            Err(CatalogueError::TooLarge {
                what: "lsx attributes on one node",
                size: LSX_MAX_ATTRS as u64 + 1,
                limit: LSX_MAX_ATTRS as u64,
            })
        );
    }
}
