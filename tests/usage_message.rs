mod common;

/// W8-ERR-7: a JSON usage error for a missing argument names the argument in `error.message`.
#[test]
fn missing_required_argument_is_named_in_the_json_message() {
    let o = common::glen(&["send", "b8e4a07"], &[]);
    assert_eq!(o.status.code(), Some(2), "{o:?}");
    assert!(o.stderr.is_empty(), "{o:?}");
    let v = common::json(&o);
    assert_eq!(v["error"]["code"], "usage");
    assert_eq!(
        v["error"]["message"],
        "the following required arguments were not provided: <MESSAGE>"
    );
}
