//! OpenAPI の path テンプレートとの照合と、画面へ渡す JSON の形。

mod common;

use common::*;
use nekoreq_curl::{match_path_template, ExportOptions, Shell};

fn caps(m: Option<nekoreq_curl::PathMatch>) -> Vec<(String, String)> {
    m.expect("一致するはず").params
}

#[test]
fn template_matches_concrete_path() {
    let m = match_path_template(
        "/api/v1/video/clients/{clientId}",
        "/api/v1/video/clients/42",
    )
    .unwrap();
    assert_eq!(m.params, [("clientId".to_string(), "42".to_string())]);
    assert_eq!(m.literal_segments, 4);
}

#[test]
fn several_params_and_decoding() {
    let m = match_path_template(
        "/clients/{clientId}/notes/{noteId}",
        "/clients/a%2Fb%20c/notes/%E6%97%A5",
    );
    assert_eq!(
        caps(m),
        [
            ("clientId".into(), "a/b c".into()),
            ("noteId".into(), "日".into())
        ]
    );
}

#[test]
fn no_match_cases() {
    assert!(match_path_template("/a/{id}", "/a").is_none());
    assert!(match_path_template("/a/{id}", "/a/").is_none());
    assert!(match_path_template("/a/{id}", "/a/1/2").is_none());
    assert!(match_path_template("/a/{id}", "/b/1").is_none());
    assert!(match_path_template("/a/b", "/a/c").is_none());
}

#[test]
fn trailing_slash_is_ignored() {
    assert!(match_path_template("/a/{id}/", "/a/1").is_some());
    assert!(match_path_template("/a/{id}", "/a/1/").is_some());
    assert!(match_path_template("/", "/").is_some());
    assert!(match_path_template("/a", "/a/").is_some());
}

#[test]
fn partial_segment_params() {
    let m = match_path_template("/files/{name}.json", "/files/report.json");
    assert_eq!(caps(m), [("name".into(), "report".into())]);
    assert!(match_path_template("/files/{name}.json", "/files/report.xml").is_none());
    let m = match_path_template("/v/{a}-{b}", "/v/x-y-z");
    assert_eq!(
        caps(m),
        [("a".into(), "x".into()), ("b".into(), "y-z".into())]
    );
}

#[test]
fn variables_in_the_path_match_a_param() {
    let m = match_path_template("/clients/{clientId}", "/clients/{{clientId}}");
    assert_eq!(caps(m), [("clientId".into(), "{{clientId}}".into())]);
}

#[test]
fn literal_segments_rank_more_specific_templates_higher() {
    let a = match_path_template("/clients/{id}", "/clients/search").unwrap();
    let b = match_path_template("/clients/search", "/clients/search").unwrap();
    assert!(b.literal_segments > a.literal_segments);
}

#[test]
fn parsed_path_can_be_passed_directly() {
    let c = p("curl 'http://localhost:8080/api/v1/video/clients/7?x=1'");
    assert!(match_path_template("/api/v1/video/clients/{clientId}", &c.path).is_some());
    // baseUrl にパスがあるとき
    let rest = c.path_after_base("http://localhost:8080/api/v1").unwrap();
    assert!(match_path_template("/video/clients/{clientId}", &rest).is_some());
}

#[test]
fn types_serialize_as_camel_case_json() {
    let c = p(
        "curl -X POST http://x/a -H 'X-A: 1' -k -d '{\"a\":1}' -H 'Content-Type: application/json'",
    );
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["method"], "POST");
    assert_eq!(v["base"], "http://x");
    assert_eq!(v["body"]["kind"], "json");
    assert_eq!(v["headers"][0]["location"], "header");
    assert_eq!(v["warnings"][0]["kind"], "ignored");
    assert_eq!(v["warnings"][0]["option"], "-k");
    let back: nekoreq_curl::ParsedCurl = serde_json::from_value(v).unwrap();
    assert_eq!(back, c);

    let o: ExportOptions = serde_json::from_str(r#"{"shell":"cmd","redact":["x"]}"#).unwrap();
    assert_eq!(o.shell, Shell::Cmd);
    assert!(o.multiline, "指定のない項目は既定値");
    assert_eq!(o.redact, ["x"]);
}
