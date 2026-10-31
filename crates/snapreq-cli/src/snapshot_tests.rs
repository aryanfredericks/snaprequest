//! Snapshot tests of everything the CLI renders, using fixed sample data so the
//! snapshots never depend on ports, dates or timing. Review changes with `cargo insta review`.

use std::io::Write;

use crate::fixtures;
use crate::output::{self, Parts};
use crate::{json_out, trace_view, waterfall};

/// Renders with colors stripped.
fn plain(f: impl FnOnce(&mut dyn Write) -> std::io::Result<()>) -> String {
    let mut buf = Vec::new();
    {
        let mut out = anstream::StripStream::new(&mut buf);
        f(&mut out).unwrap();
    }
    String::from_utf8(buf).unwrap()
}

#[test]
fn pretty_output_all_parts() {
    let parts = Parts::parse("HBhbm").unwrap();
    let text = plain(|out| {
        output::print(
            out,
            &fixtures::request(),
            &fixtures::response(),
            parts,
            true,
        )
    });
    insta::assert_snapshot!(text);
}

#[test]
fn pretty_output_default_terminal_parts() {
    let text = plain(|out| {
        output::print(
            out,
            &fixtures::request(),
            &fixtures::response(),
            Parts::default_for(true),
            true,
        )
    });
    insta::assert_snapshot!(text);
}

#[test]
fn trace_view_v() {
    let text = plain(|out| trace_view::render(out, &fixtures::trace(), 1));
    insta::assert_snapshot!(text);
}

#[test]
fn trace_view_vv() {
    let text = plain(|out| trace_view::render(out, &fixtures::trace(), 2));
    insta::assert_snapshot!(text);
}

#[test]
fn trace_view_failure() {
    let err = fixtures::connect_failure();
    let text = plain(|out| trace_view::render(out, err.trace().unwrap(), 1));
    insta::assert_snapshot!(text);
}

#[test]
fn waterfall_with_redirect() {
    let text = plain(|out| waterfall::render(out, &fixtures::trace(), 80));
    insta::assert_snapshot!(text);
}

#[test]
fn json_success_document() {
    let doc = json_out::success(&fixtures::request(), &fixtures::response(), None);
    insta::assert_snapshot!(serde_json::to_string_pretty(&doc).unwrap());
}

#[test]
fn json_failure_document() {
    let doc = json_out::failure(Some(&fixtures::request()), &fixtures::connect_failure());
    insta::assert_snapshot!(serde_json::to_string_pretty(&doc).unwrap());
}
