//! Hand-written LSX scanner (ROK-2081). Documents are hand-written in the
//! LSLib LSX shape with invented ids and handles.

use std::fmt::Write as _;

use super::bg3_loca_tests::h;
use crate::{
    CatalogueError, LSX_MAX_ATTRS, LSX_MAX_DEPTH, LSX_MAX_NODES, LSX_MAX_RETAINED_BYTES,
    LSX_MAX_TAG_ATTRS, LSX_MAX_TOTAL_ATTRS, LSX_ORIGINS, LSX_QUESTS, LsxAttr, lsx_id_handles,
    read_lsx_nodes,
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
        assert_eq!(parse_reason(bad("&#+65;")), "unknown or invalid entity");
        assert_eq!(parse_reason(bad("&#x+41;")), "unknown or invalid entity");
        assert_eq!(parse_reason(bad("&#-65;")), "unknown or invalid entity");
        assert_eq!(parse_reason(bad("&#;")), "unknown or invalid entity");
        // Attributes the scanner never reads are not decoded, so not checked.
        let ignored = r#"<node id="N" key="&nbsp;"><attribute id="A" version="&#+1;" value="ok"/></node>"#;
        assert_eq!(read_lsx_nodes(ignored, "N").unwrap()[0].attrs.len(), 1);
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

rl_test! {
    fn lsx_cdata_and_bom_are_skipped() {
        let doc = "\u{feff}<?xml version=\"1.0\"?><!DOCTYPE save>\
                   <node id=\"N\"><![CDATA[ a > b <node id=\"N\"> ]]>\
                   <attribute id=\"A\" value=\"v\"/></node>";
        let nodes = read_lsx_nodes(doc, "N").unwrap();
        assert_eq!(nodes.len(), 1, "a <node> inside CDATA is not a node: {nodes:?}");
        assert_eq!(nodes[0].attr("A").and_then(|a| a.value.as_deref()), Some("v"));
        assert_eq!(parse_reason(read_lsx_nodes("<![CDATA[ a > b ]>", "N")), "unterminated markup");
    }
}

/// A tag named `name` with `n` attributes: `id="T"` then `a1=""` … .
fn tag_with(name: &str, n: usize) -> String {
    let mut t = format!("<{name} id=\"T\"");
    for i in 1..n {
        write!(t, " a{i}=\"\"").unwrap();
    }
    t.push_str("/>");
    t
}

fn tag_cap_err() -> Result<Vec<crate::LsxNode>, CatalogueError> {
    Err(CatalogueError::TooLarge {
        what: "lsx attributes on one tag",
        size: LSX_MAX_TAG_ATTRS as u64 + 1,
        limit: LSX_MAX_TAG_ATTRS as u64,
    })
}

rl_test! {
    fn lsx_tag_attribute_cap_at_edge_and_edge_plus_one_on_any_tag() {
        // Matching node, an attribute of a matching node, and a non-matching node.
        for (doc, want_nodes) in [
            (tag_with("node", LSX_MAX_TAG_ATTRS), 1),
            (format!("<node id=\"T\">{}</node>", tag_with("attribute", LSX_MAX_TAG_ATTRS)), 1),
            (format!("<node id=\"X\">{}</node>", tag_with("region", LSX_MAX_TAG_ATTRS)), 0),
        ] {
            assert_eq!(read_lsx_nodes(&doc, "T").unwrap().len(), want_nodes, "doc {doc:?}");
        }
        for doc in [
            tag_with("node", LSX_MAX_TAG_ATTRS + 1),
            format!("<node id=\"T\">{}</node>", tag_with("attribute", LSX_MAX_TAG_ATTRS + 1)),
            format!("<node id=\"X\">{}</node>", tag_with("region", LSX_MAX_TAG_ATTRS + 1)),
        ] {
            assert_eq!(read_lsx_nodes(&doc, "T"), tag_cap_err(), "doc {doc:?}");
        }
    }
}

/// `nodes` matching nodes of `per` one-byte-id attributes each.
fn many_attrs(nodes: usize, per: usize) -> String {
    let node = format!(
        "<node id=\"M\">{}</node>",
        "<attribute id=\"k\"/>".repeat(per)
    );
    node.repeat(nodes)
}

rl_test! {
    fn lsx_total_retained_attribute_cap_at_edge_and_edge_plus_one() {
        let per = LSX_MAX_ATTRS;
        assert_eq!(LSX_MAX_TOTAL_ATTRS % per, 0, "the fixture fills whole nodes");
        let mut doc = many_attrs(LSX_MAX_TOTAL_ATTRS / per, per);
        let total: usize = read_lsx_nodes(&doc, "M").unwrap().iter().map(|n| n.attrs.len()).sum();
        assert_eq!(total, LSX_MAX_TOTAL_ATTRS);
        doc.push_str(&many_attrs(1, 1));
        assert_eq!(
            read_lsx_nodes(&doc, "M"),
            Err(CatalogueError::TooLarge {
                what: "lsx retained attributes",
                size: LSX_MAX_TOTAL_ATTRS as u64 + 1,
                limit: LSX_MAX_TOTAL_ATTRS as u64,
            })
        );
        // Attributes of non-matching nodes are never retained.
        assert!(read_lsx_nodes(&doc, "Other").unwrap().is_empty());
    }
}

rl_test! {
    fn lsx_retained_byte_cap_at_edge_and_edge_plus_one() {
        // id "k" (1 byte) + value; charged as written, before decoding.
        let doc = |value_len: usize| {
            format!("<node id=\"B\"><attribute id=\"k\" value=\"{}\"/></node>", "v".repeat(value_len))
        };
        let at = read_lsx_nodes(&doc(LSX_MAX_RETAINED_BYTES - 1), "B").unwrap();
        assert_eq!(at[0].attrs[0].value.as_ref().map(String::len), Some(LSX_MAX_RETAINED_BYTES - 1));
        assert_eq!(
            read_lsx_nodes(&doc(LSX_MAX_RETAINED_BYTES), "B"),
            Err(CatalogueError::TooLarge {
                what: "lsx retained attribute bytes",
                size: LSX_MAX_RETAINED_BYTES as u64 + 1,
                limit: LSX_MAX_RETAINED_BYTES as u64,
            })
        );
        // A huge value on a non-matching node costs nothing.
        assert!(read_lsx_nodes(&doc(LSX_MAX_RETAINED_BYTES), "Other").unwrap().is_empty());
    }
}
