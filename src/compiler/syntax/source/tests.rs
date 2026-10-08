use std::fs;

use crate::compiler::loader::load_program;

#[test]
fn imported_error_keeps_its_file_and_utf8_crlf_offset() {
    let temp = std::env::temp_dir().join(format!("argent-source-origin-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(&temp).expect("temp directory");
    fs::write(temp.join("root.ag"), "import \"./child.ag\";").expect("root source");
    fs::write(temp.join("child.ag"), "// Café\r\n@\r\n").expect("imported source");

    let error = load_program(temp.join("root.ag")).expect_err("invalid imported token");
    assert_eq!(error.path.as_deref(), Some(std::path::absolute(temp.join("child.ag")).expect("absolute imported path").as_path()));
    let location = error.location.expect("source location");
    assert_eq!((location.line, location.column, location.byte_offset), (2, 1, 10));
    assert!(error.message.contains("unexpected character"), "{error}");

    let _ = fs::remove_dir_all(temp);
}
